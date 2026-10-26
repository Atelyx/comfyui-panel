//! 界面树构建：把 `Core` 快照投影成 ViewNode 声明，纯函数、无副作用、无 IO。
//!
//! id 方案即事件契约（app 据此分派）：静态动作固定串；动态控件用前缀 + 键 `${节点id}.${输入名}`，
//! 数值/文本/布尔/下拉分前缀定型，避免「下拉回下标」与「数字回值」在事件侧歧义。节点 id 全树唯一。

use atelyx_plugin_sdk::protocol::view::{
    Align, Axis, BoxSize, ButtonVariant, CanvasEventKind, ColorSpec, ColorToken, ControlSize,
    EventKind, ImageFit, ImageSource, Interaction, Justify, NodeBody, PaddingSpec, ReportMode,
    RowHeight, SizeSpec, Surface, TextWeight, ViewNode,
};

use crate::app::{Core, Picker, PickerOption};
use crate::history::ResultImage;
use crate::runtime::{build_fields_for, Channel};
use crate::settings::ProcessMode;
use crate::workflow::files;
use crate::workflow::form::{self, FieldSpec};
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
/// 结果记录流的滚动容器（新结果贴底也认它）。
pub const ID_FLOW: &str = "flow";
/// 记录流画布节点：绘制清单与指针命中都落在这里。
pub const ID_FLOW_CANVAS: &str = "flow_canvas";
/// 面内选择浮层：浮层节点自身（点遮罩收起）、关闭钮与选项前缀。
pub const ID_PICKER_OVERLAY: &str = "picker_overlay";
pub const ID_PICKER_CLOSE: &str = "picker_close";
pub const P_PICK: &str = "pick:";
pub const ID_PARAMS_TOGGLE: &str = "params_toggle";
pub const ID_PARAM_FILTER: &str = "param_filter";
/// 高级参数浮层的关闭钮与浮层节点本身（点遮罩即点它）。
pub const ID_PARAMS_CLOSE: &str = "params_close";
pub const ID_PARAMS_OVERLAY: &str = "params_overlay";
pub const ID_PROMPT_FIELD: &str = "prompt_field";
pub const ID_PROMPT_TEXT: &str = "prompt_text";
pub const ID_RATIO: &str = "size_ratio";
pub const ID_RESOLUTION: &str = "size_resolution";
pub const ID_COUNT: &str = "size_count";
pub const ID_STEPS: &str = "quick_steps";
pub const ID_CFG: &str = "quick_cfg";
/// 快捷数值框的「输入名 → 节点 id」对应表：视图渲染与事件分派两侧同源。
pub const QUICK_NUMBER_FIELDS: &[(&str, &str)] = &[("steps", ID_STEPS), ("cfg", ID_CFG)];

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

// ---------- 通用构造 ----------

/// 辅助说明文字的字号倍率：宿主 `Text.size` 是相对正文字号的倍率（不是绝对像素），随界面字号缩放。
const TEXT_SMALL: f32 = 0.85;

fn node(id: &str, body: NodeBody) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body,
        interaction: None,
        grow: None,
        children: Vec::new(),
        events: Vec::new(),
    }
}

/// 带事件声明的叶子节点：控件类节点必须声明订阅的 EventKind，宿主才回上行事件。
fn node_events(id: &str, body: NodeBody, events: Vec<EventKind>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body,
        interaction: None,
        grow: None,
        children: Vec::new(),
        events,
    }
}

fn col(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Column {
            gap: Some(8.0),
            padding: Some(PaddingSpec::All(12.0)),
            cross_align: Some(Align::Start),
            align: None,
            size: None,
        },
        children,
        events: Vec::new(),
    }
}

fn row(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Row { gap: Some(8.0), padding: None, justify: None, cross_align: Some(Align::Center), size: None },
        children,
        events: Vec::new(),
    }
}

/// 撑满宽度的行：文本按内容收缩后，输入框与进度条这类 Fill 子项才有剩余宽度可占。
fn row_fill(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Row {
            gap: Some(8.0),
            padding: None,
            justify: None,
            cross_align: Some(Align::Center),
            size: Some(BoxSize { width: Some(SizeSpec::Fill), ..BoxSize::default() }),
        },
        children,
        events: Vec::new(),
    }
}

fn row_spread(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Row {
            gap: Some(8.0),
            padding: None,
            justify: Some(Justify::SpaceBetween),
            cross_align: Some(Align::Center),
            size: Some(BoxSize { width: Some(SizeSpec::Fill), ..BoxSize::default() }),
        },
        children,
        events: Vec::new(),
    }
}

/// 撑满宽度、子项靠右的行：叠层里的浮层章靠它贴住右上角。
fn row_end(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Row {
            gap: Some(8.0),
            padding: None,
            justify: Some(Justify::End),
            cross_align: Some(Align::Start),
            size: Some(BoxSize { width: Some(SizeSpec::Fill), ..BoxSize::default() }),
        },
        children,
        events: Vec::new(),
    }
}

fn stack(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Stack {},
        children,
        events: Vec::new(),
    }
}

fn scroll(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Scroll { direction: Some(Axis::Vertical), gap: Some(8.0), height: None, follow_end: None },
        children,
        events: Vec::new(),
    }
}

/// 声明滚动位置回发的纵向滚动区：插件据此知道用户是否停在末端（新结果要不要自动贴底）。
fn scroll_watch(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Scroll { direction: Some(Axis::Vertical), gap: Some(8.0), height: None, follow_end: None },
        children,
        events: vec![EventKind::Scroll],
    }
}

fn divider(id: &str) -> ViewNode {
    node(id, NodeBody::Divider {})
}

fn label(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: None, weight: Some(TextWeight::Medium), color: None, align: None , family: None, wrap: None, max_lines: None })
}

fn plain(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: None, weight: None, color: None, align: None , family: None, wrap: None, max_lines: None })
}

fn muted(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: Some(TEXT_SMALL), weight: None, color: Some(ColorSpec::Token(ColorToken::TextMuted)), align: None , family: None, wrap: None, max_lines: None })
}

fn warn(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: Some(TEXT_SMALL), weight: None, color: Some(ColorSpec::Token(ColorToken::Warning)), align: None , family: None, wrap: None, max_lines: None })
}

fn danger(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: Some(TEXT_SMALL), weight: None, color: Some(ColorSpec::Token(ColorToken::Danger)), align: None , family: None, wrap: None, max_lines: None })
}

fn button(id: &str, text: impl Into<String>, variant: ButtonVariant, enabled: bool) -> ViewNode {
    node_events(id, NodeBody::Button { label: text.into(), icon: None, variant: Some(variant), enabled: Some(enabled), size: None }, vec![EventKind::Click])
}

fn icon_button(id: &str, icon: &str, tooltip: &str, enabled: bool) -> ViewNode {
    node_events(
        id,
        NodeBody::IconButton {
            icon: icon.to_string(),
            tooltip: Some(tooltip.to_string()),
            enabled: Some(enabled),
            size: None,
            variant: None,
            hover_only: None,
        },
        vec![EventKind::Click],
    )
}

/// 主操作按钮（生成）：大号档，在坞底一眼定位。
fn button_lg(id: &str, text: &str, variant: ButtonVariant, enabled: bool) -> ViewNode {
    node_events(id, NodeBody::Button { label: text.to_string(), icon: None, variant: Some(variant), enabled: Some(enabled), size: Some(ControlSize::Lg) }, vec![EventKind::Click])
}

fn text_input(id: &str, value: String, multiline: bool) -> ViewNode {
    node_events(id, NodeBody::TextInput { value, placeholder: None, multiline: (multiline).then_some(true), password: None, read_only: None, report: None }, vec![EventKind::Change])
}

fn number_input(id: &str, value: Option<f64>, min: Option<f64>, max: Option<f64>, step: Option<f64>) -> ViewNode {
    node_events(id, NodeBody::NumberInput { value, min, max, step, placeholder: None, read_only: None, report: None }, vec![EventKind::Change])
}

/// 选择器外观：整宽可点的行，当前值在左、折角在右，点击由面内选择浮层接管。
/// 不用只读输入框：单行输入控件会吞掉左键按下，外层的点击回调收不到。
fn select_field(id: &str, value: String, placeholder: &str) -> ViewNode {
    let empty = value.is_empty();
    let shown = if empty { placeholder.to_string() } else { value };
    let ink = if empty { ColorToken::TextMuted } else { ColorToken::Text };
    let current = ViewNode {
        id: format!("{id}value"),
        interaction: None,
        grow: None,
        body: NodeBody::Text {
            content: shown,
            size: None,
            weight: None,
            color: Some(ColorSpec::Token(ink)),
            align: Some(Align::Start),
            family: None,
            wrap: Some(false),
            max_lines: Some(1),
        },
        children: Vec::new(),
        events: Vec::new(),
    };
    ViewNode {
        id: id.to_string(),
        interaction: Some(Interaction {
            state: None,
            normal: Some(Surface {
                background: Some(ColorSpec::Token(ColorToken::Surface)),
                border: Some(ColorSpec::Token(ColorToken::BorderStrong)),
                radius: Some(6.0),
            }),
            hover: Some(Surface {
                background: Some(ColorSpec::Token(ColorToken::Hover)),
                border: Some(ColorSpec::Token(ColorToken::BorderStrong)),
                radius: Some(6.0),
            }),
            focused: None,
            active: None,
            disabled: None,
        }),
        grow: None,
        body: NodeBody::Row {
            gap: Some(6.0),
            padding: Some(PaddingSpec::All(6.0)),
            justify: Some(Justify::SpaceBetween),
            cross_align: Some(Align::Center),
            size: Some(BoxSize { width: Some(SizeSpec::Fill), ..BoxSize::default() }),
        },
        children: vec![current, muted(&format!("{id}chev"), "▾")],
        events: vec![EventKind::Click],
    }
}

fn checkbox(id: &str, checked: bool, text: Option<String>) -> ViewNode {
    node_events(id, NodeBody::Checkbox { checked, label: text, read_only: None }, vec![EventKind::Change])
}

/// 纵向容器：内边距归零、间距自定（面板分段与生成坞各段自己的呼吸感）。
fn column(id: &str, children: Vec<ViewNode>, gap: f32) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Column {
            gap: Some(gap),
            padding: None,
            cross_align: Some(Align::Start),
            align: None,
            size: None,
        },
        children,
        events: Vec::new(),
    }
}

/// 按内容收缩的纵向容器：横向滚动区里的槽位卡片要用它——缺省宽度是填充，多槽位会互相挤压而非溢出滚动。
fn column_shrink(id: &str, children: Vec<ViewNode>, gap: f32) -> ViewNode {
    let mut node = column(id, children, gap);
    if let NodeBody::Column { size, .. } = &mut node.body {
        *size = Some(BoxSize { width: Some(SizeSpec::Shrink), ..BoxSize::default() });
    }
    node
}

/// 固定高度纵向滚动区：用于生成坞里的高级参数段，避免展开后吃掉整条记录流的高度。
fn scroll_fixed(id: &str, children: Vec<ViewNode>, height: f32) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Scroll { direction: Some(Axis::Vertical), gap: Some(6.0), height: Some(height), follow_end: None },
        children,
        events: Vec::new(),
    }
}

/// 固定高度横向滚动区：参考图槽位一条横排，槽位多时左右滑而不是把坞撑高。
fn scroll_horizontal(id: &str, children: Vec<ViewNode>, height: f32) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Scroll { direction: Some(Axis::Horizontal), gap: Some(6.0), height: Some(height), follow_end: None },
        children,
        events: Vec::new(),
    }
}

/// 自适应网格：按可用宽度自动定列数，行高随内容——参数控件排成一行，窄面板才换行。
fn grid(id: &str, children: Vec<ViewNode>, min_column_width: f32) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Grid {
            columns: None,
            min_column_width: Some(min_column_width),
            gap: Some(8.0),
            row_height: Some(RowHeight::Content),
        },
        children,
        events: Vec::new(),
    }
}

/// 带标题的网格单元：标题在上、控件在下，控件整宽占满所在列。
fn grid_field(id: &str, title: &str, control: ViewNode) -> ViewNode {
    column(&format!("{id}cell"), vec![muted(&format!("{id}lbl"), title), control], 2.0)
}

// ---------- 生成面板 ----------

/// 面内浮层卡片的宽度上限：高级参数与选择浮层居中呈现，宽面板下不横向撑满。
const OVERLAY_MAX_W: f32 = 720.0;

/// 面板三段：顶部状态条（固定）→ 中间结果记录流（占满剩余高度、自己滚）→ 底部生成坞（整宽）。
/// 高级参数展开为面内浮层（见 [`params_overlay`]），坞内只留开关钮；点结果格开灯箱（见 [`build_lightbox`]）。
/// 浮层按出现顺序叠放：高级参数 → 灯箱 → 选择浮层（最后打开的在最上层）。
pub fn build_panel(core: &Core) -> ViewNode {
    let fields = build_fields_for(&core.session.draft, &core.runtime.snap);
    let mut cells = vec![status_bar(core)];
    if let Some(line) = alert_line(core) {
        cells.push(line);
    }
    cells.push(flow_section(core));
    cells.push(divider("divdock"));
    cells.push(dock(core, &fields));
    if core.session.params_open {
        cells.push(params_overlay(core, &fields));
    }
    if core.session.lightbox_key.is_some() {
        cells.push(build_lightbox(core));
    }
    if let Some(picker) = &core.session.picker {
        cells.push(picker_overlay(picker));
    }
    ViewNode {
        id: "root".to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Column {
            gap: Some(8.0),
            padding: Some(PaddingSpec::All(12.0)),
            cross_align: Some(Align::Center),
            align: None,
            size: None,
        },
        children: cells,
        events: Vec::new(),
    }
}

fn status_bar(core: &Core) -> ViewNode {
    let mut actions = Vec::new();
    if core.settings.process_mode == ProcessMode::Managed {
        if core.host.starting {
            actions.push(muted("starting", "启动中…"));
        } else if core.host.running {
            actions.push(button(ID_STOP, "停止服务", ButtonVariant::Danger, true));
        } else {
            actions.push(button(ID_START, "启动服务", ButtonVariant::Primary, true));
        }
    }
    actions.push(icon_button(ID_REFRESH, "refresh-cw", "刷新状态", true));
    actions.push(icon_button(ID_OPEN_BROWSER, "globe", "在浏览器打开", true));
    let mut left = vec![channel_text(core)];
    if let Some(line) = queue_line(core) {
        left.push(line);
    }
    row_spread("statusbar", vec![row("statusleft", left), row("statusactions", actions)])
}

fn channel_text(core: &Core) -> ViewNode {
    let snap = &core.runtime.snap;
    let (content, color) = if snap.probing {
        ("连接中…".to_string(), ColorToken::TextMuted)
    } else {
        match snap.channel {
            Channel::Direct => (format!("已连接 {}:{}", core.settings.host, core.settings.port), ColorToken::Success),
            Channel::Offline => ("未连接".to_string(), ColorToken::TextMuted),
        }
    };
    node("channel", NodeBody::Text { content, size: None, weight: Some(TextWeight::Medium), color: Some(ColorSpec::Token(color)), align: None, family: None, wrap: None, max_lines: None })
}

/// 队列概览：离线时不占位，在线时让人看清按下去排在谁后面。
fn queue_line(core: &Core) -> Option<ViewNode> {
    let snap = &core.runtime.snap;
    if snap.channel != Channel::Direct {
        return None;
    }
    Some(muted("queue", format!("运行 {} · 等待 {}", snap.queue_running.len(), snap.queue_pending.len())))
}

/// 错误横幅优先，其次一次性提示，都没有就不占行。
fn alert_line(core: &Core) -> Option<ViewNode> {
    let snap = &core.runtime.snap;
    if !snap.error.is_empty() {
        return Some(row_spread(
            "errline",
            vec![
                node("errtext", NodeBody::Text { content: snap.error.clone(), size: None, weight: None, color: Some(ColorSpec::Token(ColorToken::Danger)), align: None , family: None, wrap: None, max_lines: None }),
                button(ID_CLEAR_ERROR, "关闭", ButtonVariant::Ghost, true),
            ],
        ));
    }
    if core.session.notice.is_empty() {
        return None;
    }
    Some(muted("notice", core.session.notice.clone()))
}

/// 中间段：一次算好的记录流画布（高度按内容定，滚动交给外层容器）。
fn flow_section(core: &Core) -> ViewNode {
    let canvas = match &core.session.flow {
        Some(flow) => ViewNode {
            id: ID_FLOW_CANVAS.to_string(),
            interaction: None,
            grow: None,
            body: NodeBody::Canvas {
                mouse_events: vec![CanvasEventKind::PointerUp, CanvasEventKind::PointerMove, CanvasEventKind::PointerLeave],
                height: Some(flow.height),
                fill: None,
                fps: None,
            },
            children: Vec::new(),
            events: Vec::new(),
        },
        None => column("flowempty", vec![muted("noresult", "尚无生成结果")], 0.0),
    };
    scroll_watch(ID_FLOW, vec![canvas])
}

/// 参考图缩略区高度：缩略图 + 一排图标动作。
const REF_THUMB_W: f32 = 56.0;
const REF_THUMB_H: f32 = 72.0;
const REF_STRIP_H: f32 = 112.0;
/// 高级参数展开后的自限高度：浮层不再占记录流的高度，仍给自滚区设上限，避免矮面板上浮层顶出屏幕。
const PARAMS_H: f32 = 300.0;
/// 选择浮层列表的自限高度：候选多时自己滚，浮层不被撑出屏幕。
const PICKER_H: f32 = 320.0;
/// 选择浮层里单个候选按钮的估高与行距：列表高度按条数收紧，空余不留大块空白。
const PICKER_ITEM_H: f32 = 24.0;
const PICKER_ITEM_GAP: f32 = 6.0;

fn picker_height(options: &[PickerOption]) -> f32 {
    let rows = options.len().max(1) as f32;
    (rows * PICKER_ITEM_H + (rows - 1.0) * PICKER_ITEM_GAP).clamp(PICKER_ITEM_H, PICKER_H)
}

fn dock(core: &Core, fields: &[FieldSpec]) -> ViewNode {
    let mut cells = vec![workflow_row(core)];
    if let Some(strip) = ref_strip(core, fields) {
        cells.push(strip);
    }
    if let Some(block) = prompt_block(core, fields) {
        cells.push(block);
    }
    if let Some(block) = params_grid(core, fields) {
        cells.push(block);
    }
    cells.push(run_row(core));
    cells.push(advanced_section(core, fields));
    // 列缺省占满宽度；根列交叉轴居中不影响 Fill 子项，坞随面板整宽。
    column("dock", cells, 6.0)
}

fn workflow_row(core: &Core) -> ViewNode {
    let current = core.session.workflow_id.as_ref().and_then(|id| {
        core.runtime
            .snap
            .workflows
            .iter()
            .find(|file| file.path == *id)
            .map(|file| files::workflow_display_name(&file.name))
    });
    let row = row_fill(
        "wfrow",
        vec![
            plain("wflbl", "工作流"),
            select_field(ID_WF, current.unwrap_or_default(), "选择工作流"),
            button(ID_SAVE_WF, "存参", ButtonVariant::Ghost, core.session.workflow_id.is_some()),
        ],
    );
    // 清单读取失败时选择器是空的，必须说明原因；否则只在文件被外部改动时提示。
    let warning = if !core.runtime.snap.workflows_error.is_empty() {
        warn("wferr", format!("工作流列表读取失败：{}", core.runtime.snap.workflows_error))
    } else if core.session.current_changed {
        warn("wfchanged", "文件已被外部修改，重新选择可载入最新内容")
    } else {
        return row;
    };
    column("wfblock", vec![row, warning], 4.0)
}

/// 参考图横排：一个未连线的 LoadImage 输入就是一个槽位。
fn ref_strip(core: &Core, fields: &[FieldSpec]) -> Option<ViewNode> {
    let slots = form::ref_slots(fields);
    if slots.is_empty() {
        return None;
    }
    let online = core.runtime.snap.channel == Channel::Direct;
    let cards = slots.iter().map(|slot| ref_card(core, slot, online)).collect();
    Some(column("refblock", vec![muted("reflbl", "参考图"), scroll_horizontal("refstrip", cards, REF_STRIP_H)], 4.0))
}

fn ref_card(core: &Core, slot: &FieldSpec, online: bool) -> ViewNode {
    let key = slot.key();
    let filename = form::display_value(form::read_field(&core.session.draft, slot).as_ref());
    let thumb = if filename.is_empty() { None } else { core.session.refs.get(&filename) };
    let visual = match thumb {
        Some(path) => node_events(
            &format!("{P_UPLOAD}{key}"),
            NodeBody::Image {
                source: ImageSource::Private { path: path.to_string() },
                fit: Some(ImageFit::Cover),
                width: Some(SizeSpec::Fixed(REF_THUMB_W)),
                height: Some(SizeSpec::Fixed(REF_THUMB_H)),
            },
            vec![EventKind::Click],
        ),
        None => icon_button(&format!("{P_UPLOAD}{key}"), "image-plus", &format!("上传参考图到「{}」", slot.node_label), online),
    };
    let mut actions = Vec::new();
    let candidates = form::image_candidates(core.runtime.snap.object_info.as_ref(), &slot.class_type);
    if !candidates.is_empty() {
        actions.push(icon_button(&format!("{P_REF_PICK}{key}"), "list", "从已有图片里选", online));
    }
    actions.push(icon_button(&format!("{P_PASTE}{key}"), "clipboard-paste", "粘贴为参考图", online));
    if !filename.is_empty() {
        actions.push(icon_button(&format!("{P_CLEAR}{key}"), "trash-2", "清空该参考图", true));
    }
    column_shrink(&format!("refcard:{key}"), vec![visual, row(&format!("refactions:{key}"), actions)], 4.0)
}

/// 主提示词区：一个大输入框，候选多于一个时可手选来源字段。
fn prompt_block(core: &Core, fields: &[FieldSpec]) -> Option<ViewNode> {
    let (field, explicit) = form::resolve_prompt_field(fields, core.session.prompt_key.as_deref())?;
    let value = form::display_value(form::read_field(&core.session.draft, field).as_ref());
    let candidates = form::prompt_candidates(fields);
    let selector = if candidates.len() > 1 {
        let current = if explicit {
            format!("{}.{}", field.node_label, form::field_label(&field.input))
        } else {
            "自动".to_string()
        };
        select_field(ID_PROMPT_FIELD, current, "选择来源")
    } else {
        muted("promptsrc", field.node_label.clone())
    };
    Some(column(
        "promptblock",
        vec![
            row_spread("prompthead", vec![plain("promptlbl", "主提示词"), selector]),
            text_input(ID_PROMPT_TEXT, value, true),
        ],
        4.0,
    ))
}

/// 参数网格：比例、分辨率、张数各一个选择器，步数与 CFG 各一个数值框，按可用宽度排成一行，
/// 窄面板才换行；下方一行给出实际换算结果。
fn params_grid(core: &Core, fields: &[FieldSpec]) -> Option<ViewNode> {
    let plan = sizing::plan(&core.session.draft, fields);
    let mut cells = Vec::new();
    for (section_id, id) in [(SECTION_RATIO, ID_RATIO), (SECTION_RESOLUTION, ID_RESOLUTION), (SECTION_COUNT, ID_COUNT)] {
        let Some(section) = plan.sections.iter().find(|section| section.id == section_id) else { continue };
        let selected = section
            .selected
            .as_ref()
            .and_then(|value| section.options.iter().find(|option| &option.value == value))
            .map(|option| option.label.clone())
            .unwrap_or_default();
        cells.push(grid_field(id, section.label, select_field(id, selected, "选择")));
    }
    for (input, id) in QUICK_NUMBER_FIELDS {
        let Some(field) = sizing::find_unconnected(fields, input) else { continue };
        let value = form::numeric_value(form::read_field(&core.session.draft, field).as_ref());
        cells.push(grid_field(id, form::field_label(input), number_input(id, value, field.min, field.max, field.step)));
    }
    if cells.is_empty() {
        return None;
    }
    let summary = match &plan.detail {
        Some(detail) => format!("{} · {}", plan.summary, detail),
        None => plan.summary.clone(),
    };
    let mut children = vec![grid("paramgrid", cells, 100.0)];
    if !summary.is_empty() {
        children.push(muted("sizesummary", summary));
    }
    Some(column("sizeparams", children, 4.0))
}

fn run_row(core: &Core) -> ViewNode {
    let snap = &core.runtime.snap;
    let online = snap.channel == Channel::Direct;
    let has_draft = snap_has_draft(core);
    let aux = row(
        "runaux",
        vec![
            icon_button(ID_INTERRUPT, "stop", "中断当前任务", online && snap.running_prompt_id.is_some()),
            icon_button(ID_FREE, "eraser", "释放显存", online),
        ],
    );
    row_spread("runrow", vec![aux, button_lg(ID_RUN, "生成", ButtonVariant::Primary, online && has_draft)])
}

fn snap_has_draft(core: &Core) -> bool {
    core.session.draft.as_object().map(|map| !map.is_empty()).unwrap_or(false)
}

/// 高级参数开关：收起态只占坞内一行；展开态内容进面内浮层（见 [`params_overlay`]）。
fn advanced_section(core: &Core, fields: &[FieldSpec]) -> ViewNode {
    let total = form::form_fields(fields).len();
    let toggle_title = if core.session.params_open { "收起高级参数".to_string() } else { format!("高级参数（{total}）") };
    row_fill(
        "paramshead",
        vec![button(ID_PARAMS_TOGGLE, toggle_title, ButtonVariant::Ghost, total > 0)],
    )
}

/// 高级参数浮层：面内模态大面板（遮罩拦下指针事件，点遮罩或关闭钮收起）；
/// 矮面板下折叠态不占记录流高度，展开态有稳定的自滚空间。
fn params_overlay(core: &Core, fields: &[FieldSpec]) -> ViewNode {
    let groups = form::group_by_node(&form::filter_fields(fields, &core.session.param_filter));
    let mut cells: Vec<ViewNode> = Vec::new();
    for group in &groups {
        let open = core.session.group_toggles.get(group.node_id).copied().unwrap_or(group.has_common);
        let title = if open { format!("▾ {} · {} 项", group.label, group.fields.len()) } else { format!("▸ {}", group.label) };
        cells.push(button(&format!("{P_GROUP}{}", group.node_id), title, ButtonVariant::Ghost, true));
        if open {
            for field in &group.fields {
                cells.push(field_control(&core.session.draft, &core.session.seed_mode, field));
            }
        }
    }
    if cells.is_empty() {
        cells.push(muted("nofield", "没有匹配的参数"));
    }
    let card = ViewNode {
        id: "ovcard".to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Card {
            gap: Some(6.0),
            padding: Some(PaddingSpec::All(12.0)),
            accent: None,
            tone: None,
            radius: None,
            bordered: None,
            align: None,
            size: Some(BoxSize { width: Some(SizeSpec::Fill), max_width: Some(OVERLAY_MAX_W), ..BoxSize::default() }),
        },
        children: vec![
            row_spread(
                "ovhead",
                vec![plain("ovtitle", "高级参数"), button(ID_PARAMS_CLOSE, "关闭", ButtonVariant::Ghost, true)],
            ),
            // 筛选只影响显示：停抖上报，减少连续键入时的整树重建
            node_events(
                ID_PARAM_FILTER,
                NodeBody::TextInput {
                    value: core.session.param_filter.clone(),
                    placeholder: Some("筛选参数名或节点".to_string()),
                    multiline: None,
                    password: None,
                    read_only: None,
                    report: Some(ReportMode::Debounced(200)),
                },
                vec![EventKind::Change],
            ),
            scroll_fixed("ovscroll", cells, PARAMS_H),
        ],
        events: Vec::new(),
    };
    // 浮层自身声明 click：宿主只在点遮罩（内容之外）时回发，据此收起
    ViewNode {
        id: ID_PARAMS_OVERLAY.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Overlay { scrim: None },
        children: vec![card],
        events: vec![EventKind::Click],
    }
}

/// 选择浮层：把下拉/菜单候选铺成面内列表，选中即回发选项序号并收起。
fn picker_overlay(picker: &Picker) -> ViewNode {
    let mut items: Vec<ViewNode> = picker
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let label = if option.current { format!("✓ {}", option.label) } else { option.label.clone() };
            let variant = if option.current { ButtonVariant::Primary } else { ButtonVariant::Ghost };
            button(&format!("{P_PICK}{index}"), label, variant, true)
        })
        .collect();
    if items.is_empty() {
        items.push(muted("pickempty", "没有可选项"));
    }
    let card = ViewNode {
        id: "pickcard".to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Card {
            gap: Some(6.0),
            padding: Some(PaddingSpec::All(12.0)),
            accent: None,
            tone: None,
            radius: None,
            bordered: None,
            align: None,
            size: Some(BoxSize { width: Some(SizeSpec::Fill), max_width: Some(OVERLAY_MAX_W), ..BoxSize::default() }),
        },
        children: vec![
            row_spread(
                "pickhead",
                vec![plain("picktitle", picker.title.clone()), button(ID_PICKER_CLOSE, "关闭", ButtonVariant::Ghost, true)],
            ),
            scroll_fixed("pickscroll", items, picker_height(&picker.options)),
        ],
        events: Vec::new(),
    };
    // 浮层自身声明 click：宿主只在点遮罩（内容之外）时回发，据此收起
    ViewNode {
        id: ID_PICKER_OVERLAY.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Overlay { scrim: None },
        children: vec![card],
        events: vec![EventKind::Click],
    }
}

fn field_control(draft: &serde_json::Value, seed_mode: &std::collections::BTreeSet<String>, field: &FieldSpec) -> ViewNode {
    let key = field.key();
    let text_label = format!("flbl:{key}");
    let label_node = plain(&text_label, input_label(field));

    if field.connected {
        return row_fill(
            &format!("frow:{key}"),
            vec![label_node, muted(&format!("fval:{key}"), form::connected_display(field.link_from.as_deref()))],
        );
    }

    let mut cells: Vec<ViewNode> = vec![label_node];
    match field.kind {
        form::FieldKind::Combo => {
            let options = field.options.clone().unwrap_or_default();
            let current = form::display_value(form::read_field(draft, field).as_ref());
            let selected = options
                .iter()
                .find(|(value, _)| *value == current)
                .map(|(_, label)| label.clone())
                .unwrap_or_default();
            cells.push(select_field(&format!("{P_COMBO}{key}"), selected, "选择"));
        }
        form::FieldKind::Int | form::FieldKind::Float => {
            let value = form::numeric_value(form::read_field(draft, field).as_ref());
            cells.push(number_input(&format!("{P_NUM}{key}"), value, field.min, field.max, field.step));
        }
        form::FieldKind::Boolean => {
            let checked = form::read_field(draft, field).and_then(|v| v.as_bool()).unwrap_or(false);
            cells.push(checkbox(&format!("{P_BOOL}{key}"), checked, None));
        }
        form::FieldKind::Str | form::FieldKind::Json => {
            let value = form::display_value(form::read_field(draft, field).as_ref());
            cells.push(text_input(&format!("{P_TEXT}{key}"), value, field.multiline));
        }
    }

    if field.is_seed {
        cells.push(checkbox(&format!("{P_SEED}{key}"), seed_mode.contains(&key), Some("每次随机".to_string())));
    }
    row_fill(&format!("frow:{key}"), cells)
}

fn input_label(field: &FieldSpec) -> String {
    let named = form::field_label(&field.input);
    if named.is_empty() {
        field.input.clone()
    } else {
        named.to_string()
    }
}

// ---------- 记录流灯箱 ----------

// 静态节点 id（灯箱浮层与其右键菜单）。
pub const ID_LIGHTBOX: &str = "lightbox";
pub const ID_LB_CARD: &str = "lb_card";
pub const ID_LB_HEAD: &str = "lb_head";
pub const ID_LB_IMAGE: &str = "lb_image";
pub const ID_LB_MENU: &str = "lb_menu";
pub const ID_LB_COPY_PROMPT: &str = "lb_copy_prompt";
pub const ID_LB_COPY_IMAGE: &str = "lb_copy_image";
pub const ID_LB_ARCHIVE: &str = "lb_archive";
pub const ID_LB_SAVE_AS: &str = "lb_save_as";

/// 记录流灯箱：面内浮层大图，点图外空白或标题行收起；右键图片弹出动作菜单。
/// 条目被淘汰或副本丢失时由 app 收起。
pub fn build_lightbox(core: &Core) -> ViewNode {
    let item = core
        .session
        .lightbox_key
        .as_deref()
        .and_then(|key| core.runtime.snap.results.iter().find(|r| r.key == key));
    let head = match item {
        Some(entry) => {
            let title = if entry.title.is_empty() { "未命名任务".to_string() } else { entry.title.clone() };
            let mut head = column("lbtext", vec![label("lbtitle", title), muted("lbfile", entry.image_ref.filename.clone())], 2.0);
            head.id = ID_LB_HEAD.to_string();
            head.events = vec![EventKind::Click];
            head
        }
        // 条目已不在记录流（理论上 app 会先收起）：给可读说明，不留空白浮层。
        None => column("lbtext", vec![label("lbtitle", "结果预览"), muted("lbgone", "该结果已不在记录流中")], 2.0),
    };
    let body = match item.and_then(ResultImage::display_source) {
        Some(path) => ViewNode {
            id: ID_LB_IMAGE.to_string(),
            interaction: None,
            // 吃掉标题行之外的剩余高度，图按 contain 完整呈现
            grow: Some(1),
            body: NodeBody::Image {
                source: ImageSource::Private { path },
                fit: Some(ImageFit::Contain),
                width: Some(SizeSpec::Fill),
                height: Some(SizeSpec::Fill),
            },
            children: Vec::new(),
            events: vec![EventKind::Click, EventKind::SecondaryClick],
        },
        None => column("lbempty", vec![muted("lbhint", "副本未就绪")], 0.0),
    };
    let body = match item {
        Some(entry) if core.session.lightbox_menu => {
            stack("lbbody", vec![body, row_end("lbmenuwrap", vec![lightbox_menu(entry)])])
        }
        _ => body,
    };
    let card = ViewNode {
        id: ID_LB_CARD.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Column {
            gap: Some(8.0),
            padding: Some(PaddingSpec::All(16.0)),
            cross_align: Some(Align::Start),
            align: None,
            size: Some(BoxSize {
                width: Some(SizeSpec::Fill),
                height: Some(SizeSpec::Fill),
                max_width: Some(1080.0),
                max_height: Some(820.0),
                ..BoxSize::default()
            }),
        },
        children: vec![head, body],
        // 图片之外的卡片留白也收起灯箱（图与菜单会先接住各自的按下）
        events: vec![EventKind::Click],
    };
    // 浮层自身声明 click：宿主只在点遮罩（内容之外）时回发，据此收起
    ViewNode {
        id: ID_LIGHTBOX.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Overlay { scrim: None },
        children: vec![card],
        events: vec![EventKind::Click],
    }
}

/// 灯箱右键菜单：在图片右上角展开结果图动作。
fn lightbox_menu(entry: &ResultImage) -> ViewNode {
    let copy_prompt = node_events(
        ID_LB_COPY_PROMPT,
        NodeBody::Button {
            label: "复制提示词".into(),
            icon: None,
            variant: Some(ButtonVariant::Ghost),
            enabled: Some(!entry.prompt_text.is_empty()),
            size: None,
        },
        vec![EventKind::Click],
    );
    let archive = if entry.saved_path.is_some() {
        button(ID_LB_ARCHIVE, "已存到仓库", ButtonVariant::Ghost, false)
    } else {
        button(ID_LB_ARCHIVE, "存到仓库", ButtonVariant::Ghost, true)
    };
    ViewNode {
        id: ID_LB_MENU.to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Card {
            gap: Some(2.0),
            padding: Some(PaddingSpec::All(4.0)),
            accent: None,
            tone: None,
            radius: None,
            bordered: None,
            align: None,
            size: Some(BoxSize { width: Some(SizeSpec::Shrink), min_width: Some(140.0), ..BoxSize::default() }),
        },
        children: vec![column(
            "lbmenuitems",
            vec![copy_prompt, button(ID_LB_COPY_IMAGE, "复制图片", ButtonVariant::Ghost, true), archive, button(ID_LB_SAVE_AS, "另存为…", ButtonVariant::Ghost, true)],
            2.0,
        )],
        events: vec![EventKind::Click],
    }
}

// ---------- 设置页 ----------

pub fn build_settings(core: &Core) -> ViewNode {
    let s = &core.settings;
    let mut cells: Vec<ViewNode> = vec![
        label("settitle", "连接"),
        field_row("host", "地址", text_input(SET_HOST, s.host.clone(), false)),
        field_row("port", "端口", number_input(SET_PORT, Some(s.port as f64), Some(1.0), Some(65535.0), Some(1.0))),
        field_row(
            "mode",
            "进程",
            row(
                "modepick",
                vec![
                    button(SET_MODE_MANAGED, "托管启动", if s.process_mode == ProcessMode::Managed { ButtonVariant::Primary } else { ButtonVariant::Ghost }, true),
                    button(SET_MODE_EXTERNAL, "外部管理", if s.process_mode == ProcessMode::External { ButtonVariant::Primary } else { ButtonVariant::Ghost }, true),
                ],
            ),
        ),
    ];

    if s.process_mode == ProcessMode::Managed {
        cells.push(divider("setdivproc"));
        cells.push(label("setproc", "托管启动"));
        cells.push(field_row("dir", "安装目录", text_input_with_pick(SET_DIR, s.comfy_dir.clone())));
        cells.push(field_row("python", "Python 路径", text_input(SET_PYTHON, s.python_path.clone(), false)));
        cells.push(field_row("extra", "附加参数", text_input(SET_EXTRA, s.extra_args.clone(), false)));
        cells.push(process_line(core));
    }

    cells.push(divider("setdivarchive"));
    cells.push(label("setarchive", "保存到仓库"));
    cells.push(field_row("archive", "仓库子目录", text_input(SET_ARCHIVE, s.archive_folder.clone(), false)));
    cells.push(muted("archivenote", "留空则写入仓库根下的 comfyui/ 目录"));

    scroll("settings_scroll", vec![col("settings", cells)])
}

fn field_row(id_tag: &str, name: &str, control: ViewNode) -> ViewNode {
    row_fill(id_tag, vec![plain(&format!("{id_tag}lbl"), name), control])
}

fn text_input_with_pick(id: &str, value: String) -> ViewNode {
    row_fill(
        &format!("{id}wrap"),
        vec![text_input(id, value, false), icon_button(SET_DIR_PICK, "folder-open", "选择目录", true)],
    )
}

fn process_line(core: &Core) -> ViewNode {
    let host = &core.host;
    let state = if host.running {
        "服务运行中".to_string()
    } else if host.starting {
        "启动中…".to_string()
    } else {
        "服务未启动".to_string()
    };
    let mut cells = vec![muted("procstate", state)];
    if host.running {
        cells.push(button(ID_STOP, "停止", ButtonVariant::Danger, true));
    } else if !host.starting {
        cells.push(button(ID_START, "启动", ButtonVariant::Primary, true));
    }
    // 未运行且留有原因时给出可读说明：启动失败取 error，进程自行退出（崩溃）取 died，
    // 免得一次性通知错过后界面只剩「服务未启动」而无从排查。
    let reason = if host.running || host.starting {
        String::new()
    } else if !host.error.is_empty() {
        host.error.clone()
    } else {
        host.died.clone()
    };
    let mut children = vec![row("procline", cells)];
    if !reason.is_empty() {
        children.push(danger("procline_reason", reason));
    }
    ViewNode {
        id: "procgroup".to_string(),
        interaction: None,
        grow: None,
        body: NodeBody::Column { gap: Some(4.0), padding: None, cross_align: Some(Align::Start), align: None, size: None },
        children,
        events: Vec::new(),
    }
}
