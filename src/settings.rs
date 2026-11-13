//! 插件设置：字段定义、磁盘值收敛与地址/解释器推导。纯数据层，落盘由 app 统一写整份状态信封。
//!
//! 磁盘读取一律走 [`from_value`] 的逐字段兜底，不用 serde 严格反序列化：手改或半写的脏值
//! （非法端口、错类型）不该让整份设置解析失败，也不该把插件带偏。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 进程接管方式：`managed` = 插件负责启停；`external` = 只检测连接，进程归用户管。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessMode {
    #[default]
    Managed,
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComfySettings {
    /// ComfyUI 监听地址（默认本机）。
    pub host: String,
    /// 监听端口：既是连接目标，也是托管启动时传给 ComfyUI 的端口。
    pub port: u16,
    pub process_mode: ProcessMode,
    /// 安装目录：托管启动的工作目录，也是推导 venv Python 路径的依据。
    pub comfy_dir: String,
    /// 留空则用安装目录下 venv 里的 python。
    pub python_path: String,
    /// 附加启动参数，原样追加在末尾。
    pub extra_args: String,
    /// 落库子目录（相对仓库根），留空则用仓库根下的 `comfyui/`。
    pub archive_folder: String,
}

impl Default for ComfySettings {
    fn default() -> Self {
        ComfySettings {
            host: "127.0.0.1".into(),
            port: 8188,
            process_mode: ProcessMode::Managed,
            comfy_dir: String::new(),
            python_path: String::new(),
            extra_args: String::new(),
            archive_folder: String::new(),
        }
    }
}

/// 逐字段收敛磁盘值；缺字段或非预期类型都回落到默认。
pub fn from_value(raw: Option<&Value>) -> ComfySettings {
    let default = ComfySettings::default();
    let Some(map) = raw.and_then(Value::as_object) else {
        return default;
    };
    let string = |key: &str| map.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let port = map
        .get("port")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse::<u64>().ok()))
        })
        .filter(|p| (1..=65535).contains(p))
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(default.port);
    ComfySettings {
        host: {
            let host = string("host").trim().to_string();
            if host.is_empty() {
                default.host
            } else {
                // 合法值收敛成可拼 URL 的形态；非法值原样保留，让设置页显示出来供用户修正。
                normalize_host(&host).unwrap_or(host)
            }
        },
        port,
        // 归一大小写与空白：手写成 `External` 时若回落到托管模式，插件会在同一端口再启一个服务进程。
        process_mode: if string("processMode").trim().eq_ignore_ascii_case("external") {
            ProcessMode::External
        } else {
            ProcessMode::Managed
        },
        comfy_dir: string("comfyDir"),
        python_path: string("pythonPath"),
        extra_args: string("extraArgs"),
        archive_folder: string("archiveFolder"),
    }
}

/// 拼 HTTP 基址（末尾不带斜杠）。
pub fn base_url(settings: &ComfySettings) -> String {
    format!("http://{}:{}", settings.host, settings.port)
}

/// 收敛用户输入的监听地址：去掉 scheme 与尾斜杠；拒绝路径与内嵌端口；
/// IPv6 字面量补方括号（base_url 直接拼 host:port）。
pub fn normalize_host(text: &str) -> Result<String, String> {
    let mut host = text.trim();
    if let Some(rest) = host.strip_prefix("http://").or_else(|| host.strip_prefix("https://")) {
        host = rest;
    }
    let host = host.trim_end_matches('/').trim();
    if host.is_empty() {
        return Err("地址不能为空".to_string());
    }
    if host.contains('/') {
        return Err("地址只填主机名或 IP，不要带路径".to_string());
    }
    if host.chars().any(char::is_whitespace) {
        return Err("地址不能包含空白".to_string());
    }
    // 只放行主机名/IP 需要的字符：地址还会拼进浏览器命令，不能带 shell 元字符。
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '%'))
    {
        return Err("地址包含非法字符".to_string());
    }
    if host.starts_with('[') && host.ends_with(']') {
        let inner = &host[1..host.len() - 1];
        if inner.is_empty() || !inner.contains(':') {
            return Err("IPv6 地址格式无效".to_string());
        }
        return Ok(host.to_string());
    }
    if host.matches(':').count() == 1 {
        return Err("端口请在端口框填写，地址里不要带端口".to_string());
    }
    if host.contains(':') {
        return Ok(format!("[{host}]"));
    }
    Ok(host.to_string())
}

/// 用户填了就用填的，否则指向安装目录下 venv 的 python；两者皆空返回空串（上层据此报缺配置）。
pub fn resolve_python(settings: &ComfySettings) -> String {
    let explicit = settings.python_path.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let dir = settings.comfy_dir.trim().trim_end_matches(['/', '\\']);
    if dir.is_empty() {
        return String::new();
    }
    // venv 内解释器位置随平台而异：Windows 在 `venv/Scripts/python.exe`，类 Unix 在 `venv/bin/python`。
    if cfg!(windows) {
        format!("{dir}/venv/Scripts/python.exe")
    } else {
        format!("{dir}/venv/bin/python")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_and_dirty_fields_fall_back() {
        let s = from_value(Some(&json!({ "port": 70000, "host": "  " })));
        assert_eq!(s.port, 8188, "越界端口回落默认");
        assert_eq!(s.host, "127.0.0.1", "空白 host 回落默认");
    }

    #[test]
    fn roundtrip_keeps_values() {
        let s = ComfySettings { port: 9000, process_mode: ProcessMode::External, ..Default::default() };
        let back = from_value(Some(&serde_json::to_value(&s).unwrap()));
        assert_eq!(back, s);
    }

    #[test]
    fn process_mode_is_case_insensitive() {
        assert_eq!(from_value(Some(&json!({ "processMode": "External" }))).process_mode, ProcessMode::External);
        assert_eq!(from_value(Some(&json!({ "processMode": " external " }))).process_mode, ProcessMode::External);
        assert_eq!(from_value(Some(&json!({ "processMode": "managed" }))).process_mode, ProcessMode::Managed);
        assert_eq!(from_value(Some(&json!({ "processMode": 1 }))).process_mode, ProcessMode::Managed);
    }

    #[test]
    fn host_normalization_strips_scheme_and_rejects_port_or_path() {
        assert_eq!(normalize_host("http://127.0.0.1/"), Ok("127.0.0.1".to_string()));
        assert_eq!(normalize_host("::1"), Ok("[::1]".to_string()));
        assert_eq!(normalize_host("[::1]"), Ok("[::1]".to_string()));
        assert!(normalize_host("localhost:8188").is_err());
        assert!(normalize_host("http://host/api").is_err());
        assert!(normalize_host("   ").is_err());
        assert!(normalize_host("x&calc").is_err(), "地址不能带 shell 元字符");
        assert!(normalize_host("host;rm").is_err());
    }

    #[test]
    fn port_string_is_accepted() {
        assert_eq!(from_value(Some(&json!({"port": "9000"}))).port, 9000);
        assert_eq!(from_value(Some(&json!({"port": "70000"}))).port, 8188);
    }

    #[test]
    fn resolve_python_falls_back_to_venv() {
        let venv_python = if cfg!(windows) { "E:/ComfyUI/venv/Scripts/python.exe" } else { "E:/ComfyUI/venv/bin/python" };
        let s = ComfySettings { comfy_dir: "E:/ComfyUI/".into(), ..Default::default() };
        assert_eq!(resolve_python(&s), venv_python);
        let s2 = ComfySettings { python_path: "C:/py/python.exe".into(), ..Default::default() };
        assert_eq!(resolve_python(&s2), "C:/py/python.exe");
    }
}
