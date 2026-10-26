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
    /// 原图片素尺寸（副本字节落盘时解析文件头）：画廊格高按它算；缺省（未抓到/旧记录）按方格。
    pub pixel: Option<(u32, u32)>,
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

/// 图片的固有像素尺寸（PNG / JPEG / GIF / BMP / WebP 文件头）；认不出返回 `None`。
/// 副本字节落盘时解析进 `ResultImage::pixel`（画廊格高按它算），只看文件头不解码像素。
pub fn image_size(header: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = png_size(header)
        .or_else(|| gif_size(header))
        .or_else(|| bmp_size(header))
        .or_else(|| webp_size(header))
        .or_else(|| jpeg_size(header))?;
    (w > 0 && h > 0).then_some((w, h))
}

/// PNG：8 字节签名后是 IHDR，宽高各 4 字节大端（偏移 16 / 20）。
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let signature = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if !bytes.starts_with(&signature) {
        return None;
    }
    Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
}

/// GIF：`GIF87a` / `GIF89a` 后是逻辑屏幕宽高，各 2 字节小端（偏移 6 / 8）。
fn gif_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a") {
        return None;
    }
    Some((u32::from(le_u16(bytes, 6)?), u32::from(le_u16(bytes, 8)?)))
}

/// BMP：`BM` 后 DIB 头，宽高各 4 字节小端（偏移 18 / 22；高度为负表示自上而下存储，取绝对值）。
fn bmp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"BM") {
        return None;
    }
    let width = le_u32(bytes, 18)?;
    let height = le_u32(bytes, 22).map(|value| value as i32).map(i32::unsigned_abs)?;
    Some((width, height))
}

/// WebP：`RIFF` + 尺寸 + `WEBP`，之后按块定尺寸——VP8X 扩展块带 24 位画布尺寸，
/// VP8 无损块把 14 位宽高打包进 4 字节，VP8 有损块在帧头后各 2 字节（14 位有效）。
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 16 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8X" if bytes.len() >= 30 => {
            let w = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]) + 1;
            let h = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]) + 1;
            Some((w, h))
        }
        b"VP8L" if bytes.len() >= 25 => {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8 " if bytes.len() >= 30 => {
            // 帧头 3 字节 + 同步码 3 字节后是 16 位宽高（各 14 位有效）
            let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]])) & 0x3fff;
            let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]])) & 0x3fff;
            Some((w, h))
        }
        _ => None,
    }
}

/// JPEG：逐段扫描到 SOF0–SOF15（帧头），其中高 4 字节起是宽高各 2 字节大端。
/// 段长自描述，遇 SOS（扫描数据）即止；不展开 EXIF 等大段。
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xff {
            at += 1; // 填充字节：跳过
            continue;
        }
        let marker = bytes[at + 1];
        match marker {
            0xd8 | 0x01 | 0xd0..=0xd7 => at += 2, // 无载荷标记
            0xda => return None,                  // SOS：像素数据开始，找不到帧头
            0xc0..=0xcf if marker != 0xc4 && marker != 0xc8 && marker != 0xcc => {
                // SOF（排除 JPG 与 DAC 两个非帧标记）：段长 2 字节 + 精度 1 + 高 2 + 宽 2
                let h = be_u16(bytes, at + 5)?;
                let w = be_u16(bytes, at + 7)?;
                return Some((u32::from(w), u32::from(h)));
            }
            _ => {
                let len = be_u16(bytes, at + 2)? as usize;
                if len < 2 {
                    return None;
                }
                at += 2 + len;
            }
        }
    }
    None
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at + 2).map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).and_then(|quad| quad.try_into().ok()).map(u32::from_be_bytes)
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at + 2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).and_then(|quad| quad.try_into().ok()).map(u32::from_le_bytes)
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

    /// 副本的绝对路径：宿主仓库导入面（`vault.importFile`）只收私有目录内的绝对路径。
    pub fn abs_path(&self, relative: &str) -> PathBuf {
        self.abs(relative)
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

    #[test]
    fn image_size_reads_common_container_headers() {
        // PNG：IHDR 宽高（大端）
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]); // IHDR 长
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1024u32.to_be_bytes());
        png.extend_from_slice(&768u32.to_be_bytes());
        assert_eq!(image_size(&png), Some((1024, 768)));

        // GIF：逻辑屏幕宽高（小端）
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&640u16.to_le_bytes());
        gif.extend_from_slice(&480u16.to_le_bytes());
        assert_eq!(image_size(&gif), Some((640, 480)));

        // BMP：DIB 头宽高（小端，负高度取绝对值）
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(&800i32.to_le_bytes());
        bmp.extend_from_slice(&(-600i32).to_le_bytes());
        assert_eq!(image_size(&bmp), Some((800, 600)));

        // WebP 三种块形态
        let mut vp8x = b"RIFF".to_vec();
        vp8x.extend_from_slice(&[0; 4]);
        vp8x.extend_from_slice(b"WEBPVP8X");
        vp8x.extend_from_slice(&[0; 4]);
        vp8x.extend_from_slice(&[0; 4]); // 标志位 + 保留字段
        vp8x.extend_from_slice(&999u32.to_le_bytes()[..3]);
        vp8x.extend_from_slice(&499u32.to_le_bytes()[..3]);
        assert_eq!(image_size(&vp8x), Some((1000, 500)), "VP8X 画布尺寸各 +1");

        let mut vp8l = b"RIFF".to_vec();
        vp8l.extend_from_slice(&[0; 4]);
        vp8l.extend_from_slice(b"WEBPVP8L");
        vp8l.extend_from_slice(&[0; 4]);
        vp8l.push(0x2f);
        let packed = (511u32) | (511u32 << 14);
        vp8l.extend_from_slice(&packed.to_le_bytes());
        assert_eq!(image_size(&vp8l), Some((512, 512)), "VP8L 14 位宽高各 +1");

        let mut vp8 = b"RIFF".to_vec();
        vp8.extend_from_slice(&[0; 4]);
        vp8.extend_from_slice(b"WEBPVP8 ");
        vp8.extend_from_slice(&[0; 4]);
        vp8.extend_from_slice(&[0; 3]); // 帧标签
        vp8.extend_from_slice(&[0x9d, 0x01, 0x2a]); // 同步码
        vp8.extend_from_slice(&320u16.to_le_bytes());
        vp8.extend_from_slice(&240u16.to_le_bytes());
        assert_eq!(image_size(&vp8), Some((320, 240)));
    }

    #[test]
    fn image_size_scans_jpeg_segments() {
        // SOI + APP0(JFIF) + SOF0 + 之后的数据：须跳过 APP 段读到帧头
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend_from_slice(&[0xff, 0xe0, 0x00, 0x06]); // APP0 段长 6
        jpeg.extend_from_slice(b"JFIF\0");
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08]); // SOF0：精度 8
        jpeg.extend_from_slice(&1080u16.to_be_bytes());
        jpeg.extend_from_slice(&1920u16.to_be_bytes());
        jpeg.extend_from_slice(&[0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        jpeg.extend_from_slice(&[0xff, 0xda, 0x00, 0x08]); // SOS
        assert_eq!(image_size(&jpeg), Some((1920, 1080)));
    }

    #[test]
    fn image_size_rejects_unknown_and_truncated() {
        assert_eq!(image_size(b"not an image at all"), None);
        assert_eq!(image_size(&[]), None);
        assert_eq!(image_size(&[0x89, b'P', b'N', b'G']), None, "签名不全");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(image_size(&png), None, "零尺寸按认不出处理");
    }
}
