//! 工作流文件清单：把 ComfyUI 用户目录 `workflows/` 当唯一来源，经 `/userdata` 接口列出与读写，
//! 不在插件本地另存副本。列表条目带 mtime，用于探测用户在浏览器里的外部改动。

use std::collections::HashMap;

use crate::comfy::client::{ComfyClient, UserDataEntry};

/// 用户目录下的工作流子目录名（无斜杠）。
pub const WORKFLOWS_DIR: &str = "workflows";

/// ComfyUI 内部收藏索引，不是工作流，列表里排除。
const INTERNAL_FILES: [&str; 1] = [".index.json"];

/// 一个工作流文件条目。`path` 相对用户目录（`workflows/xx.json`），同时充当工作流 id。
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowFileInfo {
    pub path: String,
    /// 文件名（含 `.json` 后缀）。
    pub name: String,
    /// 服务端给出的修改时间（毫秒）。
    pub modified: f64,
    pub size: f64,
}

/// 末段文件名（最后一个 `/` 之后）。
fn leaf(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// 是否工作流条目：排除内部索引、非 `.json`（大小写不敏感）、以 `.` 开头的隐藏文件。
fn is_workflow_entry(entry: &UserDataEntry) -> bool {
    if INTERNAL_FILES.contains(&entry.path.as_str()) {
        return false;
    }
    if !entry.path.to_lowercase().ends_with(".json") {
        return false;
    }
    !leaf(&entry.path).starts_with('.')
}

/// 列表返回的 path 相对 `dir`（无 `workflows/` 前缀），此处补成完整用户相对路径。
fn to_workflow_file(entry: &UserDataEntry) -> WorkflowFileInfo {
    WorkflowFileInfo {
        path: format!("{WORKFLOWS_DIR}/{}", entry.path),
        name: leaf(&entry.path).to_string(),
        modified: entry.modified,
        size: entry.size,
    }
}

/// 列出工作流文件并按名排序（展示稳定，不随服务端返回顺序跳变）。
pub async fn list_workflow_files(client: &ComfyClient) -> Result<Vec<WorkflowFileInfo>, String> {
    let entries = client.list_user_dir(WORKFLOWS_DIR).await?;
    let mut files: Vec<WorkflowFileInfo> = entries.iter().filter(|e| is_workflow_entry(e)).map(to_workflow_file).collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// 展示名：末段去目录、去 `.json` 后缀（判定大小写不敏感，去固定 5 字符）。
pub fn workflow_display_name(path: &str) -> String {
    let name = leaf(path);
    if name.to_lowercase().ends_with(".json") {
        name[..name.len() - 5].to_string()
    } else {
        name.to_string()
    }
}

/// 对比两次列表快照的变化条目：新增或 mtime/size 变动入 `changed`，消失入 `removed`。只看元数据不读正文。
pub fn diff_workflows(
    previous: &HashMap<String, WorkflowFileInfo>,
    next: &[WorkflowFileInfo],
) -> (Vec<WorkflowFileInfo>, Vec<String>) {
    let next_map: HashMap<&str, &WorkflowFileInfo> = next.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut changed = Vec::new();
    for file in next {
        match previous.get(&file.path) {
            Some(prev) if prev.modified == file.modified && prev.size == file.size => {}
            _ => changed.push(file.clone()),
        }
    }
    let removed: Vec<String> = previous
        .keys()
        .filter(|path| !next_map.contains_key(path.as_str()))
        .cloned()
        .collect();
    (changed, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, modified: f64, size: f64) -> UserDataEntry {
        UserDataEntry { path: path.into(), modified, size }
    }

    #[test]
    fn filters_internal_hidden_and_non_json() {
        assert!(!is_workflow_entry(&entry(".index.json", 0.0, 0.0)));
        assert!(!is_workflow_entry(&entry("notes.txt", 0.0, 0.0)));
        assert!(!is_workflow_entry(&entry(".draft.json", 0.0, 0.0)));
        assert!(is_workflow_entry(&entry("a.json", 0.0, 0.0)));
        assert!(is_workflow_entry(&entry("b.JSON", 0.0, 0.0)));
    }

    #[test]
    fn path_gets_prefix_and_leaf_name() {
        let file = to_workflow_file(&entry("sub/x.json", 5.0, 9.0));
        assert_eq!(file.path, "workflows/sub/x.json");
        assert_eq!(file.name, "x.json");
        assert_eq!(workflow_display_name(&file.path), "x");
    }

    #[test]
    fn diff_compares_mtime_and_size_only() {
        let prev = HashMap::from([
            ("workflows/a.json".to_string(), WorkflowFileInfo { path: "workflows/a.json".into(), name: "a.json".into(), modified: 1.0, size: 2.0 }),
            ("workflows/gone.json".to_string(), WorkflowFileInfo { path: "workflows/gone.json".into(), name: "gone.json".into(), modified: 0.0, size: 0.0 }),
        ]);
        let next = vec![
            WorkflowFileInfo { path: "workflows/a.json".into(), name: "a.json".into(), modified: 1.0, size: 2.0 },
            WorkflowFileInfo { path: "workflows/new.json".into(), name: "new.json".into(), modified: 7.0, size: 3.0 },
        ];
        let (changed, removed) = diff_workflows(&prev, &next);
        assert_eq!(changed.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["workflows/new.json"]);
        assert_eq!(removed, vec!["workflows/gone.json".to_string()]);
    }
}
