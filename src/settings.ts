/**
 * 插件设置：`ctx.state` 的单对象状态。
 *
 * 缺字段一律用默认值补齐（新增设置项时老状态仍可用）；读取只做内存合并，
 * 写盘仅发生在用户改设置时——启用插件不该产生写盘。
 */
import type { AtelyxCtx } from "./ctx";

/** 托管=插件负责启停；外部=只检测，进程归用户管。 */
export type ProcessMode = "managed" | "external";

/** 采样预览方式：ComfyUI 默认不发预览，不注入参数就收不到生成中的画面。 */
export type PreviewMethod = "none" | "auto" | "latent2rgb" | "taesd";

export interface ComfySettings {
  /** ComfyUI 监听地址（默认本机）。 */
  host: string;
  /** 监听端口：既是连接目标，也是托管启动时传给 ComfyUI 的端口。 */
  port: number;
  /** 进程接管方式。 */
  processMode: ProcessMode;
  /** 安装目录：托管启动的工作目录，也是停止进程时的默认识别依据。 */
  comfyDir: string;
  /** 留空则用安装目录下 venv 里的 python。 */
  pythonPath: string;
  /** 附加启动参数，原样追加在末尾（端口与跨域放行由插件注入）。 */
  extraArgs: string;
  /** 是否注入跨域放行参数：直连通道的前提，关掉只剩保底通道。 */
  autoCors: boolean;
  /** 采样预览方式。 */
  previewMethod: PreviewMethod;
  /** 落库子目录，留空则用仓库的附件目录。 */
  archiveFolder: string;
  /** 落库后是否把图片追加到当前笔记。 */
  appendToNote: boolean;
}

/** 状态文件形状（顶层带版本号）。 */
interface PluginState {
  version: number;
  settings: ComfySettings;
}

const STATE_VERSION = 1;

export const DEFAULT_SETTINGS: ComfySettings = {
  host: "127.0.0.1",
  port: 8188,
  processMode: "managed",
  comfyDir: "",
  pythonPath: "",
  extraArgs: "",
  autoCors: true,
  // 默认开预览：代价是采样时多算一张小图，换来生成中能看见画面
  previewMethod: "latent2rgb",
  archiveFolder: "",
  appendToNote: false,
};

/** 收敛磁盘值：手改或半写的脏数据不该把插件带偏。 */
function coerce(raw: unknown): ComfySettings {
  const source = (raw ?? {}) as Record<string, unknown>;
  const str = (key: keyof ComfySettings, fallback: string): string => {
    const value = source[key];
    return typeof value === "string" ? value : fallback;
  };
  const bool = (key: keyof ComfySettings, fallback: boolean): boolean => {
    const value = source[key];
    return typeof value === "boolean" ? value : fallback;
  };
  const port = source.port;
  const mode = source.processMode;
  const preview = source.previewMethod;
  return {
    host: str("host", DEFAULT_SETTINGS.host).trim() || DEFAULT_SETTINGS.host,
    // 非法端口在此挡掉，后面拼地址的地方无需再判
    port: typeof port === "number" && Number.isInteger(port) && port > 0 && port < 65536 ? port : DEFAULT_SETTINGS.port,
    processMode: mode === "external" ? "external" : "managed",
    comfyDir: str("comfyDir", DEFAULT_SETTINGS.comfyDir),
    pythonPath: str("pythonPath", DEFAULT_SETTINGS.pythonPath),
    extraArgs: str("extraArgs", DEFAULT_SETTINGS.extraArgs),
    autoCors: bool("autoCors", DEFAULT_SETTINGS.autoCors),
    previewMethod:
      preview === "none" || preview === "auto" || preview === "latent2rgb" || preview === "taesd"
        ? preview
        : DEFAULT_SETTINGS.previewMethod,
    archiveFolder: str("archiveFolder", DEFAULT_SETTINGS.archiveFolder),
    appendToNote: bool("appendToNote", DEFAULT_SETTINGS.appendToNote),
  };
}

/** 状态缺失或损坏一律回落默认，不抛错。 */
export async function loadSettings(ctx: AtelyxCtx): Promise<ComfySettings> {
  try {
    const raw = (await ctx.state.read()) as PluginState | null;
    return coerce(raw?.settings);
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

/** 整体覆盖写入。 */
export async function saveSettings(ctx: AtelyxCtx, settings: ComfySettings): Promise<void> {
  const payload: PluginState = { version: STATE_VERSION, settings };
  await ctx.state.write(payload);
}

/** 拼 HTTP 基址（末尾不带斜杠）。 */
export function baseUrl(settings: ComfySettings): string {
  return `http://${settings.host}:${settings.port}`;
}

/** 用户填了就用填的，否则指向安装目录下 venv 的 python。 */
export function resolvePython(settings: ComfySettings): string {
  const explicit = settings.pythonPath.trim();
  if (explicit) return explicit;
  const dir = settings.comfyDir.trim().replace(/[\\/]+$/, "");
  if (!dir) return "";
  return `${dir}/venv/Scripts/python.exe`;
}
