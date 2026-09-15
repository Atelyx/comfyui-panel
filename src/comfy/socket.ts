/**
 * ComfyUI 的 WebSocket 客户端：实时进度、执行状态与生成中预览都只有这条通道有。
 *
 * 帧格式（三种预览帧的载荷结构不同，解析时不能混）：
 * - 文本帧 `{"type": ..., "data": ...}`；
 * - 二进制帧 = 4 字节大端事件类型 + 载荷；
 * - `PREVIEW_IMAGE`(1) 的载荷是 `4 字节格式码 + 图片字节`；
 *   `PREVIEW_IMAGE_WITH_METADATA`(4) 是 `4 字节元数据长度 + 元数据 JSON + 图片字节`（图片本身不带格式头）。
 *
 */

/** 二进制帧的事件类型（与 ComfyUI `protocol.BinaryEventTypes` 对齐）。 */
const BINARY_PREVIEW_IMAGE = 1;
const BINARY_UNENCODED_PREVIEW_IMAGE = 2;
const BINARY_TEXT = 3;
const BINARY_PREVIEW_IMAGE_WITH_METADATA = 4;

/** 预览图格式码 → MIME。 */
function imageMime(typeCode: number): string {
  return typeCode === 2 ? "image/png" : "image/jpeg";
}

/** 从带元数据帧的元数据 JSON 里取 MIME（取不到按 PNG 兜底，它是最常见的预览格式）。 */
function mimeFromMetadata(json: string): string {
  try {
    const parsed = JSON.parse(json) as { image_type?: unknown };
    if (typeof parsed.image_type === "string" && parsed.image_type.startsWith("image/")) {
      return parsed.image_type;
    }
  } catch {
    // 元数据不可解析时不影响图片本身
  }
  return "image/png";
}

export interface SocketHandlers {
  /** 文本事件（事件名 + 载荷对象）。 */
  onEvent(type: string, data: unknown): void;
  /** 生成中的预览帧（dataURL，可直接给 `<img>`）。 */
  onPreview(dataUrl: string): void;
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
      // 声明支持带元数据的预览帧，否则服务端会走旧的 UNENCODED 分支
      try {
        ws.send(JSON.stringify({ type: "feature_flags", data: { supports_preview_metadata: true } }));
      } catch {
        }
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
    const payload = bytes.subarray(4);
    if (eventType === BINARY_PREVIEW_IMAGE) {
      // 载荷 = 4 字节图片格式码 + 图片字节
      if (payload.length < 4) return;
      const header = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
      this.emitPreview(payload.subarray(4), imageMime(header.getUint32(0, false)));
      return;
    }
    if (eventType === BINARY_PREVIEW_IMAGE_WITH_METADATA) {
      // 载荷 = 4 字节元数据长度 + 元数据 JSON + 图片字节（图片字节不带格式头，MIME 在元数据里）
      if (payload.length < 4) return;
      const metaView = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
      const metaLength = metaView.getUint32(0, false);
      const imageStart = 4 + metaLength;
      if (payload.length < imageStart) return;
      const metaJson = new TextDecoder().decode(payload.subarray(4, imageStart));
      this.emitPreview(payload.subarray(imageStart), mimeFromMetadata(metaJson));
      return;
    }
    if (eventType === BINARY_UNENCODED_PREVIEW_IMAGE) {
      // 旧式帧没有格式信息（已声明支持带元数据的版本，正常收不到）；按 JPEG 兜底，浏览器会按内容嗅探
      this.emitPreview(payload, "image/jpeg");
      return;
    }
    if (eventType === BINARY_TEXT) {
      this.handleText(new TextDecoder().decode(payload));
    }
  }

  /** 预览帧 → dataURL。 */
  private emitPreview(data: Uint8Array, mime: string): void {
    if (data.length === 0) return;
    this.handlers.onPreview(`data:${mime};base64,${bytesToBase64(data)}`);
  }

  /** 主动关闭（不再重连）。 */
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

/** 分块编码并拼接：预览图可达数百 KB，一次性传参会撑爆调用栈。 */
export function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}
