//! 两面的绘制与输入：宿主重绘拍里按当前状态同步出画；
//! 原始输入经控件层翻成语义事件（焦点 / 光标 / 选区 / 滚动就地更新），分派成状态迁移后再请求重绘。

use std::sync::Arc;
use std::time::Duration;

use atelyx_plugin_protocol::frame::{FrameInput, FrameInputParams, Key, KeyAction, NamedKey};
use atelyx_plugin_sdk::frame_ui::UiEvent;
use atelyx_plugin_sdk::{DrawFrame, FrameUi};
use serde_json::Value;

use crate::app::{self, App, SURFACE_PANEL, SURFACE_SETTINGS};
use crate::gallery;
use crate::settings::ProcessMode;
use crate::views::{
    self, PanelSnapshot, SettingsSnapshot, P_BOOL, P_CLEAR, P_COMBO, P_GROUP, P_NUM, P_PASTE, P_PICK, P_REF_PICK, P_SEED,
    P_TEXT, P_UPLOAD,
};

/// 整面重绘的合帧间隔：轮询拍与逐键输入并成一次请求。
const RENDER_GAP: Duration = Duration::from_millis(120);

/// 宿主重绘拍里按面同步绘制：只读当前状态，面尺寸由 `DrawFrame` 给出。
pub fn draw_surface(app: &Arc<App>, frame: &mut DrawFrame<'_, '_>) {
    match frame.surface_id() {
        SURFACE_PANEL => {
            let snapshot = PanelSnapshot::read(&mut app.core(), frame.size());
            let _ = frame.build(|ui| views::panel_frame(ui, &snapshot));
        }
        SURFACE_SETTINGS => {
            let snapshot = {
                let core = app.core();
                SettingsSnapshot::read(&core)
            };
            let _ = frame.build(|ui| views::settings_frame(ui, &snapshot));
        }
        _ => {}
    }
}

/// 面帧的原始输入：翻成语义事件 → 逐条应用 → 原始按键里的收起 → 请求重绘。
pub async fn handle_frame_input(app: &Arc<App>, params: FrameInputParams, ui: &mut FrameUi) {
    match params.surface_id.as_str() {
        SURFACE_PANEL => {
            // 选择浮层要贴住触发控件：记下指针落点，供下一步开菜单时换算控件上沿。
            if let FrameInput::PointerDown { x, y, .. } | FrameInput::PointerUp { x, y, .. } = &params.input {
                app.core().session.pointer = Some((*x, *y));
            }
            let events = ui
                .event(&app.ctx, &params.input, |id| panel_value(app, id))
                .await;
            for event in events {
                apply_panel(app, event).await;
            }
            dismiss_on_escape(app, &params.input);
        }
        SURFACE_SETTINGS => {
            let events = ui
                .event(&app.ctx, &params.input, |id| settings_value(app, id))
                .await;
            for event in events {
                apply_settings(app, event).await;
            }
        }
        other => {
            app.ctx.log_error(&format!("收到未知面的输入：{other}"));
            return;
        }
    }
    request_redraw(app).await;
}

/// 请求宿主重绘一次：绘制在下一拍由 [`draw_surface`] 完成。
async fn request_redraw(app: &Arc<App>) {
    if let Err(err) = app.ctx.request_redraw().await {
        app.ctx.log_error(&format!("请求重绘失败：{err}"));
    }
}

pub async fn render_loop(app: Arc<App>) {
    let mut rx = app.changes();
    let mut stop = app.stop_signal();
    // 订阅前的唤醒不可感知（watch 不补发）：先按当前状态渲染一遍已挂载的面
    render_attached(&app).await;
    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = stop.changed() => return,
        }
        if !app.sleep_or_stop(RENDER_GAP).await {
            return;
        }
        render_attached(&app).await;
    }
}

/// 状态变了就请求宿主重绘（整窗重绘，各面在重绘拍里按当前状态出画）。
async fn render_attached(app: &Arc<App>) {
    request_redraw(app).await;
}

// ---------- 生成面板 ----------

/// 输入框的编辑基线：编辑永远从草稿 / 会话事实出发，不从上一帧的绘制快照出发。
fn panel_value(app: &Arc<App>, id: &str) -> Option<String> {
    let core = app.core();
    if let Some(key) = id.strip_prefix(P_NUM) {
        return app::number_text(&core, key);
    }
    if let Some(key) = id.strip_prefix(P_TEXT) {
        return app::field_text(&core, key);
    }
    if let Some((input, _)) = views::QUICK_NUMBER_FIELDS.iter().find(|(_, field_id)| *field_id == id) {
        return app::quick_value(&core, input);
    }
    match id {
        views::ID_PROMPT_TEXT => app::prompt_value(&core),
        views::ID_PARAM_FILTER => Some(core.session.param_filter.clone()),
        _ => None,
    }
}

async fn apply_panel(app: &Arc<App>, event: UiEvent) {
    match event {
        UiEvent::Click { id } => click_panel(app, &id).await,
        UiEvent::Text { id, value } => text_panel(app, &id, value),
        UiEvent::Scroll { id, offset, max_offset } if id == views::ID_FLOW => set_flow_end(app, offset, max_offset),
        _ => {}
    }
}

fn text_panel(app: &Arc<App>, id: &str, value: String) {
    if let Some(key) = id.strip_prefix(P_NUM) {
        app.write_field_value(key, app::text_number(&value));
    } else if let Some(key) = id.strip_prefix(P_TEXT) {
        app.write_field_value(key, Value::String(value));
    } else if let Some((input, _)) = views::QUICK_NUMBER_FIELDS.iter().find(|(_, field_id)| *field_id == id) {
        app.write_quick_number(input, app::text_number(&value));
    } else if id == views::ID_PROMPT_TEXT {
        app.write_prompt_text(Value::String(value));
    } else if id == views::ID_PARAM_FILTER {
        app.set_param_filter(value);
    }
}

/// 记录流滚动位置：只记「是否停在末端」，决定新结果要不要贴底。
fn set_flow_end(app: &Arc<App>, offset: f32, max_offset: f32) {
    let at_end = max_offset <= 0.0 || offset >= max_offset - 0.5;
    let changed = {
        let mut core = app.core();
        let changed = core.session.flow_at_bottom != at_end;
        core.session.flow_at_bottom = at_end;
        changed
    };
    if changed {
        app.invalidate();
    }
}

fn lightbox_key(app: &Arc<App>) -> Option<String> {
    app.core().session.lightbox_key.clone()
}

fn lightbox_prompt_id(app: &Arc<App>) -> Option<String> {
    let core = app.core();
    let key = core.session.lightbox_key.as_ref()?;
    core.runtime.snap.results.iter().find(|item| &item.key == key).map(|item| item.prompt_id.clone())
}

/// 把需要网络/磁盘/对话框的动作挪到后台运行时：宿主通知 worker 是串行的，
/// 在输入回调里 await 慢操作会挡住其后的全部通知。
async fn click_panel(app: &Arc<App>, id: &str) {
    if click_flow(app, id).await {
        return;
    }
    match id {
        views::ID_REFRESH => {
            let worker = app.clone();
            app.spawn_task(async move { worker.refresh_status().await });
        }
        views::ID_OPEN_BROWSER => {
            let worker = app.clone();
            app.spawn_task(async move { worker.open_browser().await });
        }
        views::ID_CLEAR_ERROR => app.clear_error(),
        views::ID_START => app.start_service(),
        views::ID_STOP => app.stop_service(),
        views::ID_FREE => {
            let worker = app.clone();
            app.spawn_task(async move { worker.free_memory().await });
        }
        views::ID_RUN => {
            let worker = app.clone();
            app.spawn_task(async move { worker.run_generation().await });
        }
        views::ID_INTERRUPT => {
            let worker = app.clone();
            app.spawn_task(async move { worker.interrupt().await });
        }
        views::ID_SAVE_WF => {
            let worker = app.clone();
            app.spawn_task(async move { worker.save_current_workflow().await });
        }
        views::ID_WF => app.open_workflow_picker(),
        views::ID_EMPTY_WF => app.open_workflow_picker(),
        views::ID_EMPTY_START => app.start_service(),
        views::ID_EMPTY_REFRESH => {
            let worker = app.clone();
            app.spawn_task(async move { worker.refresh_status().await });
        }
        views::ID_FLOW_MORE => app.show_earlier(),
        views::ID_PARAMS_TOGGLE => app.toggle_params(),
        views::ID_PARAMS_CLOSE => app.close_params(),
        views::ID_PROMPT_FIELD => app.open_prompt_picker(),
        views::ID_RATIO | views::ID_RESOLUTION | views::ID_COUNT => app.open_size_picker(id),
        views::ID_PICKER_DISMISS => app.close_picker(),
        views::ID_LB_DISMISS => app.close_lightbox(),
        views::ID_LB_COPY_PROMPT => {
            if let Some(prompt_id) = lightbox_prompt_id(app) {
                let worker = app.clone();
                app.spawn_task(async move { worker.copy_prompt(&prompt_id).await });
            }
        }
        views::ID_LB_COPY_IMAGE => {
            if let Some(key) = lightbox_key(app) {
                let worker = app.clone();
                app.spawn_task(async move { worker.copy_result_image(key).await });
            }
        }
        views::ID_LB_ARCHIVE => {
            if let Some(key) = lightbox_key(app) {
                let worker = app.clone();
                app.spawn_task(async move { worker.archive_result(key).await });
            }
        }
        views::ID_LB_SAVE_AS => {
            if let Some(key) = lightbox_key(app) {
                let worker = app.clone();
                app.spawn_task(async move { worker.save_result_as(key).await });
            }
        }
        _ => click_control(app, id).await,
    }
}

/// 记录流画布的命中 id：动作按上一版布局回查，用户点的就是他刚看到的那一版。
/// 返回 false 表示该 id 不是画布命中，交固定控件分派。
async fn click_flow(app: &Arc<App>, id: &str) -> bool {
    let action = {
        let core = app.core();
        core.session.flow.as_ref().and_then(|flow| flow.action_of(id)).cloned()
    };
    match action {
        Some(gallery::FlowAction::Preview(key)) => app.open_lightbox(key).await,
        Some(gallery::FlowAction::Save(key)) => {
            let worker = app.clone();
            app.spawn_task(async move { worker.archive_result(key).await });
        }
        Some(gallery::FlowAction::CopyPrompt(prompt_id)) => {
            let worker = app.clone();
            app.spawn_task(async move { worker.copy_prompt(&prompt_id).await });
        }
        Some(gallery::FlowAction::CopyRun) => {
            let worker = app.clone();
            app.spawn_task(async move { worker.copy_running_prompt().await });
        }
        Some(gallery::FlowAction::Interrupt) => {
            let worker = app.clone();
            app.spawn_task(async move { worker.interrupt().await });
        }
        None => return false,
    }
    true
}

/// 选择浮层、节点分组与字段控件：id 前缀即控件类型，键为 `${节点id}.${输入名}`。
async fn click_control(app: &Arc<App>, id: &str) {
    if let Some(id) = id.strip_prefix(P_PICK) {
        let action = {
            let mut core = app.core();
            core.session.picker.take().and_then(|picker| {
                picker
                    .ids
                    .iter()
                    .position(|item| item.as_str() == id)
                    .map(|index| picker.options[index].action.clone())
            })
        };
        app.invalidate();
        if let Some(action) = action {
            let worker = app.clone();
            app.spawn_task(async move { worker.apply_picker_action(action).await });
        }
        return;
    }
    if let Some(node_id) = id.strip_prefix(P_GROUP) {
        app.toggle_group(node_id);
        return;
    }
    if let Some(key) = id.strip_prefix(P_REF_PICK) {
        app.open_ref_picker(key);
        return;
    }
    let (prefix, key) = app::split_prefixed(id);
    match prefix {
        Some(P_COMBO) => app.open_combo_picker(key),
        Some(P_BOOL) => app.toggle_bool(key),
        Some(P_SEED) => {
            let on = {
                let core = app.core();
                !core.session.seed_mode.contains(key)
            };
            app.toggle_seed(key, on);
        }
        Some(P_UPLOAD) => {
            let key = key.to_string();
            let worker = app.clone();
            app.spawn_task(async move { worker.upload_reference(key).await });
        }
        Some(P_PASTE) => {
            let key = key.to_string();
            let worker = app.clone();
            app.spawn_task(async move { worker.paste_reference(key).await });
        }
        Some(P_CLEAR) => app.clear_reference(key),
        _ => {}
    }
}

/// Esc 收起最上层的行内面板：选择浮层 → 灯箱 → 高级参数（面板态没有输入控件占焦点）。
fn dismiss_on_escape(app: &Arc<App>, input: &FrameInput) {
    let FrameInput::Key { key: Key::Named { name: NamedKey::Escape }, action: KeyAction::Press, .. } = input else {
        return;
    };
    let (picker, lightbox, params) = {
        let core = app.core();
        (core.session.picker.is_some(), core.session.lightbox_key.is_some(), core.session.params_open)
    };
    if picker {
        app.close_picker();
    } else if lightbox {
        app.close_lightbox();
    } else if params {
        app.close_params();
    }
}

// ---------- 设置页 ----------

fn settings_value(app: &Arc<App>, id: &str) -> Option<String> {
    let core = app.core();
    match id {
        views::SET_HOST => Some(core.settings.host.clone()),
        views::SET_PORT => Some(core.settings.port.to_string()),
        views::SET_DIR => Some(core.settings.comfy_dir.clone()),
        views::SET_PYTHON => Some(core.settings.python_path.clone()),
        views::SET_EXTRA => Some(core.settings.extra_args.clone()),
        views::SET_ARCHIVE => Some(core.settings.archive_folder.clone()),
        _ => None,
    }
}

async fn apply_settings(app: &Arc<App>, event: UiEvent) {
    match event {
        UiEvent::Text { id, value } => app.apply_settings_text(&id, &value),
        UiEvent::Click { id } => match id.as_str() {
            views::ID_START => app.start_service(),
            views::ID_STOP => app.stop_service(),
            views::ID_UPDATE => app.start_update(),
            views::ID_CLEAR_ERROR => app.clear_error(),
            views::SET_DIR_PICK => {
                let worker = app.clone();
                app.spawn_task(async move { worker.pick_comfy_dir().await });
            }
            views::SET_MODE_MANAGED => app.set_process_mode(ProcessMode::Managed),
            views::SET_MODE_EXTERNAL => app.set_process_mode(ProcessMode::External),
            _ => {}
        },
        _ => {}
    }
}
