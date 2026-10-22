//! 剪贴板取参考图：宿主 `clipboard` 能力面直接返回 PNG 字节与像素尺寸，本模块只做解码与命名。

use atelyx_plugin_sdk::Context;
use base64::Engine;

/// 读系统剪贴板图片的 PNG 字节；没有图片或宿主读失败时回可读原因。
pub async fn read_png_bytes(ctx: &Context) -> Result<Vec<u8>, String> {
    let image = ctx
        .clipboard_read_image()
        .await
        .map_err(|e| format!("读取剪贴板失败：{e}"))?
        .ok_or_else(|| "剪贴板里没有图片".to_string())?;
    if image.width == 0 || image.height == 0 {
        return Err("图片内容为空".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(image.base64.as_bytes())
        .map_err(|e| format!("图片数据解码失败：{e}"))
}

/// 粘贴件的上传文件名：`clipboard-{毫秒}.png`，与用户自选文件区分。
pub fn file_name() -> String {
    format!("clipboard-{}.png", chrono::Utc::now().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_file_is_png_named_by_timestamp() {
        let name = file_name();
        assert!(name.starts_with("clipboard-") && name.ends_with(".png"), "实际：{name}");
        assert!(name["clipboard-".len()..name.len() - ".png".len()].chars().all(|c| c.is_ascii_digit()));
    }
}
