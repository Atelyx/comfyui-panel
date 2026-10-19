//! 托管进程控制器：启动/停止 ComfyUI 进程、就绪裁决、处理宿主回报的进程事件。
//!
//! 就绪判定只看 API 可达（`/system_stats` 2xx），不扫日志——日志里出现端口监听不等于接口可用。
//! 进程事件带 pid，与当前 pid 不符的迟到事件一律忽略，避免上一代进程收尾时误改新一代状态。

use std::time::{Duration, Instant};

use atelyx_plugin_protocol::method::{ProcessEvent, ProcessEventParams};

use crate::app::App;
use crate::comfy::client::ComfyClient;
use crate::launcher;
use crate::settings::{base_url, ComfySettings};

/// 就绪等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// 就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(1500);
/// 错误详情里附带末条 stderr 的截断长度。
const ERROR_DETAIL_MAX: usize = 200;

/// 托管进程的可变状态（存于 Core）。
#[derive(Default)]
pub struct HostState {
    /// 进程由本插件拉起且在跑。
    pub running: bool,
    /// 正在启动并等待就绪。
    pub starting: bool,
    pub pid: Option<u32>,
    /// 展示的启动命令（预览用）。
    pub start_line: String,
    /// 上一次失败原因。
    pub error: String,
    /// 非空表示进程已终结，附原因；就绪循环据此提前收场。
    pub died: String,
    /// 末条 stderr（退出时拼进原因）。
    pub last_stderr: String,
    /// 本次启动是否已弹过失败通知（每轮至多一次）。
    pub notified: bool,
    /// 用户主动停止：进程退出按预期收场，不记为失败。
    pub cancelled: bool,
}

/// API 是否可达（独立于运行时快照，供就绪循环用）。
async fn probe_online(http: &reqwest::Client, settings: &ComfySettings) -> bool {
    ComfyClient::new(base_url(settings), http.clone()).probe().await
}

/// 启动托管进程，返回是否就绪。幂等：已在启动或已运行时直接返回 false。
pub async fn start(app: &std::sync::Arc<App>) -> bool {
    // 检查与占位（置 starting、复位各旗标）在同一把锁内完成，杜绝两个并发 start 都通过
    // 检查、各起一个进程；命令解析失败则不占位，只落错误原因。
    let command = {
        let mut core = app.core();
        if core.host.starting || core.host.pid.is_some() {
            return false;
        }
        let parsed = launcher::start_command(&core.settings);
        match &parsed {
            Ok(_) => {
                core.host.starting = true;
                core.host.start_line =
                    launcher::describe_start_command(&core.settings).unwrap_or_default();
                core.host.error.clear();
                core.host.died.clear();
                core.host.last_stderr.clear();
                core.host.notified = false;
                core.host.cancelled = false;
            }
            Err(err) => core.host.error = err.clone(),
        }
        parsed
    };
    app.invalidate();
    let (program, args, cwd) = match command {
        Ok(triple) => triple,
        // 解析失败已在锁内写入 error，此处只补一次通知（不占位，无需回滚 starting）。
        Err(err) => {
            app.notify("ComfyUI 启动失败", Some(&err));
            return false;
        }
    };

    match app.ctx.spawn(&program, &args, cwd.as_deref(), None).await {
        Ok(pid) => {
            // 取 pid 期间可能已收到退出/取消事件；分段取锁，绝不让 MutexGuard 跨过 await。
            let died_now = {
                let core = app.core();
                !core.host.died.is_empty()
            };
            if died_now {
                let died = app.core().host.died.clone();
                fail_start(app, died);
                return false;
            }
            let cancelled = app.core().host.cancelled;
            if cancelled {
                // 等待 pid 落地期间用户已点停止：立刻回收，不留下游离进程。
                let _ = app.ctx.kill(pid).await;
                return false;
            }
            {
                let mut core = app.core();
                core.host.pid = Some(pid);
                core.host.running = true;
            }
            app.invalidate();
        }
        Err(err) => {
            fail_start(app, err.to_string());
            return false;
        }
    }

    let ready = wait_ready(app).await;
    if ready {
        app.core().host.starting = false;
    } else {
        let (died, cancelled, pid) = {
            let core = app.core();
            (core.host.died.clone(), core.host.cancelled, core.host.pid)
        };
        // 超时未就绪而进程仍在：回收它，否则 pid 卡在原地，用户只能先停止再启动。
        if died.is_empty() && !cancelled {
            if let Some(pid) = pid {
                let _ = app.ctx.kill(pid).await;
            }
        }
        fail_start(app, died);
    }
    app.invalidate();
    ready
}

/// 就绪裁决：反复探测 API 可达；进程中途退出或用户停止则提前失败。
async fn wait_ready(app: &std::sync::Arc<App>) -> bool {
    let started = Instant::now();
    loop {
        {
            let core = app.core();
            if !core.host.died.is_empty() || core.host.cancelled {
                return false;
            }
        }
        let (settings, http) = {
            let core = app.core();
            (core.settings.clone(), app.http.clone())
        };
        if probe_online(&http, &settings).await {
            return true;
        }
        {
            let core = app.core();
            if !core.host.died.is_empty() {
                return false;
            }
        }
        if started.elapsed() >= READY_TIMEOUT {
            return false;
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// 停止托管进程；启动中点击则标记为取消，让就绪循环与退出事件都不计为失败。
pub async fn stop(app: &std::sync::Arc<App>) {
    let pid = {
        let mut core = app.core();
        if core.host.starting {
            core.host.cancelled = true;
        }
        core.host.pid
    };
    let Some(pid) = pid else {
        let mut core = app.core();
        core.host.running = false;
        core.host.starting = false;
        drop(core);
        app.invalidate();
        return;
    };
    if let Err(err) = app.ctx.kill(pid).await {
        app.core().host.error = err.to_string();
    }
    // 回收后落定状态；随后到达的 Terminated 事件因 pid 已清空被忽略。
    let mut core = app.core();
    core.host.pid = None;
    core.host.running = false;
    core.host.starting = false;
    drop(core);
    app.invalidate();
}

/// 启动失败收口：写原因并按去重弹一次通知（用户主动停止不算失败）。
/// 收口后不再持有托管进程——`pid`/`running` 一并清掉，「启动」按钮才不会被卡住。
fn fail_start(app: &std::sync::Arc<App>, message: String) {
    let mut core = app.core();
    if core.host.cancelled {
        core.host.starting = false;
        drop(core);
        app.invalidate();
        return;
    }
    let message = if message.is_empty() {
        format!("启动后 {}s 内服务未就绪", READY_TIMEOUT.as_secs())
    } else {
        message
    };
    core.host.starting = false;
    core.host.pid = None;
    core.host.running = false;
    core.host.error = message.clone();
    let should_notify = !core.host.notified;
    core.host.notified = true;
    drop(core);
    app.invalidate();
    if should_notify {
        app.notify("ComfyUI 启动失败", Some(&message));
    }
}

/// 宿主回报的进程事件：按 pid 归属过滤后处理日志尾、退出定论、错误。
pub async fn on_process_event(app: &std::sync::Arc<App>, params: ProcessEventParams) {
    if Some(params.pid) != app.core().host.pid {
        return; // 上一代进程的迟到事件
    }
    match params.event {
        ProcessEvent::Stdout { .. } => {}
        ProcessEvent::Stderr { data } => {
            let line = data.trim_end();
            if !line.is_empty() {
                app.core().host.last_stderr = line.to_string();
            }
        }
        ProcessEvent::Terminated { code } => {
            let (message, starting) = {
                let mut core = app.core();
                core.host.pid = None;
                core.host.running = false;
                let detail = if core.host.last_stderr.is_empty() {
                    String::new()
                } else {
                    format!("：{}", truncate(&core.host.last_stderr, ERROR_DETAIL_MAX))
                };
                let message = format!(
                    "进程已退出（退出码 {}）{detail}",
                    code.map(|c| c.to_string()).unwrap_or_else(|| "未知".into())
                );
                core.host.died = message.clone();
                // 启动阶段的退出由 fail_start 统一弹通知，已就绪后才退出的没人收口，在此补一次。
                (message, core.host.starting)
            };
            app.invalidate();
            if !starting {
                app.notify("ComfyUI 进程已退出", Some(&message));
            }
        }
        ProcessEvent::Error { .. } => {
            // 非终止信号：读管道失败或某行非 UTF8 时宿主逐次上报此事件，进程仍在运行。
            // 真正的结束只由 Terminated 决定，故此处不改 pid/running/died，否则会把存活进程
            // 误判为已退出，令下一次启动与本进程并存。
        }
    }
}

/// 按字符截断，尾部加省略号。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}
