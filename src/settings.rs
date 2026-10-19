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
#[serde(rename_all = "camelCase", default)]
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
    /// 随插件装载自动托管启动（仅托管模式生效）。
    pub auto_start: bool,
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
            auto_start: false,
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
    let boolean = |key: &str, fallback: bool| map.get(key).and_then(Value::as_bool).unwrap_or(fallback);
    let port = map
        .get("port")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p))
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(default.port);
    ComfySettings {
        host: {
            let host = string("host").trim().to_string();
            if host.is_empty() { default.host } else { host }
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
        auto_start: boolean("autoStart", default.auto_start),
        archive_folder: string("archiveFolder"),
    }
}

/// 拼 HTTP 基址（末尾不带斜杠）。
pub fn base_url(settings: &ComfySettings) -> String {
    format!("http://{}:{}", settings.host, settings.port)
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
        let s = from_value(Some(&json!({ "port": 70000, "host": "  ", "autoStart": "yes" })));
        assert_eq!(s.port, 8188, "越界端口回落默认");
        assert_eq!(s.host, "127.0.0.1", "空白 host 回落默认");
        assert!(!s.auto_start, "错类型布尔回落默认");
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
    fn resolve_python_falls_back_to_venv() {
        let venv_python = if cfg!(windows) { "E:/ComfyUI/venv/Scripts/python.exe" } else { "E:/ComfyUI/venv/bin/python" };
        let s = ComfySettings { comfy_dir: "E:/ComfyUI/".into(), ..Default::default() };
        assert_eq!(resolve_python(&s), venv_python);
        let s2 = ComfySettings { python_path: "C:/py/python.exe".into(), ..Default::default() };
        assert_eq!(resolve_python(&s2), "C:/py/python.exe");
    }
}
