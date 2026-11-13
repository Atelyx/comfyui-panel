//! 托管启动的命令组装与解释器定位。
//! 参数经宿主进程面逐项直传操作系统（不经 shell 转述），附加参数里的引号与元字符按字面到达 ComfyUI。
//! 启动程序 = Python 解释器本身，其余全为程序参数；工作目录取 ComfyUI 安装目录。

use std::collections::BTreeMap;

use crate::settings::{resolve_python, ComfySettings};

/// ComfyUI 主脚本名（相对安装目录）。
const MAIN_SCRIPT: &str = "main.py";

/// 组装启动参数（不含解释器）：`main.py --listen <地址> --port <端口>` 再追加用户附加参数。
/// 监听地址取设置里的 host，让探测目标与实际监听地址一致；host 为空时交给 ComfyUI 用默认回环。
/// 解释器为空即配置不全，返回 Err 由调用方给出可操作提示。
pub fn build_start_args(settings: &ComfySettings) -> Result<Vec<String>, String> {
    if resolve_python(settings).is_empty() {
        return Err("未配置 ComfyUI 目录或 Python 路径".into());
    }
    let mut args = vec![MAIN_SCRIPT.to_string()];
    let host = settings.host.trim();
    // 设置里的 host 为拼 URL 可能带 IPv6 方括号；传给 --listen 时要去掉。
    let listen = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host);
    if !listen.is_empty() {
        args.extend(["--listen".to_string(), listen.to_string()]);
    }
    args.extend(["--port".to_string(), settings.port.to_string()]);
    args.extend(split_args(&settings.extra_args));
    Ok(args)
}

/// 按命令行习惯切分附加参数：空白分隔，双引号内保留空白并去掉引号。
///
/// 不能简单按空白切：参数值常是含空格的路径，如 `--extra-model-paths-config "E:/my models/x.yaml"`，
/// 切开会把它拆成多段，最终传给服务端的是错路径。
pub fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    for ch in text.chars() {
        if ch == '"' {
            quoted = !quoted;
            started = true;
            continue;
        }
        if !quoted && ch.is_whitespace() {
            if started {
                out.push(std::mem::take(&mut current));
            }
            started = false;
            continue;
        }
        current.push(ch);
        started = true;
    }
    if started {
        out.push(current);
    }
    out
}

/// 含空白或引号时加引号（仅供展示的命令行预览用，执行不走它）。
pub fn shell_quote(value: &str) -> String {
    if value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// 供界面展示的命令行（解释器 + 参数，逐项加引号可读）。
pub fn describe_start_command(settings: &ComfySettings) -> Result<String, String> {
    let python = resolve_python(settings);
    if python.is_empty() {
        return Err("未配置 ComfyUI 目录或 Python 路径".into());
    }
    let args = build_start_args(settings)?;
    let mut tokens = Vec::with_capacity(args.len() + 1);
    tokens.push(shell_quote(&python));
    tokens.extend(args.iter().map(|a| shell_quote(a)));
    Ok(tokens.join(" "))
}

/// 更新命令：(程序, 参数, 工作目录, 环境变量)。
pub type UpdateCommand = (String, Vec<String>, Option<String>, BTreeMap<String, String>);

/// 一键更新命令：在安装目录执行 `git pull --ff-only`，并禁掉终端凭据提示——
/// 宿主起的进程没有可交互终端，等待输入会把更新卡到超时。
pub fn update_command(settings: &ComfySettings) -> Result<UpdateCommand, String> {
    let dir = settings.comfy_dir.trim();
    if dir.is_empty() {
        return Err("未配置 ComfyUI 安装目录".to_string());
    }
    let args = vec![
        "-C".to_string(),
        dir.to_string(),
        "pull".to_string(),
        "--ff-only".to_string(),
    ];
    let env = BTreeMap::from([("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())]);
    Ok(("git".to_string(), args, None, env))
}

/// 依赖更新命令：用安装目录的 Python 执行 `-m pip install -r requirements.txt`。
/// 与上游手动更新步骤一致（不带 --upgrade，只补齐 requirements 声明的缺口）。
pub fn dependency_command(settings: &ComfySettings) -> Result<UpdateCommand, String> {
    let python = resolve_python(settings);
    if python.is_empty() {
        return Err("未配置 Python 路径或安装目录，无法更新依赖".to_string());
    }
    let dir = settings.comfy_dir.trim();
    if dir.is_empty() {
        return Err("未配置 ComfyUI 安装目录".to_string());
    }
    let requirements = std::path::Path::new(dir).join("requirements.txt");
    if !requirements.is_file() {
        return Err(format!("安装目录下没有 requirements.txt：{}", requirements.display()));
    }
    let args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "install".to_string(),
        "-r".to_string(),
        requirements.to_string_lossy().into_owned(),
    ];
    let env = BTreeMap::from([("PIP_DISABLE_PIP_VERSION_CHECK".to_string(), "1".to_string())]);
    Ok((python, args, Some(dir.to_string()), env))
}

/// 启动命令三元组：(解释器程序, 参数, 工作目录)。工作目录取安装目录，空则交宿主按默认。
pub fn start_command(settings: &ComfySettings) -> Result<(String, Vec<String>, Option<String>), String> {
    let python = resolve_python(settings);
    if python.is_empty() {
        return Err("未配置 ComfyUI 目录或 Python 路径".into());
    }
    let args = build_start_args(settings)?;
    let cwd = {
        let dir = settings.comfy_dir.trim();
        (!dir.is_empty()).then(|| dir.to_string())
    };
    Ok((python, args, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ComfySettings;

    fn with_comfy(extra: &str) -> ComfySettings {
        ComfySettings {
            comfy_dir: "E:/ComfyUI".into(),
            port: 8188,
            extra_args: extra.into(),
            ..Default::default()
        }
    }

    #[test]
    fn args_start_at_main_script_then_listen_port_and_extra() {
        let args = build_start_args(&with_comfy("--cpu --extra-model-paths-config \"E:/my models/x.yaml\"")).unwrap();
        assert_eq!(
            args,
            vec!["main.py", "--listen", "127.0.0.1", "--port", "8188", "--cpu", "--extra-model-paths-config", "E:/my models/x.yaml"]
        );
    }

    #[test]
    fn blank_host_omits_listen() {
        let settings = ComfySettings { comfy_dir: "E:/ComfyUI".into(), host: "  ".into(), ..Default::default() };
        assert_eq!(build_start_args(&settings).unwrap(), vec!["main.py", "--port", "8188"]);
    }

    #[test]
    fn ipv6_brackets_are_stripped_for_listen() {
        let settings = ComfySettings { comfy_dir: "E:/ComfyUI".into(), host: "[::1]".into(), ..Default::default() };
        assert_eq!(
            build_start_args(&settings).unwrap(),
            vec!["main.py", "--listen", "::1", "--port", "8188"]
        );
    }

    #[test]
    fn split_args_keeps_quoted_whitespace() {
        assert_eq!(split_args("a b  \"c d\""), vec!["a", "b", "c d"]);
        assert_eq!(split_args("\"\""), vec![""], "空引号也是一个参数");
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn start_command_splits_program_and_args() {
        let venv_python = if cfg!(windows) { "E:/ComfyUI/venv/Scripts/python.exe" } else { "E:/ComfyUI/venv/bin/python" };
        let (program, args, cwd) = start_command(&with_comfy("--cpu")).unwrap();
        assert_eq!(program, venv_python);
        assert_eq!(args, vec!["main.py", "--listen", "127.0.0.1", "--port", "8188", "--cpu"]);
        assert_eq!(cwd.as_deref(), Some("E:/ComfyUI"));
    }

    #[test]
    fn missing_config_is_error() {
        assert!(build_start_args(&ComfySettings::default()).is_err());
    }

    #[test]
    fn dependency_command_uses_install_python_and_requirements() {
        let dir = std::env::temp_dir().join(format!("comfyui-panel-deps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("requirements.txt"), "torch\n").unwrap();
        let settings = ComfySettings {
            comfy_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let (program, args, cwd, env) = dependency_command(&settings).unwrap();
        let expected_requirements = dir.join("requirements.txt").to_string_lossy().into_owned();
        let expected_dir = dir.to_string_lossy().into_owned();
        assert_eq!(program, resolve_python(&settings));
        assert_eq!(&args[..4], ["-m", "pip", "install", "-r"]);
        assert_eq!(args[4], expected_requirements);
        assert_eq!(cwd.as_deref(), Some(expected_dir.as_str()));
        assert_eq!(env.get("PIP_DISABLE_PIP_VERSION_CHECK").map(String::as_str), Some("1"));
        std::fs::remove_dir_all(&dir).ok();
        assert!(dependency_command(&ComfySettings::default()).is_err());
    }

    #[test]
    fn update_command_runs_git_pull_ff_only_in_install_dir() {
        let (program, args, cwd, env) = update_command(&with_comfy("")).unwrap();
        assert_eq!(program, "git");
        assert_eq!(args, vec!["-C", "E:/ComfyUI", "pull", "--ff-only"]);
        assert_eq!(cwd, None);
        assert_eq!(env.get("GIT_TERMINAL_PROMPT").map(String::as_str), Some("0"));
        assert!(update_command(&ComfySettings::default()).is_err());
    }
}
