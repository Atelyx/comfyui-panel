//! 托管进程控制器：启动/停止 ComfyUI 进程、就绪裁决、一键更新、处理宿主回报的进程事件。
//!
//! 就绪判定只看 API 可达（`/system_stats` 2xx），不扫日志——日志里出现端口监听不等于接口可用。
//! 进程事件带 pid，与当前 pid 不符的迟到事件一律忽略；启动期 pid 未定前的事件先按序暂存，
//! pid 落地后补发，避免快速失败进程的退出码与 stderr 被丢掉。
//! 更新在安装目录执行 `git pull --ff-only`：托管服务先停、更新后按需重启，非 git 安装直接报可读原因。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use atelyx_plugin_protocol::method::{NotificationLevel, ProcessEvent, ProcessEventParams};

use crate::app::App;
use crate::comfy::client::ComfyClient;
use crate::launcher;
use crate::settings::{base_url, ComfySettings, ProcessMode};

/// 就绪等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// 就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(1500);
/// 错误详情里附带末条 stderr 的截断长度。
const ERROR_DETAIL_MAX: usize = 200;
/// 代码更新（git pull）等待上限。
const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);
/// 依赖安装等待上限（可能要下载较大的包）。
const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(1800);
/// 更新进程状态轮询间隔。
const UPDATE_POLL: Duration = Duration::from_millis(250);
/// 更新日志保留的字符上限（只留尾部，供界面显示）。
const UPDATE_LOG_MAX: usize = 4000;
/// 启动期暂存的进程事件上限。
const PENDING_EVENTS_MAX: usize = 64;

/// 托管进程的可变状态（存于 Core）。
#[derive(Default)]
pub struct HostState {
    /// 进程由本插件拉起且在跑。
    pub running: bool,
    /// 正在启动并等待就绪。
    pub starting: bool,
    /// 正在停止：Terminated 按预期收场，不弹「自行退出」通知。
    pub stopping: bool,
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
    /// 上一代进程 pid：启动期 pid 未定时用来丢弃它的迟到事件。
    pub last_pid: Option<u32>,
    /// 启动期 pid 未定前暂存的进程事件；pid 落地后补发。
    pub pending_events: Vec<ProcessEventParams>,
    /// 正在执行一键更新。
    pub updating: bool,
    /// 更新进程 pid（与 ComfyUI 进程分开记账）。
    pub update_pid: Option<u32>,
    /// 更新进程 pid 未定前暂存的事件；pid 落地后补发。
    pub pending_update_events: Vec<ProcessEventParams>,
    /// 更新输出（尾部截断）。
    pub update_log: String,
    /// 更新失败原因。
    pub update_error: String,
    /// 更新进程最近一次退出是否成功（None = 尚未退出）。
    pub update_ok: Option<bool>,
    /// 更新成功提示。
    pub update_notice: String,
    /// 更新前服务是否在运行（更新后据此重启）。
    pub update_was_running: bool,
}

/// API 是否可达（独立于运行时快照，供就绪循环用）。
async fn probe_online(http: &reqwest::Client, settings: &ComfySettings) -> bool {
    ComfyClient::new(base_url(settings), http.clone()).probe().await
}

/// 启动托管进程，返回是否就绪。幂等：已在启动、已运行或正在更新时直接返回 false。
pub async fn start(app: &std::sync::Arc<App>) -> bool {
    // 检查与占位（置 starting、复位各旗标）在同一把锁内完成，杜绝两个并发 start 都通过
    // 检查、各起一个进程；命令解析失败则不占位，只落错误原因。
    let command = {
        let mut core = app.core();
        if core.host.updating || core.host.starting || core.host.pid.is_some() {
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
                core.host.stopping = false;
                core.host.pending_events.clear();
                core.host.pending_update_events.clear();
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
            app.ctx.log_error(&format!("ComfyUI 启动失败：{err}"));
            app.notify(NotificationLevel::Error, "ComfyUI 启动失败", Some(&err));
            return false;
        }
    };

    match app.ctx.spawn(&program, &args, cwd.as_deref(), None).await {
        Ok(pid) => {
            // 宿主在 spawn 应答前就可能补发该进程的早期事件；pid 落地后按序补发再判断。
            let pending = {
                let mut core = app.core();
                core.host.pid = Some(pid);
                core.host.running = true;
                core.host.last_pid = None;
                std::mem::take(&mut core.host.pending_events)
            };
            for event in pending {
                apply_process_event(app, event);
            }
            let (died, cancelled) = {
                let core = app.core();
                (core.host.died.clone(), core.host.cancelled)
            };
            if !died.is_empty() {
                fail_start(app, died);
                return false;
            }
            if cancelled {
                // 等待 pid 落地期间用户已点取消：立刻回收，不留游离进程，并复位占位旗标。
                if let Err(err) = app.ctx.kill(pid).await {
                    app.ctx.log_error(&format!("取消启动时结束进程 {pid} 失败：{err}"));
                }
                let mut core = app.core();
                core.host.starting = false;
                core.host.stopping = false;
                core.host.pid = None;
                core.host.running = false;
                drop(core);
                app.invalidate();
                return false;
            }
            app.invalidate();
        }
        Err(err) => {
            app.ctx.log_error(&format!("拉起 ComfyUI 进程失败：{err}"));
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
        let mut kill_failed = false;
        if died.is_empty() && !cancelled {
            if let Some(pid) = pid {
                if let Err(err) = app.ctx.kill(pid).await {
                    app.ctx.log_error(&format!("未就绪超时后结束进程 {pid} 失败：{err}"));
                    kill_failed = true;
                }
            }
        }
        if kill_failed {
            // 进程可能还在跑：保留 pid/running，让「停止服务」能继续收尾。
            let mut core = app.core();
            core.host.starting = false;
            core.host.error = "启动超时且无法结束进程，请手动停止".to_string();
            let should_notify = !core.host.notified;
            core.host.notified = true;
            drop(core);
            app.invalidate();
            if should_notify {
                app.notify(
                    NotificationLevel::Error,
                    "ComfyUI 启动失败",
                    Some("启动超时且无法结束进程，请手动停止"),
                );
            }
            return false;
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

/// 停止托管进程。启动中点击则标记为取消，让就绪循环与退出事件都不计为失败。
/// 结束失败时保留 pid/running（进程可能还在跑），让用户能再次停止。
pub async fn stop(app: &std::sync::Arc<App>) -> Result<(), String> {
    let pid = {
        let mut core = app.core();
        core.host.stopping = true;
        if core.host.starting {
            core.host.cancelled = true;
            // spawn 还在途（pid 未落地）：保留 starting 占位防重复启动，由 start 的取消分支收场
            if core.host.pid.is_none() {
                drop(core);
                app.invalidate();
                return Ok(());
            }
        }
        core.host.pid
    };
    let Some(pid) = pid else {
        // 空闲态的防御性复位：没有进程可停，把旗标归零
        let mut core = app.core();
        core.host.running = false;
        core.host.starting = false;
        core.host.stopping = false;
        drop(core);
        app.invalidate();
        return Ok(());
    };
    if let Err(err) = app.ctx.kill(pid).await {
        let message = err.to_string();
        // 宿主已因 Terminated 摘除记账：进程已经不在，按成功收口。
        if message.contains("不是本插件启动") {
            let mut core = app.core();
            core.host.pid = None;
            core.host.running = false;
            core.host.starting = false;
            core.host.stopping = false;
            drop(core);
            app.invalidate();
            return Ok(());
        }
        app.ctx.log_error(&format!("结束 ComfyUI 进程 {pid} 失败：{message}"));
        let mut core = app.core();
        core.host.stopping = false;
        core.host.error = format!("结束进程失败：{message}");
        drop(core);
        app.invalidate();
        app.notify(NotificationLevel::Error, "ComfyUI 停止失败", Some(&message));
        return Err(message);
    }
    // 回收后落定状态；随后到达的 Terminated 事件因 pid 已清空被忽略。
    let mut core = app.core();
    core.host.last_pid = core.host.pid;
    core.host.pid = None;
    core.host.running = false;
    core.host.starting = false;
    core.host.stopping = false;
    core.host.pending_events.clear();
    drop(core);
    app.invalidate();
    Ok(())
}

/// 启动失败收口：写原因并按去重弹一次通知（用户主动停止不算失败）。
/// 收口后不再持有托管进程——`pid`/`running` 一并清掉，「启动」按钮才不会被卡住。
fn fail_start(app: &std::sync::Arc<App>, message: String) {
    let mut core = app.core();
    core.host.pending_events.clear();
    if core.host.cancelled {
        core.host.starting = false;
        core.host.stopping = false;
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
    core.host.stopping = false;
    core.host.pid = None;
    core.host.running = false;
    core.host.error = message.clone();
    let should_notify = !core.host.notified;
    core.host.notified = true;
    drop(core);
    app.ctx.log_error(&format!("ComfyUI 启动失败：{message}"));
    app.invalidate();
    if should_notify {
        app.notify(NotificationLevel::Error, "ComfyUI 启动失败", Some(&message));
    }
}

/// 一键更新：必要时停托管服务 → `git pull --ff-only` → `pip install -r requirements.txt` → 按需重启。
/// 更新进程与 ComfyUI 进程分开记账，事件在 [`on_process_event`] 里按 pid 分流。
pub async fn update(app: &std::sync::Arc<App>) {
    let settings = {
        let mut core = app.core();
        if core.host.updating {
            return;
        }
        if core.host.starting {
            core.host.update_notice.clear();
            core.host.update_error = "正在启动服务，请稍后再更新".to_string();
            drop(core);
            app.invalidate();
            return;
        }
        if core.settings.comfy_dir.trim().is_empty() {
            core.host.update_notice.clear();
            core.host.update_error = "未配置 ComfyUI 安装目录".to_string();
            drop(core);
            app.invalidate();
            return;
        }
        core.host.updating = true;
        core.host.update_ok = None;
        core.host.update_pid = None;
        core.host.pending_update_events.clear();
        core.host.update_log.clear();
        core.host.update_error.clear();
        core.host.update_notice.clear();
        core.host.update_was_running = core.host.running || core.host.pid.is_some();
        core.settings.clone()
    };
    app.invalidate();

    let was_running = { app.core().host.update_was_running };
    if was_running {
        if let Err(err) = stop(app).await {
            let mut core = app.core();
            core.host.updating = false;
            core.host.update_ok = Some(false);
            core.host.pending_update_events.clear();
            core.host.update_error = format!("停止服务失败，已取消更新：{err}");
            drop(core);
            app.invalidate();
            return;
        }
    }

    let mut result = match launcher::update_command(&settings) {
        Ok((program, args, cwd, env)) => {
            run_update_process(app, &program, &args, cwd.as_deref(), &env, UPDATE_TIMEOUT, "git pull").await
        }
        Err(err) => Err(err),
    };
    if result.is_ok() {
        match launcher::dependency_command(&settings) {
            Ok((program, args, cwd, env)) => {
                result = run_update_process(
                    app,
                    &program,
                    &args,
                    cwd.as_deref(),
                    &env,
                    DEPENDENCY_TIMEOUT,
                    "更新依赖",
                )
                .await;
            }
            Err(err) => result = Err(err),
        }
    }

    let ok = result.is_ok();
    let error = result.err().unwrap_or_default();
    {
        let mut core = app.core();
        core.host.updating = false;
        core.host.update_pid = None;
        core.host.pending_update_events.clear();
        core.host.update_error = error.clone();
        if ok {
            core.host.update_notice = "更新完成".to_string();
        }
        drop(core);
    }
    app.invalidate();

    let managed_now = app.core().settings.process_mode == ProcessMode::Managed;
    if was_running && managed_now {
        let restarted = start(app).await;
        let mut core = app.core();
        if restarted {
            core.host.update_notice = if ok {
                "更新完成，服务已重启".to_string()
            } else {
                "更新失败，服务已按原配置重启".to_string()
            };
        } else if error.is_empty() {
            core.host.update_error = "更新后服务重启失败".to_string();
        } else {
            core.host.update_error = format!("{error}；服务重启失败");
        }
        drop(core);
        app.invalidate();
    }
}

/// 跑一段更新命令并等它结束；输出进 update_log，退出码进 update_ok。
async fn run_update_process(
    app: &std::sync::Arc<App>,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
    timeout: Duration,
    label: &str,
) -> Result<(), String> {
    push_update_log(app, &format!("== {label} =="));
    {
        let mut core = app.core();
        core.host.updating = true;
        core.host.update_ok = None;
        core.host.update_pid = None;
        core.host.pending_update_events.clear();
        core.host.update_error.clear();
    }
    app.invalidate();
    let pid = match app.ctx.spawn(program, args, cwd, Some(env)).await {
        Ok(pid) => pid,
        Err(err) => {
            let message = format!("启动 {label} 失败：{err}");
            finish_update_error(app, message.clone());
            return Err(message);
        }
    };
    // 与 ComfyUI 启动同理：宿主可能在应答前补发进程的早期事件，先暂存后补发。
    let pending = {
        let mut core = app.core();
        core.host.update_pid = Some(pid);
        std::mem::take(&mut core.host.pending_update_events)
    };
    for event in pending {
        handle_update_event(app, event);
    }
    app.invalidate();
    let started = Instant::now();
    loop {
        let (updating, current_pid) = {
            let core = app.core();
            (core.host.updating, core.host.update_pid)
        };
        if !updating {
            break;
        }
        if started.elapsed() >= timeout {
            if let Some(pid) = current_pid {
                let _ = app.ctx.kill(pid).await;
            }
            let message = format!("{label} 超时（{}s），已结束进程", timeout.as_secs());
            let mut core = app.core();
            core.host.updating = false;
            core.host.update_pid = None;
            core.host.pending_update_events.clear();
            core.host.update_error = message.clone();
            drop(core);
            app.invalidate();
            return Err(message);
        }
        tokio::time::sleep(UPDATE_POLL).await;
    }
    let (ok, error) = {
        let core = app.core();
        (core.host.update_ok, core.host.update_error.clone())
    };
    if ok == Some(true) {
        Ok(())
    } else if error.is_empty() {
        Err(format!("{label} 失败"))
    } else {
        Err(format!("{label}：{error}"))
    }
}

/// 更新在启动 git 前就失败：只落原因，不再等进程事件。
fn finish_update_error(app: &std::sync::Arc<App>, message: String) {
    let mut core = app.core();
    core.host.updating = false;
    core.host.update_ok = Some(false);
    core.host.update_pid = None;
    core.host.pending_update_events.clear();
    core.host.update_error = message;
    drop(core);
    app.invalidate();
}

/// 宿主回报的进程事件：更新进程与 ComfyUI 进程按 pid 分流；启动期 pid 未定的事件先暂存。
pub async fn on_process_event(app: &std::sync::Arc<App>, params: ProcessEventParams) {
    let (update_pid, pid, starting) = {
        let core = app.core();
        (core.host.update_pid, core.host.pid, core.host.starting)
    };
    if update_pid == Some(params.pid) {
        handle_update_event(app, params);
        return;
    }
    if pid == Some(params.pid) {
        apply_process_event(app, params);
        return;
    }
    if starting && pid.is_none() {
        let mut core = app.core();
        // 上一代进程的迟到事件不能算到新进程头上。
        if core.host.last_pid == Some(params.pid) {
            return;
        }
        if core.host.pending_events.len() >= PENDING_EVENTS_MAX {
            core.host.pending_events.remove(0);
        }
        core.host.pending_events.push(params);
        return;
    }
    // git 进程 pid 落地前的事件同理：先暂存，spawn 应答后补发。
    let updating = app.core().host.updating;
    if updating && update_pid.is_none() {
        let mut core = app.core();
        if core.host.last_pid == Some(params.pid) {
            return;
        }
        if core.host.pending_update_events.len() >= PENDING_EVENTS_MAX {
            core.host.pending_update_events.remove(0);
        }
        core.host.pending_update_events.push(params);
    }
}

/// 更新进程输出与退出：只记日志与结果，不参与 ComfyUI 的 died/notify 语义。
fn handle_update_event(app: &std::sync::Arc<App>, params: ProcessEventParams) {
    match params.event {
        ProcessEvent::Stdout { data } | ProcessEvent::Stderr { data } => {
            push_update_log(app, &data);
            app.invalidate();
        }
        ProcessEvent::Terminated { code } => {
            let ok = code == Some(0);
            let mut core = app.core();
            core.host.update_pid = None;
            core.host.updating = false;
            core.host.update_ok = Some(ok);
            if ok {
                core.host.update_error.clear();
            } else {
                core.host.update_error = format!(
                    "进程退出码 {}",
                    code.map(|code| code.to_string()).unwrap_or_else(|| "未知".to_string())
                );
            }
            drop(core);
            app.invalidate();
        }
        // 管道读失败不等于进程结束：结束只由 Terminated 定论。
        ProcessEvent::Error { .. } => {}
    }
}

/// 追加更新日志（尾部截断），供设置页显示。
fn push_update_log(app: &std::sync::Arc<App>, data: &str) {
    let line = data.trim_end();
    if line.is_empty() {
        return;
    }
    let mut core = app.core();
    if !core.host.update_log.is_empty() {
        core.host.update_log.push('\n');
    }
    core.host.update_log.push_str(line);
    let length = core.host.update_log.chars().count();
    if length > UPDATE_LOG_MAX {
        core.host.update_log = core.host.update_log.chars().skip(length - UPDATE_LOG_MAX).collect();
    }
}

/// ComfyUI 进程事件落地：日志尾、退出定论、错误。
fn apply_process_event(app: &std::sync::Arc<App>, params: ProcessEventParams) {
    match params.event {
        ProcessEvent::Stdout { .. } => {}
        ProcessEvent::Stderr { data } => {
            let line = data.trim_end();
            if !line.is_empty() {
                app.core().host.last_stderr = line.to_string();
            }
        }
        ProcessEvent::Terminated { code } => {
            let (message, starting, stopping) = {
                let mut core = app.core();
                core.host.last_pid = core.host.pid;
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
                // 启动阶段的退出由 fail_start 统一弹通知；已就绪后的自行退出在此补一次。
                // 用户主动停止（stopping / cancelled）按预期收场，不弹错误。
                (message, core.host.starting, core.host.stopping)
            };
            app.invalidate();
            if !starting && !stopping {
                app.ctx.log_error(&format!("ComfyUI 进程已退出：{message}"));
                app.notify(NotificationLevel::Error, "ComfyUI 进程已退出", Some(&message));
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
