//! 结果记录流的画布布局：把「旧→新」的记录分组排成一张纵向图，一次算出绘制指令与命中区域。
//!
//! 绘制与命中同源于一次 [`layout`]：坐标只在结构里算一遍，指令与热区因此不会漂移。
//! 单元格按列平铺（列数随画布宽度定），图面 cover 裁切，底部一条文件名与动作区；
//! 运行中的批次排在最末（贴底即最新），以静态占位格呈现，格数与比例取队列里该任务的真实规格。
//! 命中返回的 id 就是动作契约（`HIT_*` 一组常量），与界面控件的动作语义一一对应，
//! 由 app 的事件分派直接消费，画布不引入第二套动作语义。

use serde_json::Value;

use atelyx_plugin_sdk::protocol::canvas::{CanvasColor, DrawCommand};
use atelyx_plugin_sdk::protocol::view::{ColorToken, ImageFit, ImageSource, TextWeight};

use crate::comfy::types::queue_item_prompt;
use crate::history::ResultImage;
use crate::workflow::form::{prompt_text_of, FieldSpec};
use crate::workflow::sizing;

/// 单元格最小宽度（列数按画布宽度在此基线上折算）。
const CELL_MIN: f32 = 150.0;
/// 画布命中区 id：整格开大图、入库、另存、复制提示词、显示更早、中断。
/// 这套 id 只存在于画布热区（界面树里没有同名节点），动作语义与界面控件一致，由 app 统一分派。
pub const HIT_PREVIEW: &str = "pvw:";
pub const HIT_SAVE: &str = "save:";
pub const HIT_SAVE_AS: &str = "sva:";
pub const HIT_COPY: &str = "cpy:";
pub const HIT_MORE: &str = "more";
pub const HIT_INTERRUPT: &str = "interrupt";
/// 列数上下界。
const COLS_MIN: usize = 1;
const COLS_MAX: usize = 4;
/// 单元格高宽比上下界：极端比例按边界裁切，避免单格把整条记录拉变形。
const ASPECT_MIN: f32 = 0.5;
const ASPECT_MAX: f32 = 2.0;
/// 单元格最大高度。
const CELL_MAX_H: f32 = 320.0;
/// 卡片内边距与元素间距。
const PAD: f32 = 8.0;
const GAP: f32 = 8.0;
/// 单元格宽度下限：画布被压到极窄时按它兜底，不留零宽指令。
const CELL_FLOOR: f32 = 24.0;
/// 标题行高度。
const HEADER_H: f32 = 20.0;
/// 单元格底部动作条高度。
const BAR_H: f32 = 18.0;
/// 一个动作小章的宽度。
const CHIP_W: f32 = 30.0;
const CHIP_H: f32 = 15.0;
/// 标题行右侧时间的占宽。
const TIME_W: f32 = 64.0;

/// 中性灰底/描边：画布只这两档用自带 RGBA（跟随主题的填充没有对应 token），文字与强调色走 token。
const SOFT: [u8; 4] = [128, 128, 128, 22];
const SOFT_EDGE: [u8; 4] = [128, 128, 128, 56];
const BAR_BG: [u8; 4] = [0, 0, 0, 150];
const BAR_TEXT: [u8; 4] = [255, 255, 255, 235];
const CHIP_BG: [u8; 4] = [128, 128, 128, 40];

/// 一张图：定位 key、可绘制的本地副本、文件名、是否已入库、是否出错。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowCell {
    pub key: String,
    pub source: Option<String>,
    pub name: String,
    pub saved: bool,
    pub failed: bool,
}

/// 一条已完成记录：同一批（同 promptId）的若干张图 + 标题行信息。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowRecord {
    pub prompt_id: String,
    pub title: String,
    pub time: String,
    pub prompt_text: String,
    pub cells: Vec<FlowCell>,
}

/// 运行中批次：占位格数、比例、整批采样进度（无进度即排队中）与状态文案。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowRunning {
    pub title: String,
    pub cells: usize,
    pub aspect: f32,
    pub percent: Option<f64>,
    pub status: String,
}

/// 布局输入：画布宽度、按旧→新排好的记录、末尾的运行中批次、以及可选的「显示更早」章。
#[derive(Clone, Debug, Default)]
pub struct FlowInput {
    pub width: f32,
    pub records: Vec<FlowRecord>,
    pub running: Option<FlowRunning>,
    pub earlier_label: Option<String>,
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

/// 布局结果：内容总高、绘制指令、命中区域。
#[derive(Clone, Debug, Default)]
pub struct FlowLayout {
    pub height: f32,
    pub commands: Vec<DrawCommand>,
    pub hits: Vec<FlowHit>,
}

/// 画布可用宽度未知时的兜底宽度（首帧按它排布，收到 `canvas.resized` 后按真实宽度重排）。
pub const DEFAULT_WIDTH: f32 = 360.0;

impl FlowInput {
    /// 排布用的有效宽度：宿主还没报尺寸时按兜底值。
    fn effective_width(&self) -> f32 {
        if self.width > 80.0 { self.width } else { DEFAULT_WIDTH }
    }
}

/// 卡片内容区左沿（卡片外沿 + 卡片内边距）：格与文字都以此为起点。
fn content_left() -> f32 {
    PAD * 2.0
}

/// 卡片内容区宽度：卡片外沿各留一个 `PAD`，内容区两侧再各留一个 `PAD`。
/// 列数、格宽与格位置全部以此为准，绘制与命中因此不会漂出卡片。
fn content_width(width: f32) -> f32 {
    (width - PAD * 4.0).max(CELL_FLOOR)
}

/// 列数：内容区里能塞下几列（至少一列、最多四列）。
pub fn columns_for(width: f32) -> usize {
    let by_width = (content_width(width) + GAP) / (CELL_MIN + GAP);
    (by_width.floor() as usize).clamp(COLS_MIN, COLS_MAX)
}

/// 单元格宽高：宽度按列均分内容区，高度由比例决定并受最大高约束。
pub fn cell_size(width: f32, columns: usize, aspect: f32) -> (f32, f32) {
    let aspect = aspect.clamp(ASPECT_MIN, ASPECT_MAX);
    let columns = columns.max(1);
    let inner = (content_width(width) - GAP * (columns as f32 - 1.0)).max(CELL_FLOOR);
    let w = (inner / columns as f32).max(CELL_FLOOR);
    (w, (w / aspect).min(CELL_MAX_H))
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

/// 队列任务里的输出规格（比例、张数）：与生成坞三段同一口径（`sizing` 的字段判定），
/// 占位格按该任务自己的宽高算，不依赖当前草稿。
pub fn running_spec(prompt: &Value, fields: &[FieldSpec]) -> (f32, usize) {
    let aspect = sizing::output_size(prompt, fields)
        .map(|(w, h)| (w / h) as f32)
        .unwrap_or(1.0);
    (aspect, sizing::batch_count(prompt, fields).clamp(1, 12))
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
            FlowRecord {
                prompt_id: prompt_id.to_string(),
                title: if lead.prompt_text.is_empty() {
                    if lead.title.is_empty() { "未命名任务".to_string() } else { lead.title.clone() }
                } else {
                    lead.prompt_text.clone()
                },
                time: format_time(lead.created_at),
                prompt_text: lead.prompt_text.clone(),
                cells: items
                    .iter()
                    .map(|item| FlowCell {
                        key: item.key.clone(),
                        source: item.display_source(),
                        name: item.image_ref.filename.clone(),
                        saved: item.saved_path.is_some(),
                        failed: item.failed,
                    })
                    .collect(),
            }
        })
        .collect()
}

/// 毫秒时间戳 → `月-日 时:分`（本地时区）；无效值返回空串。
pub fn format_time(millis: f64) -> String {
    if !millis.is_finite() || millis <= 0.0 {
        return String::new();
    }
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(millis as i64).single() {
        Some(moment) => moment.format("%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

/// 从队列快照拼出运行中批次：队首任务的规格与提示词 + 运行/等待计数。
/// 字段规格由调用方按队首任务的提示词备好后经 `fields_of` 交出，画布层只做纯换算。
pub fn running_block(
    running: &[Value],
    pending: &[Value],
    percent: Option<f64>,
    fields_of: impl Fn(&Value) -> Vec<FieldSpec>,
) -> Option<FlowRunning> {
    let lead = running.first().or(pending.first())?;
    let prompt = queue_item_prompt(lead).cloned().unwrap_or(Value::Null);
    let (aspect, cells) = running_spec(&prompt, &fields_of(&prompt));
    let text = prompt_text_of(&prompt);
    Some(FlowRunning {
        title: if text.is_empty() { "生成中".to_string() } else { text },
        cells,
        aspect,
        percent: percent.filter(|value| value.is_finite()),
        status: format!("运行 {} · 等待 {}", running.len(), pending.len()),
    })
}

/// 排布整条记录流：返回绘制指令、命中区域与内容总高。
pub fn layout(input: &FlowInput) -> FlowLayout {
    let width = input.effective_width();
    let columns = columns_for(width);
    let mut out = FlowLayout::default();

    if let Some(label) = input.earlier_label.as_deref() {
        push_chip(&mut out, width, label, HIT_MORE);
    }
    for record in &input.records {
        push_record(&mut out, width, columns, record);
    }
    if let Some(running) = &input.running {
        push_running(&mut out, width, columns, running);
    }
    out.height += PAD;
    out.commands.insert(0, rect(0.0, 0.0, width, out.height, Some(rgba(SOFT)), None));
    out
}

fn push_chip(out: &mut FlowLayout, width: f32, label: &str, id: &str) {
    let y = out.height + PAD;
    let w = (text_px(label, 11.0) + 12.0).min(width - PAD * 2.0).max(1.0);
    let x = (width - w) / 2.0;
    out.commands.push(rect(x, y, w, CHIP_H, Some(rgba(CHIP_BG)), Some(rgba(SOFT_EDGE))));
    out.commands.push(text(x + 6.0, y + 2.0, label, 11.0, token(ColorToken::TextMuted), None));
    out.hits.push(FlowHit { x, y, w, h: CHIP_H, id: id.to_string() });
    out.height = y + CHIP_H + GAP;
}

/// 一条已完成记录：标题行（提示词/时间/复制）+ 图格墙（每格一条底部动作条）。
fn push_record(out: &mut FlowLayout, width: f32, columns: usize, record: &FlowRecord) {
    let top = out.height + PAD;
    let rows = record.cells.len().div_ceil(columns.max(1));
    let (cell_w, cell_h) = cell_size(width, columns, 1.0);
    let body_h = rows as f32 * cell_h + (rows.saturating_sub(1)) as f32 * GAP;
    let card_h = HEADER_H + GAP + body_h + PAD * 2.0;
    out.commands.push(rounded_card(width, top, card_h));

    let (chip_x, chip_y) = (width - content_left() - CHIP_W, top + PAD + 2.0);
    let title_w = content_width(width) - CHIP_W - TIME_W - GAP;
    out.commands.push(text(content_left(), top + PAD, &clip_text(&record.title, title_w, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    if !record.time.is_empty() {
        out.commands.push(text(chip_x - GAP - TIME_W, chip_y + 1.0, &record.time, 11.0, token(ColorToken::TextMuted), None));
    }
    if !record.prompt_text.is_empty() {
        out.commands.push(rect(chip_x, chip_y, CHIP_W, CHIP_H, Some(rgba(CHIP_BG)), None));
        out.commands.push(text(chip_x + 5.0, chip_y + 2.0, "复制", 10.0, token(ColorToken::TextMuted), None));
        out.hits.push(FlowHit { x: chip_x, y: chip_y, w: CHIP_W, h: CHIP_H, id: format!("{HIT_COPY}{}", record.prompt_id) });
    }

    for (index, cell) in record.cells.iter().enumerate() {
        let x = PAD * 2.0 + (index % columns) as f32 * (cell_w + GAP);
        let y = top + PAD + HEADER_H + GAP + (index / columns) as f32 * (cell_h + GAP);
        push_cell(out, cell, x, y, cell_w, cell_h);
    }
    out.height = top + card_h + GAP;
}

/// 一个图格：副本就绪就贴 cover 图，否则留底色占位；底部一条文件名 + 入库/另存动作区。
fn push_cell(out: &mut FlowLayout, cell: &FlowCell, x: f32, y: f32, w: f32, h: f32) {
    let border = if cell.failed { Some(token(ColorToken::Danger)) } else { Some(rgba(SOFT_EDGE)) };
    out.commands.push(rect(x, y, w, h, Some(rgba(SOFT)), border));
    if let Some(path) = &cell.source {
        out.commands.push(DrawCommand::Image {
            source: ImageSource::Private { path: path.clone() },
            x,
            y,
            w,
            h,
            fit: Some(ImageFit::Cover),
        });
    }
    out.hits.push(FlowHit { x, y, w, h, id: format!("{HIT_PREVIEW}{}", cell.key) });

    let bar_y = y + h - BAR_H;
    out.commands.push(rect(x, bar_y, w, BAR_H, Some(rgba(BAR_BG)), None));
    let name_w = w - 2.0 * (CHIP_W + 4.0) - 8.0;
    out.commands.push(text(x + 4.0, bar_y + 3.0, &clip_text(&cell.name, name_w.max(0.0), 10.0), 10.0, rgba(BAR_TEXT), None));

    let mut chip_x = x + w - CHIP_W - 4.0;
    out.commands.push(rect(chip_x, bar_y + 1.5, CHIP_W, CHIP_H, Some(rgba(CHIP_BG)), None));
    out.commands.push(text(chip_x + 5.0, bar_y + 3.5, "另存", 10.0, rgba(BAR_TEXT), None));
    out.hits.push(FlowHit { x: chip_x, y: bar_y, w: CHIP_W, h: BAR_H, id: format!("{HIT_SAVE_AS}{}", cell.key) });

    chip_x -= CHIP_W + 4.0;
    if cell.saved {
        out.commands.push(text(chip_x + 2.0, bar_y + 3.5, "已存", 10.0, token(ColorToken::Success), None));
    } else {
        out.commands.push(rect(chip_x, bar_y + 1.5, CHIP_W, CHIP_H, Some(rgba(CHIP_BG)), None));
        out.commands.push(text(chip_x + 9.0, bar_y + 3.5, "存", 10.0, rgba(BAR_TEXT), None));
        out.hits.push(FlowHit { x: chip_x, y: bar_y, w: CHIP_W, h: BAR_H, id: format!("{HIT_SAVE}{}", cell.key) });
    }
}

/// 运行中批次：静态占位格（排队/已出/进行中/待出四态），进行中格写整批进度百分比。
fn push_running(out: &mut FlowLayout, width: f32, columns: usize, running: &FlowRunning) {
    let top = out.height + PAD;
    let cells = running.cells.clamp(1, 12);
    let rows = cells.div_ceil(columns.max(1));
    let (cell_w, cell_h) = cell_size(width, columns, running.aspect);
    let body_h = rows as f32 * cell_h + (rows.saturating_sub(1)) as f32 * GAP;
    let card_h = HEADER_H + GAP + body_h + PAD * 2.0;
    out.commands.push(rounded_card(width, top, card_h));
    let (chip_x, chip_y) = (width - content_left() - CHIP_W, top + PAD + 2.0);
    let title_w = content_width(width) - CHIP_W - TIME_W - GAP;
    out.commands.push(text(content_left(), top + PAD, &clip_text(&running.title, title_w, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    out.commands.push(text(chip_x - GAP - TIME_W, chip_y + 1.0, &clip_text(&running.status, TIME_W, 11.0), 11.0, token(ColorToken::TextMuted), None));
    out.commands.push(rect(chip_x, chip_y, CHIP_W, CHIP_H, Some(rgba(CHIP_BG)), None));
    out.commands.push(text(chip_x + 5.0, chip_y + 2.0, "中断", 10.0, token(ColorToken::Danger), None));
    out.hits.push(FlowHit { x: chip_x, y: chip_y, w: CHIP_W, h: CHIP_H, id: HIT_INTERRUPT.to_string() });

    for index in 0..cells {
        let x = PAD * 2.0 + (index % columns) as f32 * (cell_w + GAP);
        let y = top + PAD + HEADER_H + GAP + (index / columns) as f32 * (cell_h + GAP);
        let state = cell_state(index, cells, running.percent);
        let fill = match state {
            CellState::Done => rgba([128, 128, 128, 46]),
            _ => rgba(SOFT),
        };
        let edge = match state {
            CellState::Done | CellState::Active => Some(token(ColorToken::Accent)),
            _ => Some(rgba(SOFT_EDGE)),
        };
        out.commands.push(rect(x, y, cell_w, cell_h, Some(fill), edge));
        let label = match state {
            CellState::Queued => "排队中".to_string(),
            CellState::Active => format!("{:.0}%", running.percent.unwrap_or(0.0).clamp(0.0, 100.0)),
            CellState::Done => "已出".to_string(),
            CellState::Waiting => String::new(),
        };
        if !label.is_empty() {
            let size = if cell_h > 120.0 { 20.0 } else if cell_h > 80.0 { 15.0 } else { 12.0 };
            out.commands.push(text(x + 8.0, y + (cell_h - size) / 2.0, &label, size, token(ColorToken::TextMuted), None));
        }
    }
    out.height = top + card_h + GAP;
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

/// 命中测试：指针的节点本地坐标落在哪个热区（后画优先，与遮挡顺序一致）。
pub fn hit_test(layout: &FlowLayout, x: f32, y: f32) -> Option<&str> {
    layout
        .hits
        .iter()
        .rev()
        .find(|hit| x >= hit.x && x <= hit.x + hit.w && y >= hit.y && y <= hit.y + hit.h)
        .map(|hit| hit.id.as_str())
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

fn rounded_card(width: f32, top: f32, height: f32) -> DrawCommand {
    DrawCommand::Rect {
        x: PAD,
        y: top,
        w: (width - PAD * 2.0).max(1.0),
        h: height,
        fill: Some(rgba(SOFT)),
        stroke: Some(rgba(SOFT_EDGE)),
        stroke_width: Some(1.0),
        corner_radius: Some(8.0),
    }
}

fn text(x: f32, y: f32, content: &str, size: f32, color: CanvasColor, weight: Option<TextWeight>) -> DrawCommand {
    DrawCommand::Text { x, y, content: content.to_string(), size, color, weight }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelyx_plugin_sdk::protocol::canvas::validate_commands;
    use crate::comfy::types::ImageRef;
    use crate::workflow::form::build_fields;
    use serde_json::json;

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
            copy_attempts: 0,
        }
    }

    #[test]
    fn columns_follow_canvas_width() {
        assert_eq!(columns_for(360.0), 2);
        assert_eq!(columns_for(520.0), 3);
        assert_eq!(columns_for(900.0), 4);
        assert_eq!(columns_for(200.0), 1, "窄面板只留一列");
        assert_eq!(columns_for(0.0), 1, "宽度未知时按一列兜底");
    }

    #[test]
    fn cell_size_clamps_aspect_and_max_height() {
        let (w, h) = cell_size(360.0, 2, 1.0);
        assert!((w - 160.0).abs() < 0.01, "两列均分内容区：{w}");
        assert!((h - w).abs() < 0.01);
        // 极端长条按比例抬高但不越最大高
        let (_, tall) = cell_size(360.0, 2, 0.1);
        assert!(tall <= CELL_MAX_H + 0.01);
    }

    #[test]
    fn cells_fill_content_box_without_overflow() {
        // 末格右沿不得超过内容区（卡片外沿 + 内边距各留一个 PAD）
        for width in [200.0, 360.0, 520.0, 900.0, 1200.0] {
            let columns = columns_for(width);
            let (cell_w, _) = cell_size(width, columns, 1.0);
            let right = content_left() + (columns as f32 - 1.0) * (cell_w + GAP) + cell_w;
            assert!(right <= width - content_left() + 0.01, "宽 {width} 末格右沿 {right} 越出内容区");
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
        // 画廊最近在前：同批两张并成一条，最新批沉在底部
        let items = vec![
            result("p2", "b", 2000.0, Some("history/b.png")),
            result("p1", "a", 1000.0, None),
            result("p1", "a2", 1000.0, Some("history/a2.png")),
        ];
        let records = records_of(&items);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].prompt_id, "p1");
        assert_eq!(records[0].cells.len(), 2);
        assert_eq!(records[0].cells[0].source, None, "缺副本的格子按占位处理");
        assert_eq!(records[1].prompt_id, "p2");
    }

    #[test]
    fn running_spec_follows_panel_field_rules() {
        // 张数与宽高按 `sizing` 的字段口径取：优先空潜像节点的宽高、首个未连线 batch_size，
        // 下游 ImageBatch 的 batch_size 不参与，避免占位格数与真实产出对不上。
        let prompt = json!({
            "1": {"class_type": "EmptyLatentImage", "inputs": {"width": 768, "height": 1368, "batch_size": 2}},
            "2": {"class_type": "ImageBatch", "inputs": {"batch_size": 4}},
        });
        let (aspect, cells) = running_spec(&prompt, &build_fields(&prompt, None));
        assert!((aspect - 768.0 / 1368.0).abs() < 0.001);
        assert_eq!(cells, 2);

        let object_info = json!({"ResolutionSelector": {"input": {"required": {
            "aspect_ratio": [["1:1 (Square)", "16:9 (Wide)"], {"default": "1:1 (Square)"}],
            "megapixels": ["FLOAT", {"default": 1.0}],
            "multiple": ["INT", {"default": 8}],
        }}}});
        let selector = json!({"1": {"class_type": "ResolutionSelector", "inputs": {"aspect_ratio": "16:9 (Wide)", "megapixels": 1.0, "multiple": 8}}});
        let (aspect, cells) = running_spec(&selector, &build_fields(&selector, Some(&object_info)));
        assert!((aspect - 16.0 / 9.0).abs() < 0.01, "选择器按换算后的宽高给比例：{aspect}");
        assert_eq!(cells, 1);

        assert_eq!(running_spec(&json!([]), &[]), (1.0, 1), "认不出规格时按 1:1 单张兜底");
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
                time: "10-01 12:00".into(),
                prompt_text: "一只猫".into(),
                cells: vec![
                    FlowCell { key: "k1".into(), source: Some("history/k1.png".into()), name: "0001.png".into(), saved: true, failed: false },
                    FlowCell { key: "k2".into(), source: None, name: "0002.png".into(), saved: false, failed: true },
                ],
            }],
            running: Some(FlowRunning { title: "生成中".into(), cells: 2, aspect: 1.0, percent: Some(40.0), status: "运行 1 · 等待 0".into() }),
            earlier_label: Some("显示更早".into()),
        }
    }

    #[test]
    fn layout_hits_are_drawn_on_top_of_their_cell() {
        let flow = layout(&sample_input());
        assert!(flow.height > 200.0, "内容总高随记录堆出：{}", flow.height);
        assert!(validate_commands(&flow.commands).is_ok(), "绘制清单须过宿主校验");

        // 章心既在格子热区内又在自己的热区内：后画的章必须赢
        let chip = flow.hits.iter().find(|hit| hit.id == "sva:k2").expect("未入库的格子有另存章");
        let center_x = chip.x + chip.w / 2.0;
        let center_y = chip.y + chip.h / 2.0;
        assert_eq!(hit_test(&flow, center_x, center_y), Some("sva:k2"));

        let preview = flow.hits.iter().find(|hit| hit.id == "pvw:k1").expect("格子可点大图");
        assert_eq!(hit_test(&flow, preview.x + 4.0, preview.y + 4.0), Some("pvw:k1"));
        assert_eq!(hit_test(&flow, -1.0, -1.0), None);
        // 已入库的格子不再有存章
        assert!(!flow.hits.iter().any(|hit| hit.id == "save:k1"));
        assert!(flow.hits.iter().any(|hit| hit.id == "save:k2"));
        assert!(flow.hits.iter().any(|hit| hit.id == "cpy:p1"));
        assert!(flow.hits.iter().any(|hit| hit.id == "more"));
        assert!(flow.hits.iter().any(|hit| hit.id == "interrupt"));
    }

    #[test]
    fn empty_flow_has_no_hits_and_minimal_height() {
        let flow = layout(&FlowInput::default());
        assert!(flow.hits.is_empty());
        assert!(flow.height > 0.0, "空流仍占一条底色带，避免画布高度为 0");
    }
}
