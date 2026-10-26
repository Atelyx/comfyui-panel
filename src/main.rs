//! 插件入口：握手、注册界面树与常驻任务装配，再把宿主通知转交 App。
//!
//! 生命周期严格单向：`on_ready` 建 App 并落到 `OnceLock`（先于注册贡献点，保证随后到达的
//! `surface.attached` 一定能取到 App）；其余通知一律「取不到 App 就跳过」，容忍早于 ready 的情形。

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
mod views;
mod workflow;

use std::sync::OnceLock;

use atelyx_plugin_sdk::protocol::method::{
    CanvasResizedParams, ProcessEventParams, SurfaceAttachedParams, SurfaceDetachedParams,
    ToolCallParams, ViewEventParams,
};
use atelyx_plugin_sdk::{run_stdio, Context, PluginHandler, PluginInfo};

use crate::app::App;

struct Handler {
    app: OnceLock<std::sync::Arc<App>>,
}

impl Handler {
    fn app(&self) -> Option<&std::sync::Arc<App>> {
        self.app.get()
    }
}

#[async_trait::async_trait]
impl PluginHandler for Handler {
    async fn on_ready(&self, ctx: &Context) {
        let app = App::bootstrap(std::sync::Arc::new(ctx.clone())).await;
        if self.app.set(app.clone()).is_err() {
            return; // 重复 ready：首个 App 已在服役
        }
        app.after_ready().await;
    }

    async fn on_view_event(&self, _ctx: &Context, params: ViewEventParams) {
        if let Some(app) = self.app() {
            app.on_view_event(params).await;
        }
    }

    async fn on_surface_attached(&self, _ctx: &Context, params: SurfaceAttachedParams) {
        if let Some(app) = self.app() {
            app.on_surface_attached(params);
        }
    }

    async fn on_surface_detached(&self, _ctx: &Context, params: SurfaceDetachedParams) {
        if let Some(app) = self.app() {
            app.on_surface_detached(params);
        }
    }

    async fn on_process_event(&self, _ctx: &Context, params: ProcessEventParams) {
        if let Some(app) = self.app() {
            controller::on_process_event(app, params).await;
        }
    }

    async fn on_canvas_resized(&self, _ctx: &Context, params: CanvasResizedParams) {
        if let Some(app) = self.app() {
            app.on_canvas_resized(params);
        }
    }

    async fn on_tool_call(&self, ctx: &Context, params: ToolCallParams) {
        // 工具调用只可能来自已注册的本插件工具，正常路径 App 一定在；仍未就绪时按未处理回报。
        if let Some(app) = self.app() {
            agent::on_tool_call(app, params).await;
        } else {
            let _ = ctx.tool_result(&params.call_id, "插件尚未就绪", true).await;
        }
    }
}

#[tokio::main]
async fn main() {
    let plugin = PluginInfo {
        id: "com.atelyx.comfyui-panel".to_string(),
        name: "ComfyUI".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Err(err) = run_stdio(plugin, Handler { app: OnceLock::new() }).await {
        eprintln!("插件退出：{err}");
        std::process::exit(1);
    }
}
