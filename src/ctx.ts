import type { Context } from "@atelyx/cordis";

/**
 * 插件上下文（`ctx`）的最小可用类型面。
 *
 * 只声明本插件实际用到的服务与方法；Atelyx在加载时按自己的实现注入，
 * 这里的类型仅为本地 `tsc --noEmit` 提供约束。
 * 依赖声明见 `index.tsx`——这里用到的平台服务在插件运行时恒在。
 */

/** 进程输出回调（`ctx.shell.spawn` 传 handlers 时启用）。 */
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

/** 同一协作空间内的一个在线成员（`ctx.collab.peers()`）。 */
export interface CollabPeer {
  peerId: number;
  nickname: string;
  color: string;
  deviceName: string;
  /** 对方应用版本号；旧客户端可缺省。 */
  version?: string;
}

/** 本端协作身份（`ctx.collab.myPeer()`；未连接时 peerId 为 null）。 */
export interface CollabMyPeer {
  peerId: number | null;
  nickname: string;
  color: string;
  deviceName: string;
}

/** 宿主事件载荷（只列本插件订阅的事件）。 */
export interface AtelyxEvents {
  /** 进仓/切仓完成广播；`root` 为 null 表示协作空间仓库（无本地根）。 */
  "vault:switch": { root: string | null };
  /** 收到同一协作空间内其他成员发来的插件消息；不含自己。 */
  "collab:message": { peerId: number; channel: string; payload: unknown };
  "collab:changed": { peers: CollabPeer[] };
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
  /** Atelyx 平台信息（平台用于决定启动命令的写法）。 */
  app: {
    version(): Promise<string>;
    platform(): Promise<string>;
  };
  state: {
    read(): Promise<unknown>;
    write(data: unknown): Promise<void>;
  };
  shell: {
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
    /** 图片 dataURL 进系统剪贴板（预览右键复制用）。 */
    copyImage(dataUrl: string): Promise<void>;
  };
  /**
   * 协作服务：同一协作空间内的成员与消息收发。
   *
   * 空间由服务器划分（`space:<spaceId>`），空间外看不到彼此；只有登录协作服务器并进入
   * 同一空间，跨机协作才成立——本地仓库不在协作范围内。
   */
  collab: {
    peers(): CollabPeer[];
    /** 发往同一协作空间内其他成员；`to` 指定则定向单播。返回是否已投递到传输层。 */
    sendMessage(channel: string, payload: unknown, opts?: { to?: number }): boolean;
    myPeer(): CollabMyPeer;
    /** 声明本插件需要协作通道，返回释放函数（随插件停用/卸载撤销）。
     *  宿主只为有活跃声明的窗口维持协作连接；旧版宿主无此方法，插件须探测降级。 */
    acquire?(): () => void;
  };
  /** 宿主事件订阅；返回撤销函数，监听器随插件停用一并撤销。 */
  events: {
    on<K extends keyof AtelyxEvents>(name: K, listener: (payload: AtelyxEvents[K]) => void): () => void;
  };
  slots: {
    registerView(opts: RegisterViewOptions): () => void;
    registerSetting(opts: RegisterSettingOptions): () => void;
    registerCommand(opts: RegisterCommandOptions): () => void;
  };
  effect(fn: () => void | (() => void)): void;
}
