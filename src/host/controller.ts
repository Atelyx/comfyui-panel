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
  /** 首次用到时向宿主问一次并缓存。 */
  private platform: Platform | null = null;
  /** 本插件启动的进程句柄；停止与「是否在运行」都依它判断。 */
  private handle: ShellProcessHandle | null = null;

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
      await runtime.connect();
      if (runtime.getSnapshot().channel === "direct") return true;
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

  /** 启动 ComfyUI 并等待服务就绪。 */
  async start(runtime: ComfyRuntime): Promise<boolean> {
    if (this.snap.starting || this.handle) return false;
    let line: string;
    try {
      line = describeStartCommand(this.settings);
    } catch (err) {
      this.emit({ error: describe(err) });
      return false;
    }
    this.emit({ starting: true, error: "", startLine: line, logs: [] });
    this.log(`$ ${line}`);

    try {
      this.handle = await startComfy(this.ctx, await this.getPlatform(), this.settings, {
        onLog: (text, stream) => this.log(text, stream),
        onExit: (code) => {
          // 无论就绪前后退出，对界面而言都是「进程不在了」
          this.handle = null;
          this.emit({ running: false, starting: false });
          this.log(`进程已退出（退出码 ${code ?? "未知"}）`, "stderr");
        },
        onError: (message) => {
          this.handle = null;
          this.emit({ starting: false, running: false, error: message });
          this.log(message, "stderr");
        },
      });
      this.emit({ running: true });
    } catch (err) {
      this.emit({ starting: false, error: describe(err) });
      return false;
    }

    const ready = await this.waitReady(runtime);
    this.emit({ starting: false });
    if (!ready) this.emit({ error: `启动后 ${Math.round(READY_TIMEOUT_MS / 1000)}s 内服务未就绪，请查看下方日志` });
    return ready;
  }

  /** 停止本插件启动的进程（结束整棵进程树，不给服务留孤儿）。 */
  async stop(): Promise<void> {
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

  /**
   * 关窗前的尽力而为清理：页面销毁在即，异步等不到返回，只把结束命令发出去
   * （命令一旦到达宿主就会执行）。宿主侧另有进程收尾，这里是销毁路径上的双保险。
   */
  killOnShutdown(): void {
    const handle = this.handle;
    if (!handle) return;
    this.handle = null;
    void handle.cancel().catch(() => {
      // 页面正在销毁，无处提示；宿主侧的登记也会随进程退出自然清理
    });
  }
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
