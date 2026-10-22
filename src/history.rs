//! 结果记录的数据形状、副本命名，以及把图片字节落到插件私有目录的读写。
//!
//! 图源只支持已落盘的本地副本（相对 `data/` 的 `history/{name}`），面板与预览据此显示。
//! 折叠历史时立即抓字节存副本，服务重启后副本仍在；副本名带 promptId 前缀天然按任务分组。
//! 参考图缩略另存 `refs/{name}`：缩略缓存只活在本次进程（[`RefCache`] 限量淘汰最旧），
//! 装载时清空 `refs/` 目录免跨进程堆积，插件内上传同名图先作废该条再重抓。
//! 删除范围始终不出私有目录。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::comfy::types::ImageRef;

/// 画廊长度，同时是持久化记录条数上限（超出按时间淘汰、淘汰项副本随之删除）。
pub const HISTORY_LIMIT: usize = 200;
/// 私有目录下存放图片副本的子目录名。
pub const COPY_DIR: &str = "history";
/// 参考图缩略副本的子目录名（与结果副本分目录，淘汰互不影响）。
pub const REF_DIR: &str = "refs";
/// 参考图缩略缓存条数上限：超出按入库先后淘汰最旧并删其副本。
pub const REF_LIMIT: usize = 24;
/// 缺副本的结果最多补抓这么多轮：源已被服务端回收时，继续重试只是每轮白耗请求。
pub const RECAPTURE_ATTEMPTS: u32 = 3;

/// 一张结果图：定位（key/promptId/引用）+ 展示（标题/提示词/时间/出错）+ 落盘（本地副本/已入库路径）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResultImage {
    /// 稳定标识 `{promptId}:{subfolder}/{filename}`，用于去重与副本回填。
    pub key: String,
    pub prompt_id: String,
    #[serde(rename = "ref")]
    pub image_ref: ImageRef,
    pub title: String,
    pub prompt_text: String,
    pub created_at: f64,
    pub failed: bool,
    /// 已保存到仓库的相对路径；保存后写入。
    pub saved_path: Option<String>,
    /// 本地副本相对私有目录的路径（`history/{name}`）；抓取成功后写入，缺省表示尚未就绪或抓取失败。
    pub local: Option<String>,
    /// 已补抓的轮数；达到 [`RECAPTURE_ATTEMPTS`] 后不再进入补抓批次。
    pub copy_attempts: u32,
}

impl ResultImage {
    /// 画廊展示图源：副本就绪时用私有目录相对路径，否则 None（界面显示占位）。
    pub fn display_source(&self) -> Option<String> {
        self.local.clone()
    }
}

/// 副本文件名：`{promptId}-{index}{ext}`；扩展名取原文件名末段，无点则按 `.png`。
pub fn copy_name(prompt_id: &str, index: usize, file_name: &str) -> String {
    let ext = match file_name.rfind('.') {
        Some(dot) if dot > 0 => &file_name[dot..],
        _ => ".png",
    };
    format!("{prompt_id}-{index}{ext}")
}

/// 补抓副本文件名：`retry-{promptId}-{拍平后的文件名}`。与 [`copy_name`] 的 index 命名
/// 不同前缀，保证补抓多次落在同名文件上（幂等）且绝不覆盖首次抓取已成功的副本。
pub fn retry_copy_name(prompt_id: &str, file_name: &str) -> String {
    format!("retry-{prompt_id}-{}", safe_stem(file_name))
}

/// 把服务端给的相对路径拍成安全的文件名片段：`_` 先双写、路径分隔符与非法字符换成单个 `_`，
/// 末尾接原文件名的校验和。`sub/a.png`、`sub_a.png` 与带冒号的同名形态因此各占一份副本，
/// 不会出现两条记录指向同一文件（谁后写谁赢、另一条显示错图）。
fn safe_stem(filename: &str) -> String {
    let escaped = filename.replace('_', "__");
    let flat: String = escaped
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let stem: String = flat.chars().take(80).collect();
    format!("{stem}-{:016x}", checksum(filename))
}

/// 64 位 FNV-1a 校验和：只用于文件名唯一性，不作内容校验。
fn checksum(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash = (hash ^ *byte as u64).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 副本在私有目录内的相对路径，同时是 `ResultImage::local` 与界面 `ImageSource::Private` 的取值。
/// [`CopyStore`] 的 `save`/`read`/`remove` 一律收这个形式，避免同一副本在读写删三侧口径不一致。
pub fn copy_path(name: &str) -> String {
    format!("{COPY_DIR}/{name}")
}

/// 参考图缩略副本名：input 目录里的相对路径按 [`safe_stem`] 拍平（扩展名随文件名原样留下）。
/// 同名即同一路径，重复抓取原地覆盖，不会产生第二份副本。
pub fn ref_copy_name(filename: &str) -> String {
    safe_stem(filename)
}

/// 参考图缩略副本在私有目录内的相对路径。
pub fn ref_path(name: &str) -> String {
    format!("{REF_DIR}/{name}")
}

/// 参考图缩略副本缓存：`input 文件名 → 私有目录相对路径`，按入库先后排队、超上限淘汰最旧。
/// 淘汰只交出路径，删除动作由调用方经 [`CopyStore::remove`] 执行，删除范围不出私有目录。
#[derive(Default)]
pub struct RefCache {
    /// 最旧在前，最新在尾。
    entries: Vec<(String, String)>,
}

impl RefCache {
    /// 取副本路径（只读，界面投影用）；命中不改动顺序。
    pub fn get(&self, filename: &str) -> Option<&str> {
        self.entries.iter().find(|(name, _)| name == filename).map(|(_, path)| path.as_str())
    }

    /// 摘掉某个文件名的条目并交出它的副本路径：内容变了要重抓时先用它作废缓存。
    pub fn remove(&mut self, filename: &str) -> Option<String> {
        self.entries.iter().position(|(name, _)| name == filename).map(|index| self.entries.remove(index).1)
    }

    /// 写入副本路径，返回需要删除的旧副本：同名的原路径（覆盖写会留第二份）与超上限弹出的最旧条目。
    pub fn put(&mut self, filename: &str, path: String) -> Vec<String> {
        let mut stale: Vec<String> = Vec::new();
        if let Some(index) = self.entries.iter().position(|(name, _)| name == filename) {
            stale.push(self.entries.remove(index).1);
        }
        self.entries.push((filename.to_string(), path.clone()));
        if self.entries.len() > REF_LIMIT {
            stale.push(self.entries.remove(0).1);
        }
        stale.retain(|old| old != &path);
        stale
    }
}

/// 图片字节副本的落盘存取：根目录固定为插件私有目录。
#[derive(Clone)]
pub struct CopyStore {
    root: PathBuf,
}

impl CopyStore {
    /// `root` 取宿主 `fs.privateDir()`（即 `data/`）。
    pub fn new(root: String) -> Self {
        CopyStore { root: PathBuf::from(root) }
    }

    /// 确保副本目录存在（私有目录根由宿主建，子目录归本模块）。
    pub fn ensure_dir(&self) {
        let _ = std::fs::create_dir_all(self.root.join(COPY_DIR));
    }

    fn abs(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn save(&self, relative: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.abs(relative);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建副本目录失败：{e}"))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("写入副本失败：{e}"))
    }

    /// 读副本字节，落库与另存优先用本地副本。
    pub fn read(&self, relative: &str) -> Option<Vec<u8>> {
        std::fs::read(self.abs(relative)).ok()
    }

    /// 删副本：尽力而为，失败（已不存在/被占用）不影响状态。
    pub fn remove(&self, relative: &str) {
        let _ = std::fs::remove_file(self.abs(relative));
    }

    /// 清空参考图缩略目录：缓存不落盘，装载时旧副本已无人认领，删掉以免跨进程无限堆积。
    /// 只删本目录一层的文件，结果副本与设置信封不受影响。
    pub fn clear_refs(&self) {
        let Ok(entries) = std::fs::read_dir(self.root.join(REF_DIR)) else { return };
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_name_ext_rules() {
        assert_eq!(copy_name("p1", 0, "a.png"), "p1-0.png");
        assert_eq!(copy_name("p1", 3, "noext"), "p1-3.png");
        assert_eq!(copy_name("p1", 1, ".hidden"), "p1-1.png", "前导点不算扩展名");
        assert_eq!(copy_name("p1", 2, "g.gif"), "p1-2.gif");
    }

    #[test]
    fn copy_path_is_relative_to_private_root() {
        assert_eq!(copy_path("p-0.png"), "history/p-0.png");
    }

    #[test]
    fn flattened_names_keep_distinct_sources_apart() {
        // 子目录、下划线、非法字符三种同名形态各占一份副本（拍平后接原文件名校验和）
        assert_ne!(retry_copy_name("p1", "sub/a.png"), retry_copy_name("p1", "a.png"));
        assert_ne!(ref_copy_name("sub/a.png"), ref_copy_name("sub_a.png"));
        assert_ne!(ref_copy_name("a:b.png"), ref_copy_name("a_b.png"));
        // 同一文件名两次命名结果一致：重复抓取原地覆盖，不留第二份
        assert_eq!(ref_copy_name("sub/a.png"), ref_copy_name("sub/a.png"));
        // 分隔符都换成下划线、扩展名随原文件名留下、超长名片段裁短
        assert!(ref_copy_name("sub/a.png").starts_with("sub_a.png"));
        assert!(ref_copy_name("sub\\a.jpg").starts_with("sub_a.jpg"));
        assert!(ref_copy_name(&"x".repeat(300)).chars().count() < 110);
        // 与首次抓取命名不同前缀，补抓不会覆盖 index 命名的副本
        assert_ne!(retry_copy_name("p1", "a.png"), copy_name("p1", 0, "a.png"));
        assert_eq!(ref_path("a.png"), "refs/a.png");
    }

    #[test]
    fn ref_cache_hands_back_stale_paths_on_replace() {
        let mut cache = RefCache::default();
        assert_eq!(cache.get("a.png"), None);
        assert!(cache.put("a.png", ref_path("a.png")).is_empty());
        assert_eq!(cache.get("a.png"), Some("refs/a.png"));
        // 同名重复抓取：原地覆盖，不产生待删的第二份
        assert!(cache.put("a.png", ref_path("a.png")).is_empty());
        // 换名（同一槽位换参考图）：旧副本交回调用方删除
        assert_eq!(cache.put("a.png", ref_path("b.png")), ["refs/a.png"]);
        assert_eq!(cache.get("a.png"), Some("refs/b.png"));
    }

    #[test]
    fn ref_cache_evicts_oldest_when_over_limit() {
        let mut cache = RefCache::default();
        let mut removed = Vec::new();
        for index in 0..=REF_LIMIT {
            let name = format!("{index}.png");
            removed.extend(cache.put(&name, ref_path(&name)));
        }
        assert_eq!(removed, vec![ref_path("0.png")], "只淘汰最先入库的一条");
        assert_eq!(cache.entries.len(), REF_LIMIT);
        assert_eq!(cache.get("0.png"), None);
        assert!(cache.get(&format!("{REF_LIMIT}.png")).is_some());
    }
}
