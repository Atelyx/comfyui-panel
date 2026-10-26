//! 结果落库：把图片字节写进当前仓库的落库目录。
//!
//! 两条写入路径：本地副本就绪时经宿主 `vault` 导入面按路径复制（字节不进 RPC，本地仓库无大小
//! 上限、协作空间上限也高于二进制写面）；副本缺失时回源取字节走二进制写面（受单次载荷上限约束）。
//! 本地仓库与协作空间同构，插件不感知容器类型。落库目录、文件名与重名避让都在这里定口径。

use std::path::Path;

use atelyx_plugin_sdk::Context;

use crate::comfy::types::ImageRef;

/// 文件名里不允许出现的字符（Windows 保留 + NUL），命中一律替换为 `_`。
const UNSAFE_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// 默认落库目录（相对仓库根）；设置里的落库目录留空时用这个。
const DEFAULT_FOLDER: &str = "comfyui";

/// 重名避让的最大尝试次数（原名 + `-2` … `-100`）。
const MAX_NAME_ATTEMPTS: u32 = 100;

/// 宿主仓库二进制写面单次上限；超过会在分派处被拒，本地先给出可操作的替代方案。
const VAULT_WRITE_LIMIT: usize = 32 * 1024 * 1024;

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

/// 净化单个路径段：非法字符（含分隔符、NUL）替换为 `_`、去前导点并 trim（宿主仓库面不开放隐藏段）。
fn sanitize_chars(seg: &str) -> String {
    seg.chars()
        .map(|c| if UNSAFE_CHARS.contains(&c) || c == '\0' { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
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

/// 重名避让：首次用原名，之后在扩展名前追加 `-2`、`-3`…（无扩展名时追加在末尾）。
fn numbered_name(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    let suffix = format!("-{}", attempt + 1);
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{}{}{}", &name[..dot], suffix, &name[dot..]),
        _ => format!("{name}{suffix}"),
    }
}

/// 是否超过仓库写面上限：等于上限仍可写，仅超出拒绝。
fn exceeds_write_limit(len: usize) -> bool {
    len > VAULT_WRITE_LIMIT
}

/// 写入 `{folder}/comfyui-{safe}`（重名自动加序号），返回相对仓库根的路径（斜杠分隔）。
pub async fn save(ctx: &Context, archive_folder: &str, image_ref: &ImageRef, bytes: &[u8]) -> Result<String, String> {
    if exceeds_write_limit(bytes.len()) {
        return Err(format!(
            "图片超过 {} MiB，无法存入仓库，请改用另存为",
            VAULT_WRITE_LIMIT / (1024 * 1024)
        ));
    }
    let folder = folder_of(archive_folder);
    let name = archive_name(&image_ref.filename);
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let relative = format!("{folder}/{}", numbered_name(&name, attempt));
        let exists = ctx.vault_exists(&relative).await.map_err(|e| format!("检查仓库文件失败：{e}"))?;
        if exists {
            continue;
        }
        ctx.vault_write_binary(&relative, bytes).await.map_err(|e| format!("写入仓库失败：{e}"))?;
        return Ok(relative);
    }
    Err(format!("同名文件过多，无法在 {folder} 下生成新文件名"))
}

/// 经私有目录导入落库：源是插件私有目录里的副本文件（绝对路径），字节由宿主直接搬运，
/// 不受二进制写面的载荷上限约束。查重与重名避让口径同 [`save`]；导入是复制语义，源文件保留。
pub async fn save_from_private(
    ctx: &Context,
    archive_folder: &str,
    image_ref: &ImageRef,
    source: &Path,
) -> Result<String, String> {
    let folder = folder_of(archive_folder);
    let name = archive_name(&image_ref.filename);
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let relative = format!("{folder}/{}", numbered_name(&name, attempt));
        let exists = ctx.vault_exists(&relative).await.map_err(|e| format!("检查仓库文件失败：{e}"))?;
        if exists {
            continue;
        }
        let source = source.to_string_lossy().into_owned();
        ctx.vault_import_file(&relative, &source).await.map_err(|e| format!("导入仓库失败：{e}"))?;
        return Ok(relative);
    }
    Err(format!("同名文件过多，无法在 {folder} 下生成新文件名"))
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
        assert_eq!(folder_of(".hidden/out"), "hidden/out", "隐藏段去掉前导点，避免宿主拒绝");
    }

    #[test]
    fn folder_blocks_traversal_keeps_nested() {
        assert_eq!(folder_of("../../evil"), "evil", "越界段被剔除，结果仍在仓库根内");
        assert_eq!(folder_of("a/../b"), "a/b", "中间 .. 被丢弃");
        assert_eq!(folder_of("out/2026"), "out/2026", "合法多级子目录保留");
        assert_eq!(folder_of("../.."), "comfyui", "全是 .. 段则回落默认目录");
    }

    #[test]
    fn write_limit_rejects_only_above_cap() {
        assert!(!exceeds_write_limit(0));
        assert!(!exceeds_write_limit(VAULT_WRITE_LIMIT), "恰好等于上限仍可写");
        assert!(exceeds_write_limit(VAULT_WRITE_LIMIT + 1));
    }

    #[test]
    fn duplicate_name_appends_suffix_before_extension() {
        assert_eq!(numbered_name("comfyui-a.png", 0), "comfyui-a.png");
        assert_eq!(numbered_name("comfyui-a.png", 1), "comfyui-a-2.png");
        assert_eq!(numbered_name("comfyui-a.tar.gz", 2), "comfyui-a.tar-3.gz", "只认最后一个点后的扩展名");
        assert_eq!(numbered_name("comfyui-a", 1), "comfyui-a-2", "无扩展名追加在末尾");
    }
}
