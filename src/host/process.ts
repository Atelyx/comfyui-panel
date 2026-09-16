/**
 * 进程托管：按设置启动与停止本机 ComfyUI。
 *
 * 两条约束决定实现方式：
 * 1. 可执行程序只有 `cmd.exe`（Windows）/`sh`（Unix）被登记，启动命令必须写成整行经
 *    `/C`、`-c` 转达，不能直接指定 python。
 * 2. 包装层是 `cmd.exe /C`——只结束包装进程会把真正的服务留成孤儿，因此经 `spawn`
 *    拿到的句柄 `cancel()` 结束整棵进程树，而不是按命令行特征去找进程。
 */
import type { AtelyxCtx, ShellProcessHandle } from "../ctx";
import type { ComfySettings } from "../settings";
import { resolvePython } from "../settings";

/** 平台决定命令写法，判错就启不动，因此取Atelyx给的值而不是猜 UA。 */
export type Platform = "windows" | "unix";

/** 命令写法只有这两种。 */
export function toPlatform(raw: string): Platform {
  return raw.startsWith("windows") ? "windows" : "unix";
}

export async function resolvePlatform(ctx: AtelyxCtx): Promise<Platform> {
  try {
    return toPlatform(await ctx.app.platform());
  } catch {
    // 取不到时按 Unix 处理，不会误用 Windows 专有写法
    return "unix";
  }
}

export interface StartOptions {
  /** 逐行日志。 */
  onLog(line: string, stream: "stdout" | "stderr"): void;
  /** 进程退出即触发；就绪前退出即启动失败。 */
  onExit(code: number | null): void;
  /** 启动失败。 */
  onError(message: string): void;
}

/**
 * 启动参数：逐项分开。
 *
 * 不把整行命令（含引号）拼成一个参数——Atelyx逐项交给进程时，参数内部的引号会被转义成
 * `\"`，而 `cmd.exe` 不认这种转义（它只认 `""` 双写），命令会被拆坏。
 * 逐项传参由Atelyx按需加引号，cmd 能正确解析。
 */
export function buildStartTokens(settings: ComfySettings): string[] {
  const python = resolvePython(settings);
  if (!python) throw new Error("未配置 ComfyUI 目录或 Python 路径");
  const tokens = [python, "main.py", "--port", String(settings.port)];
  // 插件与 ComfyUI 通信的前提，关掉后连不上
  if (settings.autoCors) tokens.push("--enable-cors-header");
  // 整串塞成一个参数会让Atelyx把它引成一个，多个开关就失效了，故按命令行习惯切分
  return [...tokens, ...splitArgs(settings.extraArgs)];
}

/**
 * 按命令行习惯切分用户填写的附加参数：空白分隔，双引号内保留空白并去掉引号。
 *
 * 不能简单地按空白切：参数值常是路径，形如 `--extra-model-paths-config "E:/my models/x.yaml"`，
 * 切开会把它拆成三段，引号还会被Atelyx转义成字面量，最终传给服务端的是错的路径。
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

/** 含空白或引号时加引号（仅用于展示与 Unix 的 `-c` 串）。 */
function shellQuote(value: string): string {
  return /[\s"]/.test(value) ? `"${value.replace(/"/g, '\\"')}"` : value;
}

/** 按平台组装执行参数。 */
function runArgs(platform: Platform, tokens: string[]): string[] {
  // Windows：`/C` 之后逐项给，引号交给Atelyx按需添加
  if (platform === "windows") return ["/C", ...tokens];
  // Unix：`-c` 只吃一个脚本文本，在这里拼成命令行
  return ["-c", tokens.map(shellQuote).join(" ")];
}

/**
 * 启动 ComfyUI 并返回句柄。
 *
 * 句柄须由调用方保存：ComfyUI 是长驻服务，只能靠它停下。Atelyx在插件停用/卸载时也会统一
 * 结束本插件启动的进程，因此不必自己兜底崩溃场景。
 */
export async function startComfy(
  ctx: AtelyxCtx,
  platform: Platform,
  settings: ComfySettings,
  options: StartOptions,
): Promise<ShellProcessHandle> {
  const cwd = settings.comfyDir.trim();
  if (!cwd) throw new Error("未配置 ComfyUI 目录");
  const tokens = buildStartTokens(settings);

  const command = platform === "windows" ? "cmd.exe" : "sh";
  const args = runArgs(platform, tokens);

  return ctx.shell.spawn({ command, args, cwd }, {
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
