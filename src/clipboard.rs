//! 剪贴板取参考图：宿主 `clipboard` 能力面只有文本，图片只能由本进程直读系统剪贴板。
//!
//! 读到的位图是 RGBA8 紧密排列的裸字节，编码成 PNG 后交给 ComfyUI 的上传接口——
//! 编码/剪贴板访问是阻塞调用，一律放 `spawn_blocking`，不占 tokio 连接泵线程。

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder};

/// 读系统剪贴板图片并编码为 PNG 字节；没有图片或平台取不到时回可读原因。
pub async fn read_png_bytes() -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(read_png_blocking)
        .await
        .map_err(|e| format!("读取被打断：{e}"))?
}

/// 粘贴件的上传文件名：`clipboard-{毫秒}.png`，与用户自选文件区分。
pub fn file_name() -> String {
    format!("clipboard-{}.png", chrono::Utc::now().timestamp_millis())
}

fn read_png_blocking() -> Result<Vec<u8>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用：{e}"))?;
    let image = clipboard
        .get_image()
        .map_err(|e| format!("没有可读取的图片：{e}"))?;
    let width = u32::try_from(image.width).map_err(|_| "图片尺寸非法".to_string())?;
    let height = u32::try_from(image.height).map_err(|_| "图片尺寸非法".to_string())?;
    if width == 0 || height == 0 || image.bytes.len() < width as usize * height as usize * 4 {
        return Err("图片内容为空".to_string());
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(image.bytes.as_ref(), width, height, ColorType::Rgba8.into())
        .map_err(|e| format!("图片编码失败：{e}"))?;
    Ok(png)
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
