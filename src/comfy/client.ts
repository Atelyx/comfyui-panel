/** ComfyUI 接口封装：拼路径、解 JSON、把失败翻成可读原因。请求一律经传输层。 */
import type {
  ApiPrompt,
  HistoryEntry,
  HistoryMap,
  ImageRef,
  ObjectInfoMap,
  PromptSubmitResult,
  QueueSnapshot,
  SystemStats,
} from "./types";
import type { ComfyTransport } from "./transport";

/** 服务端的错误体（提交校验失败时含 error 与逐节点报错）。 */
interface ErrorBody {
  error?: { type?: string; message?: string; details?: string };
  node_errors?: Record<string, unknown>;
}

/** 服务端错误体形状不统一，这里尽力取出一句可读原因。 */
function extractError(status: number, body: string): string {
  try {
    const parsed = JSON.parse(body) as ErrorBody;
    const message = parsed.error?.message ?? parsed.error?.details;
    if (message) {
      const nodeErrors = parsed.node_errors;
      if (nodeErrors && Object.keys(nodeErrors).length > 0) {
        return `${message}（节点报错：${JSON.stringify(nodeErrors).slice(0, 400)}）`;
      }
      return message;
    }
  } catch {
    // 非 JSON（如纯文本错误页）落到下面的截断原文
  }
  const text = body.trim().replace(/\s+/g, " ");
  return text ? `HTTP ${status}：${text.slice(0, 300)}` : `HTTP ${status}`;
}

export class ComfyClient {
  constructor(private readonly transport: ComfyTransport) {}

  private async getJson<T>(path: string, timeoutMs?: number): Promise<T> {
    const res = await this.transport.request(path, { timeoutMs });
    if (res.status !== 200) throw new Error(extractError(res.status, res.body));
    return JSON.parse(res.body) as T;
  }

  private async postJson<T>(path: string, payload: unknown): Promise<T> {
    const res = await this.transport.request(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (res.status !== 200) throw new Error(extractError(res.status, res.body));
    // 部分接口（/interrupt、/free、/queue 的写操作）返回空体，按 undefined 处理
    if (!res.body.trim()) return undefined as T;
    return JSON.parse(res.body) as T;
  }

  /** 系统与设备信息（连接状态、显存展示）。 */
  systemStats(): Promise<SystemStats> {
    return this.getJson<SystemStats>("/system_stats");
  }

  /** 全部节点定义（参数表单的 schema 来源；一次取回后缓存）。 */
  objectInfo(): Promise<ObjectInfoMap> {
    return this.getJson<ObjectInfoMap>("/object_info", 30000);
  }

  /** 队列快照（运行中 + 等待中；不含进度数值，进度只在 WS 上）。 */
  queue(): Promise<QueueSnapshot> {
    return this.getJson<QueueSnapshot>("/queue");
  }

  /** 历史记录（`max_items` 取最近的若干条）。 */
  history(maxItems: number): Promise<HistoryMap> {
    return this.getJson<HistoryMap>(`/history?max_items=${maxItems}`);
  }

  /** 单条历史（任务完成后精确取结果）。 */
  async historyItem(promptId: string): Promise<HistoryEntry | null> {
    const map = await this.getJson<HistoryMap>(`/history/${encodeURIComponent(promptId)}`);
    return map[promptId] ?? null;
  }

  /** 提交任务。`clientId` 与 WS 一致，服务端据此把进度推给本面板。 */
  async submit(prompt: ApiPrompt, clientId: string): Promise<PromptSubmitResult> {
    const result = await this.postJson<PromptSubmitResult>("/prompt", {
      prompt,
      client_id: clientId,
    });
    return result ?? {};
  }

  /** 中断当前执行（服务端会在下一个采样步停下）。 */
  interrupt(): Promise<void> {
    return this.postJson<void>("/interrupt", {});
  }

  /** 中断指定任务（只在该任务正在执行时才生效）。 */
  interruptPrompt(promptId: string): Promise<void> {
    return this.postJson<void>("/interrupt", { prompt_id: promptId });
  }

  /** 释放模型 / 显存（ComfyUI 的 `free`）。 */
  free(unloadModels: boolean, freeMemory: boolean): Promise<void> {
    return this.postJson<void>("/free", { unload_models: unloadModels, free_memory: freeMemory });
  }

  /** 删除队列中的等待项或历史项。 */
  deleteQueueItems(ids: string[]): Promise<void> {
    return this.postJson<void>("/queue", { delete: ids });
  }

  /** 清空队列（等待项）。 */
  clearQueue(): Promise<void> {
    return this.postJson<void>("/queue", { clear: true });
  }

  /** 取图片字节（仅直连通道可用）。 */
  imageBytes(ref: ImageRef): Promise<Uint8Array> {
    const params = new URLSearchParams({
      filename: ref.filename,
      subfolder: ref.subfolder,
      type: ref.type,
    });
    return this.transport.requestBytes(`/view?${params.toString()}`);
  }

  /** 图片展示 URL（缩略图走服务端 webp 压缩）。 */
  imageUrl(ref: ImageRef, preview = true): string {
    return this.transport.viewUrl(ref, preview);
  }

  /** 上传图片到 ComfyUI 的 input 目录（`POST /upload/image`，multipart）。 */
  async uploadImage(file: File, subfolder = ""): Promise<ImageRef> {
    const form = new FormData();
    form.append("image", file);
    form.append("overwrite", "true");
    if (subfolder) form.append("subfolder", subfolder);
    const res = await fetch(`${this.transport.baseUrl}/upload/image`, { method: "POST", body: form });
    if (!res.ok) throw new Error(extractError(res.status, await res.text()));
    const data = (await res.json()) as { name?: string; subfolder?: string; type?: string };
    return {
      filename: data.name ?? file.name,
      subfolder: data.subfolder ?? subfolder,
      type: data.type ?? "input",
    };
  }
}
