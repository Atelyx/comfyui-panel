//! ComfyUI 的 REST 封装：拼路径、发请求、解 JSON、把失败翻成可读原因。全部经 reqwest 直连。
//!
//! 与 ComfyUI 通信是本插件唯一的外联，且一律发往用户配置的地址——不做任何联网上传。

use std::time::Duration;

use serde_json::Value;

use crate::comfy::types::ImageRef;

/// 单次 JSON 请求超时：本机服务，慢于此即视作不可用。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
/// 结果图字节超时：可达数 MB，给足传输时间。
const BINARY_TIMEOUT: Duration = Duration::from_secs(20);
/// 节点定义体积较大，给宽松超时。
const OBJECT_INFO_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ComfyClient {
    base: String,
    http: reqwest::Client,
}

impl ComfyClient {
    /// 以当前地址建客户端；地址随设置变更，调用方在每次请求前用新 `base` 重建。
    pub fn new(base: String, http: reqwest::Client) -> Self {
        ComfyClient { base, http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub fn socket_url(&self, client_id: &str) -> String {
        format!("{}/ws?clientId={}", self.ws_base(), encode_uri_component(client_id))
    }

    fn ws_base(&self) -> String {
        match self.base.strip_prefix("http") {
            Some(rest) => format!("ws{rest}"),
            None => self.base.clone(),
        }
    }

    /// 探测服务是否可用（`/system_stats` 返回 2xx）。
    pub async fn probe(&self) -> bool {
        match self.http.get(self.url("/system_stats")).timeout(REQUEST_TIMEOUT).send().await {
            Ok(res) => res.status().is_success(),
            Err(_) => false,
        }
    }

    async fn get_json(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let res = self
            .http
            .get(self.url(path))
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
        if status != 200 {
            return Err(extract_error(status, &body));
        }
        serde_json::from_str(&body).map_err(|e| format!("响应非 JSON：{e}"))
    }

    async fn post_json(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let res = self
            .http
            .post(self.url(path))
            .timeout(REQUEST_TIMEOUT)
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
        if status != 200 {
            return Err(extract_error(status, &body));
        }
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        // 写接口偶尔返回非 JSON（空/纯文本），解析失败按无返回处理而非报错
        Ok(serde_json::from_str(&body).unwrap_or(Value::Null))
    }

    pub async fn system_stats(&self) -> Result<Value, String> {
        self.get_json("/system_stats", REQUEST_TIMEOUT).await
    }

    pub async fn object_info(&self) -> Result<Value, String> {
        self.get_json("/object_info", OBJECT_INFO_TIMEOUT).await
    }

    pub async fn queue(&self) -> Result<Value, String> {
        self.get_json("/queue", REQUEST_TIMEOUT).await
    }

    pub async fn history(&self, max_items: u32) -> Result<Value, String> {
        self.get_json(&format!("/history?max_items={max_items}"), REQUEST_TIMEOUT).await
    }

    /// 提交任务：`client_id` 与 WS 一致，服务端据此把进度推回本面板；`origin_id` 非空进 extra_data。
    pub async fn submit(&self, prompt: &Value, client_id: &str, origin_id: &str) -> Result<Value, String> {
        let mut body = serde_json::Map::new();
        body.insert("prompt".into(), prompt.clone());
        body.insert("client_id".into(), Value::String(client_id.to_string()));
        if !origin_id.is_empty() {
            body.insert("extra_data".into(), Value::Object(serde_json::Map::from_iter([(
                "atelyx_origin".into(),
                Value::String(origin_id.to_string()),
            )])));
        }
        self.post_json("/prompt", &Value::Object(body)).await
    }

    /// 定向中断某任务（无参全局中断会波及同服务其他来源，故只支持按 id）。
    pub async fn interrupt_prompt(&self, prompt_id: &str) -> Result<(), String> {
        let body = Value::Object(serde_json::Map::from_iter([(
            "prompt_id".into(),
            Value::String(prompt_id.to_string()),
        )]));
        self.post_json("/interrupt", &body).await.map(|_| ())
    }

    pub async fn free(&self, unload_models: bool, free_memory: bool) -> Result<(), String> {
        let mut body = serde_json::Map::new();
        body.insert("unload_models".into(), Value::Bool(unload_models));
        body.insert("free_memory".into(), Value::Bool(free_memory));
        self.post_json("/free", &Value::Object(body)).await.map(|_| ())
    }

    /// 取图片字节（结果图落盘副本、另存为、落库共用）。
    pub async fn image_bytes(&self, ref_: &ImageRef) -> Result<Vec<u8>, String> {
        let url = self.url(&format!(
            "/view?filename={}&subfolder={}&type={}",
            encode_uri_component(&ref_.filename),
            encode_uri_component(&ref_.subfolder),
            encode_uri_component(&ref_.type_),
        ));
        let res = self
            .http
            .get(url)
            .timeout(BINARY_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("取图失败：{e}"))?;
        let status = res.status().as_u16();
        if status != 200 {
            return Err(format!("取图失败：HTTP {status}"));
        }
        res.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("读取图片失败：{e}"))
    }

    /// 上传图片到 ComfyUI 的 input 目录（multipart），返回引用直接赋给 LoadImage 的 image 字段。
    pub async fn upload_image(&self, filename: &str, bytes: Vec<u8>) -> Result<ImageRef, String> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mime_of(filename))
            .map_err(|e| format!("准备上传失败：{e}"))?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("overwrite", "true");
        let res = self
            .http
            .post(self.url("/upload/image"))
            .timeout(BINARY_TIMEOUT)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("上传失败：{e}"))?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
        if status >= 300 {
            return Err(extract_error(status, &body));
        }
        let parsed: Value = serde_json::from_str(&body).map_err(|e| format!("上传响应非 JSON：{e}"))?;
        Ok(ImageRef {
            filename: parsed.get("name").and_then(Value::as_str).unwrap_or(filename).to_string(),
            subfolder: parsed.get("subfolder").and_then(Value::as_str).unwrap_or("").to_string(),
            type_: parsed.get("type").and_then(Value::as_str).unwrap_or("input").to_string(),
        })
    }

    // ---------- userdata：工作流文件读写经 ComfyUI 的用户目录接口，不落插件本地存储 ----------

    /// 列用户目录（`full_info` 返回 path/size/modified/created，modified 为毫秒）；条目 `path` 相对 `dir`。
    pub async fn list_user_dir(&self, dir: &str) -> Result<Vec<UserDataEntry>, String> {
        let path = format!("/userdata?dir={}&recurse=true&full_info=true", encode_uri_component(dir));
        let value = self.get_json(&path, REQUEST_TIMEOUT).await?;
        Ok(value.as_array().cloned().unwrap_or_default().into_iter().filter_map(UserDataEntry::from_value).collect())
    }

    /// 读用户文件原文（工作流 JSON）。路径整体百分号编码（斜杠也编码），与服务端解引用一致。
    pub async fn read_user_file(&self, path: &str) -> Result<String, String> {
        let url = self.url(&format!("/userdata/{}", encode_uri_component(path)));
        let res = self
            .http
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
        if status != 200 {
            return Err(extract_error(status, &body));
        }
        Ok(body)
    }

    /// 写用户文件：正文原样落盘，服务端默认覆盖、自动建父目录。
    pub async fn write_user_file(&self, path: &str, content: &str) -> Result<(), String> {
        let url = self.url(&format!("/userdata/{}", encode_uri_component(path)));
        let res = self
            .http
            .post(url)
            .timeout(REQUEST_TIMEOUT)
            .header("Content-Type", "application/json")
            .body(content.to_string())
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = res.status().as_u16();
        if status != 200 && status != 204 {
            let body = res.text().await.unwrap_or_default();
            return Err(extract_error(status, &body));
        }
        Ok(())
    }
}

/// userdata 列目录条目：`path` 相对本次查询的 `dir`（不带该前缀），modified 毫秒。
pub struct UserDataEntry {
    pub path: String,
    pub size: f64,
    pub modified: f64,
}

impl UserDataEntry {
    fn from_value(value: Value) -> Option<Self> {
        let path = value.get("path")?.as_str()?.to_string();
        Some(UserDataEntry {
            path,
            size: value.get("size").and_then(Value::as_f64).unwrap_or(0.0),
            modified: value.get("modified").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }
}

/// 服务端错误体形状不统一，尽力取出一句可读原因。
fn extract_error(status: u16, body: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(body) {
        let error = parsed.get("error");
        let message = error
            .and_then(|e| e.get("message").and_then(Value::as_str))
            .or_else(|| error.and_then(|e| e.get("details").and_then(Value::as_str)));
        if let Some(message) = message {
            let has_node_errors = parsed
                .get("node_errors")
                .and_then(Value::as_object)
                .is_some_and(|m| !m.is_empty());
            if has_node_errors {
                let detail = parsed.get("node_errors").map(|v| v.to_string()).unwrap_or_default();
                return format!("{message}（节点报错：{}）", truncate_chars(&detail, 400));
            }
            return message.to_string();
        }
    }
    let text = collapse_whitespace(body.trim());
    if text.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}：{}", truncate_chars(&text, 300))
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按字符（非字节）截断，避免中文/traceback 截在多字节中间。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// 按扩展名给 multipart 部件定 MIME；ComfyUI 以 png 为主，未知按 png 兜底。
fn mime_of(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// 等价 JS `encodeURIComponent`：除 `A-Za-z0-9 - _ . ! ~ * ' ( )` 外全部按 UTF-8 百分号编码（含 `/`）。
fn encode_uri_component(input: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()";
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if UNRESERVED.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
