//! 界面树构建：把 `Core` 快照投影成 ViewNode 声明，纯函数、无副作用、无 IO。
//!
//! id 方案即事件契约（app 据此分派）：静态动作固定串；动态控件用前缀 + 键 `${节点id}.${输入名}`，
//! 数值/文本/布尔/下拉分前缀定型，避免「下拉回下标」与「数字回值」在事件侧歧义。节点 id 全树唯一。

use atelyx_plugin_sdk::protocol::view::{
    Align, Axis, ButtonVariant, ColorToken, EventKind, Fill, ImageFit, ImageSource, NodeBody,
    TextWeight, ViewNode,
};

use crate::app::Core;
use crate::history::ResultImage;
use crate::runtime::{build_fields_for, Channel, VaultKind};
use crate::settings::ProcessMode;
use crate::workflow::files;
use crate::workflow::form::{self, FieldSpec};

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

// ---------- 设置页静态节点 id ----------
pub const SET_HOST: &str = "set_host";
pub const SET_PORT: &str = "set_port";
pub const SET_MODE: &str = "set_mode";
pub const SET_DIR: &str = "set_dir";
pub const SET_DIR_PICK: &str = "set_dir_pick";
pub const SET_PYTHON: &str = "set_python";
pub const SET_EXTRA: &str = "set_extra";
pub const SET_ARCHIVE: &str = "set_archive";
pub const SET_AUTO: &str = "set_auto_start";

// ---------- 动态控件 id 前缀 ----------
pub const P_COMBO: &str = "c:";
pub const P_NUM: &str = "n:";
pub const P_TEXT: &str = "t:";
pub const P_BOOL: &str = "b:";
pub const P_SEED: &str = "srand:";
pub const P_UPLOAD: &str = "up:";
pub const P_PASTE: &str = "cpaste:";
pub const P_CLEAR: &str = "clr:";
pub const P_SAVE: &str = "save:";
pub const P_SAVE_AS: &str = "sva:";
pub const P_PREVIEW: &str = "pvw:";

// ---------- 通用构造 ----------

fn node(id: &str, body: NodeBody) -> ViewNode {
    ViewNode { id: id.to_string(), body, children: Vec::new(), events: Vec::new() }
}

/// 带事件声明的叶子节点：控件类节点必须声明订阅的 EventKind，宿主才回上行事件。
fn node_events(id: &str, body: NodeBody, events: Vec<EventKind>) -> ViewNode {
    ViewNode { id: id.to_string(), body, children: Vec::new(), events }
}

fn col(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body: NodeBody::Column { gap: Some(8.0), padding: Some(12.0), cross_align: Some(Align::Start) },
        children,
        events: Vec::new(),
    }
}

fn row(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body: NodeBody::Row { gap: Some(8.0), padding: None, align: None, cross_align: Some(Align::Center), fill: None },
        children,
        events: Vec::new(),
    }
}

fn row_spread(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body: NodeBody::Row { gap: Some(8.0), padding: None, align: None, cross_align: Some(Align::Center), fill: Some(Fill::Width) },
        children,
        events: Vec::new(),
    }
}

fn scroll(id: &str, children: Vec<ViewNode>) -> ViewNode {
    ViewNode {
        id: id.to_string(),
        body: NodeBody::Scroll { direction: Some(Axis::Vertical) },
        children,
        events: Vec::new(),
    }
}

fn label(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: None, weight: Some(TextWeight::Medium), color: None })
}

fn plain(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: None, weight: None, color: None })
}

fn muted(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: Some(12.0), weight: None, color: Some(ColorToken::TextMuted) })
}

fn warn(id: &str, text: impl Into<String>) -> ViewNode {
    node(id, NodeBody::Text { content: text.into(), size: Some(12.0), weight: None, color: Some(ColorToken::Warning) })
}

fn button(id: &str, text: impl Into<String>, variant: ButtonVariant, enabled: bool) -> ViewNode {
    node_events(id, NodeBody::Button { label: text.into(), variant: Some(variant), enabled: Some(enabled) }, vec![EventKind::Click])
}

fn icon_button(id: &str, icon: &str, tooltip: &str, enabled: bool) -> ViewNode {
    node_events(id, NodeBody::IconButton { icon: icon.to_string(), tooltip: Some(tooltip.to_string()), enabled: Some(enabled) }, vec![EventKind::Click])
}

fn text_input(id: &str, value: String, multiline: bool) -> ViewNode {
    node_events(id, NodeBody::TextInput { value, placeholder: None, multiline: (multiline).then_some(true), password: None }, vec![EventKind::Change])
}

fn number_input(id: &str, value: Option<f64>, min: Option<f64>, max: Option<f64>, step: Option<f64>) -> ViewNode {
    node_events(id, NodeBody::NumberInput { value, min, max, step, placeholder: None }, vec![EventKind::Change])
}

fn dropdown(id: &str, options: Vec<String>, selected: Option<u32>) -> ViewNode {
    node_events(id, NodeBody::Dropdown { options, selected, placeholder: None }, vec![EventKind::Change])
}

fn checkbox(id: &str, checked: bool, text: Option<String>) -> ViewNode {
    node_events(id, NodeBody::Checkbox { checked, label: text }, vec![EventKind::Change])
}

// ---------- 生成面板 ----------

pub fn build_panel(core: &Core) -> ViewNode {
    let snap = &core.runtime.snap;
    let mut body: Vec<ViewNode> = Vec::new();

    if !snap.error.is_empty() {
        body.push(row(
            "errline",
            vec![
                node("errtext", NodeBody::Text { content: snap.error.clone(), size: None, weight: None, color: Some(ColorToken::Danger) }),
                button(ID_CLEAR_ERROR, "关闭", ButtonVariant::Ghost, true),
            ],
        ));
    } else if !core.session.notice.is_empty() {
        body.push(muted("notice", core.session.notice.clone()));
    }

    body.push(workflow_section(core));
    body.push(action_section(core));
    body.push(status_section(core));
    body.push(param_section(core));
    body.push(gallery_section(core));

    let root_children = vec![status_bar(core), scroll("body", body)];
    col("root", root_children)
}

fn status_bar(core: &Core) -> ViewNode {
    let mut cells = vec![channel_text(core)];

    if core.settings.process_mode == ProcessMode::Managed {
        let control = if core.host.starting {
            muted("starting", "启动中…")
        } else if core.host.running {
            button(ID_STOP, "停止服务", ButtonVariant::Danger, true)
        } else {
            button(ID_START, "启动服务", ButtonVariant::Primary, true)
        };
        cells.push(control);
    }
    cells.push(icon_button(ID_REFRESH, "refresh-cw", "刷新状态", true));
    cells.push(icon_button(ID_OPEN_BROWSER, "globe", "在浏览器打开", true));
    row_spread("statusbar", cells)
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
    node("channel", NodeBody::Text { content, size: None, weight: Some(TextWeight::Medium), color: Some(color) })
}

fn workflow_section(core: &Core) -> ViewNode {
    let names: Vec<String> = core.runtime.snap.workflows.iter().map(|f| files::workflow_display_name(&f.name)).collect();
    let selected = core.session.workflow_id.as_ref().and_then(|id| {
        core.runtime.snap.workflows.iter().position(|f| f.path == *id).map(|i| i as u32)
    });
    let cells = vec![
        plain("wflbl", "工作流"),
        dropdown(ID_WF, names, selected),
        button(ID_SAVE_WF, "保存参数", ButtonVariant::Ghost, core.session.workflow_id.is_some()),
    ];
    let mut col = col("wfsection", cells);
    // 清单读取失败时下拉是空的，必须说明原因；否则只在文件被外部改动时提示。
    if !core.runtime.snap.workflows_error.is_empty() {
        col.children.push(warn("wferr", format!("工作流列表读取失败：{}", core.runtime.snap.workflows_error)));
    } else if core.session.current_changed {
        col.children.push(warn("wfchanged", "文件已被外部修改，重新选择可载入最新内容"));
    }
    col
}

fn action_section(core: &Core) -> ViewNode {
    let online = core.runtime.snap.channel == Channel::Direct;
    let has_draft = core.session.draft.as_object().map(|m| !m.is_empty()).unwrap_or(false);
    col(
        "actions",
        vec![
            row_spread(
                "actionsrow",
                vec![
                    button(ID_RUN, "生成", ButtonVariant::Primary, online && has_draft),
                    button(ID_INTERRUPT, "中断", ButtonVariant::Ghost, online && core.runtime.snap.running_prompt_id.is_some()),
                    button(ID_FREE, "释放显存", ButtonVariant::Ghost, online),
                ],
            ),
            queue_line(core),
        ],
    )
}

fn status_section(core: &Core) -> ViewNode {
    let snap = &core.runtime.snap;
    let mut cells = Vec::new();
    if let Some(progress) = &snap.progress {
        cells.push(node("bar", NodeBody::Progress { value: progress.fraction() }));
        cells.push(muted("step", format!("{:.0}/{}", progress.value, progress.max)));
    }
    if cells.is_empty() {
        return col("statussec", Vec::new());
    }
    col("statussec", vec![row_spread("progressrow", cells)])
}

fn queue_line(core: &Core) -> ViewNode {
    let snap = &core.runtime.snap;
    let running = snap.queue_running.len();
    let pending = snap.queue_pending.len();
    muted("queue", format!("队列：运行 {running} · 等待 {pending}"))
}

fn param_section(core: &Core) -> ViewNode {
    let fields = build_fields_for(&core.session.draft, &core.runtime.snap);
    if fields.is_empty() {
        return col("params", vec![muted("noparam", "选择工作流后在此调参")]);
    }
    let mut cells: Vec<ViewNode> = Vec::new();
    let mut current_node: Option<String> = None;
    for field in &fields {
        if current_node.as_deref() != Some(field.node_id.as_str()) {
            cells.push(label(&format!("hd:{}", field.node_id), field.node_label.clone()));
            current_node = Some(field.node_id.clone());
        }
        cells.push(field_control(&core.session.draft, &core.session.seed_mode, field));
    }
    col("params", cells)
}

fn field_control(draft: &serde_json::Value, seed_mode: &std::collections::BTreeSet<String>, field: &FieldSpec) -> ViewNode {
    let key = field.key();
    let text_label = format!("flbl:{key}");
    let label_node = plain(&text_label, input_label(field));

    if field.connected {
        return row(
            &format!("frow:{key}"),
            vec![label_node, muted(&format!("fval:{key}"), form::connected_display(field.link_from.as_deref()))],
        );
    }

    let image = form::is_image_input(field);
    let mut cells: Vec<ViewNode> = vec![label_node];

    if image {
        let value = form::display_value(form::read_field(draft, field).as_ref());
        cells.push(text_input(&format!("{P_TEXT}{key}"), value, false));
        cells.push(button(&format!("{P_UPLOAD}{key}"), "上传", ButtonVariant::Ghost, true));
        cells.push(button(&format!("{P_PASTE}{key}"), "粘贴", ButtonVariant::Ghost, true));
        cells.push(button(&format!("{P_CLEAR}{key}"), "清除", ButtonVariant::Ghost, true));
        return row(&format!("frow:{key}"), cells);
    }

    match field.kind {
        form::FieldKind::Combo => {
            let options = field.options.clone().unwrap_or_default();
            let labels: Vec<String> = options.iter().map(|(_, label)| label.clone()).collect();
            let current = form::display_value(form::read_field(draft, field).as_ref());
            let selected = options.iter().position(|(value, _)| *value == current).map(|i| i as u32);
            cells.push(dropdown(&format!("{P_COMBO}{key}"), labels, selected));
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
    row(&format!("frow:{key}"), cells)
}

fn input_label(field: &FieldSpec) -> String {
    let named = form::field_label(&field.input);
    if named.is_empty() {
        field.input.clone()
    } else {
        named.to_string()
    }
}

fn gallery_section(core: &Core) -> ViewNode {
    let results = &core.runtime.snap.results;
    if results.is_empty() {
        return col("gallery", vec![plain("gallerytitle", "结果"), muted("noresult", "尚无生成结果")]);
    }
    let can_archive = core.runtime.snap.vault_kind == VaultKind::Local;
    let cards: Vec<ViewNode> = results.iter().map(|item| result_card(item, can_archive)).collect();
    let grid = ViewNode {
        id: "resgrid".to_string(),
        body: NodeBody::Grid { columns: 2, gap: Some(10.0) },
        children: cards,
        events: Vec::new(),
    };
    col("gallery", vec![label("gallerytitle", "结果"), grid])
}

fn result_card(item: &ResultImage, can_archive: bool) -> ViewNode {
    let key = item.key.as_str();
    // 预览与卡片缩略图同一个判定：只认本地副本（图面按私有目录相对路径取源，没有副本就没有可呈现的字节）。
    let source = item.display_source();
    let has_copy = source.is_some();
    let visual = match source {
        Some(path) => node(
            &format!("img:{key}"),
            NodeBody::Image { source: ImageSource::Private { path }, fit: Some(ImageFit::Contain) },
        ),
        None => muted(&format!("noimg:{key}"), "副本未就绪"),
    };
    let mut cells = vec![
        visual,
        label(&format!("rt:{key}"), if item.title.is_empty() { "未命名任务".to_string() } else { item.title.clone() }),
    ];
    if item.failed {
        cells.push(node(
            &format!("rfail:{key}"),
            NodeBody::Text { content: "执行出错".into(), size: Some(12.0), weight: None, color: Some(ColorToken::Danger) },
        ));
    }
    if !item.prompt_text.is_empty() {
        cells.push(muted(&format!("rp:{key}"), clip(&item.prompt_text, 60)));
    }
    let tail = match &item.saved_path {
        Some(path) => muted(&format!("rsaved:{key}"), format!("已存 {path}")),
        None => button(&format!("{P_SAVE}{key}"), "存到仓库", ButtonVariant::Ghost, can_archive),
    };
    let actions = vec![
        tail,
        icon_button(&format!("{P_PREVIEW}{key}"), "maximize-2", "大图预览", has_copy),
        icon_button(&format!("{P_SAVE_AS}{key}"), "download", "另存为…", true),
    ];
    cells.push(row(&format!("ract:{key}"), actions));
    ViewNode {
        id: format!("card:{key}"),
        body: NodeBody::Card { padding: Some(8.0), accent: None },
        children: cells,
        events: Vec::new(),
    }
}

fn clip(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

// ---------- 大图预览窗口 ----------

/// 预览窗口面树：标题 + 铺满剩余高度的滚动区（图按自然尺寸呈现，超出窗口尺寸时可滚）。
/// 关闭窗口交给系统标题栏；面树本身无交互控件。
pub fn build_preview(core: &Core) -> ViewNode {
    let item = core
        .session
        .preview_key
        .as_deref()
        .and_then(|key| core.runtime.snap.results.iter().find(|r| r.key == key));
    let title = match item {
        Some(entry) if !entry.title.is_empty() => entry.title.clone(),
        Some(_) => "未命名任务".to_string(),
        None => "结果预览".to_string(),
    };
    let body = match item.and_then(ResultImage::display_source) {
        Some(path) => scroll("pvwscroll", vec![node("pvwimage", NodeBody::Image { source: ImageSource::Private { path }, fit: Some(ImageFit::Contain) })]),
        // 条目可能因历史淘汰而消失：给可读说明，不留空白面。
        None => col("pvwempty", vec![muted("pvwhint", "该结果已不在画廊中")]),
    };
    col("pvwroot", vec![label("pvwtitle", title), body])
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
            dropdown(SET_MODE, vec!["托管启动".to_string(), "外部管理".to_string()], Some(match s.process_mode {
                ProcessMode::Managed => 0,
                ProcessMode::External => 1,
            })),
        ),
    ];

    if s.process_mode == ProcessMode::Managed {
        cells.push(label("setproc", "托管启动"));
        cells.push(field_row("dir", "安装目录", text_input_with_pick(SET_DIR, s.comfy_dir.clone())));
        cells.push(field_row("python", "Python 路径", text_input(SET_PYTHON, s.python_path.clone(), false)));
        cells.push(field_row("extra", "附加参数", text_input(SET_EXTRA, s.extra_args.clone(), false)));
        cells.push(field_row("auto", "开机自启", checkbox(SET_AUTO, s.auto_start, None)));
        cells.push(process_line(core));
    }

    cells.push(label("setarchive", "保存到仓库"));
    cells.push(field_row("archive", "仓库子目录", text_input(SET_ARCHIVE, s.archive_folder.clone(), false)));
    cells.push(muted("archivenote", "留空则写入仓库根下的 comfyui/ 目录"));

    scroll("settings_scroll", vec![col("settings", cells)])
}

fn field_row(id_tag: &str, name: &str, control: ViewNode) -> ViewNode {
    row(id_tag, vec![plain(&format!("{id_tag}lbl"), name), control])
}

fn text_input_with_pick(id: &str, value: String) -> ViewNode {
    row(
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
        children.push(node(
            "procline_reason",
            NodeBody::Text { content: reason, size: Some(12.0), weight: None, color: Some(ColorToken::Danger) },
        ));
    }
    ViewNode {
        id: "procgroup".to_string(),
        body: NodeBody::Column { gap: Some(4.0), padding: None, cross_align: Some(Align::Start) },
        children,
        events: Vec::new(),
    }
}
