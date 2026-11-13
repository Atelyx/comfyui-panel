//! 生成面板与设置页的整面帧声明：把 `Core` 快照投影成控件，纯函数、无副作用、无 IO。
//!
//! id 方案即事件契约（app 据此分派）：静态动作固定串；动态控件用前缀 + 键 `${节点id}.${输入名}`，
//! 数值/文本/布尔/下拉分前缀定型。记录流的图格命中 id 由 `gallery::layout` 给出，本模块只负责画布。
//! 面板内的展开区（高级参数 / 选择 / 灯箱）都是行内面板，不做浮层。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use atelyx_plugin_protocol::paint::{ColorToken, ImageFit, ImageSource};
use atelyx_plugin_sdk::frame_ui::{TextRole, TextStyle, Ui};

use crate::app::{Core, Picker};
use crate::gallery::{self, FlowLayout};
use crate::history::ResultImage;
use crate::runtime::{build_fields_for, Channel};
use crate::settings::ProcessMode;
use crate::workflow::files;
use crate::workflow::form::{self, FieldKind, FieldSpec};
use crate::workflow::sizing::{self, SECTION_COUNT, SECTION_RATIO, SECTION_RESOLUTION};

// ---------- 生成面板静态节点 id ----------
pub const ID_REFRESH: &str = "refresh";
pub const ID_OPEN_BROWSER: &str = "open_browser";
pub const ID_CLEAR_ERROR: &str = "clear_error";
pub const ID_START: &str = "start_service";
pub const ID_STOP: &str = "stop_service";
pub const ID_FREE: &str = "free_memory";
pub const ID_RUN: &str = "run";
pub const ID_INTERRUPT: &str = "interrupt";
pub const ID_SAVE_WF: &str = "save_workflow";
pub const ID_WF: &str = "workflow_select";
/// 记录流的滚动区（滚动位置回发决定新结果是否贴底）。
pub const ID_FLOW: &str = "flow";
/// 「显示更早」：记录流一次多展开一批历史。
pub const ID_FLOW_MORE: &str = "flow_more";
pub const ID_PICKER_CLOSE: &str = "picker_close";
pub const P_PICK: &str = "pick:";
pub const ID_PARAMS_TOGGLE: &str = "params_toggle";
pub const ID_PARAM_FILTER: &str = "param_filter";
pub const ID_PARAMS_CLOSE: &str = "params_close";
pub const ID_PROMPT_FIELD: &str = "prompt_field";
pub const ID_PROMPT_TEXT: &str = "prompt_text";
pub const ID_RATIO: &str = "size_ratio";
pub const ID_RESOLUTION: &str = "size_resolution";
pub const ID_COUNT: &str = "size_count";
pub const ID_STEPS: &str = "quick_steps";
pub const ID_CFG: &str = "quick_cfg";
/// 快捷数值框的「输入名 → 节点 id」对应表：视图渲染与事件分派两侧同源。
pub const QUICK_NUMBER_FIELDS: &[(&str, &str)] = &[("steps", ID_STEPS), ("cfg", ID_CFG)];

// ---------- 灯箱静态节点 id ----------
pub const ID_LB_CLOSE: &str = "lb_close";
pub const ID_LB_COPY_PROMPT: &str = "lb_copy_prompt";
pub const ID_LB_COPY_IMAGE: &str = "lb_copy_image";
pub const ID_LB_ARCHIVE: &str = "lb_archive";
pub const ID_LB_SAVE_AS: &str = "lb_save_as";

// ---------- 维护 ----------
pub const ID_UPDATE: &str = "update";

// ---------- 设置页静态节点 id ----------
pub const SET_HOST: &str = "set_host";
pub const SET_PORT: &str = "set_port";
pub const SET_MODE_MANAGED: &str = "set_mode_managed";
pub const SET_MODE_EXTERNAL: &str = "set_mode_external";
pub const SET_DIR: &str = "set_dir";
pub const SET_DIR_PICK: &str = "set_dir_pick";
pub const SET_PYTHON: &str = "set_python";
pub const SET_EXTRA: &str = "set_extra";
pub const SET_ARCHIVE: &str = "set_archive";

// ---------- 动态控件 id 前缀 ----------
pub const P_COMBO: &str = "c:";
pub const P_NUM: &str = "n:";
pub const P_TEXT: &str = "t:";
pub const P_BOOL: &str = "b:";
pub const P_SEED: &str = "srand:";
pub const P_UPLOAD: &str = "up:";
pub const P_PASTE: &str = "cpaste:";
pub const P_CLEAR: &str = "clr:";
pub const P_REF_PICK: &str = "rpick:";
pub const P_GROUP: &str = "g:";

/// 面板根内边距（宽度计算与渲染同一口径）。
pub const PAD: f32 = 12.0;
/// 字节换算 GB 的基数（显存显示用）。
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// 高级参数展开区的自限高度：矮面板上也留给记录流可见空间。
const PARAMS_H: f32 = 300.0;
/// 选择面板列表的自限高度。
const PICKER_H: f32 = 320.0;
/// 选择面板里单个候选按钮的估高与行距。
const PICKER_ITEM_H: f32 = 24.0;
const PICKER_ITEM_GAP: f32 = 6.0;
/// 参考图缩略图高度。
const REF_THUMB_H: f32 = 72.0;
/// 主提示词输入区高度。
const PROMPT_H: f32 = 96.0;

fn picker_height(options: usize) -> f32 {
    let rows = options.max(1) as f32;
    (rows * PICKER_ITEM_H + (rows - 1.0) * PICKER_ITEM_GAP).clamp(PICKER_ITEM_H, PICKER_H)
}

// ---------- 快照 ----------

/// 状态条：连接态、进程态与队列概览。
pub struct StatusView {
    pub managed: bool,
    pub starting: bool,
    pub running: bool,
    pub updating: bool,
    pub probing: bool,
    pub direct: bool,
    pub host: String,
    pub port: u16,
    pub queue: Option<String>,
    /// 设备显存占用（剩余 / 总量），无系统统计时不显示。
    pub vram: Option<String>,
}

pub struct WorkflowView {
    pub current: String,
    pub has: bool,
    pub error: String,
    pub changed: bool,
}

pub struct RefView {
    pub key: String,
    pub name: String,
    pub thumb: Option<String>,
    pub can_pick: bool,
    pub filled: bool,
    pub online: bool,
}

pub struct PromptView {
    pub value: String,
    pub source: String,
    pub candidates: usize,
}

pub struct SizeCell {
    pub id: &'static str,
    pub label: &'static str,
    pub selected: String,
}

pub struct NumberCell {
    pub id: &'static str,
    pub label: String,
    pub value: String,
}

pub struct SizingView {
    pub cells: Vec<SizeCell>,
    pub numbers: Vec<NumberCell>,
    pub summary: String,
}

#[derive(Clone)]
pub enum FieldControl {
    Connected(String),
    Combo { id: String, selected: String },
    Number { id: String, value: String },
    Text { id: String, value: String, multiline: bool },
    Bool { id: String, checked: bool },
}

pub struct FieldView {
    pub label: String,
    pub control: FieldControl,
    pub seed: Option<String>,
    pub seed_on: bool,
}

pub struct GroupView {
    pub toggle: String,
    pub title: String,
    pub open: bool,
    pub fields: Vec<FieldView>,
}

pub struct AdvancedView {
    pub filter: String,
    pub groups: Vec<GroupView>,
}

pub struct PickerView {
    pub title: String,
    /// (稳定命中 id, 展示文本, 是否当前值)
    pub options: Vec<(String, String, bool)>,
}

pub struct LightboxView {
    pub title: String,
    pub file: String,
    pub source: Option<String>,
    pub image_height: f32,
    pub can_copy_prompt: bool,
    pub saved: bool,
}

/// 生成面板一帧的全部输入（锁内构建，渲染闭包只读）。
pub struct PanelSnapshot {
    pub width: f32,
    pub status: StatusView,
    pub error: String,
    pub notice: String,
    /// 上一次提交的记录流布局：命中 id → 动作由 app 回查。
    pub flow: Option<Arc<FlowLayout>>,
    pub flow_at_bottom: bool,
    pub hidden_batches: usize,
    pub workflow: WorkflowView,
    pub refs: Vec<RefView>,
    pub prompt: Option<PromptView>,
    pub sizing: Option<SizingView>,
    pub online: bool,
    pub has_draft: bool,
    pub running: bool,
    pub advanced_total: usize,
    pub params_open: bool,
    pub advanced: Option<AdvancedView>,
    pub picker: Option<PickerView>,
    pub lightbox: Option<LightboxView>,
}

impl PanelSnapshot {
    /// 读一帧的面板快照；`surface` 为面尺寸（控件层最近一次上报）。
    /// 记录流布局在这里定型并写回 `session.flow`，供随后的点击命中回查动作。
    pub fn read(core: &mut Core, surface: (f32, f32)) -> Self {
        let width = (surface.0 - PAD * 2.0).max(0.0);
        let layout = gallery::layout(&crate::app::flow_input(core, width));
        let flow = (!layout.hits.is_empty()).then(|| Arc::new(layout));
        core.session.flow = flow.clone();
        // 灯箱条目被历史淘汰后自动收起，不留空面板。
        if let Some(key) = core.session.lightbox_key.clone() {
            if !core.runtime.snap.results.iter().any(|item| item.key == key) {
                core.session.lightbox_key = None;
            }
        }

        let fields = build_fields_for(&core.session.draft, &core.runtime.snap);
        let snap = &core.runtime.snap;
        let online = snap.channel == Channel::Direct;
        let session = &core.session;

        let status = StatusView {
            managed: core.settings.process_mode == ProcessMode::Managed,
            starting: core.host.starting,
            running: core.host.running,
            updating: core.host.updating,
            probing: snap.probing,
            direct: online,
            host: core.settings.host.clone(),
            port: core.settings.port,
            queue: online.then(|| format!("运行 {} · 等待 {}", snap.queue_running.len(), snap.queue_pending.len())),
            vram: crate::comfy::types::vram_of(snap.system_stats.as_ref())
                .map(|(free, total)| format!("显存 {:.1}/{:.1} GB", free / GIB, total / GIB)),
        };

        let current = session.workflow_id.as_ref().and_then(|id| {
            snap.workflows
                .iter()
                .find(|file| file.path == *id)
                .map(|file| files::workflow_display_name(&file.name))
        });
        let workflow = WorkflowView {
            current: current.unwrap_or_default(),
            has: session.workflow_id.is_some(),
            error: snap.workflows_error.clone(),
            changed: session.current_changed,
        };

        let refs = form::ref_slots(&fields)
            .into_iter()
            .map(|slot| {
                let key = slot.key();
                let filename = form::display_value(form::read_field(&session.draft, slot).as_ref());
                let thumb = if filename.is_empty() { None } else { session.refs.get(&filename).map(str::to_string) };
                RefView {
                    can_pick: !form::image_candidates(snap.object_info.as_ref(), &slot.class_type).is_empty(),
                    filled: !filename.is_empty(),
                    thumb,
                    online,
                    name: filename,
                    key,
                }
            })
            .collect();

        let prompt = form::resolve_prompt_field(&fields, session.prompt_key.as_deref()).map(|(field, explicit)| {
            let source = if explicit {
                format!("{}.{}", field.node_label, form::field_label(&field.input))
            } else {
                "自动".to_string()
            };
            PromptView {
                value: form::display_value(form::read_field(&session.draft, field).as_ref()),
                source,
                candidates: form::prompt_candidates(&fields).len(),
            }
        });

        let sizing = {
            let plan = sizing::plan(&session.draft, &fields);
            let mut cells = Vec::new();
            for (section_id, id) in [(SECTION_RATIO, ID_RATIO), (SECTION_RESOLUTION, ID_RESOLUTION), (SECTION_COUNT, ID_COUNT)] {
                let Some(section) = plan.sections.iter().find(|section| section.id == section_id) else { continue };
                let selected = section
                    .selected
                    .as_ref()
                    .and_then(|value| section.options.iter().find(|option| &option.value == value))
                    .map(|option| option.label.clone())
                    .unwrap_or_default();
                cells.push(SizeCell { id, label: section.label, selected });
            }
            let numbers: Vec<NumberCell> = QUICK_NUMBER_FIELDS
                .iter()
                .filter_map(|(input, id)| {
                    let field = sizing::find_unconnected(&fields, input)?;
                    let value = form::numeric_value(form::read_field(&session.draft, field).as_ref());
                    Some(NumberCell {
                        id,
                        label: form::field_label(input).to_string(),
                        value: value.map(|value| value.to_string()).unwrap_or_default(),
                    })
                })
                .collect();
            if cells.is_empty() && numbers.is_empty() {
                None
            } else {
                let summary = match &plan.detail {
                    Some(detail) => format!("{} · {}", plan.summary, detail),
                    None => plan.summary.clone(),
                };
                Some(SizingView { cells, numbers, summary })
            }
        };

        let advanced_total = form::form_fields(&fields).len();
        let advanced = session.params_open.then(|| {
            let groups = form::group_by_node(&form::filter_fields(&fields, &session.param_filter));
            AdvancedView {
                filter: session.param_filter.clone(),
                groups: groups
                    .into_iter()
                    .map(|group| {
                        let open = session.group_toggles.get(group.node_id).copied().unwrap_or(group.has_common);
                        GroupView {
                            toggle: format!("{P_GROUP}{}", group.node_id),
                            title: format!("{}（{} 项）", group.label, group.fields.len()),
                            open,
                            fields: if open {
                                group.fields.iter().map(|field| field_view(field, &session.draft, &session.seed_mode)).collect()
                            } else {
                                Vec::new()
                            },
                        }
                    })
                    .collect(),
            }
        });

        let picker = session.picker.as_ref().map(picker_view);
        let lightbox = session.lightbox_key.as_deref().and_then(|key| {
            let item = snap.results.iter().find(|item| item.key == key)?;
            Some(lightbox_view(item, ((surface.1 - PAD * 2.0) * 0.45).clamp(120.0, 420.0)))
        });

        Self {
            width: surface.0,
            status,
            error: snap.error.clone(),
            notice: session.notice.clone(),
            flow,
            flow_at_bottom: session.flow_at_bottom,
            hidden_batches: crate::app::hidden_batches(core),
            workflow,
            refs,
            prompt,
            sizing,
            online,
            has_draft: crate::app::has_draft(core),
            running: snap.running_prompt_id.is_some(),
            advanced_total,
            params_open: session.params_open,
            advanced,
            picker,
            lightbox,
        }
    }
}

fn picker_view(picker: &Picker) -> PickerView {
    PickerView {
        title: picker.title.clone(),
        options: picker
            .options
            .iter()
            .zip(&picker.ids)
            .map(|(option, id)| (id.clone(), option.label.clone(), option.current))
            .collect(),
    }
}

fn lightbox_view(item: &ResultImage, image_height: f32) -> LightboxView {
    LightboxView {
        title: if item.title.is_empty() { "未命名任务".to_string() } else { item.title.clone() },
        file: item.image_ref.filename.clone(),
        source: item.display_source(),
        image_height,
        can_copy_prompt: !item.prompt_text.is_empty(),
        saved: item.saved_path.is_some(),
    }
}

fn field_view(field: &FieldSpec, draft: &Value, seed_mode: &BTreeSet<String>) -> FieldView {
    let key = field.key();
    let label = input_label(field);
    let control = if field.connected {
        FieldControl::Connected(form::connected_display(field.link_from.as_deref()))
    } else {
        match field.kind {
            FieldKind::Combo => {
                let current = form::display_value(form::read_field(draft, field).as_ref());
                let selected = field
                    .options
                    .as_ref()
                    .and_then(|options| options.iter().find(|(value, _)| *value == current))
                    .map(|(_, label)| label.clone())
                    .unwrap_or(current);
                FieldControl::Combo { id: format!("{P_COMBO}{key}"), selected }
            }
            FieldKind::Int | FieldKind::Float => {
                let value = form::numeric_value(form::read_field(draft, field).as_ref());
                FieldControl::Number {
                    id: format!("{P_NUM}{key}"),
                    value: value.map(|value| value.to_string()).unwrap_or_default(),
                }
            }
            FieldKind::Boolean => {
                let checked = form::read_field(draft, field).and_then(|value| value.as_bool()).unwrap_or(false);
                FieldControl::Bool { id: format!("{P_BOOL}{key}"), checked }
            }
            FieldKind::Str | FieldKind::Json => FieldControl::Text {
                id: format!("{P_TEXT}{key}"),
                value: form::display_value(form::read_field(draft, field).as_ref()),
                multiline: field.multiline,
            },
        }
    };
    FieldView {
        label,
        control,
        seed_on: seed_mode.contains(&key),
        seed: (field.is_seed && !field.connected).then(|| format!("{P_SEED}{key}")),
    }
}

fn input_label(field: &FieldSpec) -> String {
    let named = form::field_label(&field.input);
    if named.is_empty() {
        field.input.clone()
    } else {
        named.to_string()
    }
}

// ---------- 生成面板 ----------

/// 面板四段：状态条与告警（固定）→ 记录流（占满剩余高度、自己滚）→ 生成坞（整宽）→ 展开区。
/// 灯箱打开时占据记录流的位置；高级参数、选择面板排在坞下方，随页滚动。
pub fn panel_frame(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    ui.pad(PAD, PAD, |page| {
        page.column(8.0, |cells| {
            status_bar(cells, snapshot);
            if !snapshot.error.is_empty() {
                cells.row(8.0, |row| {
                    row.button(ID_CLEAR_ERROR, "关闭");
                    row.text_styled(snapshot.error.clone(), TextStyle::new(TextRole::Label).color(ColorToken::Danger).wrap());
                });
            } else if !snapshot.notice.is_empty() {
                cells.caption(snapshot.notice.clone());
            }
            if let Some(lightbox) = &snapshot.lightbox {
                lightbox_panel(cells, lightbox, snapshot.width);
            } else {
                flow_section(cells, snapshot);
            }
            cells.rule();
            dock(cells, snapshot);
            if let Some(advanced) = &snapshot.advanced {
                advanced_panel(cells, advanced);
            }
            if let Some(picker) = &snapshot.picker {
                picker_panel(cells, picker);
            }
        });
    });
}

fn status_bar(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let status = &snapshot.status;
    ui.row(8.0, |row| {
        let (text, color) = if status.probing {
            ("连接中…".to_string(), ColorToken::TextMuted)
        } else if status.direct {
            (format!("已连接 {}:{}", status.host, status.port), ColorToken::Success)
        } else {
            ("未连接".to_string(), ColorToken::TextMuted)
        };
        row.text_styled(
            gallery::clip_text(&text, 260.0, 12.0),
            TextStyle::new(TextRole::Heading).color(color),
        );
        if let Some(queue) = &status.queue {
            row.caption(queue.clone());
        }
        if let Some(vram) = &status.vram {
            row.caption(vram.clone());
        }
        if status.updating {
            row.caption("更新中…");
        } else if status.managed {
            if status.starting {
                row.caption("启动中…");
                row.ghost_button(ID_STOP, "取消启动");
            } else if status.running {
                row.ghost_button(ID_STOP, "停止服务");
            } else {
                row.button(ID_START, "启动服务");
            }
        }
        row.icon_button(ID_REFRESH, atelyx_plugin_sdk::frame_ui::Icon::Refresh);
        row.button(ID_OPEN_BROWSER, "打开");
    });
}

fn flow_section(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    // 记录流占满剩余高度；停在末端时新结果贴底
    let flow = snapshot.flow.clone();
    let follow = snapshot.flow_at_bottom;
    ui.scroll_follow(ID_FLOW, 0.0, follow, move |area| match flow {
        Some(layout) => {
            area.canvas(layout.height, move |canvas| gallery::paint(canvas, &layout));
        }
        None => area.caption("尚无生成结果"),
    });
    if snapshot.hidden_batches > 0 {
        ui.ghost_button(ID_FLOW_MORE, format!("显示更早（{}）", snapshot.hidden_batches));
    }
}

fn lightbox_panel(ui: &mut Ui<'_, '_>, lightbox: &LightboxView, width: f32) {
    ui.panel(|card| {
        card.row(8.0, |head| {
            head.text(lightbox.title.clone(), TextRole::Heading);
            head.ghost_button(ID_LB_CLOSE, "关闭");
        });
        card.caption(lightbox.file.clone());
        match &lightbox.source {
            Some(path) => {
                card.image_fit(ImageSource::Private { path: path.clone() }, lightbox.image_height, ImageFit::Contain);
            }
            None => card.caption("副本未就绪"),
        }
        // 窄面下动作拆两行，避免最后两个按钮被裁到可视区外。
        if width < 420.0 {
            card.row(8.0, |actions| {
                actions.ghost_button_enabled(ID_LB_COPY_PROMPT, "复制提示词", lightbox.can_copy_prompt);
                actions.ghost_button(ID_LB_COPY_IMAGE, "复制图片");
            });
            card.row(8.0, |actions| {
                actions.ghost_button_enabled(
                    ID_LB_ARCHIVE,
                    if lightbox.saved { "已存到仓库" } else { "存到仓库" },
                    !lightbox.saved,
                );
                actions.ghost_button(ID_LB_SAVE_AS, "另存为…");
            });
        } else {
            card.row(8.0, |actions| {
                actions.ghost_button_enabled(ID_LB_COPY_PROMPT, "复制提示词", lightbox.can_copy_prompt);
                actions.ghost_button(ID_LB_COPY_IMAGE, "复制图片");
                actions.ghost_button_enabled(
                    ID_LB_ARCHIVE,
                    if lightbox.saved { "已存到仓库" } else { "存到仓库" },
                    !lightbox.saved,
                );
                actions.ghost_button(ID_LB_SAVE_AS, "另存为…");
            });
        }
    });
}

fn dock(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    ui.column(6.0, |dock| {
        workflow_row(dock, snapshot);
        ref_strip(dock, snapshot);
        if let Some(prompt) = &snapshot.prompt {
            prompt_block(dock, prompt);
        }
        if let Some(sizing) = &snapshot.sizing {
            sizing_block(dock, sizing, snapshot.width);
        }
        run_row(dock, snapshot);
        let title = if snapshot.params_open {
            "收起高级参数".to_string()
        } else {
            format!("高级参数（{}）", snapshot.advanced_total)
        };
        dock.ghost_button_enabled(ID_PARAMS_TOGGLE, title, snapshot.advanced_total > 0);
    });
}

fn workflow_row(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let workflow = &snapshot.workflow;
    ui.column(4.0, |block| {
        block.row(8.0, |row| {
            row.text("工作流", TextRole::Label);
            let label = if workflow.current.is_empty() {
                "选择工作流".to_string()
            } else {
                gallery::clip_text(&workflow.current, 180.0, 12.0)
            };
            row.button(ID_WF, label);
            row.ghost_button_enabled(ID_SAVE_WF, "存参", workflow.has);
        });
        if !workflow.error.is_empty() {
            block.notice(format!("工作流列表读取失败：{}", workflow.error), ColorToken::Warning);
        } else if workflow.changed {
            block.notice("文件已被外部修改，重新选择可载入最新内容".to_string(), ColorToken::Warning);
        }
    });
}

fn ref_strip(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    if snapshot.refs.is_empty() {
        return;
    }
    ui.column(4.0, |block| {
        block.field_label("参考图");
        block.row(6.0, |row| {
            for slot in &snapshot.refs {
                row.column(4.0, |card| {
                    match &slot.thumb {
                        Some(path) => card.image_fit(ImageSource::Private { path: path.clone() }, REF_THUMB_H, ImageFit::Cover),
                        None => {
                            let label = if slot.filled {
                                slot.name.clone()
                            } else if slot.online {
                                "未选择".to_string()
                            } else {
                                "离线".to_string()
                            };
                            card.caption(label);
                        }
                    }
                    card.row(4.0, |actions| {
                        actions.ghost_button_enabled(format!("{P_UPLOAD}{}", slot.key), "上传", slot.online);
                        if slot.can_pick {
                            actions.ghost_button_enabled(format!("{P_REF_PICK}{}", slot.key), "选", slot.online);
                        }
                        actions.ghost_button_enabled(format!("{P_PASTE}{}", slot.key), "粘贴", slot.online);
                        if slot.filled {
                            actions.ghost_button(format!("{P_CLEAR}{}", slot.key), "清空");
                        }
                    });
                });
            }
        });
    });
}

fn prompt_block(ui: &mut Ui<'_, '_>, prompt: &PromptView) {
    ui.column(4.0, |block| {
        block.row(8.0, |row| {
            row.text("主提示词", TextRole::Label);
            if prompt.candidates > 1 {
                row.ghost_button(ID_PROMPT_FIELD, prompt.source.clone());
            } else {
                row.caption(prompt.source.clone());
            }
        });
        block.text_area(ID_PROMPT_TEXT, prompt.value.clone(), "描述你想生成的画面", PROMPT_H);
    });
}

fn sizing_block(ui: &mut Ui<'_, '_>, sizing: &SizingView, width: f32) {
    ui.column(4.0, |block| {
        if width < 420.0 {
            if !sizing.cells.is_empty() {
                block.row(8.0, |row| size_cells(row, &sizing.cells));
            }
            if !sizing.numbers.is_empty() {
                block.row(8.0, |row| size_numbers(row, &sizing.numbers));
            }
        } else if !sizing.cells.is_empty() || !sizing.numbers.is_empty() {
            block.row(8.0, |row| {
                size_cells(row, &sizing.cells);
                size_numbers(row, &sizing.numbers);
            });
        }
        if !sizing.summary.is_empty() {
            block.caption(sizing.summary.clone());
        }
    });
}

fn size_cells(row: &mut Ui<'_, '_>, cells: &[SizeCell]) {
    for cell in cells {
        row.column(2.0, |column| {
            column.field_label(cell.label);
            let label = if cell.selected.is_empty() { "选择".to_string() } else { cell.selected.clone() };
            column.button(cell.id, label);
        });
    }
}

fn size_numbers(row: &mut Ui<'_, '_>, numbers: &[NumberCell]) {
    for number in numbers {
        row.column(2.0, |column| {
            column.field_label(number.label.clone());
            column.text_input(number.id, number.value.clone(), "");
        });
    }
}

fn run_row(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    ui.row(8.0, |row| {
        row.ghost_button_enabled(ID_INTERRUPT, "中断", snapshot.online && snapshot.running);
        row.ghost_button_enabled(ID_FREE, "释放显存", snapshot.online);
        row.grow();
        row.button_enabled(ID_RUN, "生成", snapshot.online && snapshot.has_draft);
    });
}

fn advanced_panel(ui: &mut Ui<'_, '_>, advanced: &AdvancedView) {
    ui.panel(|card| {
        card.row(8.0, |head| {
            head.text("高级参数", TextRole::Heading);
            head.ghost_button(ID_PARAMS_CLOSE, "关闭");
        });
        card.text_input(ID_PARAM_FILTER, advanced.filter.clone(), "筛选参数名或节点");
        card.scroll_sized("params_scroll", PARAMS_H, 6.0, false, |list| {
            if advanced.groups.is_empty() {
                list.caption("没有匹配的参数");
            }
            for group in &advanced.groups {
                list.ghost_button(group.toggle.clone(), group.title.clone());
                if group.open {
                    list.column(6.0, |fields| {
                        for field in &group.fields {
                            field_row(fields, field);
                        }
                    });
                }
            }
        });
    });
}

fn picker_panel(ui: &mut Ui<'_, '_>, picker: &PickerView) {
    ui.panel(|card| {
        card.row(8.0, |head| {
            head.text(picker.title.clone(), TextRole::Heading);
            head.ghost_button(ID_PICKER_CLOSE, "关闭");
        });
        card.scroll_sized("picker_scroll", picker_height(picker.options.len()), 6.0, false, |list| {
            for (id, label, current) in &picker.options {
                let control = format!("{P_PICK}{id}");
                if *current {
                    list.button(control, label.clone());
                } else {
                    list.ghost_button(control, label.clone());
                }
            }
        });
    });
}

fn field_row(ui: &mut Ui<'_, '_>, field: &FieldView) {
    ui.column(2.0, |column| match &field.control {
        FieldControl::Connected(from) => {
            column.row(8.0, |row| {
                row.text(field.label.clone(), TextRole::Label);
                row.caption(from.clone());
            });
        }
        FieldControl::Combo { id, selected } => {
            column.field_label(field.label.clone());
            let label = if selected.is_empty() { "选择".to_string() } else { selected.clone() };
            column.button(id.clone(), label);
        }
        FieldControl::Number { id, value } => {
            column.field_label(field.label.clone());
            column.text_input(id.clone(), value.clone(), "");
        }
        FieldControl::Text { id, value, multiline } => {
            column.field_label(field.label.clone());
            if *multiline {
                column.text_area(id.clone(), value.clone(), "", 72.0);
            } else {
                column.text_input(id.clone(), value.clone(), "");
            }
        }
        FieldControl::Bool { id, checked } => {
            column.checkbox(id.clone(), *checked, field.label.clone());
        }
    });
    if let Some(seed) = &field.seed {
        ui.checkbox(seed.clone(), field.seed_on, "每次随机");
    }
}

// ---------- 设置页 ----------

pub struct ProcessView {
    pub state: String,
    pub running: bool,
    pub starting: bool,
    pub reason: String,
}

pub struct UpdateView {
    pub can_update: bool,
    pub updating: bool,
    pub status: String,
    pub is_error: bool,
}

pub struct SettingsSnapshot {
    pub error: String,
    pub host: String,
    pub port: String,
    pub managed: bool,
    pub comfy_dir: String,
    pub python_path: String,
    pub extra_args: String,
    pub archive_folder: String,
    pub start_line: String,
    pub process: ProcessView,
    pub update: UpdateView,
}

impl SettingsSnapshot {
    pub fn read(core: &Core) -> Self {
        let settings = &core.settings;
        let host = &core.host;
        let state = if host.running {
            "服务运行中".to_string()
        } else if host.starting {
            "启动中…".to_string()
        } else {
            "服务未启动".to_string()
        };
        // 未运行且留有原因时给出可读说明：一次性通知错过后仍能排查。
        let reason = if host.running || host.starting {
            String::new()
        } else if !host.error.is_empty() {
            host.error.clone()
        } else {
            host.died.clone()
        };
        let update_status = if host.updating {
            "更新中…".to_string()
        } else if !host.update_error.is_empty() {
            host.update_error.clone()
        } else if !host.update_notice.is_empty() {
            host.update_notice.clone()
        } else {
            host.update_log.lines().last().unwrap_or("").to_string()
        };
        let update = UpdateView {
            can_update: !settings.comfy_dir.trim().is_empty() && !host.updating && !host.starting,
            updating: host.updating,
            status: update_status,
            is_error: !host.updating && !host.update_error.is_empty(),
        };
        Self {
            error: core.runtime.snap.error.clone(),
            host: settings.host.clone(),
            port: settings.port.to_string(),
            managed: settings.process_mode == ProcessMode::Managed,
            comfy_dir: settings.comfy_dir.clone(),
            python_path: settings.python_path.clone(),
            extra_args: settings.extra_args.clone(),
            archive_folder: settings.archive_folder.clone(),
            start_line: host.start_line.clone(),
            process: ProcessView { state, running: host.running, starting: host.starting, reason },
            update,
        }
    }
}

pub fn settings_frame(ui: &mut Ui<'_, '_>, snapshot: &SettingsSnapshot) {
    ui.scroll("settings_scroll", 10.0, |page| {
        page.pad(16.0, 12.0, |body| {
            body.column(12.0, |sections| {
                if !snapshot.error.is_empty() {
                    sections.row(8.0, |row| {
                        row.button(ID_CLEAR_ERROR, "关闭");
                        row.text_styled(
                            snapshot.error.clone(),
                            TextStyle::new(TextRole::Label).color(ColorToken::Danger).wrap(),
                        );
                    });
                }
                sections.text("连接", TextRole::Heading);
                sections.panel(|panel| {
                    panel.row(8.0, |row| {
                        row.text("地址", TextRole::Label);
                        row.text_input(SET_HOST, snapshot.host.clone(), "");
                    });
                    panel.row(8.0, |row| {
                        row.text("端口", TextRole::Label);
                        row.text_input(SET_PORT, snapshot.port.clone(), "");
                    });
                    panel.row(8.0, |row| {
                        row.text("进程", TextRole::Label);
                        row.button_enabled(SET_MODE_MANAGED, "托管启动", snapshot.managed);
                        row.ghost_button_enabled(SET_MODE_EXTERNAL, "外部管理", !snapshot.managed);
                    });
                });
                if snapshot.managed {
                    sections.text("托管启动", TextRole::Heading);
                    sections.panel(|panel| {
                        panel.row(8.0, |row| {
                            row.text("安装目录", TextRole::Label);
                            row.text_input(SET_DIR, snapshot.comfy_dir.clone(), "");
                            row.ghost_button(SET_DIR_PICK, "选择");
                        });
                        panel.row(8.0, |row| {
                            row.text("Python 路径", TextRole::Label);
                            row.text_input(SET_PYTHON, snapshot.python_path.clone(), "");
                        });
                        panel.row(8.0, |row| {
                            row.text("附加参数", TextRole::Label);
                            row.text_input(SET_EXTRA, snapshot.extra_args.clone(), "");
                        });
                        if !snapshot.start_line.is_empty() {
                            panel.caption(format!("启动命令：{}", snapshot.start_line));
                        }
                        panel.row(8.0, |row| {
                            row.text(snapshot.process.state.clone(), TextRole::Label);
                            if snapshot.process.starting {
                                row.ghost_button_enabled(ID_STOP, "取消启动", !snapshot.update.updating);
                            } else if snapshot.process.running {
                                row.ghost_button_enabled(ID_STOP, "停止", !snapshot.update.updating);
                            } else {
                                row.button_enabled(ID_START, "启动", !snapshot.update.updating);
                            }
                        });
                        if !snapshot.process.reason.is_empty() {
                            panel.notice(snapshot.process.reason.clone(), ColorToken::Danger);
                        }
                    });
                }
                sections.text("维护", TextRole::Heading);
                sections.panel(|panel| {
                    panel.row(8.0, |row| {
                        row.text("ComfyUI 更新", TextRole::Label);
                        let label = if snapshot.update.updating { "更新中…" } else { "一键更新" };
                        row.button_enabled(ID_UPDATE, label, snapshot.update.can_update);
                    });
                    panel.caption("在安装目录执行 git pull --ff-only 并按 requirements.txt 更新依赖；托管服务会先停止，完成后自动重启");
                    if !snapshot.update.status.is_empty() {
                        let color = if snapshot.update.is_error { ColorToken::Danger } else { ColorToken::TextMuted };
                        panel.notice(snapshot.update.status.clone(), color);
                    }
                });
                sections.text("保存到仓库", TextRole::Heading);
                sections.panel(|panel| {
                    panel.row(8.0, |row| {
                        row.text("仓库子目录", TextRole::Label);
                        row.text_input(SET_ARCHIVE, snapshot.archive_folder.clone(), "");
                    });
                    panel.caption("留空则写入仓库根下的 comfyui/ 目录");
                });
            });
        });
    });
}
