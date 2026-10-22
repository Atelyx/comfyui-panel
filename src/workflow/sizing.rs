//! 尺寸与张数推导：把「比例 / 分辨率 / 张数」三段选项的候选、选中态与写回值一次算清。
//!
//! 两条路径同源共用一套换算：工作流带 `ResolutionSelector` 节点且 `aspect_ratio` 有候选时，输出尺寸
//! 由该节点的 `aspect_ratio`＋`megapixels` 决定，写回只改这两个字段；否则退到「空潜像宽高」路径，
//! 按当前比例（空潜像的宽高）与所选百万像素换算出宽高写回。换算式与分辨率选择器节点一致：
//! 面积等于 `megapixels` 的等比例宽高，再就近取整到 `multiple` 的倍数。
//!
//! 展示文本与点击写回都取自同一份 [`SizePlan`]，界面只负责渲染与把命中的 [`FieldWrite`] 落进草稿，
//! 避免「显示 1448:1448、点下去写成别的」。写回值已按字段 schema 收敛，与高级表单写回同一口径。

use serde_json::{Number, Value};

use super::form::{coerce_field_value, display_value, numeric_value, parse_aspect_pair, read_field, FieldSpec};

/// 段落标识，用于事件 id 的定位段（界面与分派两侧共用）。
pub const SECTION_RATIO: &str = "ratio";
pub const SECTION_RESOLUTION: &str = "resolution";
pub const SECTION_COUNT: &str = "count";

/// 比例预设（仅比例）：兜底路径下与「分辨率」一起算出宽高。
const RATIO_PRESETS: &[(&str, f64, f64)] = &[
    ("1:1", 1.0, 1.0),
    ("2:3", 2.0, 3.0),
    ("3:2", 3.0, 2.0),
    ("3:4", 3.0, 4.0),
    ("4:3", 4.0, 3.0),
    ("9:16", 9.0, 16.0),
    ("16:9", 16.0, 9.0),
    ("21:9", 21.0, 9.0),
];

/// 分辨率（百万像素）预设。
const MEGAPIXEL_OPTIONS: &[&str] = &["0.5", "1", "1.5", "2", "4"];

/// 张数预设。
const BATCH_OPTIONS: &[&str] = &["1", "2", "3", "4"];

/// 兜底路径的取整基数（与常见空潜像节点的 step 一致）。
const FALLBACK_MULTIPLE: f64 = 8.0;

/// 一次写回：字段定位 + 已收敛的值。
#[derive(Clone, Debug, PartialEq)]
pub struct FieldWrite {
    pub node_id: String,
    pub input: String,
    pub value: Value,
}

/// 段内一个可选项：展示文本、可选的换算提示（`宽:高`）、点击后要写的字段。
#[derive(Clone, Debug, PartialEq)]
pub struct SizeOption {
    pub value: String,
    pub label: String,
    pub hint: Option<String>,
    pub writes: Vec<FieldWrite>,
}

/// 一段选项（比例/分辨率/张数各一段）；`selected` 为当前命中的 [`SizeOption::value`]，无命中为 None。
#[derive(Clone, Debug, PartialEq)]
pub struct SizeSection {
    pub id: &'static str,
    pub label: &'static str,
    pub selected: Option<String>,
    pub options: Vec<SizeOption>,
}

/// 尺寸三段的完整快照：段落 + 收起态的一行摘要 + 悬浮提示用的实际宽高。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SizePlan {
    pub sections: Vec<SizeSection>,
    pub summary: String,
    pub detail: Option<String>,
}

impl SizePlan {
    /// 按段落 id 与选项下标取回写动作（界面事件只带这两个坐标，值由这里回查，不在界面侧重算）。
    pub fn writes_of(&self, section: &str, index: usize) -> Option<&[FieldWrite]> {
        self.sections
            .iter()
            .find(|s| s.id == section)?
            .options
            .get(index)
            .map(|o| o.writes.as_slice())
    }
}

/// 由比例与百万像素算宽高：面积相等，就近取整到 `multiple` 的倍数，下界为一个 `multiple`。
pub fn size_for_mp(wr: f64, hr: f64, mp: f64, multiple: f64) -> (i64, i64) {
    let multiple = if multiple > 0.0 { multiple } else { FALLBACK_MULTIPLE };
    let mp = if mp > 0.0 && mp.is_finite() { mp } else { 1.0 };
    if !usable_extent(wr) || !usable_extent(hr) {
        return (multiple as i64, multiple as i64);
    }
    let scale = ((mp * 1024.0 * 1024.0) / (wr * hr)).sqrt();
    (snap(wr * scale, multiple), snap(hr * scale, multiple))
}

/// 比例分量可用：有限且为正，否则算不出面积。
fn usable_extent(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn snap(n: f64, multiple: f64) -> i64 {
    if !n.is_finite() {
        return multiple.round() as i64;
    }
    (multiple).max((n / multiple).round() * multiple).round() as i64
}

/// 百万像素的数字键：整数不带小数点（`1 → "1"`、`1.5 → "1.5"`），与预设值对齐。
pub fn mp_key(mp: f64) -> String {
    if !mp.is_finite() {
        return "1".to_string();
    }
    let rounded = (mp * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{}", rounded)
    }
}

/// 分辨率选择器的候选值带注释（如 `1:1 (Square)`），宫格只展示比例部分。
pub fn aspect_label(option: &str) -> &str {
    match option.find(" (") {
        Some(pos) if pos > 0 => &option[..pos],
        _ => option,
    }
}

/// 预设列表，当前值不在其中时追加为一项，保证选中态总能在界面上看见。
fn segments_with_current(presets: &[&str], current: &str) -> Vec<String> {
    let mut list: Vec<String> = presets.iter().map(|s| s.to_string()).collect();
    if !current.is_empty() && !list.iter().any(|item| item == current) {
        list.push(current.to_string());
    }
    list
}

/// 输出宽高字段：同一节点内都未连线的 `width`+`height`，优先空潜像节点，其次按节点顺序取首个命中。
pub fn latent_size_fields<'a>(fields: &'a [FieldSpec]) -> Option<(&'a FieldSpec, &'a FieldSpec)> {
    let pair_in = |node_id: &str| -> Option<(&'a FieldSpec, &'a FieldSpec)> {
        let width = fields.iter().find(|f| f.node_id == node_id && f.input == "width" && !f.connected)?;
        let height = fields.iter().find(|f| f.node_id == node_id && f.input == "height" && !f.connected)?;
        Some((width, height))
    };
    let mut node_ids: Vec<&str> = Vec::new();
    for field in fields {
        if !node_ids.contains(&field.node_id.as_str()) {
            node_ids.push(&field.node_id);
        }
    }
    node_ids.sort_by_key(|node_id| {
        // 稳定排序：属于空潜像节点的排前面。
        !fields.iter().any(|f| &f.node_id == node_id && f.class_type.starts_with("EmptyLatentImage"))
    });
    node_ids.into_iter().find_map(pair_in)
}

/// 未连线且输入名匹配的字段（首个命中）。
pub fn find_unconnected<'a>(fields: &'a [FieldSpec], input: &str) -> Option<&'a FieldSpec> {
    fields.iter().find(|f| !f.connected && f.input == input)
}

/// 指定节点类里未连线的输入字段。
fn find_in_class<'a>(fields: &'a [FieldSpec], class_type: &str, input: &str) -> Option<&'a FieldSpec> {
    fields.iter().find(|f| !f.connected && f.class_type == class_type && f.input == input)
}

/// 分辨率选择器路径的入口字段：带候选清单的 `aspect_ratio`（`plan` 与队列规格判定同源）。
fn selector_ratio_field(fields: &[FieldSpec]) -> Option<&FieldSpec> {
    find_in_class(fields, "ResolutionSelector", "aspect_ratio")
        .filter(|f| f.options.as_ref().is_some_and(|o| !o.is_empty()))
}

/// 当前宽高对应的预设比例标签；都不为正或无命中返回空串。
fn active_ratio_label(size_w: f64, size_h: f64) -> String {
    if size_w > 0.0 && size_h > 0.0 && size_w.is_finite() && size_h.is_finite() {
        let ratio = size_w / size_h;
        for (label, w, h) in RATIO_PRESETS {
            if (ratio - w / h).abs() < 0.02 {
                return (*label).to_string();
            }
        }
    }
    String::new()
}

fn size_text(w: i64, h: i64) -> String {
    format!("{w}:{h}")
}

/// 取正有限数，否则用兜底值（用于宽高、取整基数这类「无效就当没有」的读值）。
fn positive_or(value: Option<f64>, fallback: f64) -> f64 {
    match value {
        Some(v) if v > 0.0 && v.is_finite() => v,
        _ => fallback,
    }
}

fn number_value(n: i64) -> Value {
    Value::Number(Number::from(n))
}

/// 该草稿的实际输出宽高：与三段面板同一条判定——有分辨率选择器就按 `aspect_ratio` × `megapixels`
/// 换算，否则取同节点未连线的宽高字段；两条路都给不出正数宽高就 None。
pub fn output_size(draft: &Value, fields: &[FieldSpec]) -> Option<(f64, f64)> {
    if let Some(ratio_field) = selector_ratio_field(fields) {
        let (wr, hr) = parse_aspect_pair(&display_value(read_field(draft, ratio_field).as_ref()))?;
        let mp = find_in_class(fields, "ResolutionSelector", "megapixels")
            .and_then(|field| numeric_value(read_field(draft, field).as_ref()))
            .filter(|value| *value > 0.0 && value.is_finite())
            .unwrap_or(1.0);
        let multiple = find_in_class(fields, "ResolutionSelector", "multiple")
            .and_then(|field| numeric_value(read_field(draft, field).as_ref()))
            .filter(|value| *value > 0.0 && value.is_finite())
            .unwrap_or(FALLBACK_MULTIPLE);
        let (w, h) = size_for_mp(wr, hr, mp, multiple);
        return Some((w as f64, h as f64));
    }
    let (width_field, height_field) = latent_size_fields(fields)?;
    let w = numeric_value(read_field(draft, width_field).as_ref())?;
    let h = numeric_value(read_field(draft, height_field).as_ref())?;
    (w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()).then_some((w, h))
}

/// 该草稿一次出几张：首个未连线的 `batch_size`，取不到或非法按一张。
pub fn batch_count(draft: &Value, fields: &[FieldSpec]) -> usize {
    find_unconnected(fields, "batch_size")
        .and_then(|field| numeric_value(read_field(draft, field).as_ref()))
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map(|value| value.floor() as usize)
        .unwrap_or(1)
}

/// 推导尺寸三段。`draft` 为当前参数草稿（API 格式），`fields` 为其推导出的字段规格。
pub fn plan(draft: &Value, fields: &[FieldSpec]) -> SizePlan {
    let mut sections: Vec<SizeSection> = Vec::new();
    let mut parts: Vec<String> = Vec::new();

    let selector = selector_ratio_field(fields);
    let latent = latent_size_fields(fields);

    if let Some(selector_field) = selector {
        let ratio_value = display_value(read_field(draft, selector_field).as_ref());
        let options = selector_field
            .options
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(value, _)| SizeOption {
                label: aspect_label(&value).to_string(),
                writes: vec![write_of(selector_field, &value)],
                hint: None,
                value,
            })
            .collect();
        parts.push(if ratio_value.is_empty() { "?".into() } else { aspect_label(&ratio_value).to_string() });
        sections.push(SizeSection {
            id: SECTION_RATIO,
            label: "比例",
            selected: (!ratio_value.is_empty()).then_some(ratio_value.clone()),
            options,
        });

        if let Some(mp_field) = find_in_class(fields, "ResolutionSelector", "megapixels") {
            let current_mp = numeric_value(read_field(draft, mp_field).as_ref()).unwrap_or(1.0);
            let multiple = positive_or(
                find_in_class(fields, "ResolutionSelector", "multiple")
                    .and_then(|f| numeric_value(read_field(draft, f).as_ref())),
                FALLBACK_MULTIPLE,
            );
            let pair = parse_aspect_pair(&ratio_value);
            let options = mp_options(current_mp, pair, multiple, |value| {
                if value.parse::<f64>().is_ok_and(|mp| mp > 0.0) { vec![write_of(mp_field, value)] } else { Vec::new() }
            });
            parts.push(format!("{}K", mp_key(current_mp)));
            sections.push(SizeSection {
                id: SECTION_RESOLUTION,
                label: "分辨率",
                selected: Some(mp_key(current_mp)),
                options,
            });
        }
    } else if let Some((width_field, height_field)) = latent {
        let size_w = numeric_value(read_field(draft, width_field).as_ref()).unwrap_or(0.0);
        let size_h = numeric_value(read_field(draft, height_field).as_ref()).unwrap_or(0.0);
        let has_size = size_w > 0.0 && size_h > 0.0 && size_w.is_finite() && size_h.is_finite();
        let ratio = active_ratio_label(size_w, size_h);
        let fallback_mp = if has_size { (size_w * size_h) / (1024.0 * 1024.0) } else { 1.0 };
        // 宽高写回基准：当前宽高无效时按 1:1，保证仍能算出可写的数。
        let base_w = positive_or(Some(size_w), 1.0);
        let base_h = positive_or(Some(size_h), 1.0);
        let write_size = |w: i64, h: i64| {
            vec![
                write_converted(width_field, number_value(w)),
                write_converted(height_field, number_value(h)),
            ]
        };

        let options = RATIO_PRESETS
            .iter()
            .map(|(label, w, h)| {
                let size = size_for_mp(*w, *h, fallback_mp, FALLBACK_MULTIPLE);
                SizeOption {
                    value: (*label).to_string(),
                    label: (*label).to_string(),
                    hint: Some(size_text_pair(size)),
                    writes: write_size(size.0, size.1),
                }
            })
            .collect();
        parts.push(if !ratio.is_empty() {
            ratio.clone()
        } else if has_size {
            size_text(size_w as i64, size_h as i64)
        } else {
            "?".to_string()
        });
        sections.push(SizeSection {
            id: SECTION_RATIO,
            label: "比例",
            selected: (!ratio.is_empty()).then_some(ratio),
            options,
        });

        let pair = has_size.then_some((size_w, size_h));
        let options = mp_options(fallback_mp, pair, FALLBACK_MULTIPLE, |value| {
            match value.parse::<f64>() {
                Ok(mp) if mp > 0.0 && mp.is_finite() => {
                    let size = size_for_mp(base_w, base_h, mp, FALLBACK_MULTIPLE);
                    write_size(size.0, size.1)
                }
                _ => Vec::new(),
            }
        });
        parts.push(format!("{}K", mp_key(fallback_mp)));
        sections.push(SizeSection {
            id: SECTION_RESOLUTION,
            label: "分辨率",
            selected: Some(mp_key(fallback_mp)),
            options,
        });
    }

    if let Some(batch_field) = find_unconnected(fields, "batch_size") {
        let raw = read_field(draft, batch_field);
        let current = match raw.as_ref() {
            None | Some(Value::Null) => "1".to_string(),
            Some(value) => display_value(Some(value)),
        };
        let options = segments_with_current(BATCH_OPTIONS, &current)
            .into_iter()
            .map(|value| SizeOption {
                label: value.clone(),
                writes: vec![write_of(batch_field, &value)],
                hint: None,
                value,
            })
            .collect();
        if !current.is_empty() {
            parts.push(current.clone());
        }
        sections.push(SizeSection {
            id: SECTION_COUNT,
            label: "张数",
            selected: Some(current),
            options,
        });
    }

    // 摘要里的实际宽高与队列规格走同一条换算，展示与运行卡不会各算一份。
    let detail = output_size(draft, fields).map(|(w, h)| size_text(w as i64, h as i64));
    SizePlan { sections, summary: parts.join(" · "), detail }
}

/// 分辨率段的选项：预设（含当前值追加），带按比例换算出的宽高提示。
fn mp_options(
    current_mp: f64,
    pair: Option<(f64, f64)>,
    multiple: f64,
    writes_for: impl Fn(&str) -> Vec<FieldWrite>,
) -> Vec<SizeOption> {
    segments_with_current(MEGAPIXEL_OPTIONS, &mp_key(current_mp))
        .into_iter()
        .map(|value| SizeOption {
            label: format!("{value}K"),
            hint: pair.map(|(w, h)| size_text_pair(size_for_mp(w, h, value.parse().unwrap_or(1.0), multiple))),
            writes: writes_for(&value),
            value,
        })
        .collect()
}

fn size_text_pair((w, h): (i64, i64)) -> String {
    size_text(w, h)
}

/// 把字符串控件取回的值按字段 schema 收敛后写成一次写回。
fn write_of(field: &FieldSpec, raw: &str) -> FieldWrite {
    write_converted(field, Value::String(raw.to_string()))
}

fn write_converted(field: &FieldSpec, raw: Value) -> FieldWrite {
    FieldWrite {
        node_id: field.node_id.clone(),
        input: field.input.clone(),
        value: coerce_field_value(field, &raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::form::FieldKind;
    use serde_json::json;

    fn field(node_id: &str, class_type: &str, input: &str, kind: FieldKind) -> FieldSpec {
        FieldSpec {
            node_id: node_id.into(),
            node_label: class_type.into(),
            class_type: class_type.into(),
            input: input.into(),
            kind,
            connected: false,
            link_from: None,
            options: None,
            min: None,
            max: None,
            step: None,
            multiline: false,
            is_seed: false,
        }
    }

    #[test]
    fn size_for_mp_snaps_to_multiple_and_clamps_low() {
        assert_eq!(size_for_mp(1.0, 1.0, 1.0, 8.0), (1024, 1024));
        assert_eq!(size_for_mp(9.0, 16.0, 1.0, 8.0), (768, 1368));
        // 极小分辨率也不会算出 0：下界为一个倍数
        assert_eq!(size_for_mp(1.0, 1.0, 0.00001, 8.0), (8, 8));
        // 非法倍数与分辨率按默认处理，不产出 NaN
        assert_eq!(size_for_mp(1.0, 1.0, -3.0, 0.0), (1024, 1024));
    }

    #[test]
    fn mp_key_uses_minimal_digits() {
        assert_eq!(mp_key(1.0), "1");
        assert_eq!(mp_key(1.5), "1.5");
        assert_eq!(mp_key(0.5), "0.5");
        assert_eq!(mp_key(2.34), "2.3");
        assert_eq!(mp_key(f64::NAN), "1");
    }

    #[test]
    fn output_size_and_count_follow_the_same_field_rules() {
        // 空潜像节点优先于其它带宽高的节点，张数取首个未连线的 batch_size
        let draft = json!({
            "1": {"class_type": "EmptyLatentImage", "inputs": {"width": 768, "height": 1368, "batch_size": 3}},
            "2": {"class_type": "ImageScale", "inputs": {"width": 64, "height": 64}},
            "3": {"class_type": "ImageBatch", "inputs": {"batch_size": 9}},
        });
        let fields = vec![
            field("1", "EmptyLatentImage", "width", FieldKind::Int),
            field("1", "EmptyLatentImage", "height", FieldKind::Int),
            field("1", "EmptyLatentImage", "batch_size", FieldKind::Int),
            field("2", "ImageScale", "width", FieldKind::Int),
            field("2", "ImageScale", "height", FieldKind::Int),
            field("3", "ImageBatch", "batch_size", FieldKind::Int),
        ];
        assert_eq!(output_size(&draft, &fields), Some((768.0, 1368.0)));
        assert_eq!(batch_count(&draft, &fields), 3);

        // 宽高由上游连线决定时不给尺寸，没有 batch_size 按一张
        let connected = json!({"1": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512}}});
        let fields = vec![
            {
                let mut spec = field("1", "EmptyLatentImage", "width", FieldKind::Int);
                spec.connected = true;
                spec
            },
            {
                let mut spec = field("1", "EmptyLatentImage", "height", FieldKind::Int);
                spec.connected = true;
                spec
            },
        ];
        assert_eq!(output_size(&connected, &fields), None);
        assert_eq!(batch_count(&connected, &fields), 1);

        // 选择器路径与三段式共用同一道门槛：aspect_ratio 没有候选清单就不算选择器
        let selector = json!({
            "7": {"class_type": "ResolutionSelector", "inputs": {"aspect_ratio": "16:9 (Wide)", "megapixels": 1.0, "multiple": 8}},
        });
        let mut ratio = field("7", "ResolutionSelector", "aspect_ratio", FieldKind::Combo);
        ratio.options = Some(vec![("16:9 (Wide)".into(), "16:9 (Wide)".into())]);
        let mp = field("7", "ResolutionSelector", "megapixels", FieldKind::Float);
        let multiple = field("7", "ResolutionSelector", "multiple", FieldKind::Int);
        assert_eq!(output_size(&selector, &[ratio.clone(), mp.clone(), multiple.clone()]), Some((1368.0, 768.0)));
        let mut bare = ratio;
        bare.options = None;
        assert_eq!(output_size(&selector, &[bare, mp, multiple]), None);
    }

    #[test]
    fn selector_path_writes_selector_fields() {
        let draft = json!({
            "7": {"class_type": "ResolutionSelector", "inputs": {
                "aspect_ratio": "1:1 (Square)", "megapixels": 1.5, "multiple": 16}},
            "9": {"class_type": "EmptyLatentImage", "inputs": {"batch_size": 2}},
        });
        let mut ratio = field("7", "ResolutionSelector", "aspect_ratio", FieldKind::Combo);
        ratio.options = Some(vec![
            ("1:1 (Square)".into(), "1:1 (Square)".into()),
            ("2:3 (Portrait)".into(), "2:3 (Portrait)".into()),
        ]);
        let fields = vec![
            ratio,
            field("7", "ResolutionSelector", "megapixels", FieldKind::Float),
            field("7", "ResolutionSelector", "multiple", FieldKind::Int),
            field("9", "EmptyLatentImage", "batch_size", FieldKind::Int),
        ];
        let plan = plan(&draft, &fields);
        assert_eq!(plan.summary, "1:1 · 1.5K · 2");
        // 悬浮提示按选择器自己的倍数（16）换算
        assert_eq!(plan.detail.as_deref(), Some("1248:1248"));

        let sections = &plan.sections;
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].id, SECTION_RATIO);
        assert_eq!(sections[0].selected.as_deref(), Some("1:1 (Square)"));
        assert_eq!(sections[0].options[1].label, "2:3");
        assert_eq!(sections[0].options[1].writes[0].value, json!("2:3 (Portrait)"));
        assert_eq!(sections[1].id, SECTION_RESOLUTION);
        assert_eq!(sections[1].selected.as_deref(), Some("1.5"));
        assert_eq!(sections[1].options[0].label, "0.5K");
        assert_eq!(sections[1].options[0].hint.as_deref(), Some("720:720"));
        assert_eq!(sections[1].options[2].writes[0].value, json!(1.5));
        assert_eq!(sections[2].id, SECTION_COUNT);
        assert_eq!(sections[2].selected.as_deref(), Some("2"));
        assert_eq!(sections[2].options[1].writes[0].value, json!(2));
    }

    #[test]
    fn latent_path_converts_ratio_and_megapixels_into_width_height() {
        let draft = json!({
            "3": {"class_type": "KSampler", "inputs": {"seed": 1}},
            "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024}},
        });
        let fields = vec![
            field("4", "EmptyLatentImage", "width", FieldKind::Int),
            field("4", "EmptyLatentImage", "height", FieldKind::Int),
        ];
        let plan = plan(&draft, &fields);
        assert_eq!(plan.summary, "1:1 · 1K");
        assert_eq!(plan.detail.as_deref(), Some("1024:1024"));
        assert_eq!(plan.sections[0].selected.as_deref(), Some("1:1"));

        // 选 9:16：保持当前 1MP，换算成 768x1368 并成对写回宽高
        let ratio_nine_sixteen = plan.sections[0]
            .options
            .iter()
            .find(|o| o.value == "9:16")
            .expect("预设里有 9:16");
        assert_eq!(ratio_nine_sixteen.hint.as_deref(), Some("768:1368"));
        assert_eq!(
            ratio_nine_sixteen.writes,
            vec![
                FieldWrite { node_id: "4".into(), input: "width".into(), value: json!(768) },
                FieldWrite { node_id: "4".into(), input: "height".into(), value: json!(1368) },
            ]
        );

        // 选 2K：保持当前比例 1:1，换算成 1448x1448
        let two_k = plan.sections[1]
            .options
            .iter()
            .find(|o| o.value == "2")
            .expect("预设里有 2");
        assert_eq!(two_k.hint.as_deref(), Some("1448:1448"));
        assert_eq!(two_k.writes[0].value, json!(1448));
    }

    #[test]
    fn latent_size_prefers_empty_latent_node() {
        let fields = vec![
            field("1", "SomeScaleNode", "width", FieldKind::Int),
            field("1", "SomeScaleNode", "height", FieldKind::Int),
            field("2", "EmptyLatentImage", "width", FieldKind::Int),
            field("2", "EmptyLatentImage", "height", FieldKind::Int),
        ];
        assert_eq!(latent_size_fields(&fields).map(|(w, h)| (w.node_id.as_str(), h.node_id.as_str())), Some(("2", "2")));
    }

    #[test]
    fn latent_size_requires_both_fields_on_same_node() {
        let fields = vec![
            field("1", "EmptyLatentImage", "width", FieldKind::Int),
            field("2", "EmptyLatentImage", "height", FieldKind::Int),
        ];
        assert!(latent_size_fields(&fields).is_none());
    }

    #[test]
    fn connected_size_fields_are_skipped() {
        let mut width = field("4", "EmptyLatentImage", "width", FieldKind::Int);
        width.connected = true;
        width.link_from = Some("1:0".into());
        let fields = vec![width, field("4", "EmptyLatentImage", "height", FieldKind::Int)];
        assert!(latent_size_fields(&fields).is_none());
        assert!(plan(&json!({"4": {"inputs": {"width": [1, 0], "height": 1024}}}), &fields).sections.is_empty());
    }

    #[test]
    fn current_value_outside_presets_is_appended_as_option() {
        let draft = json!({
            "7": {"class_type": "ResolutionSelector", "inputs": {"aspect_ratio": "1:1", "megapixels": 3.0}},
            "8": {"class_type": "KSampler", "inputs": {"batch_size": 6}},
        });
        let fields = vec![
            field("7", "ResolutionSelector", "megapixels", FieldKind::Float),
            field("8", "KSampler", "batch_size", FieldKind::Int),
        ];
        let plan = plan(&draft, &fields);
        // 无候选的 aspect_ratio 不算选择器路径，也没有宽高字段 → 只剩张数段
        assert_eq!(plan.sections.len(), 1);
        assert_eq!(plan.sections[0].id, SECTION_COUNT);
        let values: Vec<&str> = plan.sections[0].options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["1", "2", "3", "4", "6"]);
        assert_eq!(plan.sections[0].selected.as_deref(), Some("6"));
    }

    #[test]
    fn missing_fields_yield_empty_plan() {
        let plan = plan(&json!({}), &[]);
        assert!(plan.sections.is_empty());
        assert_eq!(plan.summary, "");
        assert_eq!(plan.detail, None);
    }

    #[test]
    fn megapixels_outside_presets_is_appended_and_selected() {
        let draft = json!({
            "7": {"class_type": "ResolutionSelector", "inputs": {"aspect_ratio": "1:1", "megapixels": 3.0}},
        });
        let mut ratio = field("7", "ResolutionSelector", "aspect_ratio", FieldKind::Combo);
        ratio.options = Some(vec![("1:1".into(), "1:1".into())]);
        let fields = vec![ratio, field("7", "ResolutionSelector", "megapixels", FieldKind::Float)];
        let plan = plan(&draft, &fields);
        let values: Vec<&str> = plan.sections[1].options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["0.5", "1", "1.5", "2", "4", "3"]);
        assert_eq!(plan.sections[1].selected.as_deref(), Some("3"));
        assert_eq!(plan.sections[1].options[5].label, "3K");
        assert_eq!(plan.summary, "1:1 · 3K");
    }

    #[test]
    fn batch_size_defaults_to_one_when_absent() {
        let draft = json!({"8": {"class_type": "K", "inputs": {"batch_size": null}}});
        let plan = plan(&draft, &[field("8", "K", "batch_size", FieldKind::Int)]);
        assert_eq!(plan.sections[0].selected.as_deref(), Some("1"));
        assert_eq!(plan.summary, "1");
    }
}
