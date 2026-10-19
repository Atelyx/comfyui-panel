//! 参数字段的推导与读写：把工作流 prompt 的输入按 `object_info` 定义解析成可渲染的字段规格，
//! 并提供草稿读取、写回、值收敛、随机种子、比例解析等纯逻辑。
//!
//! 字段三态：连线输入（值是 `[节点id, 槽位]`）只读；字面量输入按 schema 决定控件；定义缺失
//! （`object_info` 为 null 或查不到该 class_type）时退化为按值类型推断，兜底成 JSON 文本编辑。
//! 种子在 API 格式里只是数字，「每次随机」由提交前现场重随机实现（不还原 control_after_generate）。

use std::collections::BTreeSet;

use serde_json::{Map, Number, Value};

/// 控件形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Combo,
    Int,
    Float,
    Str,
    Boolean,
    Json,
}

/// 一个可编辑参数的完整规格：定位（节点/输入）+ 呈现（标签/类型/候选/约束）+ 行为（是否种子/是否连线）。
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpec {
    pub node_id: String,
    pub node_label: String,
    pub class_type: String,
    pub input: String,
    pub kind: FieldKind,
    /// 由上游连线提供 → 只读展示，不进编辑控件。
    pub connected: bool,
    /// 连线来源 `${节点id}:${槽位}`（仅 connected 时有值）。
    pub link_from: Option<String>,
    /// combo 候选：(提交值, 展示名)，无候选为 None。
    pub options: Option<Vec<(String, String)>>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub multiline: bool,
    /// 输入名是 seed/noise_seed → 可在提交前重随机。
    pub is_seed: bool,
}

impl FieldSpec {
    /// 字段在 seedMode 集合里的键（与 [`default_seed_keys`]、[`apply_seed_mode`] 一致，点分隔）。
    pub fn key(&self) -> String {
        format!("{}.{}", self.node_id, self.input)
    }
}

/// 值的展示串：字符串原样，其余用 JSON 文本（数字即字面量，对象/数组为紧凑 JSON）。
fn json_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 判定连线引用 `[节点id, 槽位]`：长度 2、首元素是节点 id（字符串或数字均可）、次元素数字、且源节点在本 prompt 内（缺任一条即视作字面量数组）。
fn as_link(value: &Value, prompt: &Map<String, Value>) -> Option<(String, i64)> {
    let arr = value.as_array()?;
    if arr.len() != 2 {
        return None;
    }
    let source = match &arr[0] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let slot = arr[1].as_i64()?;
    if !prompt.contains_key(&source) {
        return None;
    }
    Some((source, slot))
}

/// 取下拉候选，认三种声明形态：类型位是候选数组、`"COMBO"`+`options` 数组、`"COMBO"`+`options` 对象（键=提交值、值=展示名）。
fn combo_options(schema: Option<&Value>) -> Option<Vec<(String, String)>> {
    let arr = schema?.as_array()?;
    let declared = arr.first()?;
    if let Some(list) = declared.as_array() {
        return Some(list.iter().map(|item| {
            let s = json_to_string(item);
            (s.clone(), s)
        }).collect());
    }
    if declared.as_str() == Some("COMBO") {
        let options = arr.get(1).and_then(|o| o.get("options"))?;
        if let Some(list) = options.as_array() {
            return Some(list.iter().map(|item| {
                let s = json_to_string(item);
                (s.clone(), s)
            }).collect());
        }
        if let Some(map) = options.as_object() {
            return Some(map.iter().map(|(value, label)| (value.clone(), json_to_string(label))).collect());
        }
    }
    None
}

/// 由 schema 声明与当前值推断控件形态：候选优先，其次精确类型标记，最后按值类型兜底。
fn kind_of(schema: Option<&Value>, value: &Value) -> FieldKind {
    if combo_options(schema).is_some() {
        return FieldKind::Combo;
    }
    let declared = schema.and_then(Value::as_array).and_then(|arr| arr.first()).and_then(Value::as_str);
    match declared {
        Some("INT") => return FieldKind::Int,
        Some("FLOAT") => return FieldKind::Float,
        Some("STRING") => return FieldKind::Str,
        Some("BOOLEAN") => return FieldKind::Boolean,
        _ => {}
    }
    match value {
        Value::Number(n) => if n.is_i64() || n.is_u64() { FieldKind::Int } else { FieldKind::Float },
        Value::Bool(_) => FieldKind::Boolean,
        Value::String(_) => FieldKind::Str,
        _ => FieldKind::Json,
    }
}

/// schema 的第二个元素（约束字典）里取数字型约束；非数字（如 `"default"` 字符串）跳过。
fn number_option(schema: Option<&Value>, key: &str) -> Option<f64> {
    schema?
        .as_array()?
        .get(1)?
        .get(key)?
        .as_f64()
}

/// 按节点与输入声明顺序推导全部字段。`object_info` 为 None 时全部退化为按值类型推断。
pub fn build_fields(prompt: &Value, object_info: Option<&Value>) -> Vec<FieldSpec> {
    let Some(prompt_map) = prompt.as_object() else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    for (node_id, node) in prompt_map {
        let Some(node_map) = node.as_object() else { continue };
        let class_type = node_map.get("class_type").and_then(Value::as_str).unwrap_or("").to_string();
        let info = object_info.and_then(|oi| oi.get(&class_type));
        let required = info.and_then(|i| i.get("input")).and_then(|i| i.get("required"));
        let optional = info.and_then(|i| i.get("input")).and_then(|i| i.get("optional"));
        let node_label = node_map
            .get("_meta")
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| info.and_then(|i| i.get("display_name")).and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| class_type.clone());
        let inputs = node_map.get("inputs").and_then(Value::as_object);
        let empty = Map::new();
        for (input, value) in inputs.unwrap_or(&empty) {
            let schema = required
                .and_then(|r| r.get(input))
                .or_else(|| optional.and_then(|o| o.get(input)));
            let link = as_link(value, prompt_map);
            let options = combo_options(schema);
            let kind = kind_of(schema, value);
            fields.push(FieldSpec {
                node_id: node_id.clone(),
                node_label: node_label.clone(),
                class_type: class_type.clone(),
                input: input.clone(),
                kind,
                connected: link.is_some(),
                link_from: link.as_ref().map(|(id, slot)| format!("{id}:{slot}")),
                options,
                min: number_option(schema, "min"),
                max: number_option(schema, "max"),
                step: number_option(schema, "step"),
                multiline: schema
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.get(1))
                    .and_then(|o| o.get("multiline"))
                    .and_then(Value::as_bool)
                    == Some(true),
                is_seed: input == "seed" || input == "noise_seed",
            });
        }
    }
    fields
}

/// 读取字段当前值（缺省 None）。
pub fn read_field(prompt: &Value, field: &FieldSpec) -> Option<Value> {
    prompt
        .get(&field.node_id)?
        .get("inputs")?
        .get(&field.input)
        .cloned()
}

/// 数字框显示值：数字原样，数字字符串按 [`coerce_field_value`] 的同一口径解析；其余（含 `Null`）为空。
pub fn numeric_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 写回字段值（原地改写草稿）。节点或 inputs 缺失则不动（不凭空补节点）。
pub fn write_field(prompt: &mut Value, node_id: &str, input: &str, value: Value) {
    if let Some(inputs) = prompt.get_mut(node_id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        inputs.insert(input.to_string(), value);
    }
}

/// 把控件取回的值收敛成工作流应有的类型；无法转换的（非法数字/JSON）原样保留，让错误可见。
pub fn coerce_field_value(field: &FieldSpec, raw: &Value) -> Value {
    match field.kind {
        FieldKind::Boolean => match raw {
            Value::Bool(b) => Value::Bool(*b),
            Value::String(s) => Value::Bool(s == "true"),
            _ => Value::Bool(false),
        },
        FieldKind::Int | FieldKind::Float => {
            let num = match raw {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            };
            match num {
                Some(n) if n.is_finite() => {
                    if field.kind == FieldKind::Int {
                        Value::Number(Number::from(n.trunc() as i64))
                    } else {
                        Value::from(n)
                    }
                }
                _ => raw.clone(),
            }
        }
        FieldKind::Json => match raw {
            Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| raw.clone()),
            _ => raw.clone(),
        },
        _ => raw.clone(),
    }
}

/// 展示文本：null→空串，字符串原样，其余 JSON 文本。
pub fn display_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(v) => json_to_string(v),
    }
}

/// 连线来源的展示：有来源显示「← 来自 {来源}」，否则「（连线）」。
pub fn connected_display(link_from: Option<&str>) -> String {
    match link_from {
        Some(source) => format!("← 来自 {source}"),
        None => "（连线）".to_string(),
    }
}

/// 是否参考图输入：LoadImage 类节点的 `image` 输入（非连线），生成面板给它挂上传/清除按钮。
pub fn is_image_input(field: &FieldSpec) -> bool {
    !field.connected && field.input == "image" && class_suggests_image(&field.class_type)
}

fn class_suggests_image(class_type: &str) -> bool {
    let lower = class_type.to_lowercase();
    lower.contains("loadimage") || lower.contains("load_image") || lower.contains("load image")
}

/// 随机种子：4 字节系统熵按大端读作无符号 32 位，对 2^32−1 取模（避开上界）。
pub fn random_seed() -> u64 {
    let mut bytes = [0u8; 4];
    // 系统熵源在桌面平台恒可用；取不到就当 0，不 panic 拖垮插件。
    if getrandom::getrandom(&mut bytes).is_err() {
        return 0;
    }
    u32::from_be_bytes(bytes) as u64 % 4_294_967_295
}

/// 把勾选「每次随机」的字段在提交前换成新种子；返回带新种子的副本。键为 `节点id.输入`。
pub fn apply_seed_mode(prompt: &Value, seed_mode: &BTreeSet<String>) -> Value {
    let mut next = prompt.clone();
    for key in seed_mode {
        // 取第一个点切分节点 id 与输入名；前导点（节点 id 为空）跳过。
        let Some(dot) = key.find('.') else { continue };
        if dot == 0 {
            continue;
        }
        let (node_id, input) = (&key[..dot], &key[dot + 1..]);
        if next.get(node_id).is_none() {
            continue;
        }
        write_field(&mut next, node_id, input, Value::from(random_seed()));
    }
    next
}

/// 默认勾选「每次随机」的种子字段：未连线的 seed/noise_seed。连线判定与表单渲染共用 [`as_link`]，
/// 两侧对同一份 prompt 必须给出同一个「是否连线」结论，否则连线的 seed 会在提交时被覆盖成字面量。
pub fn default_seed_keys(prompt: &Value) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Some(prompt_map) = prompt.as_object() else {
        return keys;
    };
    for (node_id, node) in prompt_map {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (input, value) in inputs {
            if input != "seed" && input != "noise_seed" {
                continue;
            }
            if as_link(value, prompt_map).is_none() {
                keys.insert(format!("{node_id}.{input}"));
            }
        }
    }
    keys
}

/// 输入名的中文标签；未收录的退回原始输入名。
pub fn field_label(input: &str) -> &'static str {
    match input {
        "text" | "empty_prompt" | "prompt" => "提示词",
        "positive" => "正向提示词",
        "negative" | "negative_prompt" => "反向提示词",
        "seed" | "noise_seed" => "种子",
        "steps" => "步数",
        "cfg" => "CFG",
        "sampler_name" => "采样器",
        "scheduler" => "调度器",
        "denoise" => "降噪",
        "width" => "宽度",
        "height" => "高度",
        "batch_size" => "批次数",
        "ckpt_name" | "model_name" => "模型",
        "lora_name" => "LoRA",
        _ => "",
    }
}

/// 是否「主/正向」提示词输入名（不含 negative，反向提示词被有意排除）。
fn is_prompt_input(input: &str) -> bool {
    matches!(input, "text" | "positive" | "empty_prompt" | "prompt")
}

/// 取本批提交的主提示词文本：按节点顺序的首个未连线且非空的提示词字段值。取不到返回空串。
pub fn prompt_text_of(prompt: &Value) -> String {
    let Some(map) = prompt.as_object() else {
        return String::new();
    };
    for node in map.values() {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (input, value) in inputs {
            if !is_prompt_input(input) {
                continue;
            }
            // 连线引用是数组，字符串值必然是字面量
            if let Some(text) = value.as_str().map(str::trim) {
                if !text.is_empty() {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn combo_from_array_and_object_map() {
        let schema = json!([["a.safetensors", "b.safetensors"]]);
        assert_eq!(combo_options(Some(&schema)).unwrap().len(), 2);
        let obj = json!(["COMBO", {"options": {"x": "显示X"}}]);
        assert_eq!(combo_options(Some(&obj)).unwrap(), vec![("x".into(), "显示X".into())]);
    }

    #[test]
    fn kind_prefers_combo_then_marker_then_value() {
        assert_eq!(kind_of(Some(&json!(["INT", {"min": 0}])), &json!(5)), FieldKind::Int);
        assert_eq!(kind_of(Some(&json!(["IMAGE"])), &json!("a.png")), FieldKind::Str);
        assert_eq!(kind_of(None, &json!(true)), FieldKind::Boolean);
        assert_eq!(kind_of(None, &json!([1, 2])), FieldKind::Json);
    }

    #[test]
    fn coerce_int_truncates_toward_zero_and_keeps_bad_json() {
        let field = FieldSpec { kind: FieldKind::Int, ..string_field() };
        assert_eq!(coerce_field_value(&field, &json!("3.9")), json!(3));
        assert_eq!(coerce_field_value(&field, &json!("-3.9")), json!(-3));
        let jf = FieldSpec { kind: FieldKind::Json, ..string_field() };
        assert_eq!(coerce_field_value(&jf, &json!("[1,2]")), json!([1, 2]));
        assert_eq!(coerce_field_value(&jf, &json!("not json")), json!("not json"));
    }

    #[test]
    fn seed_mode_splits_on_first_dot() {
        let prompt = json!({"1": {"class_type": "K", "inputs": {"seed": 1, "noise_seed.sub": 2}}});
        let mut keys = BTreeSet::new();
        keys.insert("1.seed".to_string());
        keys.insert("1.noise_seed.sub".to_string());
        let out = apply_seed_mode(&prompt, &keys);
        assert_ne!(out["1"]["inputs"]["seed"], json!(1), "seed 被重随机");
        // 第一个点切分：node=1, input=noise_seed.sub
        assert!(out["1"]["inputs"].get("noise_seed.sub").is_some());
    }

    /// 连线来源既可能是字符串节点 id，也可能是数字 id（API 格式文件里的原始连线）：两种都必须判为已连线。
    #[test]
    fn seed_detection_treats_numeric_origin_as_link() {
        let prompt = json!({
            "1": {"class_type": "K", "inputs": {"seed": 111}},
            "2": {"class_type": "K", "inputs": {"seed": ["1", 0], "noise_seed": [1, 0]}},
        });
        assert_eq!(default_seed_keys(&prompt), BTreeSet::from(["1.seed".to_string()]));
    }

    #[test]
    fn prompt_text_skips_negative_and_connected() {
        let prompt = json!({
            "2": {"class_type": "C", "inputs": {"text": ["1", 0]}},
            "1": {"class_type": "P", "inputs": {"negative": "neg", "text": "hello"}},
        });
        assert_eq!(prompt_text_of(&prompt), "hello");
    }

    fn string_field() -> FieldSpec {
        FieldSpec {
            node_id: "1".into(),
            node_label: "N".into(),
            class_type: "C".into(),
            input: "x".into(),
            kind: FieldKind::Str,
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
}
