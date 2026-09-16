/**
 * 运行时：连接、队列、进度与结果的唯一权威状态，界面经 `useSyncExternalStore` 消费。
 *
 * 状态聚在这里而非散在组件里：面板与设置页都要读连接状态，轮询与长连接必须只有一份
 * （多份会互相打架）；其生命周期也要跟插件启停绑定，而不是跟某个组件挂载绑定，
 * 否则切面板就断连接。更新一律换新对象，因为订阅方靠引用比较决定是否重渲染。
 */
import { ComfyClient } from "./comfy/client";
import { ComfySocket, type SocketHandlers } from "./comfy/socket";
import { ComfyTransport, type Channel } from "./comfy/transport";
import type {
  ApiPrompt,
  ExecutingPayload,
  ExecutionErrorPayload,
  HistoryEntry,
  HistoryMap,
  ImageRef,
  ObjectInfoMap,
  OutputEntry,
  ProgressPayload,
  QueueSnapshot,
  StatusPayload,
  SystemStats,
} from "./comfy/types";
import { baseUrl, type ComfySettings } from "./settings";
import {
  listWorkflows,
  loadWorkflow as loadWorkflowFromLibrary,
  saveWorkflowAsFile as saveWorkflowFileToLibrary,
  renameWorkflow as renameWorkflowInLibrary,
  deleteWorkflow as deleteWorkflowInLibrary,
  type LoadWorkflowResult,
  type SaveWorkflowResult,
} from "./workflow/library";
import { diffWorkflows, type WorkflowFileInfo } from "./workflow/files";

/** 一条生成结果（历史里的一张图）。 */
export interface ResultImage {
  /** promptId 加文件路径，用于去重与选中。 */
  key: string;
  promptId: string;
  ref: ImageRef;
  /** 提交时记下的标题，给画廊做标签。 */
  title: string;
  createdAt: number;
  /** 失败的任务仍可能产出部分图。 */
  failed: boolean;
  /** 保存到仓库后的回填路径。 */
  savedPath?: string;
}

export interface RuntimeSnapshot {
  settings: ComfySettings;
  channel: Channel;
  /** 首屏与手动重连时为 true。 */
  probing: boolean;
  systemStats: SystemStats | null;
  queue: QueueSnapshot;
  /** 实时进度（长连接推送）。 */
  progress: ProgressPayload | null;
  /** 正在执行的任务 id。 */
  runningPromptId: string | null;
  /** 展示后由界面清除。 */
  error: string;
  /** 最近的在前。 */
  results: ResultImage[];
  objectInfo: ObjectInfoMap | null;
  /** ComfyUI 工作流目录里的文件（含 mtime），编排界面保存后秒级刷新。 */
  workflows: WorkflowFileInfo[];
  /** 列表读取失败（如旧版 ComfyUI 无 userdata 接口）时的原因。 */
  workflowsError: string;
}

/** 长连接主导进度，轮询只补队列与历史。 */
const QUEUE_POLL_MS = 2000;
/** 工作流目录轮询：编排界面保存后几秒内感知，不必追求瞬时。 */
const WORKFLOW_POLL_MS = 3000;
/** 画廊保留上限。 */
const MAX_RESULTS = 80;
/** 首屏回填条数。 */
const HISTORY_BOOTSTRAP = 30;

const EMPTY_QUEUE: QueueSnapshot = { queue_running: [], queue_pending: [] };

/** 面板与命令共用。 */
export interface RunRequest {
  prompt: ApiPrompt;
  /** 写入画廊标签。 */
  title: string;
}

export class ComfyRuntime {
  private transport: ComfyTransport;
  private client: ComfyClient;
  private socket: ComfySocket | null = null;
  private clientId: string;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private workflowPollTimer: ReturnType<typeof setInterval> | null = null;
  private readonly listeners = new Set<() => void>();
  private snap: RuntimeSnapshot;
  /** 已折进画廊的任务 id，避免轮询重复展开。 */
  private readonly ingestedPrompts = new Set<string>();
  /** 提交时记下的标题；历史回填的任务可能没有。 */
  private readonly titles = new Map<string, string>();

  constructor(settings: ComfySettings) {
    this.transport = new ComfyTransport(settings);
    this.client = new ComfyClient(this.transport);
    this.clientId = randomClientId();
    this.snap = {
      settings,
      channel: "offline",
      probing: false,
      systemStats: null,
      queue: EMPTY_QUEUE,
      progress: null,
      runningPromptId: null,
      error: "",
      results: [],
      objectInfo: null,
      workflows: [],
      workflowsError: "",
    };
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): RuntimeSnapshot => this.snap;

  private emit(patch: Partial<RuntimeSnapshot>): void {
    this.snap = { ...this.snap, ...patch };
    for (const listener of this.listeners) listener();
  }

  /** 地址或端口可能变了，需要重建传输层。 */
  applySettings(settings: ComfySettings): void {
    const baseChanged = baseUrl(settings) !== baseUrl(this.snap.settings);
    this.snap = { ...this.snap, settings };
    if (baseChanged) {
      this.teardownConnection();
      this.transport = new ComfyTransport(settings);
      this.client = new ComfyClient(this.transport);
    }
    this.emit({});
  }

  /** 成功则接管轮询与长连接。 */
  async connect(): Promise<void> {
    this.emit({ probing: true, error: "" });
    const state = await this.transport.probe();
    this.emit({ channel: state.channel, probing: false });
    if (state.channel === "offline") {
      this.teardownConnection();
      return;
    }
    await this.refreshSystemStats();
    await this.refreshQueue();
    await this.refreshHistory();
    this.startPolling();
    this.startWorkflowPolling();
    this.startSocket();
    // 节点定义体积较大，闲时取一次，不阻塞上面的首屏数据
    void this.loadObjectInfo();
  }

  /** 插件停用、切地址、服务下线时调用。 */
  disconnect(): void {
    this.teardownConnection();
    this.emit({ channel: "offline" });
  }

  private teardownConnection(): void {
    if (this.pollTimer !== null) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
    if (this.workflowPollTimer !== null) {
      clearInterval(this.workflowPollTimer);
      this.workflowPollTimer = null;
    }
    if (this.socket) {
      this.socket.close();
      this.socket = null;
    }
    this.emit({ progress: null, runningPromptId: null });
  }

  private startPolling(): void {
    if (this.pollTimer !== null) return;
    this.pollTimer = setInterval(() => {
      void this.refreshQueue();
      void this.refreshHistory();
    }, QUEUE_POLL_MS);
  }

  private startWorkflowPolling(): void {
    if (this.workflowPollTimer !== null) return;
    void this.refreshWorkflows();
    this.workflowPollTimer = setInterval(() => {
      void this.refreshWorkflows();
    }, WORKFLOW_POLL_MS);
  }

  /**
   * 拉取工作流目录列表并与上次对比，有变化才通知订阅方。
   * 只读 mtime/size，不读文件正文；失败（如旧版 ComfyUI 无 userdata 接口）记入 workflowsError。
   */
  async refreshWorkflows(): Promise<void> {
    try {
      const files = await listWorkflows(this.client);
      const previous = new Map(this.snap.workflows.map((file) => [file.path, file]));
      const { changed, removed } = diffWorkflows(previous, files);
      const anyChange = changed.length > 0 || removed.length > 0;
      if (!anyChange && files.length === previous.size) {
        if (this.snap.workflowsError) this.emit({ workflowsError: "" });
        return;
      }
      this.emit({ workflows: files, workflowsError: "" });
    } catch (err) {
      const message = describe(err);
      if (message !== this.snap.workflowsError) this.emit({ workflowsError: message });
    }
  }

  /** 读取并归一化一个工作流文件为可提交的 prompt。 */
  loadWorkflow(file: WorkflowFileInfo): Promise<LoadWorkflowResult> {
    return loadWorkflowFromLibrary(this.client, file);
  }

  /** 保存为新文件（覆盖同名），成功后立即刷新列表。 */
  async saveWorkflowFile(name: string, content: string): Promise<SaveWorkflowResult> {
    const result = await saveWorkflowFileToLibrary(this.client, name, content);
    if (result.ok) void this.refreshWorkflows();
    return result;
  }

  /** 重命名文件，成功后立即刷新列表。 */
  async renameWorkflowFile(path: string, newName: string): Promise<void> {
    await renameWorkflowInLibrary(this.client, path, newName);
    void this.refreshWorkflows();
  }

  /** 删除文件，成功后立即刷新列表。 */
  async deleteWorkflowFile(path: string): Promise<void> {
    await deleteWorkflowInLibrary(this.client, path);
    void this.refreshWorkflows();
  }

  private startSocket(): void {
    if (this.socket) this.socket.close();
    const handlers: SocketHandlers = {
      onEvent: (type, data) => this.handleSocketEvent(type, data),
      onOpen: () => undefined,
      onClose: (retrying) => {
        // 清进度但保留结果：重连后服务端会补推当前节点，进度随之恢复
        this.emit({ progress: null, runningPromptId: null });
        if (!retrying) void this.probeChannelOnly();
      },
    };
    this.socket = new ComfySocket(this.transport.socketUrl(this.clientId), handlers);
    this.socket.connect();
  }

  /** 长连接彻底断开时确认服务是否还在。 */
  private async probeChannelOnly(): Promise<void> {
    const state = await this.transport.probe();
    this.emit({ channel: state.channel });
  }

  private handleSocketEvent(type: string, data: unknown): void {
    if (type === "progress") {
      const payload = data as ProgressPayload;
      this.emit({
        progress: payload,
        runningPromptId: payload.prompt_id ?? this.snap.runningPromptId,
      });
      return;
    }
    if (type === "executing") {
      const payload = data as ExecutingPayload;
      // node 为 null 表示本轮结束
      if (payload.node === null) {
        this.emit({ progress: null, runningPromptId: null });
        void this.refreshQueue();
        void this.refreshHistory();
      } else {
        const promptId = payload.prompt_id ?? this.snap.runningPromptId;
        this.emit({ runningPromptId: promptId ?? null });
      }
      return;
    }
    if (type === "execution_error") {
      const payload = data as ExecutionErrorPayload;
      const node = payload.node_type ? `节点 ${payload.node_type}` : "执行";
      const message = payload.exception_message ?? "未知错误";
      this.emit({ error: `${node}出错：${message}`, progress: null });
      void this.refreshHistory();
      return;
    }
    if (type === "status") {
      const payload = data as StatusPayload;
      void payload;
      void this.refreshQueue();
      return;
    }
    if (type === "execution_start" || type === "execution_cached") {
      void this.refreshQueue();
    }
  }

  async refreshSystemStats(): Promise<void> {
    try {
      const stats = await this.client.systemStats();
      this.emit({ systemStats: stats });
    } catch (err) {
      this.emit({ error: describe(err) });
    }
  }

  async refreshQueue(): Promise<void> {
    try {
      const queue = await this.client.queue();
      this.emit({ queue });
    } catch {
      // 连接状态已由通道字段表达，单次轮询失败不值得打扰用户
    }
  }

  /** 取历史并把它折进画廊（只吃有图的任务）。 */
  async refreshHistory(): Promise<void> {
    try {
      const history = await this.client.history(HISTORY_BOOTSTRAP);
      this.ingestHistory(history);
    } catch {
      // 同上，静默
    }
  }

  private ingestHistory(history: HistoryMap): void {
    const fresh: ResultImage[] = [];
    for (const [promptId, entry] of Object.entries(history)) {
      if (this.ingestedPrompts.has(promptId)) continue;
      this.ingestedPrompts.add(promptId);
      fresh.push(...imagesOf(promptId, entry, this.titles.get(promptId) ?? ""));
    }
    if (fresh.length === 0) return;
    // 按时间倒序并裁到上限：画廊不分页，超出丢弃最老的
    const merged = [...fresh, ...this.snap.results]
      .sort((a, b) => b.createdAt - a.createdAt)
      .slice(0, MAX_RESULTS);
    this.emit({ results: merged });
  }

  async loadObjectInfo(): Promise<void> {
    try {
      const info = await this.client.objectInfo();
      this.emit({ objectInfo: info });
    } catch {
      // 取不到时表单退化为 JSON 编辑，不阻塞生成
    }
  }

  /** 返回任务 id 供调用方在画廊里定位结果；失败抛错由调用方展示。 */
  async run(request: RunRequest): Promise<string> {
    const result = await this.client.submit(request.prompt, this.clientId);
    const promptId = result.prompt_id;
    if (!promptId) {
      throw new Error(result.error?.message ?? "提交失败：服务端未返回任务 id");
    }
    // 标题必须在任务进历史前登记，否则轮询会先用兜底标题把它折进画廊；
    // 提交与轮询存在竞态，故这里还要回填已被折进去的结果。
    this.titles.set(promptId, request.title);
    this.emit({
      runningPromptId: promptId,
      error: "",
      progress: null,
      results: this.snap.results.map((item) =>
        item.promptId === promptId && item.title !== request.title ? { ...item, title: request.title } : item,
      ),
    });
    void this.refreshQueue();
    return promptId;
  }

  async interrupt(): Promise<void> {
    await this.client.interrupt();
    this.emit({ progress: null });
    void this.refreshQueue();
  }

  /** 等待中的任务也能指定取消。 */
  async interruptPrompt(promptId: string): Promise<void> {
    await this.client.interruptPrompt(promptId);
    void this.refreshQueue();
  }

  async cancelQueued(promptId: string): Promise<void> {
    await this.client.deleteQueueItems([promptId]);
    void this.refreshQueue();
  }

  /** 不影响队列。 */
  async freeMemory(): Promise<void> {
    await this.client.free(true, true);
  }

  /** 落库用；仅直连通道可取。 */
  imageBytes(ref: ImageRef): Promise<Uint8Array> {
    return this.client.imageBytes(ref);
  }

  /** 缩略图地址。 */
  imageUrl(ref: ImageRef, preview = true): string {
    return this.client.imageUrl(ref, preview);
  }

  /** 回填已落库路径。 */
  markSaved(key: string, savedPath: string): void {
    this.emit({
      results: this.snap.results.map((item) => (item.key === key ? { ...item, savedPath } : item)),
    });
  }

  clearError(): void {
    if (this.snap.error) this.emit({ error: "" });
  }

  /** ComfyUI 的基址（编排面板用它作内嵌地址）。 */
  get baseUrlForUI(): string {
    return this.transport.baseUrl;
  }

  /** 提交与长连接必须用同一个 id。 */
  getClientId(): string {
    return this.clientId;
  }

  /** 服务端重启后旧会话会失效，重连前换一个。 */
  rotateClientId(): void {
    this.clientId = randomClientId();
  }
}

/** 与服务端生成的会话 id 同形，避免出现它不认的字符。 */
function randomClientId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** 把历史条目里的图片拍平成画廊条目。 */
function imagesOf(promptId: string, entry: HistoryEntry, title: string): ResultImage[] {
  const outputs: Record<string, OutputEntry> = entry.outputs ?? {};
  const failed = entry.status?.status_str === "error";
  // 提交时间在附加数据里；取不到就当作刚完成——排序只需单调，不必绝对准确
  const extra = entry.prompt?.[3] as { create_time?: unknown } | undefined;
  const createdAt = typeof extra?.create_time === "number" ? extra.create_time : Date.now();
  const items: ResultImage[] = [];
  for (const output of Object.values(outputs)) {
    const refs: ImageRef[] = [];
    if (Array.isArray(output.images)) refs.push(...output.images);
    if (Array.isArray(output.gifs)) refs.push(...output.gifs);
    for (const ref of refs) {
      if (!ref?.filename) continue;
      items.push({
        key: `${promptId}:${ref.subfolder}/${ref.filename}`,
        promptId,
        ref: { filename: ref.filename, subfolder: ref.subfolder ?? "", type: ref.type ?? "output" },
        title: title || promptTitleFallback(entry),
        createdAt,
        failed,
      });
    }
  }
  return items;
}

  /** 没有用户标题时用节点标题兜底，比「未知任务」可读。 */
  function promptTitleFallback(entry: HistoryEntry): string {
    const prompt = entry.prompt?.[2];
    if (!prompt) return "未命名任务";
    for (const node of Object.values(prompt)) {
      const title = node?._meta?.title;
      if (typeof title === "string" && title.trim()) return title;
    }
    return "未命名任务";
  }

function describe(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
