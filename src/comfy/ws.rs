//! ComfyUI 的 WebSocket 客户端：实时进度与执行状态只有这条通道有。
//!
//! 帧格式：事件是文本帧 `{"type": ..., "data": ...}`；二进制帧是采样预览与节点进度文本载荷，
//! 本插件不展示实时预览，整类忽略（只用作「连接仍活着」的信号）。
//! 断线按退避重连；超过上限后回报「彻底断开」，由运行时确认服务是否还在。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

use crate::comfy::types::{ExecutingPayload, ExecutionErrorPayload, ProgressPayload};

/// 重连退避（毫秒）：本机服务，前几次快速重试即可。
const RECONNECT_DELAYS: [Duration; 5] = [
    Duration::from_millis(500),
    Duration::from_millis(1500),
    Duration::from_millis(3000),
    Duration::from_millis(5000),
    Duration::from_millis(10000),
];
/// 单次连接的握手超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// 一次 WebSocket 生命周期/消息事件。领域载荷已解析成类型，运行时直接分派。
#[derive(Clone, Debug)]
pub enum WsEvent {
    Progress(ProgressPayload),
    Executing(ExecutingPayload),
    ExecutionError(ExecutionErrorPayload),
    /// 本轮执行收场（`execution_success` / `execution_interrupted`）。
    ExecutionEnd,
    /// 队列相关：status / execution_start / execution_cached，运行时据此刷新队列。
    QueueSignal,
    /// 断线且不再重连（退避用尽）：运行时据此确认服务是否还在。
    Disconnected,
}

/// 运行 WS 客户端直到 `shutdown` 置真或退避用尽。事件经 `tx` 送往运行时。
/// 调用方换连接（改地址/服务下线）时置 `shutdown`：读帧循环与退避睡眠都会检查它并尽快收场。
pub async fn run(url: String, tx: UnboundedSender<WsEvent>, shutdown: Arc<AtomicBool>) {
    let mut retry_index = 0usize;
    while !shutdown.load(Ordering::Relaxed) {
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await {
            Ok(Ok((stream, _))) => Some(stream),
            _ => None, // 连接失败或握手超时都按失败退避
        };
        let Some(mut stream) = stream else {
            // 连不上：退避后重试，用尽即报断开
            if retry_index >= RECONNECT_DELAYS.len() {
                let _ = tx.send(WsEvent::Disconnected);
                return;
            }
            let delay = RECONNECT_DELAYS[retry_index];
            retry_index += 1;
            sleep_or_stop(delay, &shutdown).await;
            continue;
        };

        // 读帧循环：收到首个到达的帧（Text/Binary）才认定连接可用并重置退避。只在握手成功时重置
        // 会让「一连上即被服务端关闭」的连接停在最短间隔无限重连、永不回报断开。
        while let Some(frame) = stream.next().await {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
            match frame {
                Ok(Message::Text(text)) => {
                    retry_index = 0;
                    dispatch_text(&text, &tx);
                }
                Ok(Message::Binary(_)) => retry_index = 0,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {} // Ping/Pong/Frame 由库处理
            }
        }

        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        if retry_index >= RECONNECT_DELAYS.len() {
            let _ = tx.send(WsEvent::Disconnected);
            return;
        }
        let delay = RECONNECT_DELAYS[retry_index];
        retry_index += 1;
        sleep_or_stop(delay, &shutdown).await;
    }
}

async fn sleep_or_stop(delay: Duration, shutdown: &Arc<AtomicBool>) {
    // 睡眠期间也要能尽快响应停止：轮询式短睡，避免 abort 后仍空等满退避
    let mut remaining = delay;
    let tick = Duration::from_millis(150);
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let nap = remaining.min(tick);
        tokio::time::sleep(nap).await;
        remaining = remaining.saturating_sub(nap);
    }
}

/// 解析文本帧：`{"type": String, "data": ...}`，非法帧静默忽略。
fn dispatch_text(raw: &str, tx: &UnboundedSender<WsEvent>) {
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let Some(type_) = parsed.get("type").and_then(Value::as_str) else {
        return;
    };
    let empty = Value::Null;
    let data = parsed.get("data").unwrap_or(&empty);
    forward(type_, data, tx);
}

/// `progress_state`（新版服务端按节点聚合推送）：取 `max` 最大的节点作为进度来源——同一时刻推动
/// 进度条的是步数最多的采样节点，与经典 `progress` 事件的口径一致。
fn progress_from_state(data: &Value) -> Option<WsEvent> {
    let prompt_id = data.get("prompt_id").and_then(Value::as_str).map(str::to_string);
    let best = data
        .get("nodes")?
        .as_object()?
        .values()
        .filter_map(|node| {
            let value = node.get("value").and_then(Value::as_f64)?;
            let max = node.get("max").and_then(Value::as_f64)?;
            Some(ProgressPayload { value, max, prompt_id: prompt_id.clone() })
        })
        .max_by(|a, b| a.max.total_cmp(&b.max))?;
    Some(WsEvent::Progress(best))
}

fn forward(type_: &str, data: &Value, tx: &UnboundedSender<WsEvent>) {
    let event = match type_ {
        "progress" => Some(WsEvent::Progress(ProgressPayload::from_value(data))),
        "progress_state" => progress_from_state(data),
        "executing" => Some(WsEvent::Executing(ExecutingPayload::from_value(data))),
        "execution_error" => Some(WsEvent::ExecutionError(ExecutionErrorPayload::from_value(data))),
        "execution_success" | "execution_interrupted" => Some(WsEvent::ExecutionEnd),
        "status" | "execution_start" | "execution_cached" => Some(WsEvent::QueueSignal),
        _ => None,
    };
    // 接收端已关闭（运行时换连接了）就丢弃，不影响后续帧
    if let Some(event) = event {
        let _ = tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn send(type_: &str, data: Value) -> Option<WsEvent> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        forward(type_, &data, &tx);
        rx.try_recv().ok()
    }

    #[test]
    fn end_signals_map_to_execution_end() {
        for type_ in ["execution_success", "execution_interrupted"] {
            assert!(matches!(send(type_, json!({"prompt_id": "p1"})), Some(WsEvent::ExecutionEnd)), "{type_}");
        }
        assert!(matches!(send("execution_cached", json!({"nodes": []})), Some(WsEvent::QueueSignal)));
        assert!(send("b_preview_image", json!([])).is_none(), "未知事件不外发");
    }

    #[test]
    fn progress_state_picks_node_with_largest_max() {
        let data = json!({
            "prompt_id": "p1",
            "nodes": {
                "3": {"value": 1.0, "max": 4.0},
                "7": {"value": 20.0, "max": 30.0},
            },
        });
        let event = send("progress_state", data);
        let Some(WsEvent::Progress(progress)) = event else {
            panic!("应解析成进度事件：{event:?}");
        };
        assert_eq!((progress.value, progress.max), (20.0, 30.0));
        assert_eq!(progress.prompt_id.as_deref(), Some("p1"));
    }
}
