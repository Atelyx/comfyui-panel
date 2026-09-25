/**
 * 进程托管的状态中心：启停、就绪判定与运行日志。
 *
 * 与运行时分开：连接是网络层的事，进程是本机的事——用户可能自行启动服务（连接通但非本插件
 * 启动），也可能插件起了进程而服务尚未就绪，界面需要同时看到这两条信息。
 */
import type { AtelyxCtx, ShellProcessHandle } from "../ctx";
import type { ComfySettings } from "../settings";
import type { ComfyRuntime } from "../runtime";
import { describeStartCommand, resolvePlatform, startComfy, type Platform } from "./process";

export interface HostSnapshot {
  /** 是否由本插件启动。 */
  running: boolean;
  /** 正在启动（等接口就绪）。 */
  starting: boolean;
  /** 展示将要执行的命令，便于用户确认。 */
  startLine: string;
  /** 尾部若干行日志。 */
  logs: readonly string[];
  /** 最近一次操作的错误。 */
  error: string;
}

/** 够看启动报错即可，不无限增长。 */
const MAX_LOGS = 300;
/** 加载大模型或首次编译会慢，给足时间但不无限等。 */
const READY_TIMEOUT_MS = 180000;
/** 错误输出可能极长（Python traceback 单行也能几十 KB），进通知与快照前截断。 */
const ERROR_DETAIL_MAX = 200;

/** 启动请求的可选来源信息（远程启动据此标明发起方）。 */
export interface StartRequest {
  /** 触发来源，写进日志首行，便于事后分辨是谁起的进程。 */
  origin?: string;
}

const EMPTY: HostSnapshot = {
  running: false,
  starting: false,
  startLine: "",
  logs: [],
  error: "",
};

export class HostController {
  private readonly ctx: AtelyxCtx;
  private settings: ComfySettings;
  private readonly listeners = new Set<() => void>();
  private snap: HostSnapshot = EMPTY;
  /** 首次用到时向Atelyx问一次并缓存。 */
  private platform: Platform | null = null;
  /** 本插件启动的进程句柄；停止与「是否在运行」都依它判断。 */
  private handle: ShellProcessHandle | null = null;
  /** 等待就绪期间进程退出或出错的原因；非空即终止等待，不再空等满超时。 */
  private died = "";
  /** 最后一条非空错误输出：进程没打印可读原因时，它就是最接近现场的信息。 */
  private lastStderr = "";
  /** 本次启动是否已弹过失败通知：spawn 失败会同时走 error 回调与 reject，只通知一次。 */
  private notified = false;
  /** 本次启动是否被用户主动停止：收场同样是进程退出，但不是失败。 */
  private cancelled = false;

  constructor(ctx: AtelyxCtx, settings: ComfySettings) {
    this.ctx = ctx;
    this.settings = settings;
  }

  private async getPlatform(): Promise<Platform> {
    if (this.platform === null) this.platform = await resolvePlatform(this.ctx);
    return this.platform;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): HostSnapshot => this.snap;

  applySettings(settings: ComfySettings): void {
    this.settings = settings;
  }

  private emit(patch: Partial<HostSnapshot>): void {
    this.snap = { ...this.snap, ...patch };
    for (const listener of this.listeners) listener();
  }

  /** 追加并裁剪到上限。 */
  private log(line: string, stream: "stdout" | "stderr" = "stdout"): void {
    const prefix = stream === "stderr" ? "! " : "";
    const next = [...this.snap.logs, `${prefix}${line}`];
    this.emit({ logs: next.length > MAX_LOGS ? next.slice(next.length - MAX_LOGS) : next });
  }

  clearLogs(): void {
    this.emit({ logs: [] });
  }

  /** 就绪判定用「接口通」而不是「日志出现某句话」：日志文案随版本变化，接口可用才是真的可用。 */
  private async waitReady(runtime: ComfyRuntime): Promise<boolean> {
    const started = Date.now();
    let lastHeartbeat = 0;
    for (;;) {
      // 进程已退出就等不到服务，立刻定论——由调用方按 died 给出失败原因
      if (this.died) return false;
      await runtime.connect();
      if (runtime.getSnapshot().channel === "direct") return true;
      // 探测期间退出：这一轮也白等
      if (this.died) return false;
      const elapsed = Date.now() - started;
      if (elapsed >= READY_TIMEOUT_MS) return false;
      // 加载模型与首次编译可能耗时数十秒，定期留个心跳，让用户看到还在等
      if (elapsed - lastHeartbeat >= 5000) {
        lastHeartbeat = elapsed;
        this.log(`等待服务就绪…（${Math.round(elapsed / 1000)}s）`);
      }
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
  }

  /** 启动失败弹一条宿主通知：面板不可见（如远程发起）时也能知道结果与原因。 */
  private notifyFailure(message: string): void {
    if (this.notified) return;
    this.notified = true;
    try {
      this.ctx.notification.notify({ level: "error", title: "ComfyUI 启动失败", message });
    } catch {
      // 通知通道异常不改变「启动失败」这个结论
    }
  }

  /** 启动未就绪的收口：写失败原因并弹通知。用户主动停止以同样的进程退出收场，但那不是失败。 */
  private failStart(died: string): void {
    if (this.cancelled) {
      this.emit({ starting: false });
      return;
    }
    const message = died || `启动后 ${Math.round(READY_TIMEOUT_MS / 1000)}s 内服务未就绪`;
    this.emit({ starting: false, error: message });
    this.notifyFailure(message);
  }

  /**
   * 启动 ComfyUI 并等待服务就绪。
   *
   * 返回 false 有两种含义——「已有进程/正在启动」与「启动失败」，界面据此分辨：失败原因在
   * `error` 字段里，而前者不会写 error。远程受理在前一步自行判定，不靠这个返回值区分。
   */
  async start(runtime: ComfyRuntime, request: StartRequest = {}): Promise<boolean> {
    if (this.snap.starting || this.handle) return false;
    this.died = "";
    this.lastStderr = "";
    this.notified = false;
    this.cancelled = false;
    let line: string;
    try {
      line = describeStartCommand(this.settings);
    } catch (err) {
      const message = describe(err);
      this.emit({ error: message });
      this.notifyFailure(message);
      return false;
    }
    this.emit({ starting: true, error: "", startLine: line, logs: [] });
    if (request.origin) this.log(request.origin);
    this.log(`$ ${line}`);

    try {
      const handle = await startComfy(this.ctx, await this.getPlatform(), this.settings, {
        onLog: (text, stream) => {
          if (stream === "stderr" && text.trim()) this.lastStderr = text.trim();
          this.log(text, stream);
        },
        onExit: (code) => {
          // 无论就绪前后退出，对界面而言都是「进程不在了」；starting 归 start() 收口
          this.handle = null;
          const detail = this.lastStderr ? `：${truncate(this.lastStderr, ERROR_DETAIL_MAX)}` : "";
          this.died = `进程已退出（退出码 ${code ?? "未知"}）${detail}`;
          this.emit({ running: false });
          this.log(`进程已退出（退出码 ${code ?? "未知"}）`, "stderr");
        },
        onError: (message) => {
          this.handle = null;
          this.died = message;
          // 启动阶段由 start() 统一收口；就绪后出错没有这一步，直接落进快照
          if (this.snap.starting) this.emit({ running: false });
          else this.emit({ running: false, error: message });
          this.log(message, "stderr");
        },
      });
      // 句柄落地前进程就已退出：onExit 已记下原因，这里不能再把句柄写回去
      if (this.died) {
        this.failStart(this.died);
        return false;
      }
      this.handle = handle;
      this.emit({ running: true });
    } catch (err) {
      this.failStart(describe(err));
      return false;
    }

    const ready = await this.waitReady(runtime);
    if (ready) this.emit({ starting: false });
    else this.failStart(this.died);
    return ready;
  }

  /** 停止本插件启动的进程（结束整棵进程树，不给服务留孤儿）。 */
  async stop(): Promise<void> {
    // 等就绪期间用户叫停：别让 start() 把这次退出当成启动失败
    if (this.snap.starting) this.cancelled = true;
    const handle = this.handle;
    if (!handle) {
      this.emit({ running: false, starting: false });
      return;
    }
    try {
      await handle.cancel();
      this.log("已结束 ComfyUI 进程");
    } catch (err) {
      this.emit({ error: describe(err) });
    } finally {
      this.handle = null;
      this.emit({ running: false, starting: false });
    }
  }
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}…` : text;
}
