/**
 * ComfyUI 接口的数据形状。
 *
 * 只声明本插件消费/产生的字段：响应里的其余键不建模，用到时再加。
 */

/** API 格式工作流，即提交给 ComfyUI 的 `prompt` 字段形状。 */
export interface ApiPrompt {
  [nodeId: string]: ApiNode;
}

export interface ApiNode {
  class_type: string;
  inputs: Record<string, unknown>;
  /** 导出时带的展示信息，提交时原样带回即可，服务端不校验。 */
  _meta?: { title?: string; [key: string]: unknown };
}

/** 输入槽的 schema，即节点定义里 `input.required`/`input.optional` 的条目。 */
export type InputSlotSchema = [unknown, Record<string, unknown>?];

export interface ObjectInfoInput {
  required?: Record<string, InputSlotSchema>;
  optional?: Record<string, InputSlotSchema>;
}

/** 单个节点类的定义。 */
export interface ObjectInfo {
  input?: ObjectInfoInput;
  output?: string[];
  output_name?: string[];
  name?: string;
  display_name?: string;
  description?: string;
  category?: string;
  output_node?: boolean;
  deprecated?: boolean;
  experimental?: boolean;
}

export type ObjectInfoMap = Record<string, ObjectInfo>;

/** 队列条目：[序号, 任务 id, 工作流, 附加数据, 输出节点]（服务端已裁掉敏感字段）。 */
export type QueueItem = [number, string, ApiPrompt, Record<string, unknown>, string[]];

export interface QueueSnapshot {
  queue_running: QueueItem[];
  queue_pending: QueueItem[];
}

/** 某节点的输出：按输出节点类别不同含 images/gifs/text/audio 等。 */
export interface OutputEntry {
  images?: Array<{ filename: string; subfolder: string; type: string }>;
  gifs?: Array<{ filename: string; subfolder: string; type: string }>;
  text?: string[];
  audio?: Array<{ filename: string; subfolder: string; type: string }>;
  [key: string]: unknown;
}

export interface HistoryStatus {
  status_str?: string;
  completed?: boolean;
  messages?: Array<[string, Record<string, unknown>]>;
}

export interface HistoryEntry {
  prompt: QueueItem;
  outputs: Record<string, OutputEntry>;
  status?: HistoryStatus;
  meta?: Record<string, unknown>;
}

/** 历史记录：prompt_id → 条目。 */
export type HistoryMap = Record<string, HistoryEntry>;

/** 采样进度（只在 WebSocket 上）。 */
export interface ProgressPayload {
  value: number;
  max: number;
  prompt_id: string | null;
  node: string | null;
}

/** 当前执行节点（node 为 null 表示本轮结束）。 */
export interface ExecutingPayload {
  node: string | null;
  prompt_id?: string | null;
  display_node?: string | null;
}

/** 执行错误（只列展示需要的字段）。 */
export interface ExecutionErrorPayload {
  prompt_id?: string;
  node_id?: string;
  node_type?: string;
  exception_type?: string;
  exception_message?: string;
  traceback?: string[];
}

/** 队列状态。 */
export interface StatusPayload {
  status?: { exec_info?: { queue_remaining?: number } };
  sid?: string;
}

/** 设备信息（只取展示用字段）。 */
export interface SystemDevice {
  name?: string;
  type?: string;
  index?: number;
  vram_total?: number;
  vram_free?: number;
  torch_vram_total?: number;
  torch_vram_free?: number;
}

export interface SystemStats {
  system?: {
    os?: string;
    python_version?: string;
    comfyui_version?: string;
    pytorch_version?: string;
    ram_total?: number;
    ram_free?: number;
    [key: string]: unknown;
  };
  devices?: SystemDevice[];
  [key: string]: unknown;
}

/** 提交结果。 */
export interface PromptSubmitResult {
  prompt_id?: string;
  number?: number;
  node_errors?: Record<string, unknown>;
  error?: { type?: string; message?: string; details?: string };
}

/** 图片引用，即取图接口的查询参数来源。 */
export interface ImageRef {
  filename: string;
  subfolder: string;
  type: string;
}

/**
 * userdata 目录列表条目（`GET /userdata?dir=...&full_info=true`）。
 * `path` 相对当前用户目录（如 `workflows/xx.json`）；`modified` 为毫秒时间戳。
 */
export interface UserDataEntry {
  path: string;
  size: number;
  modified: number;
  created: number;
}
