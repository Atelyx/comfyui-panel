//! 两面整面帧的驱动：先取面尺寸 → 锁外建快照 → 加控件层锁提交一帧。
//!
//! 原始输入先经控件层翻成语义事件（焦点 / 光标 / 选区 / 滚动就地更新），分派成状态迁移后再重提一帧；
//! 同一面的输入由 SDK 串行回调，逐条按到达顺序处理，不另起任务。

use std::sync::Arc;
use std::time::Duration;

use atelyx_plugin_protocol::frame::{FrameInput, FrameInputParams, Key, KeyAction, NamedKey};
use atelyx_plugin_sdk::frame_ui::UiEvent;
use serde_json::Value;

use crate::app::{self, App, SURFACE_PANEL, SURFACE_SETTINGS};
use crate::gallery;
use crate::settings::ProcessMode;
use crate::views::{
    self, PanelSnapshot, SettingsSnapshot, P_BOOL, P_CLEAR, P_COMBO, P_GROUP, P_NUM, P_PASTE, P_PICK, P_REF_PICK, P_SEED,
    P_TEXT, P_UPLOAD,
};

/// 整面重提的合帧间隔：轮询拍与逐键输入并成一次提交。
const RENDER_GAP: Duration = Duration::from_millis(120);

/// 生成面板提交一帧。面尺寸只在控件层里（构建闭包读不到），先取尺寸再算快照。
/// 控件层锁分两次取：算快照要读业务锁，两把锁不能嵌套成环。
pub async fn render_panel(app: &Arc<App>) {
    let surface = app.panel_ui.lock().await.size();
    let snapshot = PanelSnapshot::read(&mut app.core(), surface);
    let mut ui = app.panel_ui.lock().await;
    if let Err(err) = ui.render(&app.ctx, SURFACE_PANEL, move |ui| views::panel_frame(ui, &snapshot)).await {
        eprintln!("生成面板提交失败：{err}");
    }
}

/// 设置页提交一帧：快照在锁外构建，渲染期间不持业务锁。
pub async fn render_settings(app: &Arc<App>) {
    let snapshot = {
        let core = app.core();
        SettingsSnapshot::read(&core)
    };
    let mut ui = app.settings_ui.lock().await;
    if let Err(err) = ui.render(&app.ctx, SURFACE_SETTINGS, move |ui| views::settings_frame(ui, &snapshot)).await {
        eprintln!("设置页提交失败：{err}");
    }
}

/// 面帧的原始输入：翻成语义事件 → 逐条应用 → 原始按键里的收起 → 重提一帧。
pub async fn handle_frame_input(app: &Arc<App>, params: FrameInputParams) {
    match params.surface_id.as_str() {
        SURFACE_PANEL => {
            let events = {
                let mut ui = app.panel_ui.lock().await;
                ui.event(&app.ctx, &params.input, |id| panel_value(app, id)).await
            };
            for event in events {
                apply_panel(app, event).await;
            }
            dismiss_on_escape(app, &params.input);
            render_panel(app).await;
        }
        SURFACE_SETTINGS => {
            let events = {
                let mut ui = app.settings_ui.lock().await;
                ui.event(&app.ctx, &params.input, |id| settings_value(app, id)).await
            };
            for event in events {
                apply_settings(app, event).await;
            }
            render_settings(app).await;
        }
        other => eprintln!("收到未知面的输入：{other}"),
    }
}

pub async fn render_loop(app: Arc<App>) {
    let mut rx = app.changes();
    // 订阅前的唤醒不可感知（watch 不补发）：先按当前状态渲染一遍已挂载的面
    render_attached(&app).await;
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        tokio::time::sleep(RENDER_GAP).await;
        render_attached(&app).await;
    }
}

async fn render_attached(app: &Arc<App>) {
    let attached: Vec<String> = app.core().attached.iter().cloned().collect();
    for surface in attached {
        match surface.as_str() {
            SURFACE_PANEL => render_panel(app).await,
            SURFACE_SETTINGS => render_settings(app).await,
            _ => {}
        }
    }
}

// ---------- 生成面板 ----------

/// 输入框的编辑基线：编辑永远从草稿 / 会话事实出发，不从上一帧的绘制快照出发。
fn panel_value(app: &Arc<App>, id: &str) -> Option<String> {
    let core = app.core();
    if let Some(key) = id.strip_prefix(P_NUM).or_else(|| id.strip_prefix(P_TEXT)) {
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

async fn click_panel(app: &Arc<App>, id: &str) {
    if click_flow(app, id).await {
        return;
    }
    match id {
        views::ID_REFRESH => app.refresh_status().await,
        views::ID_OPEN_BROWSER => app.open_browser().await,
        views::ID_CLEAR_ERROR => app.clear_error(),
        views::ID_START => app.start_service(),
        views::ID_STOP => app.stop_service(),
        views::ID_FREE => app.free_memory().await,
        views::ID_RUN => app.run_generation().await,
        views::ID_INTERRUPT => app.interrupt().await,
        views::ID_SAVE_WF => app.save_current_workflow().await,
        views::ID_WF => app.open_workflow_picker(),
        views::ID_FLOW_MORE => app.show_earlier(),
        views::ID_PARAMS_TOGGLE => app.toggle_params(),
        views::ID_PARAMS_CLOSE => app.close_params(),
        views::ID_PROMPT_FIELD => app.open_prompt_picker(),
        views::ID_RATIO | views::ID_RESOLUTION | views::ID_COUNT => app.open_size_picker(id),
        views::ID_PICKER_CLOSE => app.close_picker(),
        views::ID_LB_CLOSE => app.close_lightbox(),
        views::ID_LB_COPY_PROMPT => {
            if let Some(prompt_id) = lightbox_prompt_id(app) {
                app.copy_prompt(&prompt_id).await;
            }
        }
        views::ID_LB_COPY_IMAGE => {
            if let Some(key) = lightbox_key(app) {
                app.copy_result_image(key).await;
            }
        }
        views::ID_LB_ARCHIVE => {
            if let Some(key) = lightbox_key(app) {
                app.archive_result(key).await;
            }
        }
        views::ID_LB_SAVE_AS => {
            if let Some(key) = lightbox_key(app) {
                app.save_result_as(key).await;
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
        Some(gallery::FlowAction::Save(key)) => app.archive_result(key).await,
        Some(gallery::FlowAction::CopyPrompt(prompt_id)) => app.copy_prompt(&prompt_id).await,
        Some(gallery::FlowAction::CopyRun) => app.copy_running_prompt().await,
        Some(gallery::FlowAction::Interrupt) => app.interrupt().await,
        None => return false,
    }
    true
}

/// 选择面板、节点分组与字段控件：id 前缀即控件类型，键为 `${节点id}.${输入名}`。
async fn click_control(app: &Arc<App>, id: &str) {
    if let Some(index) = id.strip_prefix(P_PICK) {
        let action = {
            let mut core = app.core();
            let picker = core.session.picker.take();
            picker.and_then(|picker| index.parse::<usize>().ok().and_then(|index| picker.options.get(index).map(|option| option.action.clone())))
        };
        app.invalidate();
        if let Some(action) = action {
            app.apply_picker_action(action).await;
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
        Some(P_UPLOAD) => app.upload_reference(key.to_string()).await,
        Some(P_PASTE) => app.paste_reference(key.to_string()).await,
        Some(P_CLEAR) => app.clear_reference(key),
        _ => {}
    }
}

/// Esc 收起最上层的行内面板：选择面板 → 灯箱 → 高级参数（面板态没有输入控件占焦点）。
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
            views::SET_DIR_PICK => app.pick_comfy_dir().await,
            views::SET_MODE_MANAGED => app.set_process_mode(ProcessMode::Managed),
            views::SET_MODE_EXTERNAL => app.set_process_mode(ProcessMode::External),
            _ => {}
        },
        _ => {}
    }
}
