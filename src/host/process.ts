/**
 * 进程托管：按设置启动与停止本机 ComfyUI。
 *
 * 直接以 Python 可执行文件为启动程序（宿主对程序来源不设限制），不经 shell 包装，
 * 参数逐项透传、引号不会被二次转义。停进程用句柄 `cancel()` 结束整棵进程树：
 * ComfyUI 可能派生多进程子任务，只结束主进程会留孤儿。
 */
import type { AtelyxCtx, ProcessHandle } from "../ctx";
import type { ComfySettings } from "../settings";
import { resolvePython } from "../settings";


export interface StartOptions {
  /** 逐行日志。 */
  onLog(line: string, stream: "stdout" | "stderr"): void;
  /** 进程退出即触发；就绪前退出即启动失败。 */
  onExit(code: number | null): void;
  /** 启动失败。 */
  onError(message: string): void;
}

/**
 * 启动参数：逐项分开，不把整行命令拼成一个参数。
 *
 * 「允许远程启动」开启后，托管启动（含本机自行启动）一律把监听地址放在用户附加参数之后：
 * ComfyUI 的 argparse 对同名开关取最后一个，位置靠后才能覆盖用户自填的监听地址。
 * 本机自启动同样放开监听，同空间成员才能直接连上使用，而不必由对方重新发起一次启动；
 * 暴露面变大是这个开关的固有代价，设置页写明后果。
 */
export function buildStartTokens(settings: ComfySettings): string[] {
  const python = resolvePython(settings);
  if (!python) throw new Error("未配置 ComfyUI 目录或 Python 路径");
  const tokens = [python, "main.py", "--port", String(settings.port)];
  // 插件与 ComfyUI 通信的前提，关掉后连不上
  if (settings.autoCors) tokens.push("--enable-cors-header");
  const args = [...tokens, ...splitArgs(settings.extraArgs)];
  if (settings.remoteEnabled) args.push("--listen", "0.0.0.0");
  return args;
}

/**
 * 按命令行习惯切分用户填写的附加参数：空白分隔，双引号内保留空白并去掉引号。
 *
 * 不能简单地按空白切：参数值常是路径，形如 `--extra-model-paths-config "E:/my models/x.yaml"`，
 * 切开会把它拆成三段，最终传给服务端的是错的路径。
 */
function splitArgs(text: string): string[] {
  const out: string[] = [];
  let current = "";
  let started = false;
  let quoted = false;
  for (const ch of text) {
    if (ch === '"') {
      quoted = !quoted;
      started = true;
      continue;
    }
    if (!quoted && /\s/.test(ch)) {
      if (started) out.push(current);
      current = "";
      started = false;
      continue;
    }
    current += ch;
    started = true;
  }
  if (started) out.push(current);
  return out;
}

/** 供界面展示的命令行（仅为可读，执行不走它）。 */
export function describeStartCommand(settings: ComfySettings): string {
  return buildStartTokens(settings).map(shellQuote).join(" ");
}

/** 含空白或引号时加引号（仅用于展示）。 */
function shellQuote(value: string): string {
  return /[\s"]/.test(value) ? `"${value.replace(/"/g, '\\"')}"` : value;
}

/**
 * 启动 ComfyUI 并返回句柄。
 *
 * 句柄须由调用方保存：ComfyUI 是长驻服务，只能靠它停下。Atelyx在插件停用/卸载时也会统一
 * 结束本插件启动的进程，因此不必自己兜底崩溃场景。
 */
export async function startComfy(
  ctx: AtelyxCtx,
  settings: ComfySettings,
  options: StartOptions,
): Promise<ProcessHandle> {
  const cwd = settings.comfyDir.trim();
  if (!cwd) throw new Error("未配置 ComfyUI 目录");
  // tokens[0] = Python 路径，其余为程序参数
  const [python, ...args] = buildStartTokens(settings);

  return ctx.process.spawn({ command: python, args, cwd }, {
    chunk: (data) => {
      for (const raw of String(data.data).split(/\r?\n/)) {
        const text = raw.trimEnd();
        if (text) options.onLog(text, data.stream);
      }
    },
    end: (data) => options.onExit(data.code),
    error: (message) => options.onError(message),
  });
}
