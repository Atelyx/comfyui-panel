import type { Context } from "@atelyx/cordis";

/**
 * 插件上下文（`ctx`）的最小可用类型面。
 *
 * 只声明本插件实际用到的服务与方法；宿主在加载时按自己的实现注入，
 * 这里的类型仅为本地 `tsc --noEmit` 提供约束。
 * 依赖声明见 `index.tsx`——这里用到的平台服务在插件运行时恒在。
 */

/** 流式进程回调（`ctx.shell.exec`/`spawn` 传 handlers 时启用）。 */
export interface ShellStreamHandlers {
  chunk(data: { stream: "stdout" | "stderr"; data: string }): void;
  end(data: { code: number | null }): void;
  error(message: string): void;
}

/** `ctx.shell.spawn` 的进程句柄：`cancel` 结束该进程及其全部子孙。 */
export interface ShellProcessHandle {
  pid: number;
  cancel(): Promise<void>;
}

/** 仓库文件写入结果（写失败返回 ok=false 不抛断）。 */
export interface VaultWriteResult {
  ok: boolean;
  summary: string;
}

/** 系统对话框过滤器。 */
export interface DialogFilter {
  name: string;
  extensions: string[];
}

/** 视图/设置页/命令注册载荷（只列本插件用到的形状）。 */
export interface RegisterViewOptions {
  kind: string;
  label: string;
  component: (() => unknown) | unknown;
  priority?: number;
}

export interface RegisterSettingOptions {
  key: string;
  label: string;
  component: (() => unknown) | unknown;
}

export interface RegisterCommandOptions {
  id: string;
  label: string;
  run: () => unknown;
  shortcut?: string;
}

export interface AtelyxCtx extends Context {
  /** 宿主信息（平台用于决定启动命令的写法）。 */
  app: {
    version(): Promise<string>;
    platform(): Promise<string>;
  };
  state: {
    read(): Promise<unknown>;
    write(data: unknown): Promise<void>;
  };
  storage: {
    get(key: string): Promise<unknown>;
    set(key: string, value: unknown): Promise<void>;
    delete(key: string): Promise<void>;
    keys(): Promise<string[]>;
    clear(): Promise<void>;
  };
  shell: {
    exec(
      opts: { command: string; args?: string[]; cwd?: string; env?: Record<string, string> },
      handlers?: ShellStreamHandlers,
    ): Promise<{ code: number | null; stdout: string; stderr: string } | undefined>;
    /** 启动长驻进程并立即拿到句柄（不等进程结束）。 */
    spawn(
      opts: { command: string; args?: string[]; cwd?: string; env?: Record<string, string> },
      handlers?: ShellStreamHandlers,
    ): Promise<ShellProcessHandle>;
  };
  vault: {
    readFile(file: string): Promise<string>;
    writeFile(file: string, content: string): Promise<VaultWriteResult>;
    appendFile(file: string, content: string): Promise<VaultWriteResult>;
    createFolder(dir: string): Promise<{ ok: boolean; summary: string; path: string }>;
    deleteFile(path: string): Promise<{ ok: boolean; summary: string }>;
  };
  dialog: {
    pickDirectory(): Promise<string | null>;
    pickFile(filters?: DialogFilter[]): Promise<string | null>;
  };
  notification: {
    notify(input: { message: string; level?: "info" | "success" | "warning" | "error"; title?: string }): string;
  };
  native: {
    invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
  };
  clipboard: {
    readText(): Promise<string>;
    writeText(text: string): Promise<void>;
  };
  /**
   * 服务发现：用于「存在则用、不存在则降级」的可选依赖。
   *
   * `note` 服务由随应用分发的笔记插件提供（停用即消失），所以**不能**写进 apply 的 inject
   * ——那会让本插件在用户停用笔记插件时直接激活失败；这里判空使用。
   */
  services: {
    get(name: string): unknown;
  };
  slots: {
    registerView(opts: RegisterViewOptions): () => void;
    registerSetting(opts: RegisterSettingOptions): () => void;
    registerCommand(opts: RegisterCommandOptions): () => void;
  };
  effect(fn: () => void | (() => void)): void;
}
