/**
 * 传输层：直连 ComfyUI。
 *
 * 为什么要求服务端开启跨域放行：ComfyUI 默认拒绝跨站请求（界面里的 fetch、图片与长连接都算跨站），
 * 而插件运行的前端天然跨站，因此要求它以 `--enable-cors-header` 启动；插件托管启动时自动带上。
 *
 * 直连是唯一通道：REST、长连接与二进制都从它取，不必再维护一条能力受限的旁路。
 */
import { baseUrl, type ComfySettings } from "../settings";

export type Channel = "direct" | "offline";

export interface ChannelState {
  channel: Channel;
  /** 探测失败原因：界面据此给出可操作提示。 */
  reason: string;
}

export interface RequestOptions {
  method?: string;
  body?: string;
  headers?: Record<string, string>;
  timeoutMs?: number;
}

export interface TransportResult {
  status: number;
  body: string;
}

/** 单次请求超时：本机服务，慢于此即视作不可用（不拖住界面首屏）。 */
const REQUEST_TIMEOUT_MS = 4000;

/** 二进制请求超时：结果图可达数 MB，给足传输时间。 */
const BINARY_TIMEOUT_MS = 20000;

/** 把底层异常压成一句人话。 */
function describeError(err: unknown): string {
  if (err instanceof Error) {
    if (err.name === "AbortError") return "请求超时";
    return err.message || err.name;
  }
  return String(err);
}

export class ComfyTransport {
  private readonly base: string;
  /** 记住探测结果，避免每次请求都先撞一次失败。 */
  private channel: Channel = "offline";

  constructor(settings: ComfySettings) {
    this.base = baseUrl(settings);
  }

  get baseUrl(): string {
    return this.base;
  }

  get isOnline(): boolean {
    return this.channel === "direct";
  }

  private async fetchWithTimeout(url: string, init: RequestInit, timeoutMs: number): Promise<Response> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      return await fetch(url, { ...init, signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * 探测服务是否可用。
   *
   * 失败原因统一按「未开启跨域放行」表述：界面发出的请求被拒绝时，浏览器只给一个语焉不详的
   * 网络错误（拿不到响应体），而实际原因几乎总是这一条。
   */
  async probe(): Promise<ChannelState> {
    try {
      const res = await this.fetchWithTimeout(`${this.base}/system_stats`, { method: "GET" }, REQUEST_TIMEOUT_MS);
      if (res.ok) {
        this.channel = "direct";
        return { channel: "direct", reason: "" };
      }
      this.channel = "offline";
      return { channel: "offline", reason: `ComfyUI 返回 HTTP ${res.status}` };
    } catch (err) {
      this.channel = "offline";
      return {
        channel: "offline",
        reason: `无法访问 ${this.base}（${describeError(err)}）。请确认 ComfyUI 正在运行，且启动时带上了 --enable-cors-header（否则它不接受来自界面的请求）`,
      };
    }
  }

  /** 发一次 JSON API 请求。 */
  async request(path: string, opts: RequestOptions = {}): Promise<TransportResult> {
    const res = await this.fetchWithTimeout(
      `${this.base}${path}`,
      { method: opts.method ?? "GET", headers: opts.headers, body: opts.body },
      opts.timeoutMs ?? REQUEST_TIMEOUT_MS,
    );
    return { status: res.status, body: await res.text() };
  }

  /** 取原始字节（结果图等）。 */
  async requestBytes(path: string, timeoutMs = BINARY_TIMEOUT_MS): Promise<Uint8Array> {
    const res = await this.fetchWithTimeout(`${this.base}${path}`, { method: "GET" }, timeoutMs);
    if (!res.ok) throw new Error(`取图失败：HTTP ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  }

  /** 图片展示地址；缩略图走服务端压缩，避免历史动辄几十张时原图直出拖垮界面。 */
  viewUrl(ref: { filename: string; subfolder: string; type: string }, preview = true): string {
    const params = new URLSearchParams({
      filename: ref.filename,
      subfolder: ref.subfolder,
      type: ref.type,
    });
    if (preview) params.set("preview", "webp;90");
    return `${this.base}/view?${params.toString()}`;
  }

  /** WebSocket 地址。 */
  socketUrl(clientId: string): string {
    return `${this.base.replace(/^http/, "ws")}/ws?clientId=${encodeURIComponent(clientId)}`;
  }
}
