//! 结果记录流的画布布局：按批次成卡，一次算出绘制指令与命中区域。
//!
//! 卡片头是标题、出错标记、时间与复制提示词；卡内图格按各自图片的真实宽高比排布，
//! 格面 cover 裁切。鼠标停在图格上时底部浮出文件名与保存动作，已落库的图格带「已保存」
//! 角标。运行中的批次排在最后，以四态占位格呈现排队 / 进行中 / 已出 / 待出，
//! 进度条与步数画在进行中的格里。
//!
//! 绘制与命中同源于一次 [`layout`]：坐标只在结构里算一遍，指令与热区因此不会漂移。
//! 命中返回的 id 就是动作契约（`HIT_*` 一组常量），与界面控件的动作语义一一对应，
//! 由 app 的事件分派直接消费，画布不引入第二套动作语义。
//!
//! 增量提交：每条指令带稳定 id（领域键的 128 位散列或固定槽位名），宿主画布按 id 增量维护——
//! 布局没变就不发，变了只发差异（[`patch_plan`]）。可选视觉（副本图、角标、悬停条）
//! 从条目创建起就占住槽位（空文本占位），内容后到只做原位替换：追加会把叠层次序打乱。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use atelyx_plugin_sdk::protocol::canvas::{CanvasColor, CanvasUpsert, DrawCommand};
use atelyx_plugin_sdk::protocol::view::{ColorToken, ImageFit, ImageSource, TextWeight};

use crate::comfy::types::queue_item_prompt;
use crate::history::ResultImage;
use crate::workflow::form::{prompt_text_of, FieldSpec};

/// 图格最小宽度：列数按内容区宽度在此基线上折算。
const CELL_MIN: f32 = 150.0;
/// 画布命中区 id：整格开灯箱、悬停保存、复制提示词、显示更早、中断与运行中复制。
pub const HIT_PREVIEW: &str = "pvw:";
pub const HIT_SAVE: &str = "save:";
pub const HIT_COPY: &str = "copy:";
pub const HIT_COPY_RUN: &str = "copy_run";
pub const HIT_MORE: &str = "more";
pub const HIT_INTERRUPT: &str = "interrupt";
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
/// 卡片头右侧时间与状态文案的占宽。
const TIME_W: f32 = 34.0;
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

/// 布局输入：画布宽度、按旧→新排好的记录、末尾的运行中批次、悬停图格与可选的「显示更早」章。
#[derive(Clone, Debug, Default)]
pub struct FlowInput {
    pub width: f32,
    pub records: Vec<FlowRecord>,
    pub running: Option<FlowRunning>,
    pub earlier_label: Option<String>,
    /// 当前悬停的图格 key：只有它显示底部文件名与保存动作。
    pub hover: Option<String>,
}

/// 一个命中区域：矩形 + 事件 id（与界面控件同一套 id 契约）。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowHit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub id: String,
}

/// 一条绘制指令及其稳定 id：宿主画布按 id 定位（命中已有原位替换、未命中追加），
/// id 在同一画布内唯一（领域键 128 位散列，见 [`slot_prefix`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowDraw {
    pub id: String,
    pub command: DrawCommand,
}

/// 布局结果：内容总高、绘制指令、命中区域。
#[derive(Clone, Debug, Default)]
pub struct FlowLayout {
    pub height: f32,
    pub draws: Vec<FlowDraw>,
    pub hits: Vec<FlowHit>,
}

impl FlowLayout {
    /// 追加一条带 id 的指令（布局内部用）。
    fn push(&mut self, id: String, command: DrawCommand) {
        self.draws.push(FlowDraw { id, command });
    }
}

/// 画布可用宽度未知时的兜底宽度（首帧按它排布，收到 `canvas.resized` 后按真实宽度重排）。
pub const DEFAULT_WIDTH: f32 = 360.0;

impl FlowInput {
    /// 排布用的有效宽度：宿主还没报尺寸时按兜底值。
    fn effective_width(&self) -> f32 {
        if self.width > 80.0 { self.width } else { DEFAULT_WIDTH }
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

/// 毫秒时间戳 → `时:分`（本地时区）；无效值返回空串。
pub fn format_time(millis: f64) -> String {
    if !millis.is_finite() || millis <= 0.0 {
        return String::new();
    }
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(millis as i64).single() {
        Some(moment) => moment.format("%H:%M").to_string(),
        None => String::new(),
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
    let width = input.effective_width();
    let columns = columns_for(width);
    let mut out = FlowLayout::default();

    if let Some(label) = input.earlier_label.as_deref() {
        push_more(&mut out, width, label);
    }
    for record in &input.records {
        push_record(&mut out, width, columns, record, input.hover.as_deref());
    }
    if let Some(running) = &input.running {
        push_running(&mut out, width, columns, running);
    }
    out.height = (out.height - CARD_GAP).max(0.0);
    out
}

/// 「显示更早」：居中的小章，点一次多展开一批。
fn push_more(out: &mut FlowLayout, width: f32, label: &str) {
    let y = out.height;
    let w = (text_px(label, 11.0) + 16.0).min(width).max(1.0);
    let x = (width - w) / 2.0;
    out.push("more:bg".to_string(), rect(x, y, w, 24.0, Some(token(ColorToken::SurfaceMuted)), Some(token(ColorToken::Border))));
    out.push("more:text".to_string(), text(x + 8.0, y + 5.0, label, 11.0, token(ColorToken::TextMuted), None));
    out.hits.push(FlowHit { x, y, w, h: 24.0, id: HIT_MORE.to_string() });
    out.height = y + 24.0 + CARD_GAP;
}

/// 一条已完成记录：卡片头（标题/出错/时间/复制）+ 按真实比例排布的图墙。
fn push_record(out: &mut FlowLayout, width: f32, columns: usize, record: &FlowRecord, hover: Option<&str>) {
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
    let slot = slot_prefix("r", &record.prompt_id);
    out.push(format!("{slot}:card"), card(width, top, card_h));

    push_record_head(out, width, top, record, &slot);
    let mut y = top + HEADER_H + PAD;
    for (row_index, chunk) in record.cells.chunks(columns).enumerate() {
        for (col, cell) in chunk.iter().enumerate() {
            let x = PAD + col as f32 * (cell_w + GAP);
            let h = cell_size(width, columns, cell.aspect.unwrap_or(1.0)).1;
            push_cell(out, cell, x, y, cell_w, h, hover == Some(cell.key.as_str()));
        }
        y += rows[row_index] + GAP;
    }
    out.height = top + card_h + CARD_GAP;
}

fn push_record_head(out: &mut FlowLayout, width: f32, top: f32, record: &FlowRecord, slot: &str) {
    let y = top + (HEADER_H - 12.0) / 2.0;
    let right = width - PAD;
    let copy_x = if record.can_copy { right - COPY_W } else { right };
    let time_x = copy_x - HEAD_GAP - TIME_W;
    let mut limit = time_x - HEAD_GAP;
    if record.failed {
        limit -= FAILED_W + HEAD_GAP;
    }
    let title_x = PAD;
    out.push(format!("{slot}:title"), text(title_x, y, &clip_text(&record.title, limit - title_x, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    if record.failed {
        out.push(format!("{slot}:failed"), text(limit + HEAD_GAP, y + 1.0, "出错", 10.0, token(ColorToken::Danger), None));
    }
    out.push(format!("{slot}:time"), text(time_x, y + 1.0, &record.time, 10.0, token(ColorToken::TextMuted), None));
    if !record.can_copy {
        return;
    }
    let chip_y = top + (HEADER_H - CHIP_H) / 2.0;
    out.push(format!("{slot}:copybg"), rect(copy_x, chip_y, COPY_W, CHIP_H, Some(token(ColorToken::SurfaceMuted)), Some(token(ColorToken::Border))));
    out.push(format!("{slot}:copy"), text(copy_x + 6.0, chip_y + 2.0, "复制", 10.0, token(ColorToken::TextMuted), None));
    out.hits.push(FlowHit { x: copy_x, y: chip_y, w: COPY_W, h: CHIP_H, id: format!("{HIT_COPY}{}", record.prompt_id) });
}

/// 一个图格：副本就绪就贴 cover 图，否则留底色与说明，出错描红边；悬停浮出文件名与保存。
fn push_cell(out: &mut FlowLayout, cell: &FlowCell, x: f32, y: f32, w: f32, h: f32, hovered: bool) {
    let slot = slot_prefix("c", &cell.key);
    let edge = if cell.failed { token(ColorToken::Danger) } else { token(ColorToken::Border) };
    out.push(format!("{slot}:bg"), rounded(x, y, w, h, Some(token(ColorToken::SurfaceSunken)), Some(edge), 6.0));
    match &cell.source {
        Some(path) => out.push(
            format!("{slot}:img"),
            DrawCommand::Image {
                source: ImageSource::Private { path: path.clone() },
                x,
                y,
                w,
                h,
                fit: Some(ImageFit::Cover),
            },
        ),
        None => out.push(format!("{slot}:img"), blank(x, y)),
    }
    let hint = match (&cell.source, cell.failed) {
        (Some(_), _) => "",
        (None, true) => "出错",
        (None, false) => "副本未就绪",
    };
    let hint_color = if cell.failed { token(ColorToken::Danger) } else { token(ColorToken::TextMuted) };
    out.push(
        format!("{slot}:hint"),
        if hint.is_empty() {
            blank(x, y)
        } else {
            text(x + (w - text_px(hint, 11.0)) / 2.0, y + (h - 11.0) / 2.0, hint, 11.0, hint_color, None)
        },
    );
    // 已落库角标：从建立起占槽，落库后原位替换
    let badge = if cell.saved { "已保存 ✓" } else { "" };
    let badge_w = if badge.is_empty() { 0.0 } else { text_px(badge, 9.0) + 10.0 };
    out.push(format!("{slot}:badgebg"), if badge.is_empty() { blank(x, y) } else { rounded(x + w - badge_w - 4.0, y + 4.0, badge_w, 16.0, Some(rgba(BADGE_BG)), None, 4.0) });
    out.push(
        format!("{slot}:badge"),
        if badge.is_empty() {
            blank(x, y)
        } else {
            text(x + w - badge_w + 1.0, y + 7.0, badge, 9.0, rgba(BADGE_TEXT), None)
        },
    );
    out.hits.push(FlowHit { x, y, w, h, id: format!("{HIT_PREVIEW}{}", cell.key) });

    let bar_y = y + h - HOVER_H;
    out.push(format!("{slot}:bar"), if hovered { rect(x, bar_y, w, HOVER_H, Some(rgba(BAR_BG)), None) } else { blank(x, bar_y) });
    let name_w = if hovered && !cell.saved { w - 12.0 - SAVE_W - 6.0 } else { w - 12.0 };
    out.push(
        format!("{slot}:barfile"),
        if hovered {
            text(x + 6.0, bar_y + 6.0, &clip_text(&cell.name, name_w, 10.0), 10.0, rgba(BAR_TEXT), None)
        } else {
            blank(x, bar_y)
        },
    );
    let save_x = x + w - 6.0 - SAVE_W;
    let save_y = bar_y + (HOVER_H - 16.0) / 2.0;
    out.push(format!("{slot}:savebg"), if hovered && !cell.saved { rounded(save_x, save_y, SAVE_W, 16.0, Some(rgba(LIGHT_BG)), None, 4.0) } else { blank(x, bar_y) });
    out.push(
        format!("{slot}:savelbl"),
        if hovered && !cell.saved {
            text(save_x + 5.0, save_y + 2.0, "保存", 10.0, rgba(LIGHT_TEXT), None)
        } else {
            blank(x, bar_y)
        },
    );
    if hovered && !cell.saved {
        out.hits.push(FlowHit { x: save_x, y: save_y, w: SAVE_W, h: 16.0, id: format!("{HIT_SAVE}{}", cell.key) });
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
    out.push("run:card".to_string(), card(width, top, card_h));

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
    out.push("run:title".to_string(), text(PAD, y, &clip_text(&running.title, title_limit, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    out.push("run:status".to_string(), text(status_x, y + 1.0, &status, 10.0, token(ColorToken::TextMuted), None));
    let chip_y = top + (HEADER_H - CHIP_H) / 2.0;
    if running.can_copy {
        out.push("run:copybg".to_string(), rect(copy_x, chip_y, COPY_W, CHIP_H, Some(token(ColorToken::SurfaceMuted)), Some(token(ColorToken::Border))));
        out.push("run:copy".to_string(), text(copy_x + 6.0, chip_y + 2.0, "复制", 10.0, token(ColorToken::TextMuted), None));
        out.hits.push(FlowHit { x: copy_x, y: chip_y, w: COPY_W, h: CHIP_H, id: HIT_COPY_RUN.to_string() });
    }
    out.push("run:stopbg".to_string(), rect(int_x, chip_y, INTERRUPT_W, CHIP_H, None, Some(token(ColorToken::Danger))));
    out.push("run:stop".to_string(), text(int_x + 7.0, chip_y + 2.0, "中断", 10.0, token(ColorToken::Danger), None));
    out.hits.push(FlowHit { x: int_x, y: chip_y, w: INTERRUPT_W, h: CHIP_H, id: HIT_INTERRUPT.to_string() });

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
        out.push(format!("run{index}:bg"), rounded(x, y, cell_w, cell_h, fill, edge, 6.0));
        let (track, bar) = match (state, pct) {
            (CellState::Active, Some(pct)) => (
                rect(x, y + cell_h - PROG_H, cell_w, PROG_H, Some(rgba(BADGE_BG)), None),
                rect(x, y + cell_h - PROG_H, (cell_w * pct as f32).max(2.0), PROG_H, Some(token(ColorToken::Accent)), None),
            ),
            _ => (blank(x, y + cell_h - PROG_H), blank(x, y + cell_h - PROG_H)),
        };
        out.push(format!("run{index}:trk"), track);
        out.push(format!("run{index}:fill"), bar);
        let label = match state {
            CellState::Queued => "排队中".to_string(),
            CellState::Active => running
                .steps
                .clone()
                .unwrap_or_else(|| format!("{:.0}%", running.percent.unwrap_or(0.0).clamp(0.0, 100.0))),
            CellState::Done => "已出".to_string(),
            CellState::Waiting => String::new(),
        };
        let size = if cell_h > 120.0 { 18.0 } else if cell_h > 80.0 { 14.0 } else { 11.0 };
        out.push(
            format!("run{index}:lbl"),
            if label.is_empty() {
                blank(x, y)
            } else {
                text(x + (cell_w - text_px(&label, size)) / 2.0, y + (cell_h - size) / 2.0, &label, size, token(ColorToken::TextMuted), None)
            },
        );
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

/// 增量提交计划：与宿主当前清单（基线）比对，新增/变化的条目走 upsert、消失的 id 走 remove。
/// 布局期 id 全画布唯一（领域键 128 位散列），故一次请求内不会重复命中同一 id。
pub fn patch_plan(base: &[FlowDraw], next: &[FlowDraw]) -> (Vec<CanvasUpsert>, Vec<String>) {
    let index: HashMap<&str, &DrawCommand> = base.iter().map(|draw| (draw.id.as_str(), &draw.command)).collect();
    let mut upsert = Vec::new();
    for draw in next {
        if index.get(draw.id.as_str()) != Some(&&draw.command) {
            upsert.push(CanvasUpsert { id: draw.id.clone(), command: draw.command.clone() });
        }
    }
    let live: HashSet<&str> = next.iter().map(|draw| draw.id.as_str()).collect();
    let remove: Vec<String> =
        base.iter().filter(|draw| !live.contains(draw.id.as_str())).map(|draw| draw.id.clone()).collect();
    (upsert, remove)
}

/// 条目 id 前缀：领域键（promptId / 记录 key）的 128 位散列。同一键恒定同一 id，
/// 不同键不碰撞（千级条目量下概率可忽略）；长度受宿主 64 字节上限约束。
fn slot_prefix(prefix: &str, key: &str) -> String {
    let (hi, lo) = hash128(key.as_bytes());
    format!("{prefix}{hi:016x}{lo:016x}")
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

/// 命中测试：指针的节点本地坐标落在哪个热区（后画优先，与遮挡顺序一致）。
pub fn hit_test(layout: &FlowLayout, x: f32, y: f32) -> Option<&str> {
    layout
        .hits
        .iter()
        .rev()
        .find(|hit| x >= hit.x && x <= hit.x + hit.w && y >= hit.y && y <= hit.y + hit.h)
        .map(|hit| hit.id.as_str())
}

/// 命中 id → 图格 key：「整格预览」与「悬停保存」都落在同一张图上。
pub fn cell_key(id: &str) -> Option<&str> {
    id.strip_prefix(HIT_PREVIEW).or_else(|| id.strip_prefix(HIT_SAVE))
}

// ---------- 绘制指令小工具 ----------

fn rgba(value: [u8; 4]) -> CanvasColor {
    CanvasColor::Rgba { rgba: value }
}

fn token(name: ColorToken) -> CanvasColor {
    CanvasColor::Token { name }
}

fn rect(x: f32, y: f32, w: f32, h: f32, fill: Option<CanvasColor>, stroke: Option<CanvasColor>) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, fill, stroke, stroke_width: None, corner_radius: None }
}

fn rounded(x: f32, y: f32, w: f32, h: f32, fill: Option<CanvasColor>, stroke: Option<CanvasColor>, radius: f32) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, fill, stroke, stroke_width: Some(1.0), corner_radius: Some(radius) }
}

fn card(width: f32, top: f32, height: f32) -> DrawCommand {
    rounded(0.0, top, width.max(1.0), height, Some(token(ColorToken::Surface)), Some(token(ColorToken::Border)), 8.0)
}

fn text(x: f32, y: f32, content: &str, size: f32, color: CanvasColor, weight: Option<TextWeight>) -> DrawCommand {
    DrawCommand::Text { x, y, content: content.to_string(), size, color, weight }
}

/// 空占位指令：槽位先建、内容后到时保持叠层次序不变（追加会把后到的内容盖到既有图层上）。
/// 空文本不画任何内容，仅通过宿主指令校验（字号为正）。
fn blank(x: f32, y: f32) -> DrawCommand {
    text(x, y, "", 10.0, rgba(BAR_TEXT), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelyx_plugin_sdk::protocol::canvas::validate_commands;
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

    #[test]
    fn cell_aspect_drives_cell_height() {
        let input = FlowInput {
            width: 360.0,
            records: vec![FlowRecord {
                prompt_id: "p1".into(),
                title: "两张".into(),
                time: String::new(),
                failed: false,
                can_copy: true,
                cells: vec![
                    FlowCell { key: "k1".into(), name: "k1.png".into(), source: None, failed: false, saved: false, aspect: Some(1.0) },
                    FlowCell { key: "k2".into(), name: "k2.png".into(), source: None, failed: false, saved: false, aspect: Some(2.0) },
                ],
            }],
            ..FlowInput::default()
        };
        let flow = layout(&input);
        let height_of = |key: &str| {
            let slot = format!("{}:bg", slot_prefix("c", key));
            match flow.draws.iter().find(|d| d.id == slot).expect("格存在").command {
                DrawCommand::Rect { h, .. } => h,
                _ => panic!("格底应是矩形"),
            }
        };
        let square = height_of("k1");
        let wide = height_of("k2");
        assert!((wide - square / 2.0).abs() < 0.01, "横图格高应为方格一半：{square} vs {wide}");
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
            earlier_label: Some("显示更早".into()),
            hover: Some("k1".into()),
        }
    }

    #[test]
    fn layout_hits_are_drawn_on_top_of_their_cell() {
        let flow = layout(&sample_input());
        assert!(flow.height > 200.0, "内容总高随记录堆出：{}", flow.height);
        let commands: Vec<DrawCommand> = flow.draws.iter().map(|draw| draw.command.clone()).collect();
        assert!(validate_commands(&commands).is_ok(), "绘制清单须过宿主校验");

        let preview = flow.hits.iter().find(|hit| hit.id == "pvw:k1").expect("格子可点开灯箱");
        assert_eq!(hit_test(&flow, preview.x + 4.0, preview.y + 4.0), Some("pvw:k1"));
        assert_eq!(hit_test(&flow, -1.0, -1.0), None);
        assert!(flow.hits.iter().any(|hit| hit.id == "more"));
        assert!(flow.hits.iter().any(|hit| hit.id == "interrupt"));
        assert!(flow.hits.iter().any(|hit| hit.id == "copy:p1"), "卡片头可复制提示词");
        assert!(flow.hits.iter().any(|hit| hit.id == "save:k1"), "悬停格浮出保存动作");
        assert!(!flow.hits.iter().any(|hit| hit.id == "save:k2"), "未悬停的格没有保存热区");
        assert_eq!(cell_key("pvw:k1"), Some("k1"));
        assert_eq!(cell_key("save:k1"), Some("k1"));
        assert_eq!(cell_key("copy:p1"), None);
    }

    #[test]
    fn record_without_prompt_skips_copy_chip() {
        let mut input = sample_input();
        input.records[0].can_copy = false;
        let flow = layout(&input);
        assert!(!flow.hits.iter().any(|hit| hit.id.starts_with(HIT_COPY)), "没有原文不给复制热区");
        let slot = format!("{}:copybg", slot_prefix("r", "p1"));
        assert!(flow.draws.iter().all(|draw| draw.id != slot), "没有原文不画复制小章");
        assert!(flow.hits.iter().any(|hit| hit.id == "pvw:k1"), "图格命中不受影响");
    }

    #[test]
    fn saved_cell_has_badge_and_no_save_hit() {
        let mut input = sample_input();
        input.records[0].cells[0].saved = true;
        let flow = layout(&input);
        assert!(!flow.hits.iter().any(|hit| hit.id == "save:k1"), "已落库的格不再给保存热区");
        let slot = format!("{}:badge", slot_prefix("c", "k1"));
        match &flow.draws.iter().find(|draw| draw.id == slot).expect("角标槽存在").command {
            DrawCommand::Text { content, .. } => assert_eq!(content, "已保存 ✓"),
            _ => panic!("角标应是文本"),
        }
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
        let command_of = |id: &str| flow.draws.iter().find(|d| d.id == id).expect("槽存在").command.clone();
        assert!(matches!(command_of("run0:trk"), DrawCommand::Rect { .. }), "进行中格有轨道");
        assert!(matches!(command_of("run0:fill"), DrawCommand::Rect { .. }), "进行中格有填充条");
        let label = match command_of("run0:lbl") {
            DrawCommand::Text { content, .. } => content,
            _ => panic!("标签应是文本"),
        };
        assert_eq!(label, "8/25", "步数文案优先于百分比");
        for id in ["run1:trk", "run1:fill"] {
            assert!(matches!(command_of(id), DrawCommand::Text { .. }), "非进行中格 {id} 是空占位");
        }
    }

    // ---------- 增量提交 ----------

    /// 两次同一输入的布局：id 序列完全一致（增量基线稳定的前提）。
    #[test]
    fn ids_are_stable_and_unique_per_layout() {
        let first = layout(&sample_input());
        let second = layout(&sample_input());
        assert_eq!(
            first.draws.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            second.draws.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            "同一布局的 id 序列必须恒定"
        );
        let mut ids: HashSet<&str> = HashSet::new();
        for draw in &first.draws {
            assert!(draw.id.len() <= 64, "id 超出宿主上限：{}", draw.id);
            assert!(ids.insert(draw.id.as_str()), "id 重复：{}", draw.id);
        }
        assert!(first.draws.len() > 10, "样例布局应含章、卡片、图格与运行块");
    }

    /// 副本落定只换图片槽内容：id 不变、清单位置不变（原位替换）。
    #[test]
    fn late_image_replaces_its_slot_in_place() {
        let mut input = sample_input();
        input.records[0].cells[1].source = None;
        let before = layout(&input);
        input.records[0].cells[1].source = Some("history/k2.png".into());
        let after = layout(&input);

        let slot = format!("{}:img", slot_prefix("c", "k2"));
        let before_draw = before.draws.iter().find(|d| d.id == slot).expect("缺副本也占图片槽");
        let after_draw = after.draws.iter().find(|d| d.id == slot).expect("副本就绪仍用同一槽");
        assert!(matches!(before_draw.command, DrawCommand::Text { .. }), "占位是空文本");
        assert!(matches!(after_draw.command, DrawCommand::Image { .. }), "副本就绪换成图片");
        let position = |flow: &FlowLayout| flow.draws.iter().position(|d| d.id == slot).expect("槽存在");
        assert_eq!(position(&before), position(&after), "原位替换不改变清单位置");
    }

    #[test]
    fn patch_plan_is_empty_when_nothing_changed() {
        let flow = layout(&sample_input());
        let (upsert, remove) = patch_plan(&flow.draws, &flow.draws);
        assert!(upsert.is_empty() && remove.is_empty(), "布局没变不发任何增量");
    }

    #[test]
    fn patch_plan_upserts_changed_and_new_then_removes_gone() {
        let mut input = sample_input();
        let base = layout(&input);
        input.records[0].title = "两只猫".into();
        input.records.push(FlowRecord {
            prompt_id: "p2".into(),
            title: "第二幕".into(),
            time: "09:00".into(),
            failed: false,
            can_copy: true,
            cells: vec![cell("k9", None, false)],
        });
        let next = layout(&input);

        let (upsert, remove) = patch_plan(&base.draws, &next.draws);
        let upsert_ids: HashSet<&str> = upsert.iter().map(|item| item.id.as_str()).collect();
        let title_slot = format!("{}:title", slot_prefix("r", "p1"));
        assert!(upsert_ids.contains(title_slot.as_str()), "标题变化的那条要重发");
        assert!(upsert_ids.contains(format!("{}:card", slot_prefix("r", "p2")).as_str()), "新纪录整条要发");
        assert!(upsert.len() < next.draws.len(), "只发变化项：{} < {}", upsert.len(), next.draws.len());
        assert!(remove.is_empty(), "没有条目消失");
    }

    #[test]
    fn patch_plan_removes_evicted_entries() {
        let mut input = sample_input();
        let base = layout(&input);
        input.earlier_label = None;
        let next = layout(&input);
        let (upsert, remove) = patch_plan(&base.draws, &next.draws);
        assert!(remove.iter().any(|id| id == "more:bg") && remove.iter().any(|id| id == "more:text"), "章消失要走删除：{remove:?}");
        assert!(upsert.iter().all(|item| !item.id.starts_with("more:")), "章不再出现");
    }

    /// 从空基线建立 = 全量 upsert，且顺序与布局一致（宿主按序入列）。
    #[test]
    fn establishing_from_empty_base_sends_everything_in_order() {
        let flow = layout(&sample_input());
        let (upsert, remove) = patch_plan(&[], &flow.draws);
        assert!(remove.is_empty());
        assert_eq!(upsert.len(), flow.draws.len());
        for (item, draw) in upsert.iter().zip(&flow.draws) {
            assert_eq!(item.id, draw.id);
            assert_eq!(item.command, draw.command);
        }
    }
}
