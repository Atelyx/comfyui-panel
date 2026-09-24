/**
 * 参数表单：从工作流与节点定义推导出可编辑字段。
 *
 * 三类输入：连线输入（值是上游节点引用）只读展示，改它等于改拓扑；字面量输入按 schema
 * 决定控件形态；定义里查不到的输入退化为 JSON 编辑，保证自定义节点也能改值而不是整个表单卡住。
 *
 * seed 在接口格式里只是一个数字，「每次随机」由提交时现场生成新值实现。
 */
import type { ApiPrompt, InputSlotSchema, ObjectInfoMap } from "../comfy/types";

export type FieldKind = "combo" | "int" | "float" | "string" | "boolean" | "json";

export interface FieldSpec {
  nodeId: string;
  /** 节点展示名（`_meta.title` 优先，便于用户对上 ComfyUI 里的节点）。 */
  nodeLabel: string;
  classType: string;
  input: string;
  kind: FieldKind;
  /** 由连线提供（只读）。 */
  connected: boolean;
  /** 连线来源（如 `12:MODEL`）。 */
  linkFrom?: string;
  /** 下拉候选值（`kind === "combo"`）。 */
  options?: string[];
  /** 候选值的展示名（节点声明了别名映射时才有）。 */
  optionLabels?: Record<string, string>;
  min?: number;
  max?: number;
  step?: number;
  multiline?: boolean;
  /** 提交时是否重新随机。 */
  isSeed: boolean;
}

/** 判定是否连线引用：形状为 `[上流节点 id, 槽位序号]`，且上流节点须在本工作流内。 */
function asLink(value: unknown, prompt: ApiPrompt): { nodeId: string; slot: number } | null {
  if (!Array.isArray(value) || value.length !== 2) return null;
  const [source, slot] = value;
  if (typeof source !== "string" || typeof slot !== "number") return null;
  // 上流节点不在本工作流内即非连线（可能只是恰好两元素的字面量数组）
  if (!(source in prompt)) return null;
  return { nodeId: source, slot };
}

/**
 * 取下拉候选。节点定义里有两种声明形式，都要认：
 * 首元素直接是候选数组，或首元素为 `"COMBO"` 而候选在选项字典的 `options`
 * （字符串数组，或「值 → 展示名」的映射）。
 */
function comboOptions(schema: InputSlotSchema | undefined): {
  options: string[];
  labels?: Record<string, string>;
} | null {
  const declared = schema?.[0];
  if (Array.isArray(declared)) {
    return { options: declared.map((item) => String(item)) };
  }
  if (declared === "COMBO") {
    const raw = schema?.[1]?.options;
    if (Array.isArray(raw)) return { options: raw.map((item) => String(item)) };
    if (raw && typeof raw === "object") {
      // 键是提交值，值是展示名
      const entries = Object.entries(raw as Record<string, unknown>);
      const labels: Record<string, string> = {};
      for (const [value, label] of entries) labels[value] = String(label);
      return { options: entries.map(([value]) => value), labels };
    }
  }
  return null;
}

/** 由 schema 与当前值推断控件形态。 */
function kindOf(schema: InputSlotSchema | undefined, value: unknown): FieldKind {
  if (comboOptions(schema)) return "combo";
  const declared = schema?.[0];
  if (typeof declared === "string") {
    if (declared === "INT") return "int";
    if (declared === "FLOAT") return "float";
    if (declared === "STRING") return "string";
    if (declared === "BOOLEAN") return "boolean";
  }
  // schema 缺失或是复合类型（IMAGE/MODEL/LATENT…）时按实际值兜底
  if (typeof value === "number") return Number.isInteger(value) ? "int" : "float";
  if (typeof value === "boolean") return "boolean";
  if (typeof value === "string") return "string";
  return "json";
}

/** 只取数字型的约束值，跳过 `"default"` 这类字符串标记。 */
function numberOption(options: Record<string, unknown> | undefined, key: string): number | undefined {
  const value = options?.[key];
  return typeof value === "number" ? value : undefined;
}

/** 按节点与输入声明顺序推导全部字段。 */
export function buildFields(prompt: ApiPrompt, objectInfo: ObjectInfoMap | null): FieldSpec[] {
  const fields: FieldSpec[] = [];
  for (const [nodeId, node] of Object.entries(prompt)) {
    const info = objectInfo?.[node.class_type];
    const required = info?.input?.required ?? {};
    const optional = info?.input?.optional ?? {};
    const nodeLabel = node._meta?.title?.trim() || info?.display_name || node.class_type;
    for (const [input, value] of Object.entries(node.inputs ?? {})) {
      const schema = required[input] ?? optional[input];
      const link = asLink(value, prompt);
      const combo = comboOptions(schema);
      const kind = kindOf(schema, value);
      fields.push({
        nodeId,
        nodeLabel,
        classType: node.class_type,
        input,
        kind,
        connected: link !== null,
        ...(link ? { linkFrom: `${link.nodeId}:${link.slot}` } : {}),
        ...(combo ? { options: combo.options } : {}),
        ...(combo?.labels ? { optionLabels: combo.labels } : {}),
        min: numberOption(schema?.[1], "min"),
        max: numberOption(schema?.[1], "max"),
        step: numberOption(schema?.[1], "step"),
        multiline: schema?.[1]?.multiline === true,
        isSeed: input === "seed" || input === "noise_seed",
      });
    }
  }
  return fields;
}

export function readField(prompt: ApiPrompt, field: FieldSpec): unknown {
  return prompt[field.nodeId]?.inputs?.[field.input];
}

/** 返回新对象，但只拷贝改动的那条路径：工作流可有上百节点，每次按键整体深拷贝会卡。 */
export function writeField(prompt: ApiPrompt, nodeId: string, input: string, value: unknown): ApiPrompt {
  const node = prompt[nodeId];
  if (!node) return prompt;
  return {
    ...prompt,
    [nodeId]: { ...node, inputs: { ...node.inputs, [input]: value } },
  };
}

/** 控件取回的永远是字符串，这里转成工作流应有的类型。 */
export function coerceFieldValue(field: FieldSpec, raw: string | boolean | number): unknown {
  if (field.kind === "boolean") return typeof raw === "boolean" ? raw : raw === "true";
  if (field.kind === "int" || field.kind === "float") {
    const num = typeof raw === "number" ? raw : Number(raw);
    if (!Number.isFinite(num)) return raw;
    return field.kind === "int" ? Math.trunc(num) : num;
  }
  if (field.kind === "json") {
    // 非法 JSON 原样存回，让用户看得见写错的内容（提交时服务端会指出）
    if (typeof raw !== "string") return raw;
    try {
      return JSON.parse(raw) as unknown;
    } catch {
      return raw;
    }
  }
  return raw;
}

export function displayValue(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  return JSON.stringify(value) ?? "";
}

/** 避开 0 与上界，与界面里「随机」按钮的行为一致。 */
export function randomSeed(): number {
  const bytes = new Uint8Array(4);
  crypto.getRandomValues(bytes);
  const view = new DataView(bytes.buffer);
  return view.getUint32(0, false) % 4294967295;
}

/** 勾了「每次随机」的字段在提交前换成新种子（key 为 `nodeId.input`）。 */
export function applySeedMode(prompt: ApiPrompt, seedMode: ReadonlySet<string>): ApiPrompt {
  let next = prompt;
  for (const key of seedMode) {
    const dot = key.indexOf(".");
    if (dot <= 0) continue;
    const nodeId = key.slice(0, dot);
    const input = key.slice(dot + 1);
    if (!next[nodeId]) continue;
    next = writeField(next, nodeId, input, randomSeed());
  }
  return next;
}

/** 从 "16:9 (Widescreen)" 这类值里取出比例对；解析不了返回 null。 */
export function parseAspectPair(value: string): { w: number; h: number } | null {
  const match = /^(\d+):(\d+)/.exec(value.trim());
  if (!match) return null;
  const w = Number(match[1]);
  const h = Number(match[2]);
  return w > 0 && h > 0 ? { w, h } : null;
}

/**
 * 默认勾选「每次随机」的种子字段：未连线的 seed/noise_seed。
 * 文件里 seed 的 control_after_generate 在转 API 格式时被丢弃，读不回来，就默认全部随机，
 * 需要固定种子时在表单里取消勾选。
 */
export function defaultSeedKeys(prompt: ApiPrompt): ReadonlySet<string> {
  const keys = new Set<string>();
  for (const [nodeId, node] of Object.entries(prompt)) {
    const inputs = node.inputs ?? {};
    for (const [input, value] of Object.entries(inputs)) {
      if (input !== "seed" && input !== "noise_seed") continue;
      if (Array.isArray(value) && value.length === 2 && typeof value[0] === "string" && value[0] in prompt) continue;
      keys.add(`${nodeId}.${input}`);
    }
  }
  return keys;
}

/** 勾选状态与随机种子的集合键。 */
export function fieldKey(field: FieldSpec): string {
  return `${field.nodeId}.${field.input}`;
}

/** 常见输入名的中文标签；未收录时退回原始输入名。 */
const INPUT_LABELS: Record<string, string> = {
  text: "提示词",
  positive: "正向提示词",
  negative: "反向提示词",
  negative_prompt: "反向提示词",
  empty_prompt: "提示词",
  prompt: "提示词",
  seed: "种子",
  noise_seed: "种子",
  steps: "步数",
  cfg: "CFG",
  sampler_name: "采样器",
  scheduler: "调度器",
  denoise: "降噪",
  width: "宽度",
  height: "高度",
  batch_size: "批次数",
  ckpt_name: "模型",
  model_name: "模型",
  lora_name: "LoRA",
};

/** 界面标签：优先中文名，原输入名进 title 供对回 ComfyUI。 */
export function fieldLabel(field: FieldSpec): string {
  return INPUT_LABELS[field.input] ?? field.input;
}

/** 置顶的常用输入（提示词/种子/采样/尺寸），其余仍按节点分组。 */
const COMMON_INPUTS = new Set([
  "text",
  "positive",
  "negative",
  "negative_prompt",
  "empty_prompt",
  "prompt",
  "seed",
  "noise_seed",
  "steps",
  "cfg",
  "sampler_name",
  "scheduler",
  "denoise",
  "width",
  "height",
  "batch_size",
]);

export function isCommonField(field: FieldSpec): boolean {
  return COMMON_INPUTS.has(field.input);
}

/** 统计被跳过或禁用的节点（接口格式通常已剔除它们，这里只用于给用户一句提示，不拦截）。 */
export function countDisabledNodes(prompt: ApiPrompt): number {
  let count = 0;
  for (const node of Object.values(prompt)) {
    const mode = (node as { mode?: unknown }).mode;
    if (mode === 2 || mode === 4) count += 1;
  }
  return count;
}
