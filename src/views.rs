//! 生成面板与设置页的整面帧声明：把 `Core` 快照投影成控件，纯函数、无副作用、无 IO。
//!
//! id 方案即事件契约（app 据此分派）：静态动作固定串；动态控件用前缀 + 键 `${节点id}.${输入名}`，
//! 数值/文本/布尔/下拉分前缀定型。记录流的图格命中 id 由 `gallery::layout` 给出，本模块只负责画布。
//! 面板内的行内面板只有高级参数与灯箱；选择器是小浮层，贴在触发控件旁。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use atelyx_plugin_protocol::frame::HitCursor;
use atelyx_plugin_protocol::paint::{
    ColorToken, DrawCommand, ImageFit, ImageSource, PaintColor, PaintPoint, PaintShadow, TextWeight,
};
use atelyx_plugin_sdk::frame_ui::{Canvas, Icon, Rect, TextRole, TextStyle, Ui};

use crate::app::{Core, Picker, PickerAnchor};
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
pub const ID_PICKER_DISMISS: &str = "picker_dismiss";
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
/// 空态主操作：按当前状态换成启动 / 重连 / 选工作流。
pub const ID_EMPTY_START: &str = "empty_start";
pub const ID_EMPTY_REFRESH: &str = "empty_refresh";
pub const ID_EMPTY_WF: &str = "empty_workflow";
/// 快捷数值框的「输入名 → 节点 id」对应表：视图渲染与事件分派两侧同源。
pub const QUICK_NUMBER_FIELDS: &[(&str, &str)] = &[("steps", ID_STEPS), ("cfg", ID_CFG)];

// ---------- 灯箱静态节点 id ----------
pub const ID_LB_DISMISS: &str = "lb_dismiss";
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
/// 页面竖排的间距（状态条 / 主区 / 分隔线 / 生成坞同一口径）。
const GAP: f32 = 8.0;
/// 状态条与分隔线的固定高度。
const STATUS_H: f32 = 32.0;
const RULE_H: f32 = 1.0;
/// 生成坞各段的高度：字段卡与数值框的估高来自主题字号 + 内衬，换主题时按同一口径重算。
const DOCK_GAP: f32 = 8.0;
const WORKFLOW_H: f32 = 37.0;
const REF_LABEL_H: f32 = 18.0;
const REF_THUMB_H: f32 = 76.0;
const PROMPT_LABEL_H: f32 = 18.0;
const PROMPT_LABEL_BUTTON_H: f32 = 32.0;
const PROMPT_H: f32 = 60.0;
const SIZE_CARD_H: f32 = 54.0;
const NUMBER_H: f32 = 46.0;
const ACTION_H: f32 = 32.0;
/// 顶部告警行的预留高度：错误横幅按四行估，普通提示按一行；宁可下方留白也不裁掉操作区。
const ERROR_H: f32 = 80.0;
const NOTICE_H: f32 = 18.0;
const WORKFLOW_NOTICE_H: f32 = 60.0;
/// 主区至少保留的高度：矮面上优先保住可滚动的记录流。
const MIN_FLOW_H: f32 = 80.0;
/// 灯箱与参数面板自身的固定占用（标题 / 说明 / 动作 / 内衬），用于反推图片与列表高度。
const LIGHTBOX_CHROME_H: f32 = 130.0;
const LIGHTBOX_CHROME_NARROW_H: f32 = 170.0;
const PARAMS_CHROME_H: f32 = 90.0;
/// 选择浮层的量度：小菜单只容下标题与候选，超过上限在浮层内滚动。
const PICKER_PAD: f32 = 9.0;
const PICKER_HEAD_H: f32 = 18.0;
const PICKER_ROW_H: f32 = 36.0;
const PICKER_GAP: f32 = 5.0;
const PICKER_MENU_MAX_H: f32 = 300.0;
/// 空态插图边长。
const EMPTY_TILE: f32 = 60.0;
/// 自绘小件的深色浮条与白色文字（悬停说明、文件名、清除徽标）。
const BAR_TEXT: [u8; 4] = [255, 255, 255, 235];

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
    /// 打开菜单时的指针落点与控件高：浮层以触发控件为基准定位。
    pub anchor: Option<PickerAnchor>,
}

pub struct LightboxView {
    pub title: String,
    pub file: String,
    pub source: Option<String>,
    pub image_height: f32,
    pub can_copy_prompt: bool,
    pub saved: bool,
}

/// 上手三步里的一步：已完成（绿）或正在进行（强调色）。
#[derive(Clone)]
pub struct StepView {
    pub label: String,
    pub done: bool,
    pub current: bool,
}

/// 空态主操作：动作已在快照里定型，事件侧只按 id 转交。
#[derive(Clone)]
pub struct EmptyActionView {
    pub label: String,
    pub id: &'static str,
}

/// 记录流空态：插图由画布自绘，文案与主操作随连接 / 工作流 / 提示词状态切换。
#[derive(Clone)]
pub struct EmptyView {
    pub title: String,
    pub body: String,
    pub steps: Vec<StepView>,
    pub action: Option<EmptyActionView>,
    pub hint: String,
}

/// 生成面板一帧的全部输入（锁内构建，渲染闭包只读）。
pub struct PanelSnapshot {
    pub width: f32,
    pub height: f32,
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
    pub empty: Option<EmptyView>,
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
                Some(SizingView { cells, numbers })
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
            Some(lightbox_view(item, lightbox_image_height(surface, snap.error.as_str(), session.notice.as_str())))
        });
        // 无结果且不在预览时给空态：文案与主操作按连接 / 工作流 / 提示词当前状态定型。
        let empty = (flow.is_none() && lightbox.is_none())
            .then(|| empty_view(&status, &workflow, prompt.as_ref(), online));

        Self {
            width: surface.0,
            height: surface.1,
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
            empty,
        }
    }
}

/// 空态的文案与主操作：离线优先，其次选工作流，最后才是写词生成。
fn empty_view(status: &StatusView, workflow: &WorkflowView, prompt: Option<&PromptView>, online: bool) -> EmptyView {
    let prompt_filled = prompt.is_some_and(|prompt| !prompt.value.trim().is_empty());
    let steps = vec![
        StepView { label: "1 连接".to_string(), done: online, current: !online },
        StepView { label: "2 工作流".to_string(), done: workflow.has, current: online && !workflow.has },
        StepView { label: "3 生成".to_string(), done: false, current: online && workflow.has },
    ];
    let (title, body, action, hint) = if !online {
        let action = if status.starting || status.updating {
            None
        } else if status.managed && !status.running {
            Some(EmptyActionView { label: "启动服务".to_string(), id: ID_EMPTY_START })
        } else {
            Some(EmptyActionView { label: "重新连接".to_string(), id: ID_EMPTY_REFRESH })
        };
        let hint = if matches!(action.as_ref().map(|action| action.id), Some(ID_EMPTY_START)) {
            "启动后自动连接本机服务".to_string()
        } else {
            "确认服务已启动后重试".to_string()
        };
        (
            "还没有连接到 ComfyUI".to_string(),
            "连接后，结果会按批次出现在这里。".to_string(),
            action,
            hint,
        )
    } else if !workflow.has {
        if workflow.error.is_empty() {
            (
                "先挑一个工作流".to_string(),
                "工作流决定整条生成流程，可先在浏览器里编排并保存。".to_string(),
                Some(EmptyActionView { label: "选择工作流".to_string(), id: ID_EMPTY_WF }),
                "保存到 workflows 目录后会自动出现在列表里".to_string(),
            )
        } else {
            (
                "读不到工作流列表".to_string(),
                "检查 workflows 目录，保存一个工作流后重试。".to_string(),
                Some(EmptyActionView { label: "重新读取".to_string(), id: ID_EMPTY_REFRESH }),
                "失败原因见底部工作流卡片".to_string(),
            )
        }
    } else if prompt.is_none() {
        (
            "工作流已就绪".to_string(),
            "这个工作流没有提示词输入。".to_string(),
            None,
            "在下方调好参数，点「生成」开始".to_string(),
        )
    } else if !prompt_filled {
        (
            "写一句提示词就能开始".to_string(),
            "描述想看到的画面，参考图与规格都能在下方调整。".to_string(),
            None,
            "在下方输入提示词，再点「生成」".to_string(),
        )
    } else {
        (
            "已经可以生成".to_string(),
            "工作流、提示词与规格都已就绪。".to_string(),
            None,
            "调好下方参数，点「生成」开始".to_string(),
        )
    };
    EmptyView { title, body, steps, action, hint }
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
        anchor: picker.anchor,
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

/// 一帧页面的竖向预算：状态条与告警固定，主区吃剩余高度，生成坞贴底。
#[derive(Clone, Copy)]
struct PageMetrics {
    main_h: f32,
}

fn page_metrics(snapshot: &PanelSnapshot) -> PageMetrics {
    let lightbox = snapshot.lightbox.is_some();
    let dock_h = if lightbox { 0.0 } else { dock_height(snapshot) };
    let mut top = PAD * 2.0 + STATUS_H + GAP;
    if !snapshot.error.is_empty() {
        top += ERROR_H + GAP;
    } else if !snapshot.notice.is_empty() {
        top += NOTICE_H + GAP;
    }
    let bottom = if lightbox { 0.0 } else { GAP + RULE_H + GAP + dock_h };
    PageMetrics { main_h: (snapshot.height - top - bottom).max(MIN_FLOW_H) }
}

/// 生成坞高度：与 [`dock`] 的分段一一对应，主区高度据此反推，坞才能稳定贴底。
fn dock_height(snapshot: &PanelSnapshot) -> f32 {
    let mut height = WORKFLOW_H;
    if !snapshot.workflow.error.is_empty() || snapshot.workflow.changed {
        height += 4.0 + WORKFLOW_NOTICE_H;
    }
    if !snapshot.refs.is_empty() {
        let rows = ref_rows(snapshot) as f32;
        height += DOCK_GAP + REF_LABEL_H + 6.0 + rows * REF_THUMB_H + (rows - 1.0) * 6.0;
    }
    if let Some(prompt) = &snapshot.prompt {
        let head = if prompt.candidates > 1 { PROMPT_LABEL_BUTTON_H } else { PROMPT_LABEL_H };
        height += DOCK_GAP + head + 6.0 + PROMPT_H;
    }
    if let Some(sizing) = &snapshot.sizing {
        height += DOCK_GAP + sizing_height(sizing, snapshot.width);
    }
    height + DOCK_GAP + ACTION_H
}

fn ref_rows(snapshot: &PanelSnapshot) -> usize {
    let columns = ref_columns(snapshot.width);
    snapshot.refs.len().div_ceil(columns).max(1)
}

fn sizing_height(sizing: &SizingView, width: f32) -> f32 {
    let wide = width >= 620.0;
    let tiles = if !sizing.cells.is_empty() {
        SIZE_CARD_H
    } else if !sizing.numbers.is_empty() {
        NUMBER_H
    } else {
        0.0
    };
    if wide {
        tiles
    } else {
        let numbers = if !sizing.cells.is_empty() && !sizing.numbers.is_empty() { 6.0 + NUMBER_H } else { 0.0 };
        tiles + numbers
    }
}

/// 灯箱占据整面（不显示生成坞）：图片高度按面高扣除状态条与面板自身固定行反推。
fn lightbox_image_height(surface: (f32, f32), error: &str, notice: &str) -> f32 {
    let mut main = surface.1 - PAD * 2.0 - STATUS_H - GAP;
    if !error.is_empty() {
        main -= ERROR_H + GAP;
    } else if !notice.is_empty() {
        main -= NOTICE_H + GAP;
    }
    let chrome = if surface.0 < 420.0 { LIGHTBOX_CHROME_NARROW_H } else { LIGHTBOX_CHROME_H };
    (main - chrome).clamp(140.0, 900.0)
}

/// 页面三段：状态条与告警（固定）→ 主区（记录流 / 空态 / 参数 / 灯箱，吃剩余高度）→ 生成坞贴底；
/// 选择器是贴在触发控件旁的小浮层，菜单外整面点击只收起。
pub fn panel_frame(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let metrics = page_metrics(snapshot);
    ui.pad(PAD, PAD, |page| {
        page.column(GAP, |cells| {
            status_bar(cells, snapshot);
            if !snapshot.error.is_empty() {
                cells.row(GAP, |row| {
                    row.notice(snapshot.error.clone(), ColorToken::Danger);
                    row.ghost_button(ID_CLEAR_ERROR, "关闭");
                });
            } else if !snapshot.notice.is_empty() {
                cells.caption(snapshot.notice.clone());
            }
            if let Some(lightbox) = &snapshot.lightbox {
                lightbox_panel(cells, lightbox, snapshot.width);
            } else {
                if let Some(advanced) = &snapshot.advanced {
                    advanced_panel(cells, advanced, metrics.main_h);
                } else {
                    flow_section(cells, snapshot, metrics.main_h);
                }
                cells.rule();
                dock(cells, snapshot);
            }
        });
    });
    if snapshot.lightbox.is_none() {
        if let Some(picker) = &snapshot.picker {
            picker_overlay(ui, picker, snapshot.width, snapshot.height);
        }
    }
}

fn status_bar(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let status = &snapshot.status;
    // 窄面只留连接态与进程操作：队列、显存与「打开浏览器」退到宽面才出现。
    let wide = snapshot.width >= 470.0;
    ui.row(GAP, |row| {
        let (label, color) = if status.updating {
            ("更新中", ColorToken::Warning)
        } else if status.probing {
            ("连接中", ColorToken::TextMuted)
        } else if status.direct {
            ("已连接", ColorToken::Success)
        } else {
            ("未连接", ColorToken::TextMuted)
        };
        row.chip(label, color);
        let endpoint = if snapshot.width < 360.0 {
            format!(":{}", status.port)
        } else {
            format!("{}:{}", status.host, status.port)
        };
        row.text_styled(endpoint, TextStyle::new(TextRole::Caption));
        if wide {
            if let Some(queue) = &status.queue {
                row.chip(queue.clone(), ColorToken::TextMuted);
            }
            if let Some(vram) = &status.vram {
                row.chip(vram.clone(), ColorToken::TextMuted);
            }
        }
        if status.managed && !status.updating {
            if status.starting {
                row.ghost_button(ID_STOP, if wide { "取消启动" } else { "取消" });
            } else if status.running {
                row.ghost_button(ID_STOP, if wide { "停止服务" } else { "停止" });
            } else {
                row.button(ID_START, if wide { "启动服务" } else { "启动" });
            }
        }
        row.icon_button(ID_REFRESH, Icon::Refresh);
        row.ghost_button(ID_OPEN_BROWSER, if wide { "打开浏览器" } else { "打开" });
    });
}

fn flow_section(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot, flow_h: f32) {
    let hidden_h = if snapshot.hidden_batches > 0 { ACTION_H + GAP } else { 0.0 };
    let scroll_h = (flow_h - hidden_h).max(40.0);
    let flow = snapshot.flow.clone();
    let empty = snapshot.empty.clone();
    let follow = snapshot.flow_at_bottom;
    let flow_width = (snapshot.width - PAD * 2.0).max(1.0);
    ui.column(GAP, |slot| {
        slot.scroll_sized(ID_FLOW, scroll_h, 0.0, follow, move |area| match flow {
            Some(layout) => {
                area.canvas(layout.height, move |canvas| gallery::paint(canvas, &layout));
            }
            None => match empty {
                Some(empty) => empty_state(area, &empty, scroll_h, flow_width),
                None => area.caption("尚无生成结果"),
            },
        });
        if snapshot.hidden_batches > 0 {
            slot.ghost_button(ID_FLOW_MORE, format!("显示更早（{}）", snapshot.hidden_batches));
        }
    });
}

/// 空态：插图与文案一组、上手三步一组、主操作与去向提示一组；窄面贴边、宽面居中且限宽。
fn empty_state(ui: &mut Ui<'_, '_>, empty: &EmptyView, scroll_h: f32, width: f32) {
    let content = width.min(420.0);
    let side = ((width - content) / 2.0).max(16.0);
    let mut block_h = EMPTY_TILE + 12.0 + 18.4;
    if empty.action.is_some() {
        block_h += 12.0 + ACTION_H;
    } else if !empty.hint.is_empty() {
        block_h += 12.0 + 14.4;
    }
    let top_pad = ((scroll_h - block_h - 32.0) / 2.0).max(0.0);
    ui.pad(side, 0.0, |outer| {
        outer.spacer(top_pad);
        outer.pad(0.0, 16.0, |body| {
            body.column(12.0, |column| {
                column.row(14.0, |head| {
                    head.canvas_sized(EMPTY_TILE, EMPTY_TILE, paint_empty_tile);
                    head.column(4.0, |texts| {
                        texts.text(empty.title.clone(), TextRole::Title);
                        texts.caption(empty.body.clone());
                    });
                });
                column.row(6.0, |steps| {
                    for step in &empty.steps {
                        let color = if step.done {
                            ColorToken::Success
                        } else if step.current {
                            ColorToken::Accent
                        } else {
                            ColorToken::TextMuted
                        };
                        steps.chip(step.label.clone(), color);
                    }
                });
                if let Some(action) = &empty.action {
                    column.row(10.0, |line| {
                        primary_button(line, action.id, &action.label, true, 104.0);
                        if !empty.hint.is_empty() {
                            line.caption(empty.hint.clone());
                        }
                    });
                } else if !empty.hint.is_empty() {
                    column.caption(empty.hint.clone());
                }
            });
        });
    });
}

fn lightbox_panel(ui: &mut Ui<'_, '_>, lightbox: &LightboxView, width: f32) {
    // 整张卡承接空白点击，但光标保持箭头：这是预览面，不该整块变成手型。
    // 动作按钮后登记，命中仍然优先。
    ui.card_with_cursor(ID_LB_DISMISS, false, HitCursor::Default, |card| {
        card.row(8.0, |head| {
            head.text(lightbox.title.clone(), TextRole::Heading);
            head.grow();
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
    ui.column(DOCK_GAP, |dock| {
        workflow_block(dock, snapshot);
        ref_strip(dock, snapshot);
        if let Some(prompt) = &snapshot.prompt {
            prompt_block(dock, prompt);
        }
        if let Some(sizing) = &snapshot.sizing {
            sizing_block(dock, sizing, snapshot.width);
        }
        action_row(dock, snapshot);
    });
}

/// 工作流是一条单行字段卡：小标签、名称与「保存」同排；读文件失败 / 外部改动在下方说明。
fn workflow_block(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let workflow = &snapshot.workflow;
    ui.column(4.0, |block| {
        block.row(GAP, |row| {
            let name = if workflow.current.is_empty() {
                "选择工作流".to_string()
            } else {
                workflow.current.clone()
            };
            row.card(ID_WF, false, move |body| {
                body.row(8.0, move |line| {
                    line.chip("工作流", ColorToken::TextSecondary);
                    line.text(name.clone(), TextRole::Value);
                    line.icon(Icon::ChevronRight);
                });
            });
            row.button_enabled(ID_SAVE_WF, "保存", workflow.has);
        });
        if !workflow.error.is_empty() {
            block.notice(format!("工作流列表读取失败：{}", workflow.error), ColorToken::Danger);
        } else if workflow.changed {
            block.notice("文件已被外部修改，重新选择可载入最新内容".to_string(), ColorToken::Warning);
        }
    });
}

/// 参考图：缩略卡按面宽分行；图面点选已有图，右上角清除，底栏常驻上传与粘贴。
fn ref_strip(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    if snapshot.refs.is_empty() {
        return;
    }
    ui.column(6.0, |block| {
        block.field_label("参考图");
        let per_row = ref_columns(snapshot.width);
        for chunk in snapshot.refs.chunks(per_row) {
            block.row(8.0, |row| {
                for slot in chunk {
                    row.column(0.0, |cell| ref_card(cell, slot));
                }
                // 末行补齐等宽空列，缩略卡宽度不随最后一行变宽。
                for _ in chunk.len()..per_row {
                    row.column(0.0, |cell| cell.spacer(0.0));
                }
            });
        }
    });
}

/// 一行能排几张参考图：按 150 逻辑像素的舒适宽度折算，1–4 张。
fn ref_columns(width: f32) -> usize {
    (((width + 8.0) / 150.0).floor() as usize).clamp(1, 4)
}

fn ref_card(ui: &mut Ui<'_, '_>, slot: &RefView) {
    let thumb = RefThumb {
        key: slot.key.clone(),
        name: slot.name.clone(),
        thumb: slot.thumb.clone(),
        filled: slot.filled,
        can_pick: slot.can_pick,
        online: slot.online,
    };
    ui.canvas(REF_THUMB_H, move |canvas| paint_ref_thumb(canvas, &thumb));
}

fn prompt_block(ui: &mut Ui<'_, '_>, prompt: &PromptView) {
    ui.column(6.0, |block| {
        block.row(GAP, |row| {
            row.text("主提示词", TextRole::Label);
            if prompt.candidates > 1 {
                row.ghost_button(ID_PROMPT_FIELD, prompt.source.clone());
            } else {
                row.chip(prompt.source.clone(), ColorToken::TextMuted);
            }
            let chars = prompt.value.chars().count();
            if chars > 0 {
                row.chip(format!("{chars} 字"), ColorToken::TextMuted);
            }
        });
        block.text_area(ID_PROMPT_TEXT, prompt.value.clone(), "描述你想生成的画面", PROMPT_H);
    });
}

/// 规格区：三张可点的字段卡（比例 / 分辨率 / 张数）与步数、CFG 两个数值框同排等宽；
/// 窄面把数值框换到下一行，避免五个格子挤成看不清的碎块。
fn sizing_block(ui: &mut Ui<'_, '_>, sizing: &SizingView, width: f32) {
    ui.column(6.0, |block| {
        if width >= 620.0 {
            block.row(8.0, |row| {
                for cell in &sizing.cells {
                    size_tile(row, cell, false);
                }
                for number in &sizing.numbers {
                    number_field(row, number);
                }
            });
        } else {
            let tight = width < 360.0;
            if !sizing.cells.is_empty() {
                block.row(8.0, |row| {
                    for cell in &sizing.cells {
                        size_tile(row, cell, tight);
                    }
                });
            }
            if !sizing.numbers.is_empty() {
                block.row(8.0, |row| {
                    for number in &sizing.numbers {
                        number_field(row, number);
                    }
                    // 与上一行三格等宽：数值框分行后仍保持同一列宽。
                    for _ in sizing.numbers.len()..3 {
                        row.column(2.0, |cell| cell.spacer(0.0));
                    }
                });
            }
        }
    });
}

fn size_tile(ui: &mut Ui<'_, '_>, cell: &SizeCell, tight: bool) {
    let id = cell.id;
    let label = cell.label;
    let value = if cell.selected.is_empty() { "选择".to_string() } else { cell.selected.clone() };
    ui.card(id, false, move |body| {
        body.field_label(label);
        body.row(0.0, move |line| {
            // 极窄面里把值缩到 12 并省掉箭头，长分辨率才不会被裁成半截。
            line.text(value.clone(), if tight { TextRole::Label } else { TextRole::Value });
            if !tight {
                line.icon(Icon::ChevronRight);
            }
        });
    });
}

fn number_field(ui: &mut Ui<'_, '_>, number: &NumberCell) {
    ui.column(2.0, |column| {
        column.field_label(number.label.clone());
        column.text_input(number.id, number.value.clone(), "");
    });
}

/// 动作行：低强调的中断 / 释放显存在左，高级参数与生成在右，整行只留一个弹簧。
fn action_row(ui: &mut Ui<'_, '_>, snapshot: &PanelSnapshot) {
    let narrow = snapshot.width < 420.0;
    let tight = snapshot.width < 360.0;
    ui.row(if tight { 8.0 } else { GAP }, |row| {
        row.ghost_button_enabled(ID_INTERRUPT, "中断", snapshot.online && snapshot.running);
        row.ghost_button_enabled(ID_FREE, if narrow { "清显存" } else { "释放显存" }, snapshot.online);
        row.grow();
        let title = if snapshot.params_open {
            "收起参数".to_string()
        } else if narrow {
            "高级参数".to_string()
        } else {
            format!("高级参数（{}）", snapshot.advanced_total)
        };
        row.ghost_button_enabled(ID_PARAMS_TOGGLE, title, snapshot.advanced_total > 0);
        primary_button(row, ID_RUN, "生成", snapshot.online && snapshot.has_draft, if tight { 72.0 } else { 88.0 });
    });
}

/// 主操作：强调色底块 + 居中文字。控件的按钮只提供次级底色，这里自绘一个明确的主按钮；
/// 命中沿 id 上报，禁用时不登记。
fn primary_button(ui: &mut Ui<'_, '_>, id: &str, label: &str, enabled: bool, width: f32) {
    let id = id.to_string();
    let label = label.to_string();
    ui.canvas_sized(width, ACTION_H, move |canvas| {
        let area = canvas.area();
        if area.is_empty() {
            return;
        }
        let lit = enabled && (canvas.hovered(&id) || canvas.pressed(&id));
        let (fill, stroke, ink) = if !enabled {
            (ColorToken::Surface, None, ColorToken::TextMuted)
        } else if lit {
            (ColorToken::Accent, None, ColorToken::SurfaceSunken)
        } else {
            (ColorToken::AccentSoft, Some(ColorToken::Accent), ColorToken::Accent)
        };
        canvas.draw(DrawCommand::Rect {
            x: area.x,
            y: area.y,
            w: area.w,
            h: area.h,
            fill: Some(token(fill)),
            stroke: stroke.map(token),
            stroke_width: stroke.map(|_| 1.0),
            corner_radius: Some(6.0),
            shadow: None,
        });
        let (text_width, text_height) = canvas.measure(&label, 13.0);
        canvas.draw(DrawCommand::Text {
            x: area.x + (area.w - text_width) / 2.0,
            y: area.y + (area.h - text_height) / 2.0,
            content: label.clone(),
            size: 13.0,
            color: token(ink),
            weight: Some(TextWeight::SemiBold),
            family: None,
            max_width: None,
            align: None,
        });
        if enabled {
            canvas.hit(&id, area, HitCursor::Pointer);
        }
    });
}

fn advanced_panel(ui: &mut Ui<'_, '_>, advanced: &AdvancedView, height: f32) {
    let list_height = (height - PARAMS_CHROME_H).max(60.0);
    ui.panel(|card| {
        card.row(GAP, |head| {
            head.text("高级参数", TextRole::Heading);
            head.ghost_button(ID_PARAMS_CLOSE, "关闭");
        });
        card.text_input(ID_PARAM_FILTER, advanced.filter.clone(), "筛选参数名或节点");
        card.scroll_sized("params_scroll", list_height, 6.0, false, |list| {
            if advanced.groups.is_empty() {
                list.caption("没有匹配的参数");
            }
            for group in &advanced.groups {
                list.card(group.toggle.clone(), group.open, |head| {
                    head.row(0.0, |row| {
                        row.text(group.title.clone(), TextRole::Heading);
                        row.grow();
                        row.caption(if group.open { "收起" } else { "展开" });
                    });
                });
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

/// 选择浮层：全屏透明承接层负责「点外面收起」，菜单本体按触发控件定位；
/// 菜单内空白由卡片承接，不会误关。
fn picker_overlay(ui: &mut Ui<'_, '_>, picker: &PickerView, width: f32, height: f32) {
    ui.overlay(Rect::new(0.0, 0.0, width, height), |scrim| {
        scrim.canvas_sized(width, height, |canvas| {
            let area = canvas.area();
            canvas.hit(ID_PICKER_DISMISS, area, HitCursor::Default);
        });
    });
    let menu = picker_menu_rect(picker, width, height);
    ui.overlay(menu, |float| {
        float.card_with_shadow(
            "picker_menu",
            false,
            Some(PaintShadow {
                offset_x: 0.0,
                offset_y: 4.0,
                blur: 16.0,
                color: PaintColor::Rgba { rgba: [0, 0, 0, 56] },
            }),
            |card| {
                card.column(PICKER_GAP, |body| {
                    body.row(8.0, |head| {
                        head.text(picker.title.clone(), TextRole::Label);
                        head.grow();
                        head.caption(format!("{} 项", picker.options.len()));
                    });
                    let list_h = (menu.h - PICKER_PAD * 2.0 - PICKER_HEAD_H - PICKER_GAP).max(PICKER_ROW_H);
                    let scroll_id = format!("picker_scroll:{}", picker.title);
                    body.scroll_sized(scroll_id, list_h, 4.0, false, |list| {
                        for (id, label, current) in &picker.options {
                            let control = format!("{P_PICK}{id}");
                            list.card(control, *current, |item| {
                                item.row(6.0, |row| {
                                    row.text(label.clone(), TextRole::Label);
                                    if *current {
                                        row.grow();
                                        row.chip("当前", ColorToken::Accent);
                                    }
                                });
                            });
                        }
                    });
                });
            },
        );
    });
}

/// 菜单宽高：工作流名最长给 320，比例这类短项给 200；高度随候选数增长，超过上限在菜单内滚动。
fn picker_menu_rect(picker: &PickerView, width: f32, height: f32) -> Rect {
    let rows = picker.options.len() as f32;
    let content = PICKER_HEAD_H + PICKER_GAP + rows * PICKER_ROW_H + (rows - 1.0).max(0.0) * PICKER_GAP;
    let menu_h = (PICKER_PAD * 2.0 + content).clamp(
        PICKER_PAD * 2.0 + PICKER_HEAD_H + PICKER_GAP + PICKER_ROW_H,
        PICKER_MENU_MAX_H,
    );
    let preferred: f32 = if picker.title.contains("工作流") {
        320.0
    } else if picker.title.contains("提示词") || picker.title.contains("参考图") {
        260.0
    } else if picker.title.contains("比例") || picker.title.contains("分辨率") || picker.title.contains("张数") {
        200.0
    } else {
        240.0
    };
    let menu_w = preferred.min((width - PAD * 2.0).max(160.0));
    let anchor = picker.anchor.unwrap_or(PickerAnchor { x: width / 2.0, y: height / 2.0, h: 0.0 });
    let x = if picker.title.contains("工作流") {
        PAD
    } else if picker.title.contains("提示词") {
        (width - PAD - menu_w).max(PAD)
    } else {
        (anchor.x - 14.0).clamp(PAD, (width - PAD - menu_w).max(PAD))
    };
    // 控件上方的空间不足时翻到下方；两端都放不下就贴底，菜单仍可滚动。
    let above = anchor.y - anchor.h / 2.0 - 6.0 - menu_h;
    let below = anchor.y + anchor.h / 2.0 + 6.0;
    let y = if above >= PAD { above } else { below.min((height - PAD - menu_h).max(PAD)) };
    Rect::new(x, y, menu_w, menu_h)
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
            column.card(id.clone(), false, move |body| {
                body.row(0.0, move |line| {
                    line.text(label.clone(), TextRole::Value);
                    line.grow();
                    line.icon(Icon::ChevronRight);
                });
            });
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

// ---------- 自绘小件 ----------

/// 参考图缩略卡的自绘输入：点缩略图选已有、角标清除都按 key 拼命中 id。
struct RefThumb {
    key: String,
    name: String,
    thumb: Option<String>,
    filled: bool,
    can_pick: bool,
    online: bool,
}

/// 空态插图：强调软底圆角块 + 线框照片（山与太阳），给空白区一个明确的视觉落点。
fn paint_empty_tile(canvas: &mut Canvas<'_, '_>) {
    let area = canvas.area();
    if area.is_empty() {
        return;
    }
    canvas.draw(DrawCommand::Rect {
        x: area.x,
        y: area.y,
        w: area.w,
        h: area.h,
        fill: Some(token(ColorToken::AccentSoft)),
        stroke: None,
        stroke_width: None,
        corner_radius: Some(12.0),
        shadow: None,
    });
    let frame = Rect::new(area.x + 13.0, area.y + 12.0, (area.w - 26.0).max(1.0), (area.h - 24.0).max(1.0));
    canvas.draw(DrawCommand::Rect {
        x: frame.x,
        y: frame.y,
        w: frame.w,
        h: frame.h,
        fill: None,
        stroke: Some(token(ColorToken::Accent)),
        stroke_width: Some(1.4),
        corner_radius: Some(4.0),
        shadow: None,
    });
    canvas.draw(DrawCommand::Circle {
        cx: frame.right() - 6.0,
        cy: frame.y + 6.0,
        r: 2.6,
        fill: Some(token(ColorToken::Accent)),
        stroke: None,
        stroke_width: None,
    });
    let bottom = frame.bottom() - 3.0;
    canvas.draw(DrawCommand::Path {
        points: vec![
            PaintPoint { x: frame.x + 2.0, y: bottom },
            PaintPoint { x: frame.x + 8.0, y: frame.y + 13.0 },
            PaintPoint { x: frame.x + 14.0, y: frame.y + 19.0 },
            PaintPoint { x: frame.x + 20.0, y: frame.y + 10.0 },
            PaintPoint { x: frame.right() - 2.0, y: bottom },
        ],
        closed: false,
        fill: None,
        stroke: Some(token(ColorToken::Accent)),
        stroke_width: Some(1.4),
    });
}

/// 参考图缩略卡：图面点选已有图，右上角清除，底栏常驻上传与粘贴（不再重复一排大按钮）。
/// 命中按「整卡 → 底栏 → 清除」登记，底栏和角标压住整卡。
fn paint_ref_thumb(canvas: &mut Canvas<'_, '_>, thumb: &RefThumb) {
    let area = canvas.area();
    if area.is_empty() {
        return;
    }
    let pick_id = format!("{P_REF_PICK}{}", thumb.key);
    let picking = thumb.can_pick && thumb.online && canvas.hovered(&pick_id);
    canvas.draw(DrawCommand::Rect {
        x: area.x,
        y: area.y,
        w: area.w,
        h: area.h,
        fill: Some(token(ColorToken::SurfaceSunken)),
        stroke: Some(token(if picking { ColorToken::Accent } else { ColorToken::Border })),
        stroke_width: Some(1.0),
        corner_radius: Some(8.0),
        shadow: None,
    });
    // 留 2 像素内衬：图片没有圆角裁剪，内衬让方角落在边框以内，读起来像装裱而不是毛边。
    let inset = Rect::new(area.x + 2.0, area.y + 2.0, (area.w - 4.0).max(1.0), (area.h - 4.0).max(1.0));
    if let Some(path) = &thumb.thumb {
        canvas.draw(DrawCommand::Image {
            source: ImageSource::Private { path: path.clone() },
            x: inset.x,
            y: inset.y,
            w: inset.w,
            h: inset.h,
            fit: Some(ImageFit::Cover),
        });
    } else if thumb.filled {
        let label = if thumb.online { "载入中…" } else { "未载入" };
        let (width, height) = canvas.measure(label, 11.0);
        canvas.text(
            label,
            area.x + (area.w - width) / 2.0,
            area.y + (area.h - 20.0 - height) / 2.0,
            11.0,
            token(ColorToken::TextMuted),
        );
    } else {
        let glyph = 18.0;
        let color = if thumb.online { ColorToken::Accent } else { ColorToken::TextMuted };
        canvas.icon(Icon::Plus, Rect::new(area.x + (area.w - glyph) / 2.0, area.y + 16.0, glyph, glyph), token(color));
        let (width, _) = canvas.measure("选择参考图", 10.0);
        canvas.text(
            "选择参考图",
            area.x + (area.w - width) / 2.0,
            area.y + 40.0,
            10.0,
            token(ColorToken::TextMuted),
        );
    }

    // 底栏：左侧文件名 / 悬停提示，右侧上传与粘贴；底栏压在图上作为常驻工具条。
    let bar = Rect::new(area.x, area.bottom() - 20.0, area.w, 20.0);
    canvas.draw(DrawCommand::Rect {
        x: bar.x,
        y: bar.y,
        w: bar.w,
        h: bar.h,
        fill: Some(rgba([0, 0, 0, 135])),
        stroke: None,
        stroke_width: None,
        corner_radius: None,
        shadow: None,
    });
    let (paste_w, _) = canvas.measure("粘贴", 10.0);
    let (upload_w, _) = canvas.measure("上传", 10.0);
    let paste_x = area.right() - 7.0 - paste_w;
    let upload_x = paste_x - 10.0 - upload_w;
    let upload_id = format!("{P_UPLOAD}{}", thumb.key);
    let paste_id = format!("{P_PASTE}{}", thumb.key);
    let upload_hovered = thumb.online && canvas.hovered(&upload_id);
    let paste_hovered = thumb.online && canvas.hovered(&paste_id);
    let action_color = |enabled: bool, hovered: bool| {
        if !enabled {
            token(ColorToken::TextMuted)
        } else if hovered {
            token(ColorToken::Accent)
        } else {
            rgba(BAR_TEXT)
        }
    };
    canvas.text("上传", upload_x, bar.y + 5.0, 10.0, action_color(thumb.online, upload_hovered));
    canvas.text("粘贴", paste_x, bar.y + 5.0, 10.0, action_color(thumb.online, paste_hovered));
    let left = if picking {
        "选择已有".to_string()
    } else if thumb.filled {
        thumb.name.clone()
    } else {
        String::new()
    };
    if !left.is_empty() {
        let room = (upload_x - area.x - 12.0).max(1.0);
        let shown = gallery::clip_text(&left, room, 10.0);
        canvas.text(shown, area.x + 8.0, bar.y + 5.0, 10.0, rgba(BAR_TEXT));
    }

    if thumb.filled {
        let badge = Rect::new(area.right() - 25.0, area.y + 5.0, 21.0, 21.0);
        canvas.draw(DrawCommand::Rect {
            x: badge.x,
            y: badge.y,
            w: badge.w,
            h: badge.h,
            fill: Some(rgba([0, 0, 0, 185])),
            stroke: Some(token(ColorToken::BorderStrong)),
            stroke_width: Some(1.0),
            corner_radius: Some(5.0),
            shadow: None,
        });
        canvas.draw(DrawCommand::Line {
            x1: badge.x + 5.5,
            y1: badge.y + 5.5,
            x2: badge.x + 15.5,
            y2: badge.y + 15.5,
            stroke: rgba(BAR_TEXT),
            width: 1.8,
        });
        canvas.draw(DrawCommand::Line {
            x1: badge.x + 15.5,
            y1: badge.y + 5.5,
            x2: badge.x + 5.5,
            y2: badge.y + 15.5,
            stroke: rgba(BAR_TEXT),
            width: 1.8,
        });
    }

    if thumb.can_pick && thumb.online {
        canvas.hit(&pick_id, area, HitCursor::Pointer);
    }
    if thumb.online {
        let upload_hit = Rect::new(upload_x - 3.0, bar.y, upload_w + 6.0, bar.h);
        let paste_hit = Rect::new(paste_x - 3.0, bar.y, paste_w + 6.0, bar.h);
        canvas.hit(&upload_id, upload_hit, HitCursor::Pointer);
        canvas.hit(&paste_id, paste_hit, HitCursor::Pointer);
    }
    if thumb.filled {
        let badge = Rect::new(area.right() - 25.0, area.y + 5.0, 21.0, 21.0);
        canvas.hit(&format!("{P_CLEAR}{}", thumb.key), badge, HitCursor::Pointer);
    }
}

fn token(value: ColorToken) -> PaintColor {
    PaintColor::Token { name: value }
}

fn rgba(value: [u8; 4]) -> PaintColor {
    PaintColor::Rgba { rgba: value }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status(managed: bool, running: bool, starting: bool) -> StatusView {
        StatusView {
            managed,
            starting,
            running,
            updating: false,
            probing: false,
            direct: false,
            host: "127.0.0.1".into(),
            port: 8188,
            queue: None,
            vram: None,
        }
    }

    fn workflow(has: bool, error: &str) -> WorkflowView {
        let current = if has { "默认工作流".to_string() } else { String::new() };
        WorkflowView { current, has, error: error.into(), changed: false }
    }

    fn prompt(value: &str) -> PromptView {
        PromptView { value: value.into(), source: "自动".into(), candidates: 1 }
    }

    /// 离线时主操作随托管状态换：托管未启动给启动，托管运行或外部管理给重连。
    #[test]
    fn empty_action_prefers_starting_the_managed_service() {
        let empty = empty_view(&status(true, false, false), &workflow(false, ""), None, false);
        assert_eq!(empty.action.as_ref().map(|action| action.id), Some(ID_EMPTY_START));
        assert!(empty.steps[0].current, "离线时第一步是当前步");

        let external = empty_view(&status(false, false, false), &workflow(false, ""), None, false);
        assert_eq!(external.action.as_ref().map(|action| action.id), Some(ID_EMPTY_REFRESH));

        let running = empty_view(&status(true, true, false), &workflow(false, ""), None, false);
        assert_eq!(running.action.as_ref().map(|action| action.id), Some(ID_EMPTY_REFRESH), "服务在跑但还没连上只重试");

        let starting = empty_view(&status(true, false, true), &workflow(false, ""), None, false);
        assert!(starting.action.is_none(), "启动中不再给重复的启动按钮");
    }

    /// 在线后依次推进：选工作流 → 生成；读列表失败给重新读取。
    #[test]
    fn empty_action_follows_workflow_and_prompt_state() {
        let pick = empty_view(&status(true, true, false), &workflow(false, ""), None, true);
        assert_eq!(pick.action.as_ref().map(|action| action.id), Some(ID_EMPTY_WF));

        let failed = empty_view(&status(true, true, false), &workflow(false, "超时"), None, true);
        assert_eq!(failed.action.as_ref().map(|action| action.id), Some(ID_EMPTY_REFRESH));
        assert_eq!(failed.title, "读不到工作流列表");

        let ready = empty_view(&status(true, true, false), &workflow(true, ""), Some(&prompt("")), true);
        assert!(ready.action.is_none(), "生成动作在底部坞，空态不再重复主按钮");
        assert!(ready.hint.contains("生成"));
        assert!(ready.steps[1].done && ready.steps[2].current, "工作流已选，当前步是生成");

        let no_prompt = empty_view(&status(true, true, false), &workflow(true, ""), None, true);
        assert!(no_prompt.action.is_none(), "无提示词的工作流也把生成留给底部坞");
        assert!(no_prompt.hint.contains("生成"));
    }

    /// 参考图卡按面宽折算列数，末行补空列后每张卡保持同宽。
    #[test]
    fn ref_columns_stay_comfortable() {
        assert_eq!(ref_columns(200.0), 1);
        assert_eq!(ref_columns(300.0), 2);
        assert_eq!(ref_columns(380.0), 2);
        assert_eq!(ref_columns(620.0), 4);
        assert_eq!(ref_columns(1200.0), 4);
    }

    fn picker(title: &str, rows: usize, anchor: PickerAnchor) -> PickerView {
        PickerView {
            title: title.to_string(),
            options: (0..rows).map(|index| (format!("id{index}"), format!("项{index}"), index == 0)).collect(),
            anchor: Some(anchor),
        }
    }

    /// 浮层贴在触发控件上方：工作流菜单左对齐工作流卡，右边缘的尺寸菜单向左收，始终留在面内。
    #[test]
    fn picker_menu_anchors_to_control_and_stays_inside() {
        let workflow = picker("选择工作流", 6, PickerAnchor { x: 180.0, y: 700.0, h: 36.0 });
        let menu = picker_menu_rect(&workflow, 380.0, 760.0);
        assert_eq!(menu.x, PAD, "工作流菜单贴住工作流卡左缘");
        assert!(menu.y + menu.h <= 700.0 - 18.0 - 6.0, "菜单整体在触发控件上方");
        assert!(menu.y >= PAD && menu.x + menu.w <= 380.0 - PAD);

        let size = picker("选择比例", 8, PickerAnchor { x: 350.0, y: 560.0, h: 54.0 });
        let menu = picker_menu_rect(&size, 380.0, 760.0);
        assert_eq!(menu.x, 380.0 - PAD - menu.w, "靠右的菜单向左收，不越出面板");
        assert!(menu.y + menu.h <= 560.0 - 27.0 - 6.0);
    }

    /// 浮层外的点击落在收起层，菜单内空白落在菜单卡、选项落在自己的命中 id。
    #[test]
    fn picker_hit_order_separates_outside_blank_and_options() {
        use std::cell::RefCell;

        use atelyx_plugin_protocol::frame::HitRegion;
        use atelyx_plugin_protocol::method::MeasureTextResult;
        use atelyx_plugin_protocol::paint::{TextFamily, TextWeight};
        use atelyx_plugin_sdk::frame_ui::{DrawSurface, FrameUi, Theme};

        struct Capture {
            size: (f32, f32),
            hits: RefCell<Vec<HitRegion>>,
        }
        impl DrawSurface for Capture {
            fn surface_id(&self) -> &str {
                "preview"
            }
            fn size(&self) -> (f32, f32) {
                self.size
            }
            fn measure(
                &self,
                text: &str,
                size: f32,
                _weight: Option<TextWeight>,
                _family: Option<TextFamily>,
                _max_width: Option<f32>,
            ) -> MeasureTextResult {
                MeasureTextResult { width: text.chars().count() as f32 * size * 0.9, height: size * 1.4, lines: 1 }
            }
            fn markdown(&self, _content: &str) -> Vec<atelyx_plugin_protocol::method::MarkdownBlock> {
                Vec::new()
            }
            fn theme(&self) -> Theme {
                Theme::default()
            }
            fn command(&self, _command: &DrawCommand) {}
            fn hit(&self, hit: &HitRegion) {
                self.hits.borrow_mut().push(hit.clone());
            }
            fn policy(&self, _ime: Option<atelyx_plugin_protocol::frame::FrameRect>, _keys: bool, _fps: Option<u16>) {}
        }

        let surface = Capture { size: (380.0, 760.0), hits: RefCell::default() };
        let picker = picker("选择工作流", 6, PickerAnchor { x: 180.0, y: 700.0, h: 36.0 });
        let mut ui = FrameUi::new();
        ui.draw(&surface, |ui| picker_overlay(ui, &picker, 380.0, 760.0)).expect("浮层帧合法");
        let hits = surface.hits.borrow();
        let top = |x: f32, y: f32| {
            hits.iter()
                .rev()
                .find(|hit| x >= hit.x && x <= hit.x + hit.w && y >= hit.y && y <= hit.y + hit.h)
                .map(|hit| hit.id.as_str())
        };
        let menu = picker_menu_rect(&picker, 380.0, 760.0);
        assert_eq!(top(20.0, 20.0), Some(ID_PICKER_DISMISS), "菜单外由收起层承接");
        assert_eq!(top(menu.x + 20.0, menu.y + 14.0), Some("picker_menu"), "菜单内空白不收起");
        let first_option_y = menu.y + PICKER_PAD + PICKER_HEAD_H + PICKER_GAP + 18.0;
        assert!(
            top(menu.x + 20.0, first_option_y).is_some_and(|id| id.starts_with(P_PICK)),
            "选项命中自己的 id"
        );
    }
}
