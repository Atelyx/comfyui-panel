//! 结果记录的数据形状、副本命名，以及把图片字节落到插件私有目录的读写。
//!
//! 图源只支持已落盘的本地副本（相对 `data/` 的 `history/{name}`），面板与预览据此显示。
//! 折叠历史时立即抓字节存副本，服务重启后副本仍在；副本名带 promptId 前缀天然按任务分组。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::comfy::types::ImageRef;

/// 画廊长度，同时是持久化记录条数上限（超出按时间淘汰、淘汰项副本随之删除）。
pub const HISTORY_LIMIT: usize = 200;
/// 私有目录下存放图片副本的子目录名。
pub const COPY_DIR: &str = "history";
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

/// 补抓副本文件名：`retry-{promptId}-{路径分隔符换成下划线的文件名}`。与 [`copy_name`] 的 index 命名
/// 不同前缀，保证补抓多次落在同名文件上（幂等）且绝不覆盖首次抓取已成功的副本。
pub fn retry_copy_name(prompt_id: &str, file_name: &str) -> String {
    let flat = file_name.replace(['/', '\\'], "_");
    format!("retry-{prompt_id}-{flat}")
}

/// 副本在私有目录内的相对路径，同时是 `ResultImage::local` 与界面 `ImageSource::Private` 的取值。
/// [`CopyStore`] 的 `save`/`read`/`remove` 一律收这个形式，避免同一副本在读写删三侧口径不一致。
pub fn copy_path(name: &str) -> String {
    format!("{COPY_DIR}/{name}")
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
    fn retry_name_flattens_path_and_keeps_extension() {
        assert_eq!(retry_copy_name("p1", "sub/a.png"), "retry-p1-sub_a.png");
        assert_eq!(retry_copy_name("p1", "a.png"), "retry-p1-a.png");
        // 不同子目录的同名文件不撞车
        assert_ne!(retry_copy_name("p1", "sub/a.png"), retry_copy_name("p1", "a.png"));
        // 与首次抓取命名不同前缀，补抓不会覆盖 index 命名的副本
        assert_ne!(retry_copy_name("p1", "a.png"), copy_name("p1", 0, "a.png"));
    }

    #[test]
    fn copy_path_is_relative_to_private_root() {
        assert_eq!(copy_path("p-0.png"), "history/p-0.png");
    }
}
