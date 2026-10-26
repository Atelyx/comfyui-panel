//! 结果记录流的画布布局：把「旧→新」的记录分组排成一张纵向图，一次算出绘制指令与命中区域。
//!
//! 绘制与命中同源于一次 [`layout`]：坐标只在结构里算一遍，指令与热区因此不会漂移。
//! 单元格按列平铺（列数随画布宽度定），图面 cover 裁切；格高按该批图片的真实宽高比
//! （副本落盘时解析文件头所得），占位格按用户当前选中的比例。格面只有图本身：
//! 点击整格开灯箱，存档 / 另存 / 复制提示词都在灯箱里（与表格编辑器图片字段同一种交互），
//! 出错格描红边。运行中的批次排在最末（贴底即最新），进度条与步数画在占位格内。
//! 命中返回的 id 就是动作契约（`HIT_*` 一组常量），与界面控件的动作语义一一对应，
//! 由 app 的事件分派直接消费，画布不引入第二套动作语义。
//!
//! 增量提交：每条指令带稳定 id（领域键的 128 位散列），宿主画布按 id 增量维护——
//! 布局没变就不发，变了只发差异（[`patch_plan`]）。可选视觉（副本图、进度条、标签）
//! 从条目创建起就占住槽位（空文本占位），内容后到只做原位替换：追加会把叠层次序打乱。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use atelyx_plugin_sdk::protocol::canvas::{CanvasColor, CanvasUpsert, DrawCommand};
use atelyx_plugin_sdk::protocol::view::{ColorToken, ImageFit, ImageSource, TextWeight};

use crate::comfy::types::queue_item_prompt;
use crate::history::ResultImage;
use crate::workflow::form::{prompt_text_of, FieldSpec};

/// 单元格最小宽度（列数按画布宽度在此基线上折算）。
const CELL_MIN: f32 = 150.0;
/// 画布命中区 id：整格开灯箱、显示更早、中断。这套 id 只存在于画布热区（界面树里没有同名节点），
/// 动作语义与界面控件一致，由 app 统一分派。
pub const HIT_PREVIEW: &str = "pvw:";
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
/// 格内进度条厚度。
const PROG_H: f32 = 4.0;
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

/// 一张图：定位 key、可绘制的本地副本、是否出错、原图宽高比（副本落盘时解析，缺省方格）。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowCell {
    pub key: String,
    pub source: Option<String>,
    pub failed: bool,
    pub aspect: Option<f32>,
}

/// 一条已完成记录：同一批（同 promptId）的若干张图 + 标题行信息 + 批内共同的图面比例。
#[derive(Clone, Debug, PartialEq)]
pub struct FlowRecord {
    pub prompt_id: String,
    pub title: String,
    pub time: String,
    pub cells: Vec<FlowCell>,
    /// 格高基准：取批内第一张已知原图的比例（同批同尺寸），未知按方格。
    pub aspect: f32,
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
                    source: item.display_source(),
                    failed: item.failed,
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
                time: format_time(lead.created_at),
                aspect: cells.iter().find_map(|cell| cell.aspect).unwrap_or(1.0),
                cells,
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
        title: if text.is_empty() { "生成中".to_string() } else { text },
        cells: spec.1,
        aspect: spec.0,
        percent: percent.filter(|value| value.is_finite()),
        steps,
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
    out.draws.insert(0, FlowDraw { id: "bg".to_string(), command: rect(0.0, 0.0, width, out.height, Some(rgba(SOFT)), None) });
    out
}

fn push_chip(out: &mut FlowLayout, width: f32, label: &str, id: &str) {
    let y = out.height + PAD;
    let w = (text_px(label, 11.0) + 12.0).min(width - PAD * 2.0).max(1.0);
    let x = (width - w) / 2.0;
    out.push(format!("{id}:r"), rect(x, y, w, CHIP_H, Some(rgba(CHIP_BG)), Some(rgba(SOFT_EDGE))));
    out.push(format!("{id}:t"), text(x + 6.0, y + 2.0, label, 11.0, token(ColorToken::TextMuted), None));
    out.hits.push(FlowHit { x, y, w, h: CHIP_H, id: id.to_string() });
    out.height = y + CHIP_H + GAP;
}

/// 一条已完成记录：标题行（提示词/时间）+ 图格墙（格面只有图，动作进灯箱）。
fn push_record(out: &mut FlowLayout, width: f32, columns: usize, record: &FlowRecord) {
    let top = out.height + PAD;
    let rows = record.cells.len().div_ceil(columns.max(1));
    let (cell_w, cell_h) = cell_size(width, columns, record.aspect);
    let body_h = rows as f32 * cell_h + (rows.saturating_sub(1)) as f32 * GAP;
    let card_h = HEADER_H + GAP + body_h + PAD * 2.0;
    let slot = slot_prefix("r", &record.prompt_id);
    out.push(format!("{slot}:card"), rounded_card(width, top, card_h));

    let (chip_x, chip_y) = (width - content_left() - CHIP_W, top + PAD + 2.0);
    let title_w = content_width(width) - CHIP_W - TIME_W - GAP;
    out.push(format!("{slot}:title"), text(content_left(), top + PAD, &clip_text(&record.title, title_w, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    // 时间从建立起占槽：内容后到走原位替换，不打乱叠层次序
    out.push(format!("{slot}:time"), text(chip_x - GAP - TIME_W, chip_y + 1.0, &record.time, 11.0, token(ColorToken::TextMuted), None));

    for (index, cell) in record.cells.iter().enumerate() {
        let x = PAD * 2.0 + (index % columns) as f32 * (cell_w + GAP);
        let y = top + PAD + HEADER_H + GAP + (index / columns) as f32 * (cell_h + GAP);
        push_cell(out, cell, x, y, cell_w, cell_h);
    }
    out.height = top + card_h + GAP;
}

/// 一个图格：副本就绪就贴 cover 图，否则留底色占位，出错描红边；点击整格开灯箱。
fn push_cell(out: &mut FlowLayout, cell: &FlowCell, x: f32, y: f32, w: f32, h: f32) {
    let slot = slot_prefix("c", &cell.key);
    let border = if cell.failed { Some(token(ColorToken::Danger)) } else { Some(rgba(SOFT_EDGE)) };
    out.push(format!("{slot}:bg"), rect(x, y, w, h, Some(rgba(SOFT)), border));
    // 图片槽先建：副本落定后原位替换，追加会把图片盖到既有图层之上
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
    out.hits.push(FlowHit { x, y, w, h, id: format!("{HIT_PREVIEW}{}", cell.key) });
}

/// 运行中批次：静态占位格（排队/已出/进行中/待出四态），进行中格内画进度条与步数。
fn push_running(out: &mut FlowLayout, width: f32, columns: usize, running: &FlowRunning) {
    let top = out.height + PAD;
    let cells = running.cells.clamp(1, 12);
    let rows = cells.div_ceil(columns.max(1));
    let (cell_w, cell_h) = cell_size(width, columns, running.aspect);
    let body_h = rows as f32 * cell_h + (rows.saturating_sub(1)) as f32 * GAP;
    let card_h = HEADER_H + GAP + body_h + PAD * 2.0;
    out.push("run:card".to_string(), rounded_card(width, top, card_h));
    let (chip_x, chip_y) = (width - content_left() - CHIP_W, top + PAD + 2.0);
    let title_w = content_width(width) - CHIP_W - TIME_W - GAP;
    out.push("run:title".to_string(), text(content_left(), top + PAD, &clip_text(&running.title, title_w, 12.0), 12.0, token(ColorToken::Text), Some(TextWeight::Medium)));
    out.push("run:status".to_string(), text(chip_x - GAP - TIME_W, chip_y + 1.0, &clip_text(&running.status, TIME_W, 11.0), 11.0, token(ColorToken::TextMuted), None));
    out.push("run:chip:r".to_string(), rect(chip_x, chip_y, CHIP_W, CHIP_H, Some(rgba(CHIP_BG)), None));
    out.push("run:chip:t".to_string(), text(chip_x + 5.0, chip_y + 2.0, "中断", 10.0, token(ColorToken::Danger), None));
    out.hits.push(FlowHit { x: chip_x, y: chip_y, w: CHIP_W, h: CHIP_H, id: HIT_INTERRUPT.to_string() });

    let pct = running.percent.map(|value| (value / 100.0).clamp(0.0, 1.0));
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
        out.push(format!("run{index}:bg"), rect(x, y, cell_w, cell_h, Some(fill), edge));
        // 进度条画在格内底部：轨道与填充槽位从建立起占住，进度推进只做原位替换
        let (track, bar) = match (state, pct) {
            (CellState::Active, Some(pct)) => (
                rect(x, y + cell_h - PROG_H, cell_w, PROG_H, Some(rgba(BAR_BG)), None),
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
        let size = if cell_h > 120.0 { 20.0 } else if cell_h > 80.0 { 15.0 } else { 12.0 };
        out.push(format!("run{index}:lbl"), text(x + 8.0, y + (cell_h - size) / 2.0, &label, size, token(ColorToken::TextMuted), None));
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
        // 画廊最近在前：同批两张并成一条，最新批沉在底部；格高取批内已知比例
        let mut wide = result("p2", "b", 2000.0, Some("history/b.png"));
        wide.pixel = Some((1024, 512));
        let items = vec![wide, result("p1", "a", 1000.0, None), result("p1", "a2", 1000.0, Some("history/a2.png"))];
        let records = records_of(&items);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].prompt_id, "p1");
        assert_eq!(records[0].cells.len(), 2);
        assert_eq!(records[0].aspect, 1.0, "批内无已知比例按方格");
        assert_eq!(records[0].cells[0].source, None, "缺副本的格子按占位处理");
        assert_eq!(records[1].prompt_id, "p2");
        assert!((records[1].aspect - 2.0).abs() < 0.001, "批内首张已知比例生效");
    }

    #[test]
    fn record_aspect_drives_cell_height() {
        let input = FlowInput {
            width: 360.0,
            records: vec![
                FlowRecord {
                    prompt_id: "p1".into(),
                    title: "方图".into(),
                    time: String::new(),
                    aspect: 1.0,
                    cells: vec![FlowCell { key: "k1".into(), source: None, failed: false, aspect: None }],
                },
                FlowRecord {
                    prompt_id: "p2".into(),
                    title: "横图".into(),
                    time: String::new(),
                    aspect: 2.0,
                    cells: vec![FlowCell { key: "k2".into(), source: None, failed: false, aspect: Some(2.0) }],
                },
            ],
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
                time: "10-01 12:00".into(),
                aspect: 1.0,
                cells: vec![
                    FlowCell { key: "k1".into(), source: Some("history/k1.png".into()), failed: false, aspect: None },
                    FlowCell { key: "k2".into(), source: None, failed: true, aspect: None },
                ],
            }],
            running: Some(FlowRunning {
                title: "生成中".into(),
                cells: 2,
                aspect: 1.0,
                percent: Some(40.0),
                steps: Some("8/25".into()),
                status: "运行 1 · 等待 0".into(),
            }),
            earlier_label: Some("显示更早".into()),
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
        // 存档 / 另存 / 复制动作都进灯箱：格面只剩整格预览热区
        assert!(flow.hits.iter().all(|hit| hit.id.starts_with("pvw:") || hit.id == "more" || hit.id == "interrupt"));
    }

    #[test]
    fn empty_flow_has_no_hits_and_minimal_height() {
        let flow = layout(&FlowInput::default());
        assert!(flow.hits.is_empty());
        assert!(flow.height > 0.0, "空流仍占一条底色带，避免画布高度为 0");
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
            assert!(draw.id.len() <= 64, "id 超出入住上限：{}", draw.id);
            assert!(ids.insert(draw.id.as_str()), "id 重复：{}", draw.id);
        }
        assert!(first.draws.len() > 10, "样例布局应含背景、章、记录与运行块");
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
        // 变化：p1 标题；新增：p2 整条（顶部章不动——移除它会平移其后全部坐标）
        input.records[0].title = "两只猫".into();
        input.records.push(FlowRecord {
            prompt_id: "p2".into(),
            title: "第二幕".into(),
            time: "10-02 09:00".into(),
            aspect: 1.0,
            cells: vec![FlowCell { key: "k9".into(), source: None, failed: false, aspect: None }],
        });
        let next = layout(&input);

        let (upsert, remove) = patch_plan(&base.draws, &next.draws);
        let upsert_ids: HashSet<&str> = upsert.iter().map(|item| item.id.as_str()).collect();
        let title_slot = format!("{}:title", slot_prefix("r", "p1"));
        assert!(upsert_ids.contains(title_slot.as_str()), "标题变化的那条要重发");
        assert!(upsert_ids.contains(format!("{}:card", slot_prefix("r", "p2")).as_str()), "新纪录整条要发");
        // 其余条目原样不动：增量远小于整表
        assert!(upsert.len() < next.draws.len(), "只发变化项：{} < {}", upsert.len(), next.draws.len());
        assert!(remove.is_empty(), "没有条目消失");
    }

    #[test]
    fn patch_plan_removes_evicted_entries() {
        let mut input = sample_input();
        let base = layout(&input);
        input.earlier_label = None; // 章消失：坐标整体上移，其余条目照旧、章的两个槽走删除
        let next = layout(&input);
        let (upsert, remove) = patch_plan(&base.draws, &next.draws);
        assert!(remove.iter().any(|id| id == "more:r") && remove.iter().any(|id| id == "more:t"), "章消失要走删除：{remove:?}");
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
