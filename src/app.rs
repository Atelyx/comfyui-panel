//! 应用装配与编排：持状态、驱动轮询/WS/渲染三条常驻任务，把界面事件翻成状态迁移与 IO。
//!
//! Elm 单向数据流：`Core` 是唯一真源，任何变更经 `invalidate` 递增版本号，由 `render_loop`
//! 重建整棵界面树提交给已挂载的面。约束——绝不持 `MutexGuard` 跨 `await`：需要 IO 时先在锁内
//! 取出输入（克隆）、释放锁、`await` 网络/磁盘、再重新加锁写回，慢 IO 因此不卡住整棵重绘。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use atelyx_plugin_sdk::protocol::method::{
    CanvasResizedParams, ContributionSlot, FileFilter, NotificationLevel, PickFileParams, ScrollEdge,
    SurfaceKind, SurfaceAttachedParams, SurfaceDetachedParams, ViewEventParams, WindowOpenParams,
};
use atelyx_plugin_sdk::protocol::view::{CanvasPointer, EventValue, MouseButton, PaddingSpec, ViewEvent};
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
use crate::views::{
    self, P_BOOL, P_CLEAR, P_COMBO, P_GROUP, P_NUM, P_PASTE, P_REF_PICK, P_SEED, P_TEXT, P_UPLOAD,
};
use crate::workflow::form;
use crate::workflow::sizing;

/// 生成面板贡献 id（同时是挂载后的 surfaceId，不得含 `:`）。
pub const SURFACE_PANEL: &str = "generate";
/// 设置页贡献 id。
pub const SURFACE_SETTINGS: &str = "comfyui";
/// 大图预览窗口 id：经 `window.open` 按需打开，同时是该窗口面的 surfaceId。
pub const SURFACE_PREVIEW: &str = "preview";

/// 状态信封结构版本。
const STATE_VERSION: u64 = 1;
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

/// 界面之外的会话态：当前工作流、参数草稿、随机种子勾选、记录流与生成坞的展开态、参考图缩略、
/// 预览窗口条目。不落盘。
#[derive(Default)]
pub struct Session {
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_modified: Option<f64>,
    pub draft: Value,
    pub seed_mode: BTreeSet<String>,
    pub current_changed: bool,
    pub notice: String,
    /// 预览窗口当前展示的结果 key。
    pub preview_key: Option<String>,
    /// 记录流画布宽度：宿主 `canvas.resized` 回报的真实分配宽度，布局坐标与命中都按它算。
    pub gallery_width: f32,
    /// 上一次提交的记录流布局：指针命中按它算，用户点的就是他刚看到的那一版。
    pub flow: Option<gallery::FlowLayout>,
    /// 记录流已展示的记录批次数，「显示更早」递增。
    pub shown_batches: usize,
    /// 用户是否停在记录流末端（宿主回发的滚动位置）：只有在看最新一屏时，新结果才把视口贴到底。
    pub flow_at_bottom: bool,
    /// 下一次渲染后把记录流滚到底：新结果进来时代码树还没更新，滚动只能在提交之后发。
    pub flow_stick: bool,
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
                flow_stick: true,
                ..Default::default()
            },
        };

        let (version_tx, version_rx) = watch::channel(0u64);
        let app = Arc::new(App {
            ctx,
            core: Mutex::new(core),
            version: version_tx,
            http: reqwest::Client::new(),
            store,
            ws_control: Mutex::new(None),
            save_gen: AtomicU64::new(0),
        });

        for surface in [SURFACE_PANEL, SURFACE_SETTINGS, SURFACE_PREVIEW] {
            let build_app = Arc::clone(&app);
            app.ctx
                .ui()
                .mount(surface, move || match surface {
                    SURFACE_PANEL => views::build_panel(&build_app.core()),
                    SURFACE_SETTINGS => views::build_settings(&build_app.core()),
                    _ => views::build_preview(&build_app.core()),
                })
                .await;
        }
        tokio::spawn(App::render_loop(app.clone(), version_rx));
        tokio::spawn(App::poll_loop(app.clone()));
        tokio::spawn(App::workflow_poll_loop(app.clone()));
        tokio::spawn(App::ws_loop(app.clone()));
        app
    }

    /// 注册贡献点、跑首轮刷新、按设置托管自启。
    pub async fn after_ready(self: &Arc<App>) {
        // 面外壳内边距归插件自己控制（零外壳 + 根列 12），避免与宿主默认内边距叠加。
        let pad = Some(PaddingSpec::All(0.0));
        let _ = self.ctx.register(ContributionSlot::Panel, SURFACE_PANEL, "ComfyUI 生成", Some("sparkles"), pad).await;
        let _ = self.ctx.register(ContributionSlot::Settings, SURFACE_SETTINGS, "ComfyUI", Some("server"), pad).await;

        self.run_poll_cycle().await;
        self.refresh_workflows_cycle().await;

        let should_autostart = {
            let core = self.core();
            core.settings.auto_start && core.settings.process_mode == ProcessMode::Managed && !core.host.running && !core.host.starting
        };
        if should_autostart {
            let app = self.clone();
            tokio::spawn(async move {
                controller::start(&app).await;
            });
        }
    }

    // ---------- 状态访问 ----------

    pub(crate) fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn invalidate(&self) {
        let next = *self.version.borrow() + 1;
        let _ = self.version.send(next);
    }

    /// 弹一条宿主通知：尽力而为，失败静默（通知非关键路径）。
    pub(crate) fn notify(&self, title: &str, body: Option<&str>) {
        let ctx = self.ctx.clone();
        let title = title.to_string();
        let body = body.map(str::to_string);
        tokio::spawn(async move {
            let _ = ctx.notify(&title, body.as_deref(), NotificationLevel::Info).await;
        });
    }

    /// 在锁外取 IO 输入（地址随设置变，故每次克隆当前 settings）。
    fn io_ctx(&self) -> (ComfySettings, reqwest::Client) {
        let core = self.core();
        (core.settings.clone(), self.http.clone())
    }

    // ---------- 渲染 ----------

    async fn render_loop(app: Arc<App>, mut rx: watch::Receiver<u64>) {
        while rx.changed().await.is_ok() {
            let (panel_attached, commands, stick, settings_attached, preview_attached) = {
                let mut core = app.core();
                let mut commands = Vec::new();
                let panel_attached = core.attached.contains(SURFACE_PANEL);
                if panel_attached {
                    let flow = gallery::layout(&flow_input(&core));
                    // 没有任何可点条目时画布只剩一块灰底，不如让界面给出文字说明。
                    core.session.flow = (!flow.hits.is_empty()).then_some(flow);
                    if let Some(flow) = &core.session.flow {
                        commands = flow.commands.clone();
                    }
                } else {
                    core.session.flow = None;
                }
                // 贴底标志只在面板挂着时消费：面板关着时轮询也会重算，标志要留到下次打开面板那一轮。
                let stick = panel_attached && core.session.flow_stick;
                if panel_attached {
                    core.session.flow_stick = false;
                }
                let settings_attached = core.attached.contains(SURFACE_SETTINGS);
                let preview_attached = core.attached.contains(SURFACE_PREVIEW);
                (panel_attached, commands, stick, settings_attached, preview_attached)
            };
            if panel_attached {
                let _ = app.ctx.ui().notify(SURFACE_PANEL).await;
                // 画布清单必须整发整换：宿主按清单指针判重缓存，图片首帧未就绪也不会自己重画，
                // 每轮重发同一份才能保证副本落盘后缩略图补上来。
                if !commands.is_empty() {
                    let _ = app.ctx.submit_canvas(SURFACE_PANEL, views::ID_FLOW_CANVAS, commands).await;
                }
                // 新结果进来就贴底：滚动只能在提交完新树之后发，否则宿主看到的还是上一版的容器。
                if stick {
                    let _ = app.ctx.scroll_to(SURFACE_PANEL, views::ID_FLOW, ScrollEdge::End).await;
                }
            }
            if settings_attached {
                let _ = app.ctx.ui().notify(SURFACE_SETTINGS).await;
            }
            if preview_attached {
                let _ = app.ctx.ui().notify(SURFACE_PREVIEW).await;
            }
        }
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
        {
            self.core().runtime.set_probing(true);
        }
        self.invalidate();
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
                let mut core = self.core();
                core.session.flow_stick = core.session.flow_at_bottom;
                drop(core);
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

    // ---------- 界面事件 ----------

    pub async fn on_view_event(self: &Arc<App>, params: ViewEventParams) {
        match params.surface_id.as_str() {
            SURFACE_PANEL => self.on_panel_event(params.node_id, params.event).await,
            SURFACE_SETTINGS => self.on_settings_event(params.node_id, params.event).await,
            _ => {}
        }
    }

    async fn on_panel_event(self: &Arc<App>, node_id: String, event: ViewEvent) {
        // 静态动作按钮
        match node_id.as_str() {
            views::ID_REFRESH => {
                self.run_poll_cycle().await;
                self.refresh_workflows_cycle().await;
                self.invalidate();
                return;
            }
            views::ID_OPEN_BROWSER => {
                self.open_browser().await;
                return;
            }
            views::ID_CLEAR_ERROR => {
                self.core().runtime.clear_error();
                self.set_notice(String::new());
                self.invalidate();
                return;
            }
            views::ID_START => {
                let app = self.clone();
                tokio::spawn(async move {
                    controller::start(&app).await;
                });
                return;
            }
            views::ID_STOP => {
                let app = self.clone();
                tokio::spawn(async move {
                    controller::stop(&app).await;
                });
                return;
            }
            views::ID_FREE => {
                self.free_memory().await;
                return;
            }
            views::ID_RUN => {
                self.run_generation().await;
                return;
            }
            views::ID_INTERRUPT => {
                self.interrupt().await;
                return;
            }
            views::ID_SAVE_WF => {
                self.save_current_workflow().await;
                return;
            }
            views::ID_PARAMS_TOGGLE => {
                {
                    let mut core = self.core();
                    core.session.params_open = !core.session.params_open;
                }
                self.invalidate();
                return;
            }
            _ => {}
        }

        if node_id == views::ID_WF {
            if let ViewEvent::Change { value: EventValue::Number(index) } = event {
                self.select_workflow(index as i64).await;
            }
            return;
        }
        // 记录流画布：命中区域就是动作，坐标按上一版提交的布局算
        if node_id == views::ID_FLOW_CANVAS {
            if let ViewEvent::Canvas { pointer } = event {
                self.on_flow_pointer(pointer).await;
            }
            return;
        }
        // 记录流滚动位置：只记住「是否停在末端」，决定新结果要不要把视口拉到底；不触发重绘
        if node_id == views::ID_FLOW {
            if let ViewEvent::Scroll { offset, max_offset } = event {
                self.core().session.flow_at_bottom = offset >= max_offset - 1.0;
            }
            return;
        }
        // 尺寸三段与快捷数值：段落 id 定位、事件值定位选项，写回值由 SizePlan 回查
        if node_id == views::ID_RATIO || node_id == views::ID_RESOLUTION || node_id == views::ID_COUNT {
            if let ViewEvent::Change { value: EventValue::Number(index) } = event {
                self.apply_size_option(&node_id, index.max(0.0) as usize);
            }
            return;
        }
        if let Some((input, _)) = views::QUICK_NUMBER_FIELDS.iter().find(|(_, id)| *id == node_id) {
            if let ViewEvent::Change { value } = event {
                let input = *input;
                self.write_quick_number(input, number_value(&value));
            }
            return;
        }
        // 主提示词：输入框写字段值，菜单切字段来源
        if node_id == views::ID_PROMPT_TEXT {
            if let ViewEvent::Change { value } = event {
                self.write_prompt_text(text_value(&value));
            }
            return;
        }
        if node_id == views::ID_PROMPT_FIELD {
            if let ViewEvent::Change { value: EventValue::Text(action) } = event {
                {
                    let mut core = self.core();
                    core.session.prompt_key = if action.is_empty() { None } else { Some(action.clone()) };
                }
                self.invalidate();
            }
            return;
        }
        if node_id == views::ID_PARAM_FILTER {
            if let ViewEvent::Change { value } = event {
                {
                    let mut core = self.core();
                    core.session.param_filter = text_value(&value).as_str().unwrap_or("").to_string();
                }
                self.invalidate();
            }
            return;
        }
        // 节点分组折叠：只记覆盖项，未记的按「有没有常用参数」决定；判定口径与视图侧同一（含筛选）
        if let Some(node_id) = node_id.strip_prefix(P_GROUP) {
            if matches!(event, ViewEvent::Click) {
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
            return;
        }
        // 参考图槽位：从 input 目录已有图里选一个写进字段
        if let Some(key) = node_id.strip_prefix(P_REF_PICK) {
            if let ViewEvent::Change { value: EventValue::Text(name) } = event {
                self.write_field_value(key, Value::String(name.clone()));
                self.capture_ref_thumbs().await;
            }
            return;
        }

        // 字段控件：combo / number / text / bool，键 = `${节点id}.${输入名}`
        let (prefix, payload) = split_prefixed(&node_id);
        match prefix {
            Some(P_COMBO) => {
                if let ViewEvent::Change { value: EventValue::Number(index) } = event {
                    self.write_combo(payload, index as usize);
                }
            }
            Some(P_NUM) => {
                if let ViewEvent::Change { value } = event {
                    self.write_field_value(payload, number_value(&value));
                }
            }
            Some(P_TEXT) => {
                if let ViewEvent::Change { value } = event {
                    self.write_field_value(payload, text_value(&value));
                }
            }
            Some(P_BOOL) => {
                if let ViewEvent::Change { value } = event {
                    self.write_field_value(payload, bool_value(&value));
                }
            }
            Some(P_SEED) => {
                if let ViewEvent::Change { value: EventValue::Bool(on) } = event {
                    self.toggle_seed(payload, on);
                }
            }
            Some(P_UPLOAD) => {
                if matches!(event, ViewEvent::Click) {
                    self.upload_reference(payload.to_string()).await;
                }
            }
            Some(P_PASTE) => {
                if matches!(event, ViewEvent::Click) {
                    self.paste_reference(payload.to_string()).await;
                }
            }
            Some(P_CLEAR) => {
                if matches!(event, ViewEvent::Click) {
                    self.clear_reference(payload);
                }
            }
            _ => {}
        }
    }

    /// 画布命中 → 动作：整格开大图，章位落库/另存/复制提示词，「显示更早」加一批，中断走同一入口。
    async fn on_flow_pointer(self: &Arc<App>, pointer: CanvasPointer) {
        let (x, y, button) = match pointer {
            CanvasPointer::Up { x, y, button } => (x, y, button),
            _ => return,
        };
        if button != MouseButton::Left {
            return;
        }
        let hit = {
            let core = self.core();
            core.session.flow.as_ref().and_then(|flow| gallery::hit_test(flow, x, y).map(str::to_string))
        };
        let Some(hit) = hit else { return };
        if let Some(key) = hit.strip_prefix(gallery::HIT_PREVIEW) {
            self.open_preview(key.to_string()).await;
        } else if let Some(key) = hit.strip_prefix(gallery::HIT_SAVE) {
            self.archive_result(key.to_string()).await;
        } else if let Some(key) = hit.strip_prefix(gallery::HIT_SAVE_AS) {
            self.save_result_as(key.to_string()).await;
        } else if let Some(id) = hit.strip_prefix(gallery::HIT_COPY) {
            self.copy_prompt(id).await;
        } else if hit == gallery::HIT_MORE {
            self.core().session.shown_batches += FLOW_BATCH_STEP;
            self.invalidate();
        } else if hit == gallery::HIT_INTERRUPT {
            self.interrupt().await;
        }
    }

    /// 复制该批次的提示词原文：记录流里不铺开长文本，要看原文交宿主剪贴板写面。
    async fn copy_prompt(self: &Arc<App>, prompt_id: &str) {
        let text = {
            let core = self.core();
            core.runtime.snap.results.iter()
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

    /// 画布宽度回报：布局坐标与命中都按它对齐。高度不参与比较——记录流高度是宽度的函数，
    /// 比高度会在「重排→变高→再回报」之间来回抖动。
    pub fn on_canvas_resized(self: &Arc<App>, params: CanvasResizedParams) {
        if params.surface_id != SURFACE_PANEL || params.node_id != views::ID_FLOW_CANVAS {
            return;
        }
        let changed = {
            let mut core = self.core();
            if (core.session.gallery_width - params.width).abs() < 0.5 {
                false
            } else {
                core.session.gallery_width = params.width;
                true
            }
        };
        if changed {
            self.invalidate();
        }
    }

    async fn on_settings_event(self: &Arc<App>, node_id: String, event: ViewEvent) {
        // 进程控制与目录选择：动作类按钮，处理即返回，不进入设置字段的写入/落盘流程。
        match node_id.as_str() {
            views::ID_START => {
                let app = self.clone();
                tokio::spawn(async move {
                    controller::start(&app).await;
                });
                return;
            }
            views::ID_STOP => {
                let app = self.clone();
                tokio::spawn(async move {
                    controller::stop(&app).await;
                });
                return;
            }
            views::SET_DIR_PICK => {
                // 对话框等待不设超时，且取路径不持 Core 锁——绝不把 MutexGuard 跨到 await。
                match self.ctx.pick_directory(Some("选择 ComfyUI 安装目录")).await {
                    Ok(Some(dir)) => {
                        self.core().settings.comfy_dir = dir;
                        self.invalidate();
                        self.schedule_save();
                    }
                    Ok(None) => {}
                    Err(err) => self.set_error(format!("选择目录失败：{err}")),
                }
                return;
            }
            _ => {}
        }
        let reconnect = node_id == views::SET_HOST || node_id == views::SET_PORT || node_id == views::SET_MODE;
        {
            let mut core = self.core();
            match (node_id.as_str(), &event) {
                (views::SET_HOST, ViewEvent::Change { value }) => {
                    let text = text_value(value).as_str().unwrap_or("").trim().to_string();
                    core.settings.host = if text.is_empty() { crate::settings::ComfySettings::default().host } else { text };
                }
                (views::SET_PORT, ViewEvent::Change { value }) => {
                    if let Some(port) = number_value(value).as_f64().map(|n| n.clamp(1.0, 65535.0) as u32).and_then(|n| u16::try_from(n).ok()) {
                        core.settings.port = port;
                    }
                }
                (views::SET_MODE, ViewEvent::Change { value: EventValue::Number(index) }) => {
                    core.settings.process_mode = if *index == 1.0 { ProcessMode::External } else { ProcessMode::Managed };
                }
                (views::SET_DIR, ViewEvent::Change { value }) => core.settings.comfy_dir = text_value(value).as_str().unwrap_or("").to_string(),
                (views::SET_PYTHON, ViewEvent::Change { value }) => core.settings.python_path = text_value(value).as_str().unwrap_or("").to_string(),
                (views::SET_EXTRA, ViewEvent::Change { value }) => core.settings.extra_args = text_value(value).as_str().unwrap_or("").to_string(),
                (views::SET_ARCHIVE, ViewEvent::Change { value }) => core.settings.archive_folder = text_value(value).as_str().unwrap_or("").to_string(),
                (views::SET_AUTO, ViewEvent::Change { value: EventValue::Bool(on) }) => core.settings.auto_start = *on,
                _ => {}
            }
        }
        if reconnect {
            if node_id == views::SET_HOST || node_id == views::SET_PORT {
                // 换目标服务：上一台的队列、进度与节点定义都不再适用，落回离线态等下一次探测重取。
                self.core().runtime.disconnect();
            }
            self.restart_ws();
        }
        self.invalidate();
        self.schedule_save();
    }

    // ---------- 落库/参考图/工作流动作 ----------

    fn write_field_value(self: &Arc<App>, key: &str, raw: Value) {
        let mut core = self.core();
        let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
        let Some(field) = fields.into_iter().find(|field| field.key() == key) else { return };
        let value = form::coerce_field_value(&field, &raw);
        form::write_field(&mut core.session.draft, &field.node_id, &field.input, value);
        drop(core);
        self.invalidate();
    }

    fn write_combo(self: &Arc<App>, key: &str, index: usize) {
        let mut core = self.core();
        let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
        let Some(field) = fields.into_iter().find(|field| field.key() == key) else { return };
        let Some((submit, _)) = field.options.as_ref().and_then(|options| options.get(index)) else { return };
        let value = Value::String(submit.clone());
        form::write_field(&mut core.session.draft, &field.node_id, &field.input, value);
        drop(core);
        self.invalidate();
    }

    fn toggle_seed(self: &Arc<App>, key: &str, on: bool) {
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

    /// 尺寸三段的一个选项：由 `SizePlan` 回查该选项要写的字段逐条落进草稿——比例与分辨率
    /// 在兜底路径下会一次换算出宽高两个值，界面只给一次点击。
    fn apply_size_option(self: &Arc<App>, control_id: &str, index: usize) {
        let section = match control_id {
            views::ID_RATIO => sizing::SECTION_RATIO,
            views::ID_RESOLUTION => sizing::SECTION_RESOLUTION,
            views::ID_COUNT => sizing::SECTION_COUNT,
            _ => return,
        };
        let writes = {
            let core = self.core();
            let fields = runtime::build_fields_for(&core.session.draft, &core.runtime.snap);
            sizing::plan(&core.session.draft, &fields)
                .writes_of(section, index)
                .map(|items| items.to_vec())
                .unwrap_or_default()
        };
        if writes.is_empty() {
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
    fn write_quick_number(self: &Arc<App>, input: &str, raw: Value) {
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
    fn write_prompt_text(self: &Arc<App>, raw: Value) {
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
    async fn select_workflow(self: &Arc<App>, index: i64) {
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

    /// 把当前草稿写回所属工作流文件；保存后刷新其 mtime 以免误报外部改动。
    async fn save_current_workflow(self: &Arc<App>) {
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

    async fn run_generation(self: &Arc<App>) {
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

    async fn interrupt(self: &Arc<App>) {
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

    async fn free_memory(self: &Arc<App>) {
        let (settings, http) = self.io_ctx();
        match runtime::fetch_free(&settings, &http).await {
            Ok(()) => self.set_notice("已请求释放显存".to_string()),
            Err(err) => self.set_error(err),
        }
    }

    /// 落库：本地副本优先读字节，缺则回源；经宿主仓库写面写进落库目录，成功后记录相对路径。
    async fn archive_result(self: &Arc<App>, key: String) {
        let (settings, http, item, folder) = {
            let core = self.core();
            match core.runtime.snap.results.iter().find(|r| r.key == key).cloned() {
                Some(item) => (core.settings.clone(), self.http.clone(), item, core.settings.archive_folder.clone()),
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
        match crate::archive::save(&self.ctx, &folder, &item.image_ref, &bytes).await {
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
    async fn save_result_as(self: &Arc<App>, key: String) {
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

    /// 打开/复用大图预览窗口：窗口面固定一个 id，再次点击只切换展示条目。
    async fn open_preview(self: &Arc<App>, key: String) {
        let has_copy = {
            let core = self.core();
            core.runtime.snap.results.iter().any(|r| r.key == key && r.local.is_some())
        };
        if !has_copy {
            self.set_error("副本未就绪，无法预览".to_string());
            return;
        }
        // 先记条目再开窗：宿主的挂载通知可能早于开窗回执到达，此时树已能取到正确条目。
        self.core().session.preview_key = Some(key);
        self.invalidate();
        if self.core().attached.contains(SURFACE_PREVIEW) {
            return;
        }
        let params = WindowOpenParams {
            id: SURFACE_PREVIEW.to_string(),
            title: "ComfyUI 结果预览".to_string(),
            width: Some(900.0),
            height: Some(700.0),
            always_on_top: None,
            skip_taskbar: None,
            hide_on_blur: None,
        };
        if let Err(err) = self.ctx.window_open(params).await {
            // 连点竞态：两次点击都判定未挂载，后到的一次被宿主按重复 id 拒绝——窗口确实已在，忽略即可。
            if !self.core().attached.contains(SURFACE_PREVIEW) {
                self.set_error(format!("打开预览窗口失败：{err}"));
            }
        }
    }

    /// 上传用户选定的本地图片到 ComfyUI，写进对应节点的 image 输入。
    async fn upload_reference(self: &Arc<App>, key: String) {
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
    async fn paste_reference(self: &Arc<App>, key: String) {
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

    fn clear_reference(self: &Arc<App>, key: &str) {
        self.write_field_value(key, Value::String(String::new()));
    }

    /// 在系统浏览器打开本机 ComfyUI 地址：借进程托管起系统默认打开命令。
    async fn open_browser(self: &Arc<App>) {
        let url = { base_url(&self.core().settings) };
        let (program, args) = open_command(&url);
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        if let Err(err) = self.ctx.spawn(program, &args, None, None).await {
            self.set_error(format!("打开浏览器失败：{err}"));
        }
    }

    // ---------- 挂载 / 仓库事件 ----------

    pub fn on_surface_attached(self: &Arc<App>, params: SurfaceAttachedParams) {
        // 预览窗口只认 `window.open` 后随附的 Window 面，避免与同名贡献面混淆。
        let known = params.surface_id == SURFACE_PANEL
            || params.surface_id == SURFACE_SETTINGS
            || (params.surface_id == SURFACE_PREVIEW && params.kind == SurfaceKind::Window);
        if !known {
            return;
        }
        let panel = params.surface_id == SURFACE_PANEL;
        self.core().attached.insert(params.surface_id);
        if panel {
            // 挂上来就当用户在看最新一屏：首帧贴底，之后新结果才继续拉底。
            let mut core = self.core();
            core.session.flow_at_bottom = true;
            core.session.flow_stick = true;
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
fn flow_input(core: &Core) -> gallery::FlowInput {
    let snap = &core.runtime.snap;
    let records = gallery::records_of(&snap.results);
    let shown = core.session.shown_batches.max(1);
    let hidden = records.len().saturating_sub(shown);
    let running = gallery::running_block(
        &snap.queue_running,
        &snap.queue_pending,
        snap.progress.as_ref().map(|progress| progress.fraction() * 100.0),
        |prompt| runtime::build_fields_for(prompt, snap),
    );
    gallery::FlowInput {
        width: core.session.gallery_width,
        records: records.into_iter().skip(hidden).collect(),
        running,
        earlier_label: (hidden > 0).then(|| format!("显示更早（{hidden}）")),
    }
}

/// 拼落盘信封：设置 + 来源标记 + 历史（截断到上限）。
fn build_envelope(core: &Core) -> Value {
    let settings = serde_json::to_value(&core.settings).unwrap_or(Value::Null);
    let origin_id = core.runtime.book.origin_id.clone();
    let history: Vec<&ResultImage> = core.runtime.snap.results.iter().take(HISTORY_LIMIT).collect();
    json!({
        "version": STATE_VERSION,
        "settings": settings,
        "originId": origin_id,
        "history": history,
    })
}

/// 事件值 → 数字。空值（数字框被清空）原样转成 `null` 进草稿：界面保持空框，提交交服务端裁决，
/// 与非法输入「原样保留、让错误可见」的口径一致。
fn number_value(value: &EventValue) -> Value {
    match value {
        EventValue::Number(n) => serde_json::Number::from_f64(*n).map(Value::Number).unwrap_or(Value::Null),
        EventValue::Text(text) => text.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::String(text.clone())),
        EventValue::Bool(b) => Value::Bool(*b),
        EventValue::Null => Value::Null,
    }
}

fn text_value(value: &EventValue) -> Value {
    match value {
        EventValue::Text(text) => Value::String(text.clone()),
        other => Value::String(json_to_plain(other)),
    }
}

fn bool_value(value: &EventValue) -> Value {
    match value {
        EventValue::Bool(b) => Value::Bool(*b),
        EventValue::Text(text) => Value::Bool(text == "true"),
        EventValue::Number(n) => Value::Bool(*n != 0.0),
        EventValue::Null => Value::Bool(false),
    }
}

fn json_to_plain(value: &EventValue) -> String {
    match value {
        EventValue::Text(text) => text.clone(),
        EventValue::Number(n) => n.to_string(),
        EventValue::Bool(b) => b.to_string(),
        EventValue::Null => String::new(),
    }
}

/// 拆字段控件 id 的前缀与键。
fn split_prefixed(id: &str) -> (Option<&str>, &str) {
    for prefix in [P_COMBO, P_NUM, P_TEXT, P_BOOL, P_SEED, P_UPLOAD, P_PASTE, P_CLEAR] {
        if let Some(rest) = id.strip_prefix(prefix) {
            return (Some(prefix), rest);
        }
    }
    (None, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleared_control_reports_empty_form() {
        assert_eq!(number_value(&EventValue::Null), Value::Null);
        assert_eq!(text_value(&EventValue::Null), Value::String(String::new()));
        assert_eq!(bool_value(&EventValue::Null), Value::Bool(false));
        assert_eq!(json_to_plain(&EventValue::Null), "");
    }

    #[test]
    fn number_value_parses_text_and_keeps_unparsable() {
        assert_eq!(number_value(&EventValue::Text(" 12.5 ".to_string())), json!(12.5));
        assert_eq!(number_value(&EventValue::Text("abc".to_string())), json!("abc"));
        assert_eq!(number_value(&EventValue::Number(3.0)), json!(3.0));
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
            format!("{}k", gallery::HIT_SAVE_AS),
            format!("{}k", gallery::HIT_COPY),
            gallery::HIT_MORE.to_string(),
            gallery::HIT_INTERRUPT.to_string(),
        ] {
            assert_eq!(split_prefixed(&id), (None, id.as_str()), "{id} 不该命中字段控件前缀");
        }
    }

    /// 生成坞控件 id 同样先于字段前缀分派：`g:`/`rpick:` 不得被字段前缀吞掉。
    #[test]
    fn dock_ids_are_not_field_prefixes() {
        for id in [format!("{P_GROUP}3.width"), format!("{P_REF_PICK}3.image")] {
            assert_eq!(split_prefixed(&id), (None, id.as_str()), "{id} 不该命中字段控件前缀");
        }
    }
}
