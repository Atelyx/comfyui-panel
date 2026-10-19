//! ComfyUI 接口的数据形状与从 JSON 里安全取值的辅助。
//!
//! 只建模本插件消费的字段：响应里其余键不建模，用 `serde_json::Value` 原样透传。
//! 灵活结构（工作流 prompt、节点定义 object_info、队列/历史条目）保持 `Value`，取值用下面的访问器。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 图片引用，即取图接口 `/view` 的查询参数来源。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub filename: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub subfolder: String,
    #[serde(rename = "type", default = "default_output_type")]
    pub type_: String,
}

/// `null` 或缺省都按空串取（`subfolder` 常为 null）。
fn opt_string<'de, D>(de: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<String>::deserialize(de)?;
    Ok(v.unwrap_or_default())
}

fn default_output_type() -> String {
    "output".into()
}

/// 采样进度（只在 WebSocket 上）。
#[derive(Clone, Debug, Default)]
pub struct ProgressPayload {
    pub value: f64,
    pub max: f64,
    pub prompt_id: Option<String>,
}

impl ProgressPayload {
    pub fn from_value(data: &Value) -> Self {
        ProgressPayload {
            value: data.get("value").and_then(Value::as_f64).unwrap_or(0.0),
            max: data.get("max").and_then(Value::as_f64).unwrap_or(0.0),
            prompt_id: data.get("prompt_id").and_then(Value::as_str).map(str::to_string),
        }
    }

    /// 进度比例（0..1）；max 为 0 时视作 0，避免除零。
    pub fn fraction(&self) -> f64 {
        if self.max > 0.0 {
            (self.value / self.max).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// 当前执行节点；`node` 为 null 表示本轮结束。
#[derive(Clone, Debug, Default)]
pub struct ExecutingPayload {
    pub node: Option<String>,
    pub prompt_id: Option<String>,
}

impl ExecutingPayload {
    pub fn from_value(data: &Value) -> Self {
        ExecutingPayload {
            node: data.get("node").and_then(Value::as_str).map(str::to_string),
            prompt_id: data.get("prompt_id").and_then(Value::as_str).map(str::to_string),
        }
    }
}

/// 执行错误（只取展示需要的字段）。
#[derive(Clone, Debug, Default)]
pub struct ExecutionErrorPayload {
    pub node_type: Option<String>,
    pub exception_message: Option<String>,
}

impl ExecutionErrorPayload {
    pub fn from_value(data: &Value) -> Self {
        ExecutionErrorPayload {
            node_type: data.get("node_type").and_then(Value::as_str).map(str::to_string),
            exception_message: data.get("exception_message").and_then(Value::as_str).map(str::to_string),
        }
    }
}

// ---------- 队列 / 历史条目访问器 ----------
//
// 队列条目是定长数组 `[number, prompt_id, prompt, extra_data, output_ids]`；
// 历史条目是对象 `{prompt: <同上数组>, outputs: {...}, status: {...}}`。
// 用访问器从 `Value` 里安全取，避免为服务端形状建脆弱的全量结构。

/// 队列条目里取 prompt_id（下标 1）。
pub fn queue_item_prompt_id(item: &Value) -> Option<&str> {
    item.as_array()?.get(1)?.as_str()
}

/// 队列条目里取 prompt 对象（下标 2）。
pub fn queue_item_prompt(item: &Value) -> Option<&Value> {
    item.as_array()?.get(2)
}

/// 队列条目 / 历史条目里取 extra_data（数组下标 3）。
pub fn queue_item_extra(item: &Value) -> Option<&Value> {
    item.as_array()?.get(3)
}

/// extra_data 里的来源标记 `atelyx_origin`；无标记 = 其他客户端提交，返回 null。
pub fn origin_of(extra: Option<&Value>) -> Option<String> {
    let origin = extra?.get("atelyx_origin")?.as_str()?;
    (!origin.is_empty()).then(|| origin.to_string())
}

/// extra_data 里的提交时间 `create_time`（毫秒）。
pub fn create_time_of(extra: Option<&Value>) -> Option<f64> {
    extra?.get("create_time")?.as_f64()
}

/// 历史条目的 prompt 数组（`entry.prompt`）。
pub fn history_prompt(entry: &Value) -> Option<&Value> {
    entry.get("prompt")
}

/// 历史条目的 outputs 对象。
pub fn history_outputs(entry: &Value) -> Option<&Value> {
    entry.get("outputs")
}

/// 历史条目是否标记为出错（`status.status_str == "error"`）。
pub fn history_failed(entry: &Value) -> bool {
    entry
        .get("status")
        .and_then(|s| s.get("status_str"))
        .and_then(Value::as_str)
        == Some("error")
}

/// 从一条输出节点（outputs 里的 value）拍平出图片引用（images + gifs）。
pub fn output_refs(output: &Value) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for key in ["images", "gifs"] {
        if let Some(list) = output.get(key).and_then(Value::as_array) {
            for item in list {
                if let Ok(parsed) = serde_json::from_value::<ImageRef>(item.clone()) {
                    if !parsed.filename.is_empty() {
                        refs.push(parsed);
                    }
                }
            }
        }
    }
    refs
}
