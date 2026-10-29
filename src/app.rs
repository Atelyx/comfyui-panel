//! 应用装配与编排：持状态、驱动轮询/WS/渲染三条常驻任务，把界面事件翻成状态迁移与 IO。
//!
//! Elm 单向数据流：`Core` 是唯一真源，任何变更经 `invalidate` 递增版本号，界面驱动据此重提新帧。
//! 约束——绝不持 `MutexGuard` 跨 `await`：需要 IO 时先在锁内取出输入（克隆）、释放锁、`await`
//! 网络/磁盘、再重新加锁写回，慢 IO 因此不卡住整棵重绘。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use atelyx_plugin_sdk::frame_ui::FrameUi;
use atelyx_plugin_sdk::protocol::method::{
    ContributionSlot, FileFilter, NotificationLevel, PaddingSpec, PickFileParams, SurfaceAttachedParams,
    SurfaceDetachedParams,
};
use atelyx_plugin_sdk::Context;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::comfy::types::ImageRef;
use crate::comfy::ws::{run as ws_run, WsEvent};
use crate::controller::{self, HostState};
use crate::gallery;
use crate::history::{ref_copy_name, ref_path, CopyStore, RefCache, ResultImage, HISTORY_LIMIT};
use crate::runtime::{self, Channel, Runtime};
use crate::settings::{base_url, ComfySettings, ProcessMode};
use crate::views::{self, P_BOOL, P_CLEAR, P_COMBO, P_NUM, P_PASTE, P_SEED, P_TEXT, P_UPLOAD};
use crate::workflow::files;
use crate::workflow::form;
use crate::workflow::sizing;

/// 生成面板贡献 id（同时是挂载后的 surfaceId，不得含 `:`）。
pub const SURFACE_PANEL: &str = "generate";
/// 设置页贡献 id。
pub const SURFACE_SETTINGS: &str = "comfyui";

/// 连接/队列/进度轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(2000);
/// 工作流清单轮询间隔（探测浏览器侧外部改动）。
const WORKFLOW_INTERVAL: Duration = Duration::from_millis(3000);
/// WS 退避用尽后被动断开的重连间隔。
const WS_RECONNECT: Duration = Duration::from_millis(4000);
/// 状态落盘防抖：合并短时间内的连续变更（打字、连点）。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);
/// 每轮补抓缺口的最大条数（限制抓取重试的开销，优先补最近的结果）。
const RECAPTURE_BATCH: usize = 4;
/// 记录流首屏展示的记录批次数（一批 = 一个 promptId 的同批产出）。
const FLOW_BATCH_INIT: usize = 8;
/// 「显示更早」每次追加的批次数。
const FLOW_BATCH_STEP: usize = 8;

/// 选择浮层的一个候选：标签、是否当前值、选中后执行的动作。
#[derive(Clone, Debug, PartialEq)]
pub struct PickerOption {
    pub label: String,
    pub current: bool,
    pub action: PickerAction,
}

/// 选择浮层的动作：携带稳定的目标标识（路径 / 键 / 值），执行时按当前状态回查，避免下标漂移。
#[derive(Clone, Debug, PartialEq)]
pub enum PickerAction {
    Workflow(String),
    Size { section: &'static str, value: String },
    PromptAuto,
    PromptField(String),
    RefImage { key: String, name: String },
    Combo { key: String, submit: String },
}

/// 面内选择浮层的当前内容。
#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub title: String,
    pub options: Vec<PickerOption>,
}

/// 界面之外的会话态：当前工作流、参数草稿、随机种子勾选、记录流与生成坞的展开态、参考图缩略、
/// 灯箱与悬停、选择浮层。不落盘。
#[derive(Default)]
pub struct Session {
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_modified: Option<f64>,
    pub draft: Value,
    pub seed_mode: BTreeSet<String>,
    pub current_changed: bool,
    pub notice: String,
    /// 记录流灯箱当前展示的结果 key（面内行内面板）。
    pub lightbox_key: Option<String>,
    /// 面内选择面板：工作流、尺寸三段、提示词来源、参考图与字段下拉共用。
    pub picker: Option<Picker>,
    /// 上一次提交的记录流布局：命中 id → 动作的回查依据，用户点的就是他刚看到的那一版。
    pub flow: Option<Arc<gallery::FlowLayout>>,
    /// 记录流已展示的记录批次数，「显示更早」递增。
    pub shown_batches: usize,
    /// 用户是否停在记录流末端（滚动区回发的滚动位置）：只有看最新一屏时新结果才贴底。
    pub flow_at_bottom: bool,
    pub params_open: bool,
    pub param_filter: String,
    /// 节点分组展开覆盖项：未记录的节点按「有没有常用参数」决定默认展开。
    pub group_toggles: BTreeMap<String, bool>,
    /// 主提示词字段手选键（`节点id.输入名`），缺省按自动推断。
    pub prompt_key: Option<String>,
    /// 参考图缩略副本缓存（私有目录 `refs/`）。
    pub refs: RefCache,
}

/// 状态真源。
pub struct Core {
    pub settings: ComfySettings,
    pub attached: BTreeSet<String>,
    pub runtime: Runtime,
    pub host: HostState,
    pub session: Session,
}

/// 一次 WS 连接的关停把手：标志供读任务自检退避，句柄供监督任务在空闲期直接中止。
struct WsHandle {
    shutdown: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

/// 插件应用：跨任务共享，一律经 `Arc<App>` 持有。
pub struct App {
    pub(crate) ctx: Arc<Context>,
    core: Mutex<Core>,
    version: watch::Sender<u64>,
    pub(crate) http: reqwest::Client,
    store: CopyStore,
    /// 生成面板的控件层（交互态 / 量算缓存跨帧保留），面重新挂载时重建。
    pub(crate) panel_ui: tokio::sync::Mutex<FrameUi>,
    /// 设置面的控件层。
    pub(crate) settings_ui: tokio::sync::Mutex<FrameUi>,
    /// 当前连接的关停把手：改地址/服务下线时置关停标志并中止读任务——空闲无帧时读循环
    /// 正 await 下一帧，只有中止能让它立刻收场。
    ws_control: Mutex<Option<WsHandle>>,
    /// 落盘防抖代数：每次调度自增，仅最新一次真正写。
    save_gen: AtomicU64,
}

impl App {
    // ---------- 生命周期 ----------

    /// 读持久化状态 → 建 Core → 起三条常驻任务。返回尚未注册贡献点，留给 [`App::after_ready`]。
    pub async fn bootstrap(ctx: Arc<Context>) -> Arc<App> {
        let raw = ctx.state_read().await.unwrap_or(Value::Null);
        let settings = crate::settings::from_value(raw.get("settings"));
        let origin_id = raw.get("originId").and_then(Value::as_str).unwrap_or("").to_string();
        let history: Vec<ResultImage> = raw
            .get("history")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
            .unwrap_or_default();

        let private = ctx.private_dir().await.unwrap_or_else(|_| "data".into());
        let store = CopyStore::new(private);
        store.ensure_dir();
        // 缩略缓存不跨进程：上一代留下的参考图副本已无人认领，装载时清掉，避免私有目录只增不减。
        store.clear_refs();

        let mut runtime = Runtime::new();
        runtime.load_persisted(origin_id, history);

        let core = Core {
            settings,
            attached: BTreeSet::new(),
            runtime,
            host: HostState::default(),
            session: Session {
                draft: json!({}),
                shown_batches: FLOW_BATCH_INIT,
                flow_at_bottom: true,
                ..Default::default()
            },
        };

        let (version_tx, _) = watch::channel(0u64);
        let app = Arc::new(App {
            ctx,
            core: Mutex::new(core),
            version: version_tx,
            http: reqwest::Client::new(),
            store,
            panel_ui: tokio::sync::Mutex::new(FrameUi::new()),
            settings_ui: tokio::sync::Mutex::new(FrameUi::new()),
            ws_control: Mutex::new(None),
            save_gen: AtomicU64::new(0),
        });

        tokio::spawn(crate::ui::render_loop(app.clone()));
        tokio::spawn(App::poll_loop(app.clone()));
        tokio::spawn(App::workflow_poll_loop(app.clone()));
        tokio::spawn(App::ws_loop(app.clone()));
        app
    }

    /// 注册贡献点、跑首轮刷新。
    pub async fn after_ready(self: &Arc<App>) {
        // 面外壳内边距归插件自己控制（零外壳 + 根列 12），避免与宿主默认内边距叠加。
        let pad = Some(PaddingSpec::All(0.0));
        let _ = self.ctx.register(ContributionSlot::Panel, SURFACE_PANEL, "ComfyUI 生成", Some("sparkles"), pad).await;
        let _ = self.ctx.register(ContributionSlot::Settings, SURFACE_SETTINGS, "ComfyUI", Some("server"), pad).await;
        // Agent 工具：旧宿主没有这个方法时静默降级（工具不出现，面板功能不受影响）
        crate::agent::register(&self.ctx).await;

        self.run_poll_cycle().await;
        self.refresh_workflows_cycle().await;
    }

    // ---------- 状态访问 ----------

    pub(crate) fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn invalidate(&self) {
        let next = *self.version.borrow() + 1;
        let _ = self.version.send(next);
    }

    /// 状态版本通道：任一变更经 [`App::invalidate`] 触达界面驱动。
    pub(crate) fn changes(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    /// 弹一条宿主通知：尽力而为，失败静默（通知非关键路径）。
    pub(crate) fn notify(&self, level: NotificationLevel, title: &str, body: Option<&str>) {
        let ctx = self.ctx.clone();
        let title = title.to_string();
        let body = body.map(str::to_string);
        tokio::spawn(async move {
            let _ = ctx.notify(&title, body.as_deref(), level).await;
        });
    }

    /// 在锁外取 IO 输入（地址随设置变，故每次克隆当前 settings）。
    fn io_ctx(&self) -> (ComfySettings, reqwest::Client) {
        let core = self.core();
        (core.settings.clone(), self.http.clone())
    }

    // ---------- 轮询 ----------

    async fn poll_loop(app: Arc<App>) {
        loop {
            app.run_poll_cycle().await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn run_poll_cycle(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        // 探测态只在「不在连接态」时点亮：稳态轮询不打翻转「已连接/连接中」，状态条不闪。
        {
            let mut core = self.core();
            if core.runtime.snap.channel != Channel::Direct {
                core.runtime.set_probing(true);
                self.invalidate();
            }
        }
        let channel = runtime::fetch_probe(&settings, &http).await;
        {
            let mut core = self.core();
            core.runtime.apply_probe(channel);
            if channel != Channel::Direct {
                core.runtime.disconnect();
            }
        }
        self.invalidate();
        if channel != Channel::Direct {
            return;
        }

        self.refresh_queue().await;
        self.refresh_history().await;
        self.refresh_system_stats().await;
        if self.core().runtime.snap.object_info.is_none() {
            self.refresh_object_info().await;
        }
        self.invalidate();
    }

    async fn refresh_queue(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        if let Ok(queue) = runtime::fetch_queue(&settings, &http).await {
            self.core().runtime.apply_queue(&queue);
        }
    }

    async fn refresh_system_stats(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        let stats = runtime::fetch_system_stats(&settings, &http).await;
        self.core().runtime.apply_system_stats(stats);
    }

    async fn refresh_object_info(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        let info = runtime::fetch_object_info(&settings, &http).await;
        self.core().runtime.apply_object_info(info);
    }

    /// 取新 → 释放锁抓副本 → 重新加锁并入；随后补抓上一轮抓取失败的缺口。
    async fn refresh_history(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        let Ok(history) = runtime::fetch_history(&settings, &http).await else {
            return; // 历史刷新失败静默：通道字段已表达连接存活
        };
        let mut fresh = { self.core().runtime.take_fresh_results(&history) };
        if !fresh.is_empty() {
            runtime::capture_copies(&mut fresh, &settings, &http, &self.store).await;
            let merged = self.core().runtime.merge_results(fresh, &self.store);
            if merged {
                self.invalidate();
                self.schedule_save();
            }
        }

        // 补抓：上一轮抓取因网络/写盘抖动失败会留下缺副本的条目，按最近若干条重试，抓不到的若干轮后放弃。
        let mut pending = self.core().runtime.take_uncopied(RECAPTURE_BATCH);
        if !pending.is_empty() {
            runtime::recapture_copies(&mut pending, &settings, &http, &self.store).await;
            if self.core().runtime.apply_recapture(&pending) > 0 {
                self.invalidate();
                self.schedule_save();
            }
        }
    }

    async fn workflow_poll_loop(app: Arc<App>) {
        loop {
            app.refresh_workflows_cycle().await;
            tokio::time::sleep(WORKFLOW_INTERVAL).await;
        }
    }

    async fn refresh_workflows_cycle(self: &Arc<App>) {
        if self.core().runtime.snap.channel != Channel::Direct {
            return; // 离线时列目录必然失败，不刷错误
        }
        let (settings, http) = self.io_ctx();
        let result = runtime::fetch_workflows(&settings, &http).await;
        let changed = {
            let mut core = self.core();
            let changed = core.runtime.apply_workflows(result);
            if changed {
                if let Some(id) = core.session.workflow_id.clone() {
                    core.session.current_changed = match core.runtime.snap.workflows.iter().find(|f| f.path == id) {
                        Some(file) => core.session.workflow_modified != Some(file.modified),
                        None => true, // 文件被删：视同外部改动，提示重新载入
                    };
                }
            }
            changed
        };
        if changed {
            self.invalidate();
        }
    }

    // ---------- WebSocket ----------

    /// 连接监督循环：每次连接建独立事件通道，断开后按原因决定重连节奏。
    async fn ws_loop(app: Arc<App>) {
        loop {
            let url = {
                let core = app.core();
                runtime::socket_url(&core.settings, &core.runtime.book.client_id, &app.http)
            };
            let shutdown = Arc::new(AtomicBool::new(false));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(ws_run(url, tx, shutdown.clone()));
            *app.ws_control.lock().unwrap() = Some(WsHandle { shutdown: shutdown.clone(), task });

            // 发送端只此一份，随读任务收场而 drop：rx 返回 None 即本次连接结束。
            while let Some(event) = rx.recv().await {
                if matches!(event, WsEvent::Disconnected) {
                    break;
                }
                app.handle_ws_event(event).await;
            }
            let handle = app.ws_control.lock().unwrap().take();
            if let Some(handle) = handle {
                let _ = handle.task.await;
            }

            // 被改地址/服务下线置换（shutdown=true）：立即用新设置重连，无需退避。
            if !shutdown.load(Ordering::Relaxed) {
                tokio::time::sleep(WS_RECONNECT).await;
            }
        }
    }

    async fn handle_ws_event(self: &Arc<App>, event: WsEvent) {
        match event {
            WsEvent::Progress(progress) => {
                self.core().runtime.apply_ws_progress(progress);
                self.invalidate();
            }
            WsEvent::Executing(executing) => {
                let ended = { self.core().runtime.apply_ws_executing(executing) };
                self.invalidate();
                if ended {
                    self.finish_run().await;
                }
            }
            WsEvent::ExecutionEnd => {
                let ended = { self.core().runtime.apply_execution_end() };
                self.invalidate();
                if ended {
                    self.finish_run().await;
                }
            }
            WsEvent::ExecutionError(error) => {
                self.core().runtime.apply_ws_error(error);
                self.refresh_history().await;
                self.invalidate();
            }
            WsEvent::QueueSignal => {
                self.refresh_queue().await;
                self.invalidate();
            }
            WsEvent::Disconnected => {}
        }
    }

    /// 一轮执行收场：刷新队列与历史，让刚生成的结果立刻进画廊。
    async fn finish_run(self: &Arc<App>) {
        self.refresh_queue().await;
        self.refresh_history().await;
        self.invalidate();
    }

    /// 改地址/服务下线：置关停标志并中止读任务，监督循环随即用新设置重连。
    fn restart_ws(&self) {
        if let Some(handle) = self.ws_control.lock().unwrap().as_ref() {
            handle.shutdown.store(true, Ordering::Relaxed);
            handle.task.abort();
        }
    }

    // ---------- 状态落盘 ----------

    fn schedule_save(self: &Arc<App>) {
        let gen = self.save_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SAVE_DEBOUNCE).await;
            if app.save_gen.load(Ordering::SeqCst) != gen {
                return; // 期间又有变更，交给最新那次落盘
            }
            let envelope = build_envelope(&app.core());
            let _ = app.ctx.state_write(envelope).await;
        });
    }

    // ---------- 界面动作 ----------

    /// 关闭灯箱。
    pub(crate) fn close_lightbox(self: &Arc<App>) {
        self.core().session.lightbox_key = None;
        self.invalidate();
    }

    /// 设置字段的文本值写回：地址 / 端口改动会重置连接状态并重连。
    pub(crate) fn apply_settings_text(self: &Arc<App>, id: &str, text: &str) {
        let reconnect = id == views::SET_HOST || id == views::SET_PORT;
        {
            let mut core = self.core();
            match id {
                views::SET_HOST => {
                    let text = text.trim().to_string();
                    core.settings.host = if text.is_empty() { ComfySettings::default().host } else { text };
                }
                views::SET_PORT => {
                    if let Ok(port) = text.trim().parse::<u32>() {
                        if let Ok(port) = u16::try_from(port.clamp(1, 65535)) {
                            core.settings.port = port;
                        }
                    }
                }
                views::SET_DIR => core.settings.comfy_dir = text.to_string(),
                views::SET_PYTHON => core.settings.python_path = text.to_string(),
                views::SET_EXTRA => core.settings.extra_args = text.to_string(),
                views::SET_ARCHIVE => core.settings.archive_folder = text.to_string(),
                _ => return,
            }
        }
        if reconnect {
            // 换目标服务：上一台的队列、进度与节点定义都不再适用，落回离线态等下一次探测重取。
            self.core().runtime.disconnect();
            self.restart_ws();
        }
        self.invalidate();
        self.schedule_save();
    }

    /// 进程接管方式切换；重复点当前值不落盘。
    pub(crate) fn set_process_mode(self: &Arc<App>, mode: ProcessMode) {
        let changed = {
            let mut core = self.core();
            if core.settings.process_mode == mode {
                false
            } else {
                core.settings.process_mode = mode;
                true
            }
        };
        if changed {
            self.invalidate();
            self.schedule_save();
        }
    }

    /// 选择 ComfyUI 安装目录：对话框等待不持 Core 锁。
    pub(crate) async fn pick_comfy_dir(self: &Arc<App>) {
        match self.ctx.pick_directory(Some("选择 ComfyUI 安装目录")).await {
            Ok(Some(dir)) => {
                self.core().settings.comfy_dir = dir;
                self.invalidate();
                self.schedule_save();
            }
            Ok(None) => {}
            Err(err) => self.set_error(format!("选择目录失败：{err}")),
        }
    }

    /// 清掉错误横幅与一次性提示。
    pub(crate) fn clear_error(self: &Arc<App>) {
        self.core().runtime.clear_error();
        self.set_notice(String::new());
        self.invalidate();
    }

    /// 高级参数展开态开关。
    pub(crate) fn toggle_params(self: &Arc<App>) {
        {
            let mut core = self.core();
            core.session.params_open = !core.session.params_open;
        }
        self.invalidate();
    }

    pub(crate) fn close_params(self: &Arc<App>) {
        self.core().session.params_open = false;
        self.invalidate();
    }

    /// 刷新状态与工作流清单。
    pub(crate) async fn refresh_status(self: &Arc<App>) {
        self.run_poll_cycle().await;
        self.refresh_workflows_cycle().await;
        self.invalidate();
    }

    /// 记录流多展开一批历史。
    pub(crate) fn show_earlier(self: &Arc<App>) {
        self.core().session.shown_batches += FLOW_BATCH_STEP;
        self.invalidate();
    }

    /// 收起面内选择面板。
    pub(crate) fn close_picker(self: &Arc<App>) {
        if self.core().session.picker.take().is_some() {
            self.invalidate();
        }
    }

    /// 高级参数筛选词：只影响列表投影，不落盘。
    pub(crate) fn set_param_filter(self: &Arc<App>, text: String) {
        self.core().session.param_filter = text;
        self.invalidate();
    }

    /// 节点分组折叠：只记覆盖项，未记的按「有没有常用参数」决定；判定口径与视图侧同一（含筛选）。
    pub(crate) fn toggle_group(self: &Arc<App>, node_id: &str) {
        let mut core = self.core();
        let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
        let group = form::group_by_node(&form::filter_fields(&fields, &core.session.param_filter))
            .into_iter()
            .find(|group| group.node_id == node_id)
            .map(|group| group.has_common);
        let open = core.session.group_toggles.get(node_id).copied().unwrap_or(group.unwrap_or(true));
        core.session.group_toggles.insert(node_id.to_string(), !open);
        drop(core);
        self.invalidate();
    }

    /// 布尔字段勾选：按草稿现值取反，再走统一写回（含值收敛）。
    pub(crate) fn toggle_bool(self: &Arc<App>, key: &str) {
        let next = !field_raw(&self.core(), key).and_then(|value| value.as_bool()).unwrap_or(false);
        self.write_field_value(key, Value::Bool(next));
    }

    /// 托管进程启动：进程生命周期由 controller 自持，界面不等它跑完。
    pub(crate) fn start_service(self: &Arc<App>) {
        let app = self.clone();
        tokio::spawn(async move { controller::start(&app).await });
    }

    pub(crate) fn stop_service(self: &Arc<App>) {
        let app = self.clone();
        tokio::spawn(async move { controller::stop(&app).await });
    }

    // ---------- 选择浮层 ----------

    fn show_picker(self: &Arc<App>, title: &str, options: Vec<PickerOption>) {
        if options.is_empty() {
            self.set_notice(format!("{title}：没有可选项"));
            return;
        }
        self.core().session.picker = Some(Picker { title: title.to_string(), options });
        self.invalidate();
    }

    pub(crate) fn open_workflow_picker(self: &Arc<App>) {
        let options = {
            let core = self.core();
            let current = core.session.workflow_id.as_deref();
            core.runtime
                .snap
                .workflows
                .iter()
                .map(|file| PickerOption {
                    label: files::workflow_display_name(&file.name),
                    current: current == Some(file.path.as_str()),
                    action: PickerAction::Workflow(file.path.clone()),
                })
                .collect()
        };
        self.show_picker("选择工作流", options);
    }

    pub(crate) fn open_size_picker(self: &Arc<App>, control_id: &str) {
        let section_id = match control_id {
            views::ID_RATIO => sizing::SECTION_RATIO,
            views::ID_RESOLUTION => sizing::SECTION_RESOLUTION,
            views::ID_COUNT => sizing::SECTION_COUNT,
            _ => return,
        };
        let picked = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            sizing::plan(&core.session.draft, &fields)
                .sections
                .into_iter()
                .find(|section| section.id == section_id)
        };
        let Some(section) = picked else { return };
        let options = section
            .options
            .iter()
            .map(|option| PickerOption {
                label: option.label.clone(),
                current: section.selected.as_deref() == Some(option.value.as_str()),
                action: PickerAction::Size { section: section.id, value: option.value.clone() },
            })
            .collect();
        self.show_picker(&format!("选择{}", section.label), options);
    }

    pub(crate) fn open_prompt_picker(self: &Arc<App>) {
        let options = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            let (field, explicit) = match form::resolve_prompt_field(&fields, core.session.prompt_key.as_deref()) {
                Some(resolved) => resolved,
                None => return,
            };
            let current_key = field.key();
            let mut options = vec![PickerOption {
                label: "自动".to_string(),
                current: !explicit,
                action: PickerAction::PromptAuto,
            }];
            options.extend(form::prompt_candidates(&fields).into_iter().map(|candidate| PickerOption {
                label: format!("{}.{}", candidate.node_label, form::field_label(&candidate.input)),
                current: explicit && candidate.key() == current_key,
                action: PickerAction::PromptField(candidate.key()),
            }));
            options
        };
        self.show_picker("选择提示词来源", options);
    }

    pub(crate) fn open_ref_picker(self: &Arc<App>, key: &str) {
        let picked = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            form::ref_slots(&fields)
                .into_iter()
                .find(|slot| slot.key() == key)
                .map(|slot| {
                    let current = form::display_value(form::read_field(&core.session.draft, slot).as_ref());
                    let names = form::image_candidates(core.runtime.snap.object_info.as_ref(), &slot.class_type);
                    (slot.class_type.clone(), current, names)
                })
        };
        let Some((_class, current, names)) = picked else { return };
        let options = names
            .into_iter()
            .map(|name| PickerOption {
                current: name == current,
                label: name.clone(),
                action: PickerAction::RefImage { key: key.to_string(), name },
            })
            .collect();
        self.show_picker("选择参考图", options);
    }

    pub(crate) fn open_combo_picker(self: &Arc<App>, key: &str) {
        let picked = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            let Some(field) = fields.iter().find(|field| field.key() == key) else { return };
            let current = form::display_value(form::read_field(&core.session.draft, field).as_ref());
            let named = form::field_label(&field.input);
            let title = if named.is_empty() { field.input.clone() } else { named.to_string() };
            let options = field.options.clone().unwrap_or_default();
            (title, current, options)
        };
        let (title, current, options) = picked;
        let options = options
            .into_iter()
            .map(|(submit, label)| PickerOption {
                current: submit == current,
                label,
                action: PickerAction::Combo { key: key.to_string(), submit },
            })
            .collect();
        self.show_picker(&format!("选择{title}"), options);
    }

    pub(crate) async fn apply_picker_action(self: &Arc<App>, action: PickerAction) {
        match action {
            PickerAction::Workflow(path) => {
                let exists = {
                    let core = self.core();
                    core.runtime.snap.workflows.iter().any(|file| file.path == path)
                };
                if !exists {
                    self.set_error("工作流已不在列表中，刷新后重试".to_string());
                    return;
                }
                self.select_workflow_by_path(&path).await;
            }
            PickerAction::Size { section, value } => self.apply_size_option(section, &value),
            PickerAction::PromptAuto => {
                self.core().session.prompt_key = None;
                self.invalidate();
            }
            PickerAction::PromptField(key) => {
                self.core().session.prompt_key = Some(key);
                self.invalidate();
            }
            PickerAction::RefImage { key, name } => {
                self.write_field_value(&key, Value::String(name));
                self.capture_ref_thumbs().await;
            }
            PickerAction::Combo { key, submit } => self.write_field_value(&key, Value::String(submit)),
        }
    }

    // ---------- 落库/参考图/工作流动作 ----------

    pub(crate) fn write_field_value(self: &Arc<App>, key: &str, raw: Value) {
        let mut core = self.core();
        let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
        let Some(field) = fields.into_iter().find(|field| field.key() == key) else { return };
        let value = form::coerce_field_value(&field, &raw);
        form::write_field(&mut core.session.draft, &field.node_id, &field.input, value);
        drop(core);
        self.invalidate();
    }

    pub(crate) fn toggle_seed(self: &Arc<App>, key: &str, on: bool) {
        {
            let mut core = self.core();
            if on {
                core.session.seed_mode.insert(key.to_string());
            } else {
                core.session.seed_mode.remove(key);
            }
        }
        self.invalidate();
    }

    /// 尺寸三段的一个选项：按选项值回查当前计划里要写的字段逐条落进草稿——比例与分辨率
    /// 在兜底路径下会一次换算出宽高两个值，界面只给一次点击。
    pub(crate) fn apply_size_option(self: &Arc<App>, section: &str, value: &str) {
        let writes = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            let plan = sizing::plan(&core.session.draft, &fields);
            plan.sections
                .iter()
                .find(|item| item.id == section)
                .and_then(|item| item.options.iter().position(|option| option.value == value))
                .and_then(|index| plan.writes_of(section, index))
                .map(|items| items.to_vec())
                .unwrap_or_default()
        };
        if writes.is_empty() {
            self.set_error("当前工作流没有该尺寸选项".to_string());
            return;
        }
        {
            let mut core = self.core();
            for write in writes {
                form::write_field(&mut core.session.draft, &write.node_id, &write.input, write.value);
            }
        }
        self.invalidate();
    }

    /// 步数/CFG 快捷框：写第一个未连线的同名字段，与高级表单里的是同一个位置。
    pub(crate) fn write_quick_number(self: &Arc<App>, input: &str, raw: Value) {
        let target = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            sizing::find_unconnected(&fields, input)
                .map(|field| (field.node_id.clone(), field.input.clone(), form::coerce_field_value(field, &raw)))
        };
        let Some((node_id, name, value)) = target else { return };
        {
            let mut core = self.core();
            form::write_field(&mut core.session.draft, &node_id, &name, value);
        }
        self.invalidate();
    }

    /// 主提示词框：写当前定位到的字段（手选优先，未手选按自动推断），与高级表单同一写入通路（含值收敛）。
    pub(crate) fn write_prompt_text(self: &Arc<App>, raw: Value) {
        let key = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            form::resolve_prompt_field(&fields, core.session.prompt_key.as_deref()).map(|(field, _)| field.key())
        };
        let Some(key) = key else { return };
        self.write_field_value(&key, raw);
    }

    /// 参考图缩略：给「已填文件名但本地还没副本」的槽位抓 `/view?type=input` 字节落进 `refs/`。
    /// 抓不到就留上传按钮占位，不报错——缩略只影响展示，提交用的仍是服务端里的原文件。
    async fn capture_ref_thumbs(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        let missing: Vec<String> = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            form::ref_slots(&fields)
                .into_iter()
                .filter_map(|slot| {
                    let name = form::read_field(&core.session.draft, slot)
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default();
                    (!name.is_empty() && core.session.refs.get(&name).is_none()).then_some(name)
                })
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let mut changed = false;
        let mut failed: Vec<String> = Vec::new();
        for name in missing {
            let image_ref = ImageRef { filename: name.clone(), subfolder: String::new(), type_: "input".to_string() };
            let fetched = match runtime::fetch_image_bytes(&settings, &http, &image_ref).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    failed.push(name);
                    continue;
                }
            };
            let relative = ref_path(&ref_copy_name(&name));
            if self.store.save(&relative, &fetched).is_err() {
                failed.push(name);
                continue;
            }
            let stale = self.core().session.refs.put(&name, relative);
            for old in stale {
                self.store.remove(&old);
            }
            changed = true;
        }
        if !failed.is_empty() {
            // 缩略只是省事显示，抓不到就留一行可定位原因，不当成失败中止
            self.set_notice(format!("参考图缩略抓取失败 {} 张：{}", failed.len(), failed.join("、")));
        }
        if changed || !failed.is_empty() {
            self.invalidate();
        }
    }

    /// 载入下拉选中项：读文件并归一化成可提交 prompt，草稿与随机字段重置。
    pub(crate) async fn select_workflow(self: &Arc<App>, index: i64) {
        let file = {
            let core = self.core();
            core.runtime.snap.workflows.get(index.max(0) as usize).cloned()
        };
        let Some(file) = file else { return };
        let (settings, http) = self.io_ctx();
        match runtime::load_workflow(&settings, &http, &file).await {
            Ok(stored) => {
                {
                    let mut core = self.core();
                    core.session.workflow_id = Some(stored.id);
                    core.session.workflow_name = Some(stored.name);
                    core.session.workflow_modified = Some(stored.modified);
                    core.session.draft = stored.prompt;
                    core.session.seed_mode = form::default_seed_keys(&core.session.draft);
                    core.session.current_changed = false;
                    core.session.notice.clear();
                    core.session.params_open = false;
                    core.session.param_filter.clear();
                    core.session.group_toggles.clear();
                    core.session.prompt_key = None;
                }
                self.invalidate();
                self.capture_ref_thumbs().await;
            }
            Err(err) => self.set_error(err),
        }
    }

    /// 按路径选中并载入工作流（AI 工具用：名称匹配在锁外做，路径比下拉下标稳定）。
    /// 返回 false 表示清单里已无该文件或载入失败（原因已由界面错误横幅给出）。
    pub(crate) async fn select_workflow_by_path(self: &Arc<App>, path: &str) -> bool {
        let index = {
            let core = self.core();
            core.runtime.snap.workflows.iter().position(|f| f.path == path)
        };
        let Some(index) = index else { return false };
        self.select_workflow(index as i64).await;
        self.core().session.workflow_id.as_deref() == Some(path)
    }

    /// 把当前草稿写回所属工作流文件；保存后刷新其 mtime 以免误报外部改动。
    pub(crate) async fn save_current_workflow(self: &Arc<App>) {
        let (settings, http, file, draft) = {
            let core = self.core();
            let Some(id) = core.session.workflow_id.as_ref() else {
                drop(core);
                self.set_error("未载入工作流，无法保存参数".to_string());
                return;
            };
            match core.runtime.snap.workflows.iter().find(|f| f.path == *id).cloned() {
                Some(file) => (core.settings.clone(), self.http.clone(), file, core.session.draft.clone()),
                None => {
                    drop(core);
                    self.set_error("工作流已不存在，请重新选择".to_string());
                    return;
                }
            }
        };
        match runtime::save_workflow(&settings, &http, &file, &draft).await {
            Ok(()) => {
                self.refresh_workflows_cycle().await;
                let mut core = self.core();
                if let Some(current) = core.runtime.snap.workflows.iter().find(|f| f.path == file.path) {
                    core.session.workflow_modified = Some(current.modified);
                }
                core.session.current_changed = false;
                core.session.notice = format!("已保存到 {}", file.name);
                drop(core);
                self.invalidate();
            }
            Err(err) => self.set_error(err),
        }
    }

    pub(crate) async fn run_generation(self: &Arc<App>) {
        let (settings, http, client_id, origin_id, prompt, title) = {
            let core = self.core();
            if core.session.draft.as_object().map(|map| map.is_empty()).unwrap_or(true) {
                drop(core);
                self.set_error("请先选择工作流".to_string());
                return;
            }
            let prompt = form::apply_seed_mode(&core.session.draft, &core.session.seed_mode);
            let title = core.session.workflow_name.clone().unwrap_or_else(|| "手动工作流".to_string());
            (core.settings.clone(), self.http.clone(), core.runtime.book.client_id.clone(), core.runtime.book.origin_id.clone(), prompt, title)
        };
        match runtime::fetch_submit(&settings, &http, &client_id, &origin_id, &prompt).await {
            Ok(prompt_id) => {
                self.core().runtime.apply_submit(&prompt_id, &title);
                self.invalidate();
            }
            Err(err) => self.set_error(err),
        }
    }

    pub(crate) async fn interrupt(self: &Arc<App>) {
        let (settings, http, target) = {
            let core = self.core();
            (core.settings.clone(), self.http.clone(), core.runtime.interrupt_target())
        };
        if let Some(prompt_id) = target {
            runtime::fetch_interrupt(&settings, &http, &prompt_id).await;
        }
        self.core().runtime.apply_interrupt();
        self.invalidate();
    }

    pub(crate) async fn free_memory(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        match runtime::fetch_free(&settings, &http).await {
            Ok(()) => self.set_notice("已请求释放显存".to_string()),
            Err(err) => self.set_error(err),
        }
    }

    /// 复制该批次的提示词原文：记录流里不铺开长文本，要看原文交宿主剪贴板写面。
    pub(crate) async fn copy_prompt(self: &Arc<App>, prompt_id: &str) {
        let text = {
            let core = self.core();
            core.runtime
                .snap
                .results
                .iter()
                .find(|item| item.prompt_id == prompt_id && !item.prompt_text.is_empty())
                .map(|item| item.prompt_text.clone())
        };
        match text {
            Some(text) => match self.ctx.clipboard_write(&text).await {
                Ok(()) => self.set_notice("已复制提示词".to_string()),
                Err(err) => self.set_error(format!("复制失败：{err}")),
            },
            None => self.set_error("这条记录没有提示词原文".to_string()),
        }
    }

    /// 复制运行中批次的提示词：记录流顶部的复制小章只认队首任务。
    pub(crate) async fn copy_running_prompt(self: &Arc<App>) {
        let (text, queued) = {
            let core = self.core();
            let lead = core.runtime.snap.queue_running.first().or(core.runtime.snap.queue_pending.first());
            let text = lead.and_then(crate::comfy::types::queue_item_prompt).map(form::prompt_text_of).filter(|text| !text.is_empty());
            (text, lead.is_some())
        };
        match text {
            Some(text) => match self.ctx.clipboard_write(&text).await {
                Ok(()) => self.set_notice("已复制提示词".to_string()),
                Err(err) => self.set_error(format!("复制失败：{err}")),
            },
            None if queued => self.set_error("当前任务没有提示词原文".to_string()),
            None => self.set_error("队列里没有可复制的任务".to_string()),
        }
    }

    /// 复制结果图到剪贴板：宿主只收 PNG，非 PNG 给可读原因而不是让宿主解码失败。
    pub(crate) async fn copy_result_image(self: &Arc<App>, key: String) {
        let (settings, http, item) = {
            let core = self.core();
            match core.runtime.snap.results.iter().find(|item| item.key == key).cloned() {
                Some(item) => (core.settings.clone(), self.http.clone(), item),
                None => return,
            }
        };
        let bytes = match read_result_bytes(&self.store, &settings, &http, &item).await {
            Ok(bytes) => bytes,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        if !crate::clipboard::is_png(&bytes) {
            self.set_error("结果图不是 PNG 格式，无法复制到剪贴板".to_string());
            return;
        }
        match self.ctx.clipboard_write_image(&bytes).await {
            Ok(()) => self.set_notice("已复制图片".to_string()),
            Err(err) => self.set_error(format!("复制图片失败：{err}")),
        }
    }

    /// 落库：本地副本就绪时经私有目录按路径导入（无载荷上限），缺副本才回源取字节走二进制写面。
    pub(crate) async fn archive_result(self: &Arc<App>, key: String) {
        let (settings, http, item, folder, local) = {
            let core = self.core();
            match core.runtime.snap.results.iter().find(|r| r.key == key).cloned() {
                Some(item) => {
                    let local = item.local.clone();
                    (core.settings.clone(), self.http.clone(), item, core.settings.archive_folder.clone(), local)
                }
                None => return,
            }
        };
        let saved = match local.as_deref() {
            Some(local) => {
                crate::archive::save_from_private(&self.ctx, &folder, &item.image_ref, &self.store.abs_path(local)).await
            }
            None => {
                let bytes = match read_result_bytes(&self.store, &settings, &http, &item).await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        self.set_error(err);
                        return;
                    }
                };
                crate::archive::save(&self.ctx, &folder, &item.image_ref, &bytes).await
            }
        };
        match saved {
            Ok(relative) => {
                self.core().runtime.mark_saved(&key, relative.clone());
                self.invalidate();
                self.schedule_save();
                self.set_notice(format!("已保存到仓库：{relative}"));
            }
            Err(err) => self.set_error(err),
        }
    }

    /// 另存为：取字节（本地副本优先，缺则回源）→ 保存对话框定路径 → 写盘。产物在仓库外，不记入状态。
    pub(crate) async fn save_result_as(self: &Arc<App>, key: String) {
        let (settings, http, item) = {
            let core = self.core();
            match core.runtime.snap.results.iter().find(|r| r.key == key).cloned() {
                Some(item) => (core.settings.clone(), self.http.clone(), item),
                None => return,
            }
        };
        let bytes = match read_result_bytes(&self.store, &settings, &http, &item).await {
            Ok(bytes) => bytes,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        // 只预填一个候选文件名：最终路径与扩展名都按用户在对话框里的选择原样写盘。
        let params = PickFileParams {
            title: Some("另存结果图".to_string()),
            save: Some(true),
            default_path: Some(crate::archive::save_as_name(&item.image_ref.filename)),
            filters: Vec::new(),
        };
        let path = match self.ctx.pick_file(params).await {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(err) => {
                self.set_error(format!("选择保存位置失败：{err}"));
                return;
            }
        };
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.set_notice(format!("已另存到 {path}")),
            Err(err) => self.set_error(format!("写入文件失败：{err}")),
        }
    }

    /// 打开记录流灯箱：面内浮层大图（与表格编辑器图片字段同一种交互），存档/另存/复制都在灯箱里。
    pub(crate) async fn open_lightbox(self: &Arc<App>, key: String) {
        let ready = {
            let core = self.core();
            core.runtime.snap.results.iter().any(|r| r.key == key && r.local.is_some())
        };
        if !ready {
            self.set_error("副本未就绪，无法预览".to_string());
            return;
        }
        {
            let mut core = self.core();
            core.session.lightbox_key = Some(key);
        }
        self.invalidate();
    }

    /// 上传用户选定的本地图片到 ComfyUI，写进对应节点的 image 输入。
    pub(crate) async fn upload_reference(self: &Arc<App>, key: String) {
        let params = PickFileParams {
            title: Some("选择参考图".to_string()),
            save: Some(false),
            default_path: None,
            filters: vec![FileFilter { name: "图片".to_string(), extensions: vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into(), "bmp".into()] }],
        };
        let path = match self.ctx.pick_file(params).await {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(err) => {
                self.set_error(format!("选择文件失败：{err}"));
                return;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.set_error(format!("读取图片失败：{err}"));
                return;
            }
        };
        let filename = path.rsplit(['/', '\\']).next().unwrap_or("image.png").to_string();
        self.upload_bytes(&key, &filename, bytes).await;
    }

    /// 粘贴参考图：宿主剪贴板面直接给 PNG 字节，取到后上传。
    pub(crate) async fn paste_reference(self: &Arc<App>, key: String) {
        match crate::clipboard::read_png_bytes(&self.ctx).await {
            Ok(bytes) => {
                let filename = crate::clipboard::file_name();
                self.upload_bytes(&key, &filename, bytes).await;
            }
            Err(err) => self.set_error(format!("粘贴失败：{err}")),
        }
    }

    /// 把图片字节传到 ComfyUI 的 `/upload/image` 并把返回的文件名写进字段。
    async fn upload_bytes(self: &Arc<App>, key: &str, filename: &str, bytes: Vec<u8>) {
        let (settings, http) = self.io_ctx();
        match runtime::fetch_upload(&settings, &http, filename, bytes).await {
            Ok(image_ref) => {
                self.set_notice(format!("已上传：{}", image_ref.filename));
                self.write_field_value(key, Value::String(image_ref.filename.clone()));
                // 服务端同名覆盖后旧缩略已失真：先作废这一条，再按新字节重抓
                if let Some(old) = self.core().session.refs.remove(&image_ref.filename) {
                    self.store.remove(&old);
                }
                self.capture_ref_thumbs().await;
            }
            Err(err) => self.set_error(err),
        }
    }

    pub(crate) fn clear_reference(self: &Arc<App>, key: &str) {
        self.write_field_value(key, Value::String(String::new()));
    }

    /// 在系统浏览器打开本机 ComfyUI 地址：借进程托管起系统默认打开命令。
    pub(crate) async fn open_browser(self: &Arc<App>) {
        let url = { base_url(&self.core().settings) };
        let (program, args) = open_command(&url);
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        if let Err(err) = self.ctx.spawn(program, &args, None, None).await {
            self.set_error(format!("打开浏览器失败：{err}"));
        }
    }

    // ---------- 挂载 / 仓库事件 ----------

    pub async fn on_surface_attached(self: &Arc<App>, params: SurfaceAttachedParams) {
        let known = params.surface_id == SURFACE_PANEL || params.surface_id == SURFACE_SETTINGS;
        if !known {
            return;
        }
        {
            let mut core = self.core();
            core.attached.insert(params.surface_id.clone());
            if params.surface_id == SURFACE_PANEL {
                // 挂上来就当用户在看最新一屏：首帧贴底，之后新结果才继续拉底。
                core.session.flow_at_bottom = true;
            }
        }
        // 控件层随面重建：旧面的焦点与滚动位置不跨挂载沿用。
        if params.surface_id == SURFACE_PANEL {
            *self.panel_ui.lock().await = FrameUi::new();
        } else {
            *self.settings_ui.lock().await = FrameUi::new();
        }
        self.invalidate();
    }

    pub fn on_surface_detached(self: &Arc<App>, params: SurfaceDetachedParams) {
        self.core().attached.remove(&params.surface_id);
    }

    // ---------- 错误/提示落状态 ----------

    fn set_error(self: &Arc<App>, message: String) {
        self.core().runtime.snap.error = message;
        self.invalidate();
    }

    fn set_notice(self: &Arc<App>, message: String) {
        self.core().session.notice = message;
        self.invalidate();
    }
}

/// 读结果字节：本地副本命中优先（存活于服务重启后），否则回源取图。
async fn read_result_bytes(store: &CopyStore, settings: &ComfySettings, http: &reqwest::Client, item: &ResultImage) -> Result<Vec<u8>, String> {
    if let Some(local) = item.local.as_deref() {
        if let Some(bytes) = store.read(local) {
            return Ok(bytes);
        }
    }
    runtime::fetch_image_bytes(settings, http, &item.image_ref).await.map_err(|e| format!("取回结果图失败：{e}"))
}

/// 系统默认打开 URL 的命令。
fn open_command(url: &str) -> (&'static str, Vec<&str>) {
    match std::env::consts::OS {
        "windows" => ("cmd", vec!["/C", "start", "", url]),
        "macos" => ("open", vec![url]),
        _ => ("xdg-open", vec![url]),
    }
}

/// 拼记录流布局输入：画廊聚合后按「已展示批次数」从最新往回裁切，末尾接运行中批次。
/// 裁切以批次（一个 promptId 的同批产出）为单位，一次点击就多展开一批。
pub(crate) fn flow_input(core: &Core, width: f32) -> gallery::FlowInput {
    let snap = &core.runtime.snap;
    let records = gallery::records_of(&snap.results);
    let hidden = hidden_batches(core);
    // 占位格规格跟用户当前选中的比例与张数走（生成坞三段同一判定）；面板还没载工作流时按队首任务推导。
    let spec = {
        if has_draft(core) {
            let fields = runtime::build_fields_for(&core.session.draft, snap);
            gallery::running_spec(&core.session.draft, &fields)
        } else {
            let lead = snap.queue_running.first().or(snap.queue_pending.first());
            let prompt = lead.and_then(crate::comfy::types::queue_item_prompt).cloned().unwrap_or(Value::Null);
            let fields = runtime::build_fields_for(&prompt, snap);
            gallery::running_spec(&prompt, &fields)
        }
    };
    let progress = snap.progress.as_ref();
    let running = gallery::running_block(
        &snap.queue_running,
        &snap.queue_pending,
        progress.map(|progress| progress.fraction() * 100.0),
        progress.map(|progress| format!("{:.0}/{:.0}", progress.value, progress.max)),
        spec,
    );
    gallery::FlowInput {
        width,
        records: records.into_iter().skip(hidden).collect(),
        running,
    }
}

/// 未展示的历史批次数（「显示更早」的计数与标签来源）。
pub(crate) fn hidden_batches(core: &Core) -> usize {
    gallery::records_of(&core.runtime.snap.results)
        .len()
        .saturating_sub(core.session.shown_batches.max(1))
}

/// 草稿里是否有可提交的工作流参数。
pub(crate) fn has_draft(core: &Core) -> bool {
    core.session.draft.as_object().map(|map| !map.is_empty()).unwrap_or(false)
}

/// 事件文本 → 数字：空串进草稿为 `null`（界面保持空框，提交交服务端裁决），非法输入原样保留让错误可见。
pub(crate) fn text_number(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    trimmed
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(text.to_string()))
}

/// 草稿里某个字段的展示串（输入框编辑基线，事件到达时按当前事实重取）。
pub(crate) fn field_text(core: &Core, key: &str) -> Option<String> {
    field_raw(core, key).map(|value| form::display_value(Some(&value)))
}

/// 草稿里某个字段的原始值：键为 `${节点id}.${输入名}`。
pub(crate) fn field_raw(core: &Core, key: &str) -> Option<Value> {
    let (node, input) = key.split_once('.')?;
    core.session.draft.get(node)?.get("inputs")?.get(input).cloned()
}

/// 主提示词输入框的当前值。
pub(crate) fn prompt_value(core: &Core) -> Option<String> {
    let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
    let (field, _) = form::resolve_prompt_field(&fields, core.session.prompt_key.as_deref())?;
    Some(form::display_value(form::read_field(&core.session.draft, field).as_ref()))
}

/// 步数 / CFG 快捷框的当前值。
pub(crate) fn quick_value(core: &Core, input: &str) -> Option<String> {
    let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
    let field = sizing::find_unconnected(&fields, input)?;
    let value = form::numeric_value(form::read_field(&core.session.draft, field).as_ref());
    Some(value.map(|value| value.to_string()).unwrap_or_default())
}

/// 拆字段控件 id 的前缀与键。
pub(crate) fn split_prefixed(id: &str) -> (Option<&str>, &str) {
    for prefix in [P_COMBO, P_NUM, P_TEXT, P_BOOL, P_SEED, P_UPLOAD, P_PASTE, P_CLEAR] {
        if let Some(rest) = id.strip_prefix(prefix) {
            return (Some(prefix), rest);
        }
    }
    (None, id)
}

/// 拼落盘信封：设置 + 来源标记 + 历史（截断到上限）。
fn build_envelope(core: &Core) -> Value {
    let settings = serde_json::to_value(&core.settings).unwrap_or(Value::Null);
    let origin_id = core.runtime.book.origin_id.clone();
    let history: Vec<&ResultImage> = core.runtime.snap.results.iter().take(HISTORY_LIMIT).collect();
    json!({
        "settings": settings,
        "originId": origin_id,
        "history": history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{P_GROUP, P_PICK, P_REF_PICK};

    #[test]
    fn empty_number_input_lands_as_null_and_unparsable_text_survives() {
        assert_eq!(text_number(""), Value::Null);
        assert_eq!(text_number("  "), Value::Null);
        assert_eq!(text_number(" 12.5 "), json!(12.5));
        assert_eq!(text_number("abc"), json!("abc"));
    }

    #[test]
    fn split_prefixed_covers_field_controls_only() {
        let key = format!("{}{}", P_NUM, "1.width");
        assert_eq!(split_prefixed(&key), (Some(P_NUM), "1.width"));
        assert_eq!(split_prefixed(views::ID_RUN), (None, views::ID_RUN));
    }

    /// 画布命中 id 在 `split_prefixed` 之前分派，前缀不得与字段控件前缀互相吞并。
    #[test]
    fn canvas_hit_ids_are_not_field_prefixes() {
        for id in [
            format!("{}k", gallery::HIT_PREVIEW),
            format!("{}k", gallery::HIT_SAVE),
            format!("{}p1", gallery::HIT_COPY),
            gallery::HIT_COPY_RUN.to_string(),
            gallery::HIT_INTERRUPT.to_string(),
        ] {
            assert_eq!(split_prefixed(&id), (None, id.as_str()), "{id} 不该命中字段控件前缀");
        }
    }

    /// 生成坞控件 id 同样先于字段前缀分派：`g:`/`rpick:` 不得被字段前缀吞掉。
    #[test]
    fn dock_ids_are_not_field_prefixes() {
        for id in [
            format!("{P_GROUP}3.width"),
            format!("{P_REF_PICK}3.image"),
            format!("{P_PICK}0"),
        ] {
            assert_eq!(split_prefixed(&id), (None, id.as_str()), "{id} 不该命中字段控件前缀");
        }
    }

    #[test]
    fn field_raw_reads_draft_by_node_and_input() {
        let mut core = Core {
            settings: ComfySettings::default(),
            attached: BTreeSet::new(),
            runtime: Runtime::new(),
            host: HostState::default(),
            session: Session { draft: json!({"3": {"inputs": {"seed": 42, "text": "hi"}}}), ..Default::default() },
        };
        assert_eq!(field_raw(&core, "3.seed"), Some(json!(42)));
        assert_eq!(field_text(&core, "3.text").as_deref(), Some("hi"));
        assert_eq!(field_raw(&core, "3.missing"), None);
        core.session.draft = json!({});
        assert_eq!(field_raw(&core, "3.seed"), None);
    }
}
