/**
 * 工作流库：导入、校验与持久化 API 格式工作流。
 *
 * 为什么只收 API 格式：接口只接受「节点 id → {class_type, inputs}」，而本工具默认导出的是
 * 界面格式（含节点位置、连线、子图），服务端没有转换通道。自行把界面格式编译成接口格式要处理
 * 子图展平、控件值到输入名的映射、跳过/禁用语义，在自定义节点上极易出错；要求用户导出后再导入，
 * 换来的是提交的那张图正是用户在 ComfyUI 里验证过的。
 *
 * 正文与索引分开存：列表页不必把全部工作流正文读进内存。
 */
import type { AtelyxCtx } from "../ctx";
import type { ApiNode, ApiPrompt } from "../comfy/types";

const INDEX_KEY = "wf-index";
const ITEM_PREFIX = "wf:";

/** 列表项（不含正文）。 */
export interface WorkflowSummary {
  id: string;
  name: string;
  importedAt: number;
  nodeCount: number;
}

export interface StoredWorkflow extends WorkflowSummary {
  prompt: ApiPrompt;
}

/** 校验结果：不合法时指出哪里不对。 */
export interface ValidationResult {
  ok: boolean;
  error?: string;
  prompt?: ApiPrompt;
}

/** 十六进制 id：无路径语义，也不需要引 uuid 依赖。 */
function newId(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * 校验并归一化一份 API 格式工作流。
 *
 * 只做结构性校验，不检查节点类型是否存在：那取决于用户装了哪些自定义节点，
 * 交给服务端在提交时给出准确报错（它还会指出缺哪个节点）。
 */
export function validateApiPrompt(raw: unknown): ValidationResult {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch (err) {
      return { ok: false, error: `不是合法 JSON：${err instanceof Error ? err.message : String(err)}` };
    }
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ok: false, error: "顶层应是一个对象（节点 id → 节点定义）" };
  }
  const record = value as Record<string, unknown>;
  const ids = Object.keys(record);
  if (ids.length === 0) {
    return { ok: false, error: "工作流是空的" };
  }
  // 界面格式被误当接口格式导入时，给出可操作的指引而不是说「形状不对」
  if ("nodes" in record && Array.isArray(record.nodes)) {
    return {
      ok: false,
      error: "这是 UI 格式工作流。请在 ComfyUI 里用「工作流 → 导出（API）」导出后再导入",
    };
  }
  const prompt: ApiPrompt = {};
  let withClassType = 0;
  for (const id of ids) {
    const node = record[id];
    if (!node || typeof node !== "object" || Array.isArray(node)) {
      return { ok: false, error: `节点 ${id} 不是对象` };
    }
    const entry = node as Record<string, unknown>;
    const classType = entry.class_type;
    if (typeof classType !== "string" || !classType.trim()) {
      return { ok: false, error: `节点 ${id} 缺少 class_type` };
    }
    const inputs = entry.inputs;
    if (inputs !== undefined && (typeof inputs !== "object" || inputs === null || Array.isArray(inputs))) {
      return { ok: false, error: `节点 ${id} 的 inputs 不是对象` };
    }
    withClassType += 1;
    prompt[id] = {
      class_type: classType,
      inputs: (inputs as Record<string, unknown> | undefined) ?? {},
      ...(entry._meta ? { _meta: entry._meta as ApiNode["_meta"] } : {}),
    };
  }
  if (withClassType === 0) {
    return { ok: false, error: "没有任何带 class_type 的节点——看起来不是 API 格式工作流" };
  }
  return { ok: true, prompt };
}

async function readIndex(ctx: AtelyxCtx): Promise<WorkflowSummary[]> {
  try {
    const raw = await ctx.storage.get(INDEX_KEY);
    if (!Array.isArray(raw)) return [];
    // 索引虽由本插件写入，仍收敛一次形状（手改可能留下脏项）
    return raw.flatMap((item): WorkflowSummary[] => {
      if (!item || typeof item !== "object") return [];
      const row = item as Record<string, unknown>;
      if (typeof row.id !== "string" || typeof row.name !== "string") return [];
      return [
        {
          id: row.id,
          name: row.name,
          importedAt: typeof row.importedAt === "number" ? row.importedAt : 0,
          nodeCount: typeof row.nodeCount === "number" ? row.nodeCount : 0,
        },
      ];
    });
  } catch {
    return [];
  }
}

async function writeIndex(ctx: AtelyxCtx, index: WorkflowSummary[]): Promise<void> {
  await ctx.storage.set(INDEX_KEY, index);
}

/** 按导入时间倒序列出。 */
export async function listWorkflows(ctx: AtelyxCtx): Promise<WorkflowSummary[]> {
  const index = await readIndex(ctx);
  return index.sort((a, b) => b.importedAt - a.importedAt);
}

/** 读正文，不存在返回 null。 */
export async function loadWorkflow(ctx: AtelyxCtx, id: string): Promise<StoredWorkflow | null> {
  try {
    const raw = await ctx.storage.get(`${ITEM_PREFIX}${id}`);
    if (!raw || typeof raw !== "object") return null;
    const row = raw as Record<string, unknown>;
    const checked = validateApiPrompt(row.prompt);
    if (!checked.ok || !checked.prompt) return null;
    return {
      id,
      name: typeof row.name === "string" ? row.name : id,
      importedAt: typeof row.importedAt === "number" ? row.importedAt : 0,
      nodeCount: Object.keys(checked.prompt).length,
      prompt: checked.prompt,
    };
  } catch {
    return null;
  }
}

/** 未给名字时按节点构成自动起名。 */
export async function addWorkflow(
  ctx: AtelyxCtx,
  prompt: ApiPrompt,
  name?: string,
): Promise<StoredWorkflow> {
  const id = newId();
  const resolvedName = (name ?? "").trim() || suggestName(prompt);
  const stored: StoredWorkflow = {
    id,
    name: resolvedName,
    importedAt: Date.now(),
    nodeCount: Object.keys(prompt).length,
    prompt,
  };
  await ctx.storage.set(`${ITEM_PREFIX}${id}`, {
    id: stored.id,
    name: stored.name,
    importedAt: stored.importedAt,
    prompt: stored.prompt,
  });
  const index = await readIndex(ctx);
  index.push({
    id: stored.id,
    name: stored.name,
    importedAt: stored.importedAt,
    nodeCount: stored.nodeCount,
  });
  await writeIndex(ctx, index);
  return stored;
}

export async function renameWorkflow(ctx: AtelyxCtx, id: string, name: string): Promise<void> {
  const trimmed = name.trim();
  if (!trimmed) return;
  const stored = await loadWorkflow(ctx, id);
  if (!stored) return;
  await ctx.storage.set(`${ITEM_PREFIX}${id}`, {
    id: stored.id,
    name: trimmed,
    importedAt: stored.importedAt,
    prompt: stored.prompt,
  });
  const index = await readIndex(ctx);
  await writeIndex(
    ctx,
    index.map((item) => (item.id === id ? { ...item, name: trimmed } : item)),
  );
}

/** 正文与索引一并清掉，避免索引里留悬空项。 */
export async function deleteWorkflow(ctx: AtelyxCtx, id: string): Promise<void> {
  await ctx.storage.delete(`${ITEM_PREFIX}${id}`);
  const index = await readIndex(ctx);
  await writeIndex(
    ctx,
    index.filter((item) => item.id !== id),
  );
}

/** 导出为可直接分享的 JSON 文本。 */
export function exportWorkflow(workflow: StoredWorkflow): string {
  return JSON.stringify(workflow.prompt, null, 2);
}

/** 从节点构成里挑关键词起名：一排「工作流 1/2/3」用户无从分辨，类名天然可区分。 */
function suggestName(prompt: ApiPrompt): string {
  const classTypes = Object.values(prompt).map((node) => node.class_type);
  const priority = [
    "KSampler",
    "KSamplerAdvanced",
    "SamplerCustom",
    "QwenMultiangleCameraNode",
    "TextEncodeQwenImageEditPlus",
    "CheckpointLoaderSimple",
    "UNETLoader",
    "LoaderGGUF",
    "VAEDecode",
  ];
  const picked = priority.filter((name) => classTypes.includes(name)).slice(0, 2);
  const label = picked.length > 0 ? picked.join(" + ") : (classTypes[0] ?? "工作流");
  return `${label}`;
}
