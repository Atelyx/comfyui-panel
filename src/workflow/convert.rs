//! UI 格式 → API 格式转换，以及把参数草稿写回 UI 格式的 widget 槽位。
//!
//! ComfyUI 编辑器保存的工作流是 UI 格式（nodes/links/widgets_values），而提交 `/prompt`
//! 只接受 API 格式（节点 id → {class_type, inputs}）。本模块按文件结构做 UI→API 映射，
//! 只依赖文件结构、不查节点定义：
//! - 跳过静音（mode 2）、旁路（mode 4）与虚拟节点（groupNode / Reroute）；
//! - Reroute 是前端虚拟节点，不进 prompt，其输入连线沿链路穿透到真实节点；静音/旁路节点不是透传层，
//!   指向它们的连线直接丢弃，收尾时按「必需输入缺失」交给服务端报错，而不是给一份错位的 prompt；
//! - widget 值取 `widgets_values_named`（按名）或 `widgets_values`（按序）；按序取值时
//!   seed/noise_seed 之后各占一个 control_after_generate 槽位，多出的尾部值视为纯展示 widget 忽略；
//! - 指向已剔除节点的连线（悬空）在收尾时删掉。
//!
//! 子图节点（type 命中 definitions.subgraphs 的 id）无法从文件单点还原，转换直接失败，
//! 由界面提示用户在浏览器编排——给一份错位的 prompt 比拒绝更糟。

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

/// seed 类输入后，前端会插一个 control_after_generate widget，占用一个 widgets_values 槽位。
const SEED_INPUTS: [&str; 2] = ["seed", "noise_seed"];
/// 虚拟节点类型：不进 prompt。Reroute 穿透，groupNode 整节点剔除。
const SKIP_NODE_TYPES: [&str; 2] = ["groupNode", "Reroute"];
/// 不进 prompt 的节点模式：静音 `2`（NEVER）与旁路 `4`（BYPASS），与前端 graphToPrompt 的跳过条件一致。
const SKIP_NODE_MODES: [i64; 2] = [2, 4];

/// 节点是否整体不进 prompt（静音/旁路/虚拟）。转换与草稿写回共用此判定，两处口径必须一致。
fn is_excluded_node(node: &Map<String, Value>) -> bool {
    node.get("mode").and_then(Value::as_i64).is_some_and(|m| SKIP_NODE_MODES.contains(&m))
        || node
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| SKIP_NODE_TYPES.contains(&t))
}

/// 节点 id 可能是数字或字符串，统一成字符串作为各映射的键。
fn id_key(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 已是 API 格式：顶层对象、无 `nodes` 键、且至少一个值是带字符串 `class_type` 的对象。
pub fn is_api_format(raw: &Value) -> bool {
    let Some(map) = raw.as_object() else {
        return false;
    };
    if map.contains_key("nodes") {
        return false;
    }
    map.values().any(|v| {
        v.as_object()
            .and_then(|o| o.get("class_type"))
            .and_then(Value::as_str)
            .is_some()
    })
}

/// 转换一份 UI 格式工作流为可提交的 API 格式 prompt（对象）。失败给出可展示原因。
pub fn convert_ui_to_api(raw: &Value) -> Result<Value, String> {
    let Some(workflow) = raw.as_object() else {
        return Err("顶层应是一个对象".into());
    };
    if is_api_format(raw) {
        return Err("已是 API 格式，无需转换".into());
    }
    let Some(nodes) = workflow.get("nodes").and_then(Value::as_array) else {
        return Err("不是可识别的工作流 JSON（缺少 nodes 数组）".into());
    };

    // 子图节点的 type 命中 definitions.subgraphs 里某项的 id
    let subgraph_ids: HashSet<String> = workflow
        .get("definitions")
        .and_then(|d| d.get("subgraphs"))
        .and_then(Value::as_array)
        .map(|subs| subs.iter().filter_map(|s| id_key(s.get("id"))).collect())
        .unwrap_or_default();
    for node in nodes.iter().filter_map(Value::as_object) {
        if let (Some(id), Some(ty)) = (id_key(node.get("id")), node.get("type").and_then(Value::as_str)) {
            if subgraph_ids.contains(ty) {
                return Err(format!("节点 {id} 是子图，面板不支持转换，请在浏览器中编排"));
            }
        }
    }

    // 节点 id → 节点对象
    let node_by_id: HashMap<String, &Map<String, Value>> = nodes
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|node| id_key(node.get("id")).map(|id| (id, node)))
        .collect();

    // link id → (origin 节点 id, origin 槽位)
    let mut link_by_id: HashMap<String, (String, i64)> = HashMap::new();
    for link in workflow.get("links").and_then(Value::as_array).into_iter().flatten() {
        let Some(arr) = link.as_array() else { continue };
        if arr.len() < 5 {
            continue;
        }
        let (Some(id), Some(origin), Some(slot)) =
            (id_key(arr.first()), id_key(arr.get(1)), arr.get(2).and_then(Value::as_i64))
        else {
            continue;
        };
        link_by_id.insert(id, (origin, slot));
    }

    // 剔除集：静音/旁路节点与虚拟节点。它们的连线要么穿透（Reroute），要么收尾丢弃。
    let excluded: HashSet<String> = nodes
        .iter()
        .filter_map(Value::as_object)
        .filter(|node| is_excluded_node(node))
        .filter_map(|node| id_key(node.get("id")))
        .collect();

    // 仅 Reroute 沿链穿透：静音/旁路节点不是透传层，指向它们的连线应落到「丢弃」而非「绕过到其上游」。
    let reroutes: HashSet<String> = nodes
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|node| {
            let id = id_key(node.get("id"))?;
            (node.get("type").and_then(Value::as_str) == Some("Reroute")).then_some(id)
        })
        .collect();

    // 沿 Reroute 穿透到真实节点。限深 32 防环；终点若是禁用/已剔除节点（不在保留集）则返回 None。
    let resolve_origin = |origin_id: String, origin_slot: i64| -> Option<(String, i64)> {
        let mut current = origin_id;
        let mut slot = origin_slot;
        for _ in 0..32 {
            if !reroutes.contains(&current) {
                let kept = node_by_id.contains_key(&current) && !excluded.contains(&current);
                return kept.then_some((current, slot));
            }
            let node = node_by_id.get(&current)?;
            let linked = node
                .get("inputs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|input| input.get("link").map(|l| !l.is_null()).unwrap_or(false))?;
            let link_id = id_key(linked.get("link"))?;
            let (next, next_slot) = link_by_id.get(&link_id)?;
            current = next.clone();
            slot = *next_slot;
        }
        None
    };

    let mut prompt = Map::new();
    for node in nodes.iter().filter_map(Value::as_object) {
        let Some(id) = id_key(node.get("id")) else { continue };
        if excluded.contains(&id) {
            continue;
        }
        let mut inputs = Map::new();
        let positional = node.get("widgets_values").and_then(Value::as_array);
        let named = node.get("widgets_values_named").and_then(Value::as_object);
        let mut widget_idx = 0usize;
        for input in node.get("inputs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object) {
            let Some(name) = input.get("name").and_then(Value::as_str) else { continue };
            // 连线输入：link 为 null 时 id_key 返回 None，自然落回字面量分支
            if let Some(link_id) = id_key(input.get("link")) {
                if let Some((origin, slot)) = link_by_id.get(&link_id) {
                    if let Some((resolved_id, resolved_slot)) = resolve_origin(origin.clone(), *slot) {
                        inputs.insert(name.to_string(), Value::Array(vec![
                            Value::String(resolved_id),
                            Value::from(resolved_slot),
                        ]));
                    }
                }
                continue;
            }
            // 非 widget 输入跳过
            let has_widget = input.get("widget").map(|w| !w.is_null()).unwrap_or(false);
            if !has_widget {
                continue;
            }
            // 字面量：有 named 按名取，否则按序从 widgets_values 取
            if let Some(named_map) = named {
                let widget_name = input.get("widget").and_then(|w| w.get("name")).and_then(Value::as_str);
                let value = named_map
                    .get(name)
                    .or_else(|| widget_name.and_then(|wn| named_map.get(wn)));
                match value {
                    Some(v) => inputs.insert(name.to_string(), v.clone()),
                    None => None,
                };
            } else if let Some(list) = positional {
                if widget_idx >= list.len() {
                    continue;
                }
                inputs.insert(name.to_string(), list[widget_idx].clone());
                widget_idx += 1;
                if SEED_INPUTS.contains(&name) && widget_idx < list.len() {
                    widget_idx += 1;
                }
            }
        }
        let mut entry = Map::new();
        entry.insert("class_type".into(), Value::String(node.get("type").and_then(Value::as_str).unwrap_or("").to_string()));
        entry.insert("inputs".into(), Value::Object(inputs));
        if let Some(title) = node.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            let mut meta = Map::new();
            meta.insert("title".into(), Value::String(title.to_string()));
            entry.insert("_meta".into(), Value::Object(meta));
        }
        prompt.insert(id, Value::Object(entry));
    }

    if prompt.is_empty() {
        return Err("没有可执行的节点".into());
    }

    // 悬空连线收尾：引用指向不在 prompt 里的节点的删掉。先只读收集待删坐标，再逐个删除，
    // 避免边遍历边改。
    let mut dangling: Vec<(String, String)> = Vec::new();
    for (node_id, node) in prompt.iter() {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs.iter() {
            let Some(arr) = value.as_array() else { continue };
            if arr.len() != 2 || !arr[0].is_string() {
                continue;
            }
            let target = arr[0].as_str().unwrap();
            if !prompt.contains_key(target) {
                dangling.push((node_id.clone(), name.clone()));
            }
        }
    }
    for (node_id, name) in dangling {
        if let Some(inputs) = prompt.get_mut(&node_id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
            inputs.remove(&name);
        }
    }

    Ok(Value::Object(prompt))
}

/// 把草稿（API 格式）的字面量参数写回 UI 格式文件的 widget 槽位。
///
/// 沿用与 [`convert_ui_to_api`] 相同的 widget 槽位规则（含 seed 后的 control_after_generate 跳过），
/// 只覆盖字面量值；连线引用与节点结构不动。草稿里没有的输入保持文件原值，但槽位照常前进以免错位。
/// 文件同时存在命名数组与按序数组时两者都写，保证面板读回与浏览器读回一致。
pub fn apply_draft_to_ui(raw: &mut Value, prompt: &Value) -> Result<(), String> {
    let Some(workflow) = raw.as_object_mut() else {
        return Err("顶层应是一个对象".into());
    };
    let Some(nodes) = workflow.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Err("不是可识别的工作流 JSON（缺少 nodes 数组）".into());
    };
    let excluded: HashSet<String> = nodes
        .iter()
        .filter_map(Value::as_object)
        .filter(|node| is_excluded_node(node))
        .filter_map(|node| id_key(node.get("id")))
        .collect();

    let prompt_map = prompt.as_object();
    for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
        let Some(id) = id_key(node.get("id")) else { continue };
        if excluded.contains(&id) {
            continue;
        }
        let Some(entry_inputs) = prompt_map.and_then(|p| p.get(&id)).and_then(|e| e.get("inputs")).and_then(Value::as_object) else {
            continue;
        };

        // 先只读扫描 inputs，收集要写回的 (按名) 与 (按下标) 值，避免读写同一节点借用冲突
        let inputs = node.get("inputs").and_then(Value::as_array).cloned().unwrap_or_default();
        let named_map = node.get("widgets_values_named").and_then(Value::as_object).cloned();
        let positional_len = node.get("widgets_values").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);

        let mut named_writes: Vec<(String, Value)> = Vec::new();
        let mut positional_writes: Vec<(usize, Value)> = Vec::new();
        let mut widget_idx = 0usize;
        for input in inputs.iter().filter_map(Value::as_object) {
            let Some(name) = input.get("name").and_then(Value::as_str) else { continue };
            let has_link = input.get("link").map(|l| !l.is_null()).unwrap_or(false);
            let has_widget = input.get("widget").map(|w| !w.is_null()).unwrap_or(false);
            if has_link || !has_widget {
                continue;
            }
            // 每个 widget 输入占一个按序槽位，seed 输入再跳过其后的 control_after_generate 槽。
            // 即使草稿缺这个值，槽位也照常前进——否则后续输入会整体错位写进别的 widget。
            let slot = widget_idx;
            widget_idx += 1;
            if SEED_INPUTS.contains(&name) {
                widget_idx += 1;
            }
            let Some(value) = entry_inputs.get(name) else { continue };
            if let Some(map) = named_map.as_ref() {
                named_writes.push((name.to_string(), value.clone()));
                if let Some(wn) = input.get("widget").and_then(|w| w.get("name")).and_then(Value::as_str) {
                    if wn != name && map.contains_key(wn) {
                        named_writes.push((wn.to_string(), value.clone()));
                    }
                }
            }
            if slot < positional_len {
                positional_writes.push((slot, value.clone()));
            }
        }

        if !named_writes.is_empty() {
            if let Some(map) = node.get_mut("widgets_values_named").and_then(Value::as_object_mut) {
                for (key, value) in named_writes {
                    map.insert(key, value);
                }
            }
        }
        if !positional_writes.is_empty() {
            if let Some(list) = node.get_mut("widgets_values").and_then(Value::as_array_mut) {
                for (index, value) in positional_writes {
                    if index < list.len() {
                        list[index] = value;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_muted_and_bypassed_nodes_and_their_links() {
        let raw = json!({
            "nodes": [
                {"id": 1, "type": "K", "mode": 4, "inputs": [], "outputs": [{"name": "out", "links": [10]}]},
                {"id": 2, "type": "K", "inputs": [{"name": "in", "link": 10}], "outputs": []},
                {"id": 3, "type": "K", "mode": 2, "inputs": [], "outputs": []},
            ],
            "links": [[10, 1, 0, 2, 0]],
        });
        let prompt = convert_ui_to_api(&raw).unwrap();
        assert!(prompt.get("1").is_none() && prompt.get("3").is_none(), "旁路与静音节点都不进 prompt");
        let node2 = prompt.get("2").unwrap();
        assert!(node2["inputs"].get("in").is_none(), "指向被剔除节点的连线被丢弃，而非绕过");
    }

    #[test]
    fn traverses_reroute_to_real_upstream() {
        let raw = json!({
            "nodes": [
                {"id": 1, "type": "K", "inputs": [], "outputs": [{"name": "out", "links": [20]}]},
                {"id": 5, "type": "Reroute", "inputs": [{"name": "input", "link": 20}], "outputs": [{"name": "output", "links": [21]}]},
                {"id": 2, "type": "K", "inputs": [{"name": "in", "link": 21}], "outputs": []},
            ],
            "links": [[20, 1, 0, 5, 0], [21, 5, 0, 2, 0]],
        });
        let prompt = convert_ui_to_api(&raw).unwrap();
        assert!(prompt.get("5").is_none(), "Reroute 虚拟节点不进 prompt");
        assert_eq!(prompt["2"]["inputs"]["in"], json!(["1", 0]), "连线穿透到真实上游节点 1");
    }

    #[test]
    fn draft_positional_advances_slot_when_value_missing() {
        let mut raw = json!({
            "nodes": [{
                "id": 5, "type": "K",
                "inputs": [
                    {"name": "foo", "widget": {"name": "foo"}},
                    {"name": "bar", "widget": {"name": "bar"}},
                ],
                "widgets_values": ["a", "b"],
            }],
        });
        let prompt = json!({"5": {"class_type": "K", "inputs": {"bar": "B"}}});
        apply_draft_to_ui(&mut raw, &prompt).unwrap();
        // foo 草稿缺值：槽位仍前进，bar 写进第 1 槽而非挤到第 0 槽
        assert_eq!(raw["nodes"][0]["widgets_values"], json!(["a", "B"]));
    }

    #[test]
    fn draft_writes_named_and_positional_when_both_exist() {
        let mut raw = json!({
            "nodes": [{
                "id": 5, "type": "K",
                "inputs": [
                    {"name": "foo", "widget": {"name": "foo"}},
                    {"name": "bar", "widget": {"name": "bar"}},
                ],
                "widgets_values_named": {"foo": "a", "bar": "b"},
                "widgets_values": ["a", "b"],
            }],
        });
        let prompt = json!({"5": {"class_type": "K", "inputs": {"foo": "X", "bar": "Y"}}});
        apply_draft_to_ui(&mut raw, &prompt).unwrap();
        assert_eq!(raw["nodes"][0]["widgets_values_named"], json!({"foo": "X", "bar": "Y"}));
        assert_eq!(raw["nodes"][0]["widgets_values"], json!(["X", "Y"]));
    }
}
