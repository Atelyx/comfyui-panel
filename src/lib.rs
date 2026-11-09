//! 插件入口：导出 C ABI 函数表，把宿主回调转交 App。
//!
//! 插件常驻任务放在自建的后台线程与独立 tokio 运行时上：`on_ready` 启动，`on_shutdown` 发停止信号、
//! 等线程退出并释放运行时；App 及其持有的 `Context` 一并释放，不跨实例销毁存活。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atelyx_plugin_protocol::frame::FrameInputParams;
use atelyx_plugin_sdk::protocol::method::{
    ProcessEventParams, SurfaceAttachedParams, SurfaceDetachedParams, ToolCallParams,
};
use atelyx_plugin_sdk::{Context, DrawFrame, FrameUi, PluginHandler};

use crate::app::App;

/// 后台运行时收尾的等待上限：超过后强制丢弃残留任务。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// 收尾耗时超过该值时记一条错误日志；阈值高于 `SHUTDOWN_GRACE`，正常耗尽收尾上限不算异常。
const SHUTDOWN_WARN: Duration = Duration::from_secs(3);

/// 一次插件实例：应用与承接常驻任务的后台线程。
struct Service {
    app: Arc<App>,
    background: Background,
}

/// 后台线程把柄：tokio 运行时随线程闭包持有，`block_on` 退出后限时收尾并丢弃。
struct Background {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Background {
    fn start(app: Arc<App>) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        // 回调里起的任务必须落在本运行时上，先交回句柄再启动线程。
        app.attach_runtime(runtime.handle().clone());
        let stop = app.stop_signal();
        let thread = std::thread::Builder::new()
            .name("comfyui-panel".into())
            .spawn(move || {
                runtime.block_on(async move {
                    app.start_tasks().await;
                    let mut stop = stop;
                    let _ = stop.changed().await;
                });
                runtime.shutdown_timeout(SHUTDOWN_GRACE);
            })?;
        Ok(Background { thread: Some(thread) })
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread.join().map_err(|_| "后台线程异常退出".to_string())
    }
}

struct Handler {
    service: Mutex<Option<Service>>,
}

impl Handler {
    fn new() -> Self {
        Handler { service: Mutex::new(None) }
    }

    fn app(&self) -> Option<Arc<App>> {
        self.service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|service| service.app.clone())
    }
}

#[async_trait::async_trait]
impl PluginHandler for Handler {
    async fn on_ready(&self, ctx: &Context) {
        if self
            .service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            return; // 重复 ready：首个实例已在服役
        }
        let app = App::bootstrap(Arc::new(ctx.clone())).await;
        let background = match Background::start(app.clone()) {
            Ok(background) => background,
            Err(err) => {
                ctx.log_error(&format!("创建插件后台运行时失败：{err}"));
                return;
            }
        };
        *self.service.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Service { app: app.clone(), background });
        app.after_ready().await;
    }

    async fn on_shutdown(&self, ctx: &Context) {
        let service = self
            .service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(mut service) = service else {
            return;
        };
        service.app.signal_stop();
        let started = Instant::now();
        if let Err(err) = service.background.shutdown() {
            ctx.log_error(&format!("插件后台线程收尾失败：{err}"));
        }
        if started.elapsed() > SHUTDOWN_WARN {
            ctx.log_error(&format!(
                "插件后台线程收尾耗时 {}ms，残留任务已强制丢弃",
                started.elapsed().as_millis()
            ));
        }
        // App 持有 Context 克隆与控件缓存，实例销毁前显式释放。
        drop(service.app);
    }

    fn on_draw(&self, frame: &mut DrawFrame<'_, '_>) {
        if let Some(app) = self.app() {
            ui::draw_surface(&app, frame);
        }
    }

    /// 面帧输入：SDK 逐条串行回调，同一面的输入按到达顺序就地处理并请求重绘。
    async fn on_frame_input(&self, _ctx: &Context, params: FrameInputParams, ui: &mut FrameUi) {
        if let Some(app) = self.app() {
            ui::handle_frame_input(&app, params, ui).await;
        }
    }

    async fn on_surface_attached(&self, _ctx: &Context, params: SurfaceAttachedParams) {
        if let Some(app) = self.app() {
            app.on_surface_attached(params).await;
        }
    }

    async fn on_surface_detached(&self, _ctx: &Context, params: SurfaceDetachedParams) {
        if let Some(app) = self.app() {
            app.on_surface_detached(params);
        }
    }

    async fn on_process_event(&self, _ctx: &Context, params: ProcessEventParams) {
        if let Some(app) = self.app() {
            controller::on_process_event(&app, params).await;
        }
    }

    async fn on_tool_call(&self, ctx: &Context, params: ToolCallParams) {
        // 工具调用只可能来自已注册的本插件工具，正常路径 App 一定在；仍未就绪时按未处理回报。
        if let Some(app) = self.app() {
            agent::on_tool_call(&app, params).await;
        } else {
            let _ = ctx.tool_result(&params.call_id, "插件尚未就绪", true).await;
        }
    }
}

// 插件以 cdylib 交付：宿主按 C ABI 符号建会话，与真实插件同一条通路
atelyx_plugin_sdk::export_in_process_plugin!(Handler::new());

mod app;
mod agent;
mod archive;
mod clipboard;
mod comfy;
mod controller;
mod gallery;
mod history;
mod launcher;
mod runtime;
mod settings;
mod ui;
mod views;
mod workflow;
