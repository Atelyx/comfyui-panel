//! 结果记录流的画布布局：按批次成卡，一次算出绘制指令与命中区域。
//!
//! 卡片头是标题、出错标记、时间与复制提示词；卡内图格按各自图片的真实宽高比排布，
//! 格面 cover 裁切。鼠标停在图格上时底部浮出文件名与保存动作，已落库的图格带「已保存」
//! 角标。运行中的批次排在最后，以四态占位格呈现排队 / 进行中 / 已出 / 待出，
//! 进度条与步数画在进行中的格里。
//!
//! 绘制与命中同源于一次 [`layout`]：坐标只在结构里算一遍，指令与热区因此不会漂移。
//! 命中 id 经 [`FlowLayout::action_of`] 映射成动作，由 app 的事件分派直接消费，
//! 画布不引入第二套动作语义。
//!
//! 整面帧没有裁剪指令：图格与文字只画得进视口时才画，底色块按视口裁短；
//! 悬停浮层（文件名与保存章）只在指针落在该格或其保存章上时出现。

use serde_json::Value;

use atelyx_plugin_protocol::frame::HitCursor;
use atelyx_plugin_protocol::paint::{
    ColorToken, DrawCommand, ImageFit, ImageSource, PaintColor, TextWeight,
};
use atelyx_plugin_sdk::frame_ui::Canvas;

use crate::comfy::types::queue_item_prompt;
use crate::history::ResultImage;
use crate::workflow::form::{prompt_text_of, FieldSpec};

/// 图格最小宽度：列数按内容区宽度在此基线上折算。
const CELL_MIN: f32 = 150.0;
/// 命中区 id 前缀：整格开灯箱、悬停保存、复制提示词、运行中复制与中断。
pub const HIT_PREVIEW: &str = "pvw:";
pub const HIT_SAVE: &str = "save:";
pub const HIT_COPY: &str = "copy:";
pub const HIT_COPY_RUN: &str = "flow:copy_run";
pub const HIT_INTERRUPT: &str = "flow:interrupt";
/// 图格比例上下界：极端比例按边界裁切，避免超宽压成一线、超高撑满一屏。
const ASPECT_MIN: f32 = 0.6;
const ASPECT_MAX: f32 = 2.4;
/// 图格最大高度。
const CELL_MAX_H: f32 = 320.0;
/// 画布被压到极窄时图格宽度的兜底值，不留零宽指令。
const CELL_FLOOR: f32 = 24.0;

/// 卡片外沿间距与卡片间距。
const CARD_GAP: f32 = 10.0;
/// 卡片内边距。
const PAD: f32 = 10.0;
/// 卡片头高度（含上下留白）。
const HEADER_H: f32 = 28.0;
/// 格与格、头与图墙之间的间距。
const GAP: f32 = 8.0;
/// 图格内进度条厚度。
const PROG_H: f32 = 4.0;
/// 悬停条高度。
const HOVER_H: f32 = 24.0;
/// 卡片头小章的尺寸。
const CHIP_H: f32 = 16.0;
const COPY_W: f32 = 32.0;
const INTERRUPT_W: f32 = 42.0;
const SAVE_W: f32 = 30.0;
/// 卡片头右侧状态文案的占宽。
const STATUS_W: f32 = 100.0;
/// 「出错」标记占宽。
const FAILED_W: f32 = 26.0;
/// 卡片头里元素之间的间距。
const HEAD_GAP: f32 = 6.0;

/// 中性档位：卡片与图格走主题 token，深色浮条与浅色小章用自带 RGBA。
const BAR_BG: [u8; 4] = [0, 0, 0, 155];
const BAR_TEXT: [u8; 4] = [255, 255, 255, 235];
const LIGHT_BG: [u8; 4] = [255, 255, 255, 230];
const LIGHT_TEXT: [u8; 4] = [17, 17, 17, 255];
const BADGE_BG: [u8; 4] = [0, 0, 0, 160];
const BADGE_TEXT: [u8; 4] = [255, 255, 255, 240];

/// 一张图：定位 key、文件名、可绘制的本地副本、是否出错、是否已落库、原图宽高比。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowCell {
    pub key: String,
    pub name: String,
    pub source: Option<String>,
    pub failed: bool,
    pub saved: bool,
    pub aspect: Option<f32>,
}

/// 一条已完成记录：同一批（同 promptId）的若干张图 + 卡片头信息。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowRecord {
    pub prompt_id: String,
    pub title: String,
    pub time: String,
    pub failed: bool,
    /// 该批是否有提示词原文：没有就不画复制小章。
    pub can_copy: bool,
    pub cells: Vec<FlowCell>,
}

/// 运行中批次：占位格数、比例、整批采样进度（无进度即排队中）、步数文案与状态行。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowRunning {
    pub title: String,
    pub cells: usize,
    pub aspect: f32,
    pub percent: Option<f64>,
    pub steps: Option<String>,
    pub status: String,
    /// 队首任务是否带提示词原文：没有就不画复制小章。
    pub can_copy: bool,
}

/// 布局输入：内容区宽度、按旧→新排好的记录与末尾的运行中批次。
#[derive(Clone, Debug, Default)]
pub struct FlowInput {
    pub width: f32,
    pub records: Vec<FlowRecord>,
    pub running: Option<FlowRunning>,
}

/// 命中区对应的动作：目标标识在这里定型，app 执行时按当前状态回查。
#[derive(Clone, Debug, PartialEq)]
pub enum FlowAction {
    Preview(String),
    Save(String),
    CopyPrompt(String),
    CopyRun,
    Interrupt,
}

/// 一个命中区域：矩形 + 事件 id + 动作。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub id: String,
    pub action: FlowAction,
}

/// 悬停浮层的开闭条件：指针落在整格或保存章上都算悬停。
#[derive(Clone, Debug)]
pub struct HoverGate {
    pub preview: String,
    pub save: String,
}

/// 一条绘制指令及其悬停条件。
#[derive(Clone, Debug)]
pub struct FlowOp {
    pub command: DrawCommand,
    pub hover: Option<std::sync::Arc<HoverGate>>,
}

/// 布局结果：内容总高、绘制指令、命中区域。
#[derive(Clone, Debug, Default)]
pub struct FlowLayout {
    pub height: f32,
    pub ops: Vec<FlowOp>,
    pub hits: Vec<FlowHit>,
}

impl FlowLayout {
    /// 命中 id → 动作：app 的事件分派据此执行。
    pub fn action_of(&self, id: &str) -> Option<&FlowAction> {
        self.hits.iter().find(|hit| hit.id == id).map(|hit| &hit.action)
    }

    fn push(&mut self, command: DrawCommand) {
        self.ops.push(FlowOp { command, hover: None });
    }

    fn push_hover(&mut self, command: DrawCommand, gate: &std::sync::Arc<HoverGate>) {
        self.ops.push(FlowOp { command, hover: Some(gate.clone()) });
    }

    fn push_hit(&mut self, x: f32, y: f32, w: f32, h: f32, id: String, action: FlowAction) {
        self.hits.push(FlowHit { x, y, w, h, id, action });
    }
}

/// 图墙内容区宽度：卡片两侧各留一个内边距。
fn content_width(width: f32) -> f32 {
    (width - PAD * 2.0).max(CELL_FLOOR)
}

/// 列数：内容区里能塞下几列（至少一列）。
pub fn columns_for(width: f32) -> usize {
    let by_width = (content_width(width) + GAP) / (CELL_MIN + GAP);
    (by_width.floor() as usize).max(1)
}

/// 单元格宽度：列均分内容区；高度由比例决定并受最大高约束。
pub fn cell_size(width: f32, columns: usize, aspect: f32) -> (f32, f32) {
    let aspect = clamp_aspect(aspect);
    let columns = columns.max(1);
    let inner = (content_width(width) - GAP * (columns as f32 - 1.0)).max(CELL_FLOOR);
    let w = (inner / columns as f32).max(CELL_FLOOR);
    (w, (w / aspect).min(CELL_MAX_H))
}

fn clamp_aspect(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 { aspect.clamp(ASPECT_MIN, ASPECT_MAX) } else { 1.0 }
}

/// 文本像素宽度估算：中日韩按整宽、其余按半宽（与 [`clip_text`] 同一口径）。
pub fn text_px(text: &str, size: f32) -> f32 {
    if size <= 0.0 {
        return 0.0;
    }
    text.chars().map(|c| if is_wide(c) { size } else { size * 0.55 }).sum()
}

/// 把文本裁到指定像素宽度内：中日韩按整宽、其余按半宽估算，超出补省略号。
pub fn clip_text(text: &str, max_px: f32, size: f32) -> String {
    if size <= 0.0 || max_px <= 0.0 {
        return String::new();
    }
    let unit = |c: char| if is_wide(c) { size } else { size * 0.55 };
    if text_px(text, size) <= max_px {
        return text.to_string();
    }
    let budget = max_px - unit('…');
    let mut used = 0.0;
    let mut out = String::new();
    for c in text.chars() {
        let w = unit(c);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// 队列任务里的输出规格（比例、张数）：与生成坞三段同一口径（`sizing` 的字段判定）。
pub fn running_spec(prompt: &Value, fields: &[FieldSpec]) -> (f32, usize) {
    let aspect = crate::workflow::sizing::output_size(prompt, fields)
        .map(|(w, h)| (w / h) as f32)
        .unwrap_or(1.0);
    (aspect, crate::workflow::sizing::batch_count(prompt, fields).clamp(1, 12))
}

/// 画廊条目按 promptId 聚合成记录（同批多张并成一条），展示顺序旧→新（最新沉在底部）。
pub fn records_of(results: &[ResultImage]) -> Vec<FlowRecord> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: Vec<Vec<&ResultImage>> = Vec::new();
    for item in results {
        match order.iter().position(|id| *id == item.prompt_id.as_str()) {
            Some(index) => groups[index].push(item),
            None => {
                order.push(&item.prompt_id);
                groups.push(vec![item]);
            }
        }
    }
    // 画廊最近在前：反转为旧→新，同批内保持原有图序
    order
        .into_iter()
        .zip(groups)
        .rev()
        .map(|(prompt_id, items)| {
            let lead = items[0];
            let cells: Vec<FlowCell> = items
                .iter()
                .map(|item| FlowCell {
                    key: item.key.clone(),
                    name: item.image_ref.filename.clone(),
                    source: item.display_source(),
                    failed: item.failed,
                    saved: item.saved_path.is_some(),
                    aspect: item.pixel.map(|(w, h)| if h > 0 { w as f32 / h as f32 } else { 1.0 }),
                })
                .collect();
            FlowRecord {
                prompt_id: prompt_id.to_string(),
                title: if lead.prompt_text.is_empty() {
                    if lead.title.is_empty() { "未命名任务".to_string() } else { lead.title.clone() }
                } else {
                    lead.prompt_text.clone()
                },
                can_copy: items.iter().any(|item| !item.prompt_text.is_empty()),
                time: format_time(lead.created_at),
                failed: items.iter().any(|item| item.failed),
                cells,
            }
        })
        .collect()
}

/// 毫秒时间戳 → 本地时刻：当天给 `时:分`，跨天补 `月-日`，多天历史不混淆；无效值返回空串。
pub fn format_time(millis: f64) -> String {
    if !millis.is_finite() || millis <= 0.0 {
        return String::new();
    }
    use chrono::{Local, TimeZone};
    let Some(moment) = Local.timestamp_millis_opt(millis as i64).single() else {
        return String::new();
    };
    if moment.date_naive() == Local::now().date_naive() {
        moment.format("%H:%M").to_string()
    } else {
        moment.format("%m-%d %H:%M").to_string()
    }
}

/// 从队列快照拼出运行中批次：规格（比例、张数）由调用方按当前草稿或队首任务备好，
/// 画布层只做纯换算；标题取队首任务的提示词。
pub fn running_block(
    running: &[Value],
    pending: &[Value],
    percent: Option<f64>,
    steps: Option<String>,
    spec: (f32, usize),
) -> Option<FlowRunning> {
    let lead = running.first().or(pending.first())?;
    let text = prompt_text_of(&queue_item_prompt(lead).cloned().unwrap_or(Value::Null));
    Some(FlowRunning {
        title: if text.is_empty() { "生成中".to_string() } else { text.clone() },
        cells: spec.1,
        aspect: spec.0,
        percent: percent.filter(|value| value.is_finite()),
        steps,
        status: format!("运行 {} · 等待 {}", running.len(), pending.len()),
        can_copy: !text.is_empty(),
    })
}

/// 排布整条记录流：返回绘制指令、命中区域与内容总高。
pub fn layout(input: &FlowInput) -> FlowLayout {
    let width = input.width.max(CELL_FLOOR);
    let columns = columns_for(width);
    let mut out = FlowLayout::default();
    for record in &input.records {
        push_record(&mut out, width, columns, record);
    }
    if let Some(running) = &input.running {
        push_running(&mut out, width, columns, running);
    }
    out.height = (out.height - CARD_GAP).max(0.0);
    out
}

/// 一条已完成记录：卡片头（标题/出错/时间/复制）+ 按真实比例排布的图墙。
fn push_record(out: &mut FlowLayout, width: f32, columns: usize, record: &FlowRecord) {
    let top = out.height;
    let (cell_w, _) = cell_size(width, columns, 1.0);
    let rows: Vec<f32> = record
        .cells
        .chunks(columns)
        .map(|chunk| {
            chunk
                .iter()
                .map(|cell| cell_size(width, columns, cell.aspect.unwrap_or(1.0)).1)
                .fold(0.0, f32::max)
        })
        .collect();
    let body_h = rows.iter().sum::<f32>() + GAP * rows.len().saturating_sub(1) as f32;
    let card_h = HEADER_H + PAD * 2.0 + body_h;
    out.push(card(width, top, card_h));

    push_record_head(out, width, top, record);
    let mut y = top + HEADER_H + PAD;
    for (row_index, chunk) in record.cells.chunks(columns).enumerate() {
        for (col, cell) in chunk.iter().enumerate() {
            let x = PAD + col as f32 * (cell_w + GAP);
            let h = cell_size(width, columns, cell.aspect.unwrap_or(1.0)).1;
            push_cell(out, cell, x, y, cell_w, h);
        }
        y += rows[row_index] + GAP;
    }
    out.height = top + card_h + CARD_GAP;
}

fn push_record_head(out: &mut FlowLayout, width: f32, top: f32, record: &FlowRecord) {
    let y = top + (HEADER_H - 12.0) / 2.0;
    let right = width - PAD;
    let copy_x = if record.can_copy { right - COPY_W } else { right };
    // 时间占宽按实际文案算：跨天的记录带日期，比「时:分」宽一截
    let time_w = text_px(&record.time, 10.0) + 4.0;
    let time_x = copy_x - HEAD_GAP - time_w;
    let mut limit = time_x - HEAD_GAP;
    if record.failed {
        limit -= FAILED_W + HEAD_GAP;
    }
    let title_x = PAD;
    out.push(text(
        title_x,
        y,
        &clip_text(&record.title, limit - title_x, 12.0),
        12.0,
        token(ColorToken::Text),
        Some(TextWeight::Medium),
    ));
    if record.failed {
        out.push(text(limit + HEAD_GAP, y + 1.0, "出错", 10.0, token(ColorToken::Danger), None));
    }
    out.push(text(time_x, y + 1.0, &record.time, 10.0, token(ColorToken::TextMuted), None));
    if !record.can_copy {
        return;
    }
    let chip_y = top + (HEADER_H - CHIP_H) / 2.0;
    out.push(rect(copy_x, chip_y, COPY_W, CHIP_H, Some(token(ColorToken::SurfaceMuted)), Some(token(ColorToken::Border))));
    out.push(text(copy_x + 6.0, chip_y + 2.0, "复制", 10.0, token(ColorToken::TextMuted), None));
    out.push_hit(
        copy_x,
        chip_y,
        COPY_W,
        CHIP_H,
        format!("{HIT_COPY}{}", digest(&record.prompt_id)),
        FlowAction::CopyPrompt(record.prompt_id.clone()),
    );
}

/// 一个图格：副本就绪就贴 cover 图，否则留底色与说明，出错描红边；悬停浮出文件名与保存。
fn push_cell(out: &mut FlowLayout, cell: &FlowCell, x: f32, y: f32, w: f32, h: f32) {
    let edge = if cell.failed { token(ColorToken::Danger) } else { token(ColorToken::Border) };
    out.push(rounded(x, y, w, h, Some(token(ColorToken::SurfaceSunken)), Some(edge), 6.0));
    if let Some(path) = &cell.source {
        out.push(DrawCommand::Image {
            source: ImageSource::Private { path: path.clone() },
            x,
            y,
            w,
            h,
            fit: Some(ImageFit::Cover),
        });
    }
    let hint = match (&cell.source, cell.failed) {
        (Some(_), _) => "",
        (None, true) => "出错",
        (None, false) => "副本未就绪",
    };
    if !hint.is_empty() {
        let color = if cell.failed { token(ColorToken::Danger) } else { token(ColorToken::TextMuted) };
        out.push(text(x + (w - text_px(hint, 11.0)) / 2.0, y + (h - 11.0) / 2.0, hint, 11.0, color, None));
    }
    if cell.saved {
        let badge = "已保存 ✓";
        let badge_w = text_px(badge, 9.0) + 10.0;
        out.push(rounded(x + w - badge_w - 4.0, y + 4.0, badge_w, 16.0, Some(rgba(BADGE_BG)), None, 4.0));
        out.push(text(x + w - badge_w + 1.0, y + 7.0, badge, 9.0, rgba(BADGE_TEXT), None));
    }

    let preview_id = format!("{HIT_PREVIEW}{}", digest(&cell.key));
    let save_id = format!("{HIT_SAVE}{}", digest(&cell.key));
    out.push_hit(x, y, w, h, preview_id.clone(), FlowAction::Preview(cell.key.clone()));
    let bar_y = y + h - HOVER_H;
    let gate = std::sync::Arc::new(HoverGate { preview: preview_id, save: save_id.clone() });
    out.push_hover(rect(x, bar_y, w, HOVER_H, Some(rgba(BAR_BG)), None), &gate);
    let name_w = if cell.saved { w - 12.0 } else { w - 12.0 - SAVE_W - 6.0 };
    out.push_hover(
        text(x + 6.0, bar_y + 6.0, &clip_text(&cell.name, name_w, 10.0), 10.0, rgba(BAR_TEXT), None),
        &gate,
    );
    if !cell.saved {
        let save_x = x + w - 6.0 - SAVE_W;
        let save_y = bar_y + (HOVER_H - 16.0) / 2.0;
        out.push_hover(rounded(save_x, save_y, SAVE_W, 16.0, Some(rgba(LIGHT_BG)), None, 4.0), &gate);
        out.push_hover(text(save_x + 5.0, save_y + 2.0, "保存", 10.0, rgba(LIGHT_TEXT), None), &gate);
        out.push_hit(save_x, save_y, SAVE_W, 16.0, save_id, FlowAction::Save(cell.key.clone()));
    }
}

/// 运行中批次：四态占位格，进行中格内画进度条与步数。
fn push_running(out: &mut FlowLayout, width: f32, columns: usize, running: &FlowRunning) {
    let top = out.height;
    let cells = running.cells.clamp(1, 12);
    let (cell_w, cell_h) = cell_size(width, columns, running.aspect);
    let rows = cells.div_ceil(columns);
    let body_h = rows as f32 * cell_h + rows.saturating_sub(1) as f32 * GAP;
    let card_h = HEADER_H + PAD * 2.0 + body_h;
    out.push(card(width, top, card_h));

    let y = top + (HEADER_H - 12.0) / 2.0;
    let right = width - PAD;
    let int_x = right - INTERRUPT_W;
    let copy_x = if running.can_copy { int_x - HEAD_GAP - COPY_W } else { int_x };
    let status_x = copy_x - HEAD_GAP - STATUS_W;
    // 窄画布下状态行让位给标题：放不下就不画，避免标题被压成空串、文案跑到画布外。
    let status = if status_x - HEAD_GAP - PAD >= 48.0 {
        clip_text(&running.status, STATUS_W, 10.0)
    } else {
        String::new()
    };
    let title_limit = if status.is_empty() { copy_x - HEAD_GAP - PAD } else { status_x - HEAD_GAP - PAD };
    out.push(text(PAD, y, &clip_text(&running.title, title_limit, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    out.push(text(status_x, y + 1.0, &status, 10.0, token(ColorToken::TextMuted), None));
    let chip_y = top + (HEADER_H - CHIP_H) / 2.0;
    if running.can_copy {
        out.push(rect(copy_x, chip_y, COPY_W, CHIP_H, Some(token(ColorToken::SurfaceMuted)), Some(token(ColorToken::Border))));
        out.push(text(copy_x + 6.0, chip_y + 2.0, "复制", 10.0, token(ColorToken::TextMuted), None));
        out.push_hit(copy_x, chip_y, COPY_W, CHIP_H, HIT_COPY_RUN.to_string(), FlowAction::CopyRun);
    }
    out.push(rect(int_x, chip_y, INTERRUPT_W, CHIP_H, None, Some(token(ColorToken::Danger))));
    out.push(text(int_x + 7.0, chip_y + 2.0, "中断", 10.0, token(ColorToken::Danger), None));
    out.push_hit(int_x, chip_y, INTERRUPT_W, CHIP_H, HIT_INTERRUPT.to_string(), FlowAction::Interrupt);

    let pct = running.percent.map(|value| (value / 100.0).clamp(0.0, 1.0));
    let mut y = top + HEADER_H + PAD;
    for index in 0..cells {
        if index > 0 && index % columns == 0 {
            y += cell_h + GAP;
        }
        let x = PAD + (index % columns) as f32 * (cell_w + GAP);
        let state = cell_state(index, cells, running.percent);
        let edge = match state {
            CellState::Done | CellState::Active => Some(token(ColorToken::Accent)),
            _ => Some(token(ColorToken::Border)),
        };
        let fill = match state {
            CellState::Done => Some(token(ColorToken::AccentSoft)),
            _ => Some(token(ColorToken::SurfaceSunken)),
        };
        out.push(rounded(x, y, cell_w, cell_h, fill, edge, 6.0));
        if let (CellState::Active, Some(pct)) = (state, pct) {
            out.push(rect(x, y + cell_h - PROG_H, cell_w, PROG_H, Some(rgba(BADGE_BG)), None));
            out.push(rect(x, y + cell_h - PROG_H, (cell_w * pct as f32).max(2.0), PROG_H, Some(token(ColorToken::Accent)), None));
        }
        let label = match state {
            CellState::Queued => "排队中".to_string(),
            CellState::Active => running
                .steps
                .clone()
                .unwrap_or_else(|| format!("{:.0}%", running.percent.unwrap_or(0.0).clamp(0.0, 100.0))),
            CellState::Done => "已出".to_string(),
            CellState::Waiting => String::new(),
        };
        if !label.is_empty() {
            let size = if cell_h > 120.0 { 18.0 } else if cell_h > 80.0 { 14.0 } else { 11.0 };
            out.push(text(
                x + (cell_w - text_px(&label, size)) / 2.0,
                y + (cell_h - size) / 2.0,
                &label,
                size,
                token(ColorToken::TextMuted),
                None,
            ));
        }
    }
    out.height = top + card_h + CARD_GAP;
}

/// 占位格四态：没有进度即整批排队；有进度时按整批进度切出「已出 / 进行中 / 待出」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellState {
    Queued,
    Done,
    Active,
    Waiting,
}

fn cell_state(index: usize, cells: usize, percent: Option<f64>) -> CellState {
    let Some(raw) = percent else { return CellState::Queued };
    let percent = raw.clamp(0.0, 100.0);
    if percent >= 100.0 {
        return CellState::Done;
    }
    let active = ((percent / 100.0) * cells as f64).floor() as usize;
    if index < active {
        CellState::Done
    } else if index == active {
        CellState::Active
    } else {
        CellState::Waiting
    }
}

/// 把整份布局画进自绘块：坐标按块左上角平移，超出视口的块裁短，画不全的图与文字整项跳过。
pub fn paint(canvas: &mut Canvas<'_, '_>, layout: &FlowLayout) {
    let area = canvas.area();
    let visible = canvas.visible();
    for op in &layout.ops {
        if let Some(gate) = &op.hover {
            if !(canvas.hovered(&gate.preview) || canvas.hovered(&gate.save)) {
                continue;
            }
        }
        paint_command(canvas, &op.command, area.x, area.y, visible);
    }
    // 命中区最后登记：后声明的在上，保存章盖住整格
    for hit in &layout.hits {
        let rect = atelyx_plugin_sdk::frame_ui::Rect::new(area.x + hit.x, area.y + hit.y, hit.w, hit.h);
        let clipped = rect.intersect(visible);
        if clipped.is_empty() {
            continue;
        }
        canvas.hit(&hit.id, clipped, HitCursor::Pointer);
    }
}

fn paint_command(canvas: &mut Canvas<'_, '_>, command: &DrawCommand, dx: f32, dy: f32, visible: atelyx_plugin_sdk::frame_ui::Rect) {
    match command {
        DrawCommand::Rect { x, y, w, h, fill, stroke, stroke_width, corner_radius, shadow: _ } => {
            let rect = atelyx_plugin_sdk::frame_ui::Rect::new(x + dx, y + dy, *w, *h).intersect(visible);
            if rect.is_empty() {
                return;
            }
            canvas.draw(DrawCommand::Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
                fill: *fill,
                stroke: *stroke,
                stroke_width: *stroke_width,
                corner_radius: *corner_radius,
                shadow: None,
            });
        }
        DrawCommand::Image { source, x, y, w, h, fit } => {
            let rect = atelyx_plugin_sdk::frame_ui::Rect::new(x + dx, y + dy, *w, *h);
            // 指令集没有裁剪：图片画得进视口才画，半截露出会盖住下面的界面
            if !rect_inside(rect, visible) {
                return;
            }
            canvas.draw(DrawCommand::Image {
                source: source.clone(),
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
                fit: *fit,
            });
        }
        DrawCommand::Text {
            x,
            y,
            content,
            size,
            color,
            weight,
            family,
            max_width,
            align,
        } => {
            let width = content.chars().map(|c| if is_wide(c) { *size } else { *size * 0.55 }).sum::<f32>();
            let rect = atelyx_plugin_sdk::frame_ui::Rect::new(x + dx, y + dy, width, size * 1.2);
            if !rect_inside(rect, visible) {
                return;
            }
            canvas.draw(DrawCommand::Text {
                x: rect.x,
                y: rect.y,
                content: content.clone(),
                size: *size,
                color: *color,
                weight: *weight,
                family: *family,
                max_width: *max_width,
                align: *align,
            });
        }
        other => {
            let shifted = shift(other, dx, dy);
            if command_inside(&shifted, visible) {
                canvas.draw(shifted);
            }
        }
    }
}

fn rect_inside(rect: atelyx_plugin_sdk::frame_ui::Rect, visible: atelyx_plugin_sdk::frame_ui::Rect) -> bool {
    rect.x >= visible.x
        && rect.y >= visible.y
        && rect.right() <= visible.right()
        && rect.bottom() <= visible.bottom()
}

fn shift(command: &DrawCommand, dx: f32, dy: f32) -> DrawCommand {
    let mut shifted = command.clone();
    match &mut shifted {
        DrawCommand::Line { x1, y1, x2, y2, .. } => {
            *x1 += dx;
            *y1 += dy;
            *x2 += dx;
            *y2 += dy;
        }
        DrawCommand::Rect { x, y, .. } => {
            *x += dx;
            *y += dy;
        }
        DrawCommand::Path { points, .. } => {
            for point in points {
                point.x += dx;
                point.y += dy;
            }
        }
        DrawCommand::Circle { cx, cy, .. } => {
            *cx += dx;
            *cy += dy;
        }
        DrawCommand::Text { x, y, .. } => {
            *x += dx;
            *y += dy;
        }
        DrawCommand::Image { x, y, .. } => {
            *x += dx;
            *y += dy;
        }
    }
    shifted
}

fn command_inside(command: &DrawCommand, visible: atelyx_plugin_sdk::frame_ui::Rect) -> bool {
    match command {
        DrawCommand::Line { x1, y1, x2, y2, .. } => {
            visible.contains(*x1, *y1) && visible.contains(*x2, *y2)
        }
        DrawCommand::Rect { x, y, w, h, .. } | DrawCommand::Image { x, y, w, h, .. } => {
            rect_inside(atelyx_plugin_sdk::frame_ui::Rect::new(*x, *y, *w, *h), visible)
        }
        DrawCommand::Circle { cx, cy, r, .. } => {
            visible.contains(cx - r, cy - r) && visible.contains(cx + r, cy + r)
        }
        DrawCommand::Path { points, .. } => points.iter().all(|point| visible.contains(point.x, point.y)),
        DrawCommand::Text { x, y, size, .. } => {
            rect_inside(atelyx_plugin_sdk::frame_ui::Rect::new(*x, *y, *size, *size * 1.2), visible)
        }
    }
}

/// 领域键的 128 位散列：同一键恒定同一 id，不同键不碰撞（千级条目量下概率可忽略）。
/// 命中 id 长度受宿主 64 字节上限约束，promptId 与结果 key 一律散列后再拼前缀。
pub(crate) fn digest(key: &str) -> String {
    let (hi, lo) = hash128(key.as_bytes());
    format!("{hi:016x}{lo:016x}")
}

/// 128 位 FNV-1a：两轮不同偏移量并行推进，输出拼接成 32 位十六进制。
fn hash128(bytes: &[u8]) -> (u64, u64) {
    let mut hi: u64 = 0xcbf2_9ce4_8422_2325;
    let mut lo: u64 = 0x9e37_79b9_7f4a_7c15;
    for byte in bytes {
        hi = (hi ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        lo = (lo ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hi, lo)
}

// ---------- 绘制指令小工具 ----------

fn rgba(value: [u8; 4]) -> PaintColor {
    PaintColor::Rgba { rgba: value }
}

fn token(value: ColorToken) -> PaintColor {
    PaintColor::Token { name: value }
}

fn rect(x: f32, y: f32, w: f32, h: f32, fill: Option<PaintColor>, stroke: Option<PaintColor>) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, fill, stroke, stroke_width: None, corner_radius: None, shadow: None }
}

fn rounded(x: f32, y: f32, w: f32, h: f32, fill: Option<PaintColor>, stroke: Option<PaintColor>, radius: f32) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, fill, stroke, stroke_width: Some(1.0), corner_radius: Some(radius), shadow: None }
}

fn card(width: f32, top: f32, height: f32) -> DrawCommand {
    rounded(0.0, top, width.max(1.0), height, Some(token(ColorToken::Surface)), Some(token(ColorToken::Border)), 8.0)
}

fn text(x: f32, y: f32, content: &str, size: f32, color: PaintColor, weight: Option<TextWeight>) -> DrawCommand {
    DrawCommand::Text {
        x,
        y,
        content: content.to_string(),
        size,
        color,
        weight,
        family: None,
        max_width: None,
        align: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelyx_plugin_protocol::paint::validate_commands;
    use crate::comfy::types::ImageRef;

    fn result(prompt_id: &str, key: &str, created: f64, local: Option<&str>) -> ResultImage {
        ResultImage {
            key: key.to_string(),
            prompt_id: prompt_id.to_string(),
            image_ref: ImageRef { filename: format!("{key}.png"), subfolder: String::new(), type_: "output".into() },
            title: "工作流".into(),
            prompt_text: "猫".into(),
            created_at: created,
            failed: false,
            saved_path: None,
            local: local.map(str::to_string),
            pixel: None,
            copy_attempts: 0,
        }
    }

    fn cell(key: &str, source: Option<&str>, failed: bool) -> FlowCell {
        FlowCell {
            key: key.into(),
            name: format!("{key}.png"),
            source: source.map(str::to_string),
            failed,
            saved: false,
            aspect: None,
        }
    }

    #[test]
    fn columns_follow_canvas_width() {
        assert_eq!(columns_for(360.0), 2);
        assert_eq!(columns_for(520.0), 3);
        assert_eq!(columns_for(900.0), 5, "按 150px 基线自动补列，不设上限");
        assert_eq!(columns_for(200.0), 1, "窄面板只留一列");
        assert_eq!(columns_for(0.0), 1, "宽度未知时按一列兜底");
    }

    #[test]
    fn cell_size_clamps_aspect_and_max_height() {
        let (w, h) = cell_size(360.0, 2, 1.0);
        assert!((w - 166.0).abs() < 0.01, "两列均分内容区：{w}");
        assert!((h - w).abs() < 0.01);
        // 极端长条按比例抬高但不越最大高
        let (_, tall) = cell_size(360.0, 2, 0.1);
        assert!(tall <= CELL_MAX_H + 0.01);
    }

    #[test]
    fn cells_fill_content_box_without_overflow() {
        for width in [200.0, 360.0, 520.0, 900.0, 1200.0] {
            let columns = columns_for(width);
            let (cell_w, _) = cell_size(width, columns, 1.0);
            let right = PAD + (columns as f32 - 1.0) * (cell_w + GAP) + cell_w;
            assert!(right <= width - PAD + 0.01, "宽 {width} 末格右沿 {right} 越出内容区");
        }
    }

    #[test]
    fn clip_text_counts_wide_chars_as_full_width() {
        assert_eq!(clip_text("一二三四五", 25.0, 10.0), "一…");
        assert_eq!(clip_text("abc", 30.0, 10.0), "abc");
        assert_eq!(clip_text("abc", 8.0, 10.0), "…");
        assert_eq!(clip_text("猫", 0.0, 10.0), "");
    }

    #[test]
    fn records_group_by_prompt_and_run_old_to_new() {
        let mut wide = result("p2", "b", 2000.0, Some("history/b.png"));
        wide.pixel = Some((1024, 512));
        let items = vec![wide, result("p1", "a", 1000.0, None), result("p1", "a2", 1000.0, Some("history/a2.png"))];
        let records = records_of(&items);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].prompt_id, "p1");
        assert_eq!(records[0].cells.len(), 2);
        assert!(!records[0].failed);
        assert_eq!(records[0].cells[0].source, None, "缺副本的格子按占位处理");
        assert_eq!(records[0].cells[0].name, "a.png");
        assert_eq!(records[1].prompt_id, "p2");
        assert_eq!(records[1].cells[0].aspect, Some(2.0), "逐格保留真实比例");
    }

    /// 当天只给时:分，跨天补日期；无效时刻为空串。
    #[test]
    fn time_label_carries_date_only_across_days() {
        use chrono::TimeZone;
        let now = chrono::Local::now().timestamp_millis() as f64;
        assert_eq!(format_time(now).chars().count(), 5, "当天只给时:分");
        let past = chrono::Local
            .with_ymd_and_hms(2000, 1, 2, 8, 5, 0)
            .single()
            .expect("固定时刻无歧义")
            .timestamp_millis() as f64;
        let label = format_time(past);
        assert!(label.contains("01-02"), "跨天补日期：{label}");
        assert_eq!(format_time(0.0), "");
        assert_eq!(format_time(f64::NAN), "");
    }

    fn record_of(cells: Vec<FlowCell>) -> FlowInput {
        FlowInput {
            width: 360.0,
            records: vec![FlowRecord {
                prompt_id: "p1".into(),
                title: "两张".into(),
                time: String::new(),
                failed: false,
                can_copy: true,
                cells,
            }],
            running: None,
        }
    }

    /// 横向格子比方格矮：卡片总高随图片真实比例变化。
    #[test]
    fn cell_aspect_drives_card_height() {
        let square = layout(&record_of(vec![FlowCell {
            key: "k1".into(),
            name: "k1.png".into(),
            source: None,
            failed: false,
            saved: false,
            aspect: Some(1.0),
        }]));
        let wide = layout(&record_of(vec![FlowCell {
            key: "k2".into(),
            name: "k2.png".into(),
            source: None,
            failed: false,
            saved: false,
            aspect: Some(2.0),
        }]));
        let (_, cell_h) = cell_size(360.0, 2, 1.0);
        assert!((square.height - wide.height - cell_h / 2.0).abs() < 0.01, "横图卡应矮半格：{} vs {}", square.height, wide.height);
    }

    #[test]
    fn placeholder_states_split_by_batch_progress() {
        assert_eq!(cell_state(0, 2, None), CellState::Queued);
        assert_eq!(cell_state(0, 2, Some(10.0)), CellState::Active);
        assert_eq!(cell_state(1, 2, Some(60.0)), CellState::Active, "60% 落在第二格");
        assert_eq!(cell_state(0, 2, Some(60.0)), CellState::Done);
        assert_eq!(cell_state(1, 2, Some(10.0)), CellState::Waiting);
        assert_eq!(cell_state(1, 2, Some(140.0)), CellState::Done, "越界的进度按完成收");
    }

    fn sample_input() -> FlowInput {
        FlowInput {
            width: 360.0,
            records: vec![FlowRecord {
                prompt_id: "p1".into(),
                title: "一只猫".into(),
                time: "12:00".into(),
                failed: true,
                can_copy: true,
                cells: vec![cell("k1", Some("history/k1.png"), false), cell("k2", None, true)],
            }],
            running: Some(FlowRunning {
                title: "生成中".into(),
                cells: 2,
                aspect: 1.0,
                percent: Some(40.0),
                steps: Some("8/25".into()),
                status: "运行 1 · 等待 0".into(),
                can_copy: false,
            }),
        }
    }

    #[test]
    fn layout_hits_carry_their_actions() {
        let flow = layout(&sample_input());
        assert!(flow.height > 200.0, "内容总高随记录堆出：{}", flow.height);
        let commands: Vec<DrawCommand> = flow.ops.iter().map(|op| op.command.clone()).collect();
        assert!(validate_commands(&commands).is_ok(), "绘制清单须过宿主校验");

        let preview = flow
            .hits
            .iter()
            .find(|hit| matches!(hit.action, FlowAction::Preview(_)))
            .expect("格子可点开灯箱");
        assert_eq!(flow.action_of(&preview.id), Some(&FlowAction::Preview("k1".into())));
        let save = flow
            .hits
            .iter()
            .find(|hit| matches!(hit.action, FlowAction::Save(_)))
            .expect("悬停格浮出保存动作");
        assert_eq!(flow.action_of(&save.id), Some(&FlowAction::Save("k1".into())));
        assert!(save.y > preview.y, "保存章盖在格子下沿");
        assert!(flow.hits.iter().any(|hit| hit.action == FlowAction::Interrupt));
        assert!(flow.hits.iter().any(|hit| hit.action == FlowAction::CopyPrompt("p1".into())), "卡片头可复制提示词");
        assert!(!flow.hits.iter().any(|hit| hit.action == FlowAction::CopyRun), "运行块没有原文不给复制热区");
        assert_eq!(flow.action_of("pvw:none"), None);
        // 命中 id 受宿主长度约束
        for hit in &flow.hits {
            assert!(hit.id.len() <= 64, "id 超出宿主上限：{}", hit.id);
        }
    }

    #[test]
    fn record_without_prompt_skips_copy_chip() {
        let mut input = sample_input();
        input.records[0].can_copy = false;
        let flow = layout(&input);
        assert!(!flow.hits.iter().any(|hit| matches!(hit.action, FlowAction::CopyPrompt(_))), "没有原文不给复制热区");
        assert!(!flow.ops.iter().any(|op| matches!(&op.command, DrawCommand::Text { content, .. } if content == "复制")), "没有原文不画复制小章");
        assert!(flow.hits.iter().any(|hit| matches!(hit.action, FlowAction::Preview(_))), "图格命中不受影响");
    }

    #[test]
    fn saved_cell_has_badge_and_no_save_hit() {
        let mut input = sample_input();
        input.records[0].cells[0].saved = true;
        let flow = layout(&input);
        assert!(!flow.hits.iter().any(|hit| hit.action == FlowAction::Save("k1".into())), "已落库的格不再给保存热区");
        assert!(
            flow.ops.iter().any(|op| matches!(&op.command, DrawCommand::Text { content, .. } if content == "已保存 ✓")),
            "已落库的格带角标"
        );
    }

    #[test]
    fn empty_flow_has_no_hits_and_no_height() {
        let flow = layout(&FlowInput::default());
        assert!(flow.hits.is_empty());
        assert_eq!(flow.height, 0.0, "空流不占画布高度，界面改给文字说明");
    }

    /// 进行中的占位格内画进度条（轨道 + 填充），其他态只有空占位。
    #[test]
    fn active_running_cell_carries_progress_bar() {
        let flow = layout(&sample_input());
        let bars = flow
            .ops
            .iter()
            .filter(|op| matches!(&op.command, DrawCommand::Rect { h, .. } if *h == PROG_H))
            .count();
        assert_eq!(bars, 2, "进行中格有轨道与填充，其余格没有进度条");
        assert!(
            flow.ops.iter().any(|op| matches!(&op.command, DrawCommand::Text { content, .. } if content == "8/25")),
            "步数文案优先于百分比"
        );
        assert!(
            !flow.ops.iter().any(|op| matches!(&op.command, DrawCommand::Text { content, .. } if content == "排队中")),
            "有进度就不是排队态"
        );
    }
}
