//! 工作流的载入/存回/校验：文件即工作流。读到的文件按格式分派——API 格式校验后直接用，
//! UI 格式经 [`convert_ui_to_api`] 转换；转换失败的文件仍可列出，由界面提示去浏览器打开，不产出错位 prompt。

use serde_json::Value;

use crate::comfy::client::ComfyClient;
use crate::workflow::convert::{apply_draft_to_ui, convert_ui_to_api, is_api_format};
use crate::workflow::files::{self, WorkflowFileInfo};

/// 已归一化、可直接提交的工作流。`modified` 记录载入时刻 mtime，供界面判断文件是否被外部改过。
#[derive(Clone, Debug)]
pub struct StoredWorkflow {
    pub id: String,
    pub name: String,
    pub modified: f64,
    pub prompt: Value,
}

/// 读文件并归一化出可提交 prompt。JSON 解析失败带「文件不是合法 JSON」前缀，读文件失败裸传原因。
pub async fn load_workflow(client: &ComfyClient, file: &WorkflowFileInfo) -> Result<StoredWorkflow, String> {
    let text = client.read_user_file(&file.path).await?;
    // 工作流 JSON 可能很大：解析与转换挪阻塞线程。
    let prompt = tokio::task::spawn_blocking(move || {
        let raw: Value = serde_json::from_str(&text).map_err(|e| format!("文件不是合法 JSON：{e}"))?;
        if is_api_format(&raw) {
            validate_api_prompt(&raw)
        } else {
            convert_ui_to_api(&raw)
        }
    })
    .await
    .map_err(|err| format!("解析工作流任务失败：{err}"))??;
    Ok(StoredWorkflow {
        id: file.path.clone(),
        name: files::workflow_display_name(&file.name),
        modified: file.modified,
        prompt,
    })
}

/// 写回结果：UI 格式只覆盖 widget 值，API 格式整份替换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    pub replaced_wholesale: bool,
}

/// 把参数草稿写回文件：API 格式整体替换为草稿，UI 格式只覆盖 widget 值（保留结构与连线）。统一 2 空格缩进。
/// 写回前按 `/userdata` 的 mtime 确认文件没被外部改过；不一致即中止，不覆盖用户的改动。
pub async fn save_workflow_params(
    client: &ComfyClient,
    file: &WorkflowFileInfo,
    prompt: &Value,
    expected_modified: f64,
) -> Result<SaveOutcome, String> {
    let current = files::list_workflow_files(client).await?;
    let Some(entry) = current.iter().find(|item| item.path == file.path) else {
        return Err("工作流已不在列表中，请重新选择后再保存".to_string());
    };
    if entry.modified != expected_modified {
        return Err("文件已被外部修改，请重新选择后再保存".to_string());
    }
    let text = client.read_user_file(&file.path).await?;
    let draft = prompt.clone();
    let (content, replaced_wholesale) = tokio::task::spawn_blocking(move || -> Result<(String, bool), String> {
        let parsed: Value = serde_json::from_str(&text).map_err(|e| format!("文件不是合法 JSON：{e}"))?;
        if is_api_format(&parsed) {
            let content = serde_json::to_string_pretty(&draft).map_err(|e| e.to_string())?;
            Ok((content, true))
        } else {
            let mut tree = parsed;
            apply_draft_to_ui(&mut tree, &draft)?;
            let content = serde_json::to_string_pretty(&tree).map_err(|e| e.to_string())?;
            Ok((content, false))
        }
    })
    .await
    .map_err(|err| format!("序列化工作流任务失败：{err}"))??;
    client.write_user_file(&file.path, &content).await?;
    Ok(SaveOutcome { replaced_wholesale })
}

/// 校验并归一化 API 格式：只保留 `class_type`/`inputs`/`_meta` 三键，丢弃其余节点键；不检查节点类型是否存在
/// （取决于自定义节点，交服务端提交时报错）。
pub fn validate_api_prompt(raw: &Value) -> Result<Value, String> {
    let Some(record) = raw.as_object() else {
        return Err("顶层应是一个对象（节点 id → 节点定义）".into());
    };
    if record.is_empty() {
        return Err("工作流是空的".into());
    }
    let mut prompt = serde_json::Map::new();
    for (id, node) in record {
        let Some(entry) = node.as_object() else {
            return Err(format!("节点 {id} 不是对象"));
        };
        let class_type = entry
            .get("class_type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| format!("节点 {id} 缺少 class_type"))?;
        let inputs_value = entry.get("inputs");
        if let Some(inputs_value) = inputs_value {
            if !inputs_value.is_object() {
                return Err(format!("节点 {id} 的 inputs 不是对象"));
            }
        }
        let mut normalized = serde_json::Map::new();
        normalized.insert("class_type".into(), Value::String(class_type.to_string()));
        normalized.insert(
            "inputs".into(),
            inputs_value.cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        );
        if let Some(meta) = entry.get("_meta") {
            normalized.insert("_meta".into(), meta.clone());
        }
        prompt.insert(id.clone(), Value::Object(normalized));
    }
    Ok(Value::Object(prompt))
}

/// 转发文件清单（含 mtime，按名排序）。
pub async fn list_workflows(client: &ComfyClient) -> Result<Vec<WorkflowFileInfo>, String> {
    files::list_workflow_files(client).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_normalizes_to_three_keys() {
        let raw = json!({"1": {"class_type": "K", "inputs": {"a": 1}, "mode": 0, "outputs": []}});
        let out = validate_api_prompt(&raw).unwrap();
        assert_eq!(out["1"].as_object().unwrap().len(), 2, "仅 class_type + inputs");
        assert_eq!(out["1"]["inputs"]["a"], json!(1));
    }

    #[test]
    fn validate_rejects_missing_class_type_and_empty() {
        assert_eq!(validate_api_prompt(&json!({})), Err("工作流是空的".to_string()));
        assert!(validate_api_prompt(&json!({"1": {"inputs": {}}})).unwrap_err().starts_with("节点 1 缺少 class_type"));
        assert!(validate_api_prompt(&json!("不是对象")).is_err());
    }
}
