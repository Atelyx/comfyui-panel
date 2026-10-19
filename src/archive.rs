//! 结果落库：把图片字节直接写进当前仓库的落库目录。
//!
//! 用 `std::fs` 写入仓库根下的子目录：插件本就能使用 `std::fs`，权限声明仅作透明度。
//! 仓库根与类型来自宿主 `vault:switch` 事件：个人仓库给出 root，协作空间不给路径、落库禁用。

use std::path::PathBuf;

use crate::comfy::types::ImageRef;

/// 文件名里不允许出现的字符（Windows 保留 + NUL），命中一律替换为 `_`。
const UNSAFE_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// 默认落库目录（相对仓库根）；设置里的落库目录留空时用这个。
const DEFAULT_FOLDER: &str = "comfyui";

/// 取安全文件名：非法字符替换为 `_`、去前导点、trim；空则按时间戳兜底。
/// 分隔符 `/`、`\` 一并替换，结果不含路径分隔，无法越出落库目录。
fn safe_file_name(file_name: &str) -> String {
    let cleaned: String = file_name
        .chars()
        .map(|c| if UNSAFE_CHARS.contains(&c) || c == '\0' { '_' } else { c })
        .collect::<String>()
        .trim_start_matches('.')
        .trim()
        .to_string();
    if cleaned.is_empty() {
        format!("comfyui-{}.png", chrono::Utc::now().timestamp_millis())
    } else {
        cleaned
    }
}

/// 落库文件名：加 `comfyui-` 前缀，避免与用户自有文件重名。
fn archive_name(file_name: &str) -> String {
    format!("comfyui-{}", safe_file_name(file_name))
}

/// 另存为的默认文件名：取引用路径末段净化后原样使用（不加落库前缀）；无扩展名时按 `.png` 补全，
/// 免得保存对话框交出无扩展名文件。
pub fn save_as_name(file_name: &str) -> String {
    let leaf = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let name = safe_file_name(leaf);
    if name.contains('.') {
        name
    } else {
        format!("{name}.png")
    }
}

/// 净化单个路径段：非法字符（含分隔符、NUL）替换为 `_` 并 trim。
fn sanitize_chars(seg: &str) -> String {
    seg.chars()
        .map(|c| if UNSAFE_CHARS.contains(&c) || c == '\0' { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// 落库目录（相对仓库根）：留空用默认目录。按分隔符拆段后剔除空段与 `.`/`..` 越界段、逐段净化字符，
/// 保证结果始终落在仓库根之内——用户可指定多级子目录，但不能借 `..` 写到根外。
fn folder_of(archive_folder: &str) -> String {
    let segments: Vec<String> = archive_folder
        .split(['/', '\\'])
        .filter(|seg| !matches!(seg.trim(), "" | "." | ".."))
        .map(sanitize_chars)
        .filter(|seg| !seg.is_empty())
        .collect();
    if segments.is_empty() {
        DEFAULT_FOLDER.to_string()
    } else {
        segments.join("/")
    }
}

/// 写入图片到 `{root}/{folder}/comfyui-{safe}`，返回相对仓库根的路径（斜杠分隔）。
pub fn save_to_repo(root: &str, archive_folder: &str, image_ref: &ImageRef, bytes: &[u8]) -> Result<String, String> {
    let folder = folder_of(archive_folder);
    let name = archive_name(&image_ref.filename);
    let dir = PathBuf::from(root).join(&folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败：{e}"))?;
    std::fs::write(dir.join(&name), bytes).map_err(|e| format!("写入文件失败：{e}"))?;
    Ok(format!("{folder}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_unsafe_and_leading_dots() {
        assert_eq!(safe_file_name("a/b:c*d.png"), "a_b_c_d.png");
        assert_eq!(safe_file_name("..hidden"), "hidden");
        let fallback = safe_file_name("   ");
        assert!(fallback.starts_with("comfyui-") && fallback.ends_with(".png"), "全非法字符走时间戳兜底");
    }

    #[test]
    fn archive_prefix_applied() {
        assert_eq!(archive_name("pic.png"), "comfyui-pic.png");
    }

    #[test]
    fn save_as_name_uses_leaf_and_keeps_extension() {
        assert_eq!(save_as_name("output/a.png"), "a.png");
        assert_eq!(save_as_name("sub\\b.webp"), "b.webp", "反斜杠同样取末段");
        assert_eq!(save_as_name("output/no ext"), "no ext.png", "净化后无扩展名则补 png");
        assert_eq!(save_as_name("output/a:b.png"), "a_b.png", "非法字符仍替换");
        let fallback = save_as_name("output/");
        assert!(fallback.starts_with("comfyui-") && fallback.ends_with(".png"), "空末段走时间戳兜底：{fallback}");
    }

    #[test]
    fn folder_default_and_trim() {
        assert_eq!(folder_of(""), "comfyui");
        assert_eq!(folder_of("  /out/  "), "out");
    }

    #[test]
    fn folder_blocks_traversal_keeps_nested() {
        assert_eq!(folder_of("../../evil"), "evil", "越界段被剔除，结果仍在仓库根内");
        assert_eq!(folder_of("a/../b"), "a/b", "中间 .. 被丢弃");
        assert_eq!(folder_of("out/2026"), "out/2026", "合法多级子目录保留");
        assert_eq!(folder_of("../.."), "comfyui", "全是 .. 段则回落默认目录");
    }
}
