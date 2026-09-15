/**
 * ComfyUI 的 WebSocket 客户端：实时进度与执行状态只有这条通道有。
 *
 * 帧格式：文本帧 `{"type": ..., "data": ...}`；二进制帧 = 4 字节大端事件类型 + 载荷，
 * 只消费其中的 `TEXT`(3) 帧（其余为采样预览类帧，插件不展示实时预览，直接忽略）。
 */

/** 二进制帧的事件类型（与 ComfyUI `protocol.BinaryEventTypes` 对齐）。 */
const BINARY_TEXT = 3;

export interface SocketHandlers {
  /** 文本事件（事件名 + 载荷对象）。 */
  onEvent(type: string, data: unknown): void;
  /** 连接建立。 */
  onOpen(): void;
  /** 连接关闭（`retrying` 表示插件会自动重连，UI 据此区分「掉了在重连」与「彻底断开」）。 */
  onClose(retrying: boolean): void;
}

/** 重连退避（毫秒）：本机服务，前几次快速重试即可，避免无谓打日志。 */
const RECONNECT_DELAYS_MS = [500, 1500, 3000, 5000, 10000];

export class ComfySocket {
  private ws: WebSocket | null = null;
  private closedByUs = false;
  private retryIndex = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  /** 监听器由外部持有：重连只换底层连接，不重新注册。 */
  private readonly handlers: SocketHandlers;

  constructor(
    private readonly url: string,
    handlers: SocketHandlers,
  ) {
    this.handlers = handlers;
  }

  connect(): void {
    this.closedByUs = false;
    this.open();
  }

  private open(): void {
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.url);
    } catch (err) {
      // 地址非法等同步异常直接上抛：重试同一个坏地址没有意义
      this.handlers.onClose(false);
      throw err;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.retryIndex = 0;
      this.handlers.onOpen();
    };

    ws.onmessage = (event: MessageEvent) => {
      if (typeof event.data === "string") {
        this.handleText(event.data);
        return;
      }
      if (event.data instanceof ArrayBuffer) {
        this.handleBinary(new Uint8Array(event.data));
      }
    };

    ws.onclose = () => {
      this.ws = null;
      if (this.closedByUs) {
        this.handlers.onClose(false);
        return;
      }
      const delay = RECONNECT_DELAYS_MS[Math.min(this.retryIndex, RECONNECT_DELAYS_MS.length - 1)];
      this.retryIndex += 1;
      const retrying = this.retryIndex <= RECONNECT_DELAYS_MS.length;
      this.handlers.onClose(retrying);
      if (retrying) {
        this.retryTimer = setTimeout(() => this.open(), delay);
      }
    };

    ws.onerror = () => {
      // onerror 之后必跟 onclose，重连统一在那边处理
    };
  }

  private handleText(raw: string): void {
    let parsed: { type?: unknown; data?: unknown };
    try {
      parsed = JSON.parse(raw) as { type?: unknown; data?: unknown };
    } catch {
      return;
    }
    if (typeof parsed.type !== "string") return;
    this.handlers.onEvent(parsed.type, parsed.data);
  }

  private handleBinary(bytes: Uint8Array): void {
    if (bytes.length < 4) return;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const eventType = view.getUint32(0, false);
    if (eventType === BINARY_TEXT) {
      this.handleText(new TextDecoder().decode(bytes.subarray(4)));
    }
    // 采样预览类帧（1/2/4）不展示实时预览，直接忽略
  }

  /** 主动关闭：与断线后的自动重连相对，调用后连接就此结束。 */
  close(): void {
    this.closedByUs = true;
    if (this.retryTimer !== null) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onclose = null;
      ws.onmessage = null;
      ws.onerror = null;
      try {
        ws.close();
      } catch {
        }
    }
  }
}

/** 分块编码并拼接：结果图可达数 MB，一次性传参会撑爆调用栈。 */
export function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}
