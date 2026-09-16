/**
 * UI 格式 → API 格式转换。
 *
 * ComfyUI 编辑器保存的工作流是 UI 格式（nodes/links/widgets_values），而提交 `/prompt`
 * 只接受 API 格式（节点 id → {class_type, inputs}）。本模块照搬 ComfyUI 前端
 * `graphToPrompt` 的映射规则，只依赖文件里的结构，不查节点定义：
 *
 * - 跳过禁用（mode 4）与 groupNode 节点；
 * - Reroute 是前端虚拟节点，不进 prompt，输入连线沿它的输入链路穿透；
 * - widget 值取 `widgets_values_named`（按名）或 `widgets_values`（按序）；按序取值时
 *   seed/noise_seed 之后各占一个 control_after_generate 槽位，多出的尾部值视为
 *   纯展示 widget，忽略；
 * - 指向已剔除节点的连线（悬空）在收尾时删掉，与前端行为一致。
 *
 * 子图节点（type 是 definitions.subgraphs 里的 id）无法从文件单点还原，转换直接失败，
 * 由调用方提示用户在编排界面打开——给出一份错位的 prompt 比拒绝更糟。
 */
import type { ApiNode, ApiPrompt } from "../comfy/types";

/** 转换失败时给出原因；成功时给出可提交的 prompt。 */
export type ConvertResult = { ok: true; prompt: ApiPrompt } | { ok: false; error: string };

/** 文件里节点 `inputs` 数组的条目。 */
interface UiInputEntry {
  name?: string;
  type?: string;
  link?: number | null;
  widget?: { name?: string } | null;
}

interface UiNode {
  id: number | string;
  type?: string;
  mode?: number;
  title?: string;
  inputs?: UiInputEntry[];
  widgets_values?: unknown[];
  widgets_values_named?: Record<string, unknown> | null;
}

/** 文件里 links 数组条目：[id, origin_id, origin_slot, target_id, target_slot, type]。 */
type UiLink = [number | string, number | string, number, number | string, number, string];

/** 子图节点与普通节点混在 nodes 里，靠 definitions.subgraphs 的 id 识别。 */
interface UiWorkflow {
  nodes?: UiNode[];
  links?: Array<UiLink | Record<string, unknown>>;
  definitions?: { subgraphs?: Array<{ id?: string }> };
}

/** seed 类输入后，前端会插入一个 control_after_generate widget，占用一个 widgets_values 槽位。 */
const SEED_INPUTS = new Set(["seed", "noise_seed"]);

/** 纯展示 widget 的类型（不进 API，不占可编辑输入位）。 */
const SKIP_NODE_TYPES = new Set(["groupNode", "Reroute"]);

/** 是否已经是 API 格式（顶层节点 id → {class_type}）。 */
export function isApiFormat(raw: unknown): boolean {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return false;
  const record = raw as Record<string, unknown>;
  if ("nodes" in record) return false;
  return Object.values(record).some(
    (value) => !!value && typeof value === "object" && !Array.isArray(value)
      && typeof (value as Record<string, unknown>).class_type === "string",
  );
}

/**
 * 转换一份 UI 格式工作流。
 * `raw` 可以是 JSON 文本或已解析对象；顶层若不是 UI 格式会直接失败。
 */
export function convertUiToApi(raw: unknown): ConvertResult {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch (err) {
      return { ok: false, error: `不是合法 JSON：${err instanceof Error ? err.message : String(err)}` };
    }
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ok: false, error: "顶层应是一个对象" };
  }
  if (isApiFormat(value)) {
    return { ok: false, error: "这已是 API 格式，无需转换" };
  }
  const workflow = value as UiWorkflow;
  if (!Array.isArray(workflow.nodes)) {
    return { ok: false, error: "不是可识别的工作流 JSON（缺少 nodes 数组）" };
  }

  // 子图节点的 type 是 definitions.subgraphs 里的 id
  const subgraphIds = new Set<string>();
  for (const sub of workflow.definitions?.subgraphs ?? []) {
    if (sub?.id) subgraphIds.add(sub.id);
  }
  for (const node of workflow.nodes) {
    if (node?.type && subgraphIds.has(node.type)) {
      return {
        ok: false,
        error: `节点 ${String(node.id)} 是子图，暂不支持在生成面板转换，请在编排界面打开`,
      };
    }
  }

  const nodeById = new Map<number | string, UiNode>();
  for (const node of workflow.nodes) {
    if (node && node.id !== undefined && node.id !== null) nodeById.set(node.id, node);
  }

  const linkById = new Map<number, [number | string, number]>();
  for (const link of workflow.links ?? []) {
    if (!Array.isArray(link) || link.length < 5) continue;
    const [id, originId, originSlot] = link as UiLink;
    if (originId === null || originId === undefined || id === null || id === undefined) continue;
    linkById.set(id as number, [originId, originSlot]);
  }

  // 被剔除的节点：禁用（mode 4）、虚拟节点。它们的输入连线要么穿透（Reroute），要么悬空丢弃
  const excluded = new Set<number | string>();
  for (const node of workflow.nodes) {
    if (!node) continue;
    if (node.mode === 4 || SKIP_NODE_TYPES.has(node.type ?? "")) excluded.add(node.id);
  }

  /** 沿虚拟节点穿透到真实节点：Reroute 的输入就是它要透传的连线。 */
  const resolveOrigin = (originId: number | string, originSlot: number): [string, number] | null => {
    let current = originId;
    let slot = originSlot;
    for (let depth = 0; depth < 32; depth += 1) {
      if (!excluded.has(current)) {
        return nodeById.has(current) ? [String(current), slot] : null;
      }
      const node = nodeById.get(current);
      if (!node) return null;
      const linked = node.inputs?.find((input) => input.link != null);
      if (!linked || linked.link == null) return null;
      const next = linkById.get(linked.link);
      if (!next) return null;
      current = next[0];
      slot = next[1];
    }
    return null;
  };

  const prompt: ApiPrompt = {};
  for (const node of workflow.nodes) {
    if (!node || node.id === undefined || node.id === null) continue;
    if (excluded.has(node.id)) continue;
    const nodeId = String(node.id);
    const inputs: Record<string, unknown> = {};
    const positional = Array.isArray(node.widgets_values) ? node.widgets_values : null;
    const named = node.widgets_values_named;
    let widgetIdx = 0;
    for (const input of node.inputs ?? []) {
      const name = input.name;
      if (!name) continue;
      if (input.link != null) {
        const target = linkById.get(input.link);
        if (target) {
          const resolved = resolveOrigin(target[0], target[1]);
          if (resolved) inputs[name] = [resolved[0], resolved[1]];
        }
        continue;
      }
      if (!input.widget) continue;
      // 字面量值：有 named 映射按名取，否则按序从 widgets_values 取
      let value: unknown;
      if (named && typeof named === "object") {
        value = named[name];
        const widgetName = input.widget.name;
        if (value === undefined && widgetName) value = named[widgetName];
        if (value === undefined) continue;
      } else if (positional) {
        if (widgetIdx >= positional.length) continue;
        value = positional[widgetIdx];
        widgetIdx += 1;
        if (SEED_INPUTS.has(name) && widgetIdx < positional.length) widgetIdx += 1;
      } else {
        continue;
      }
      inputs[name] = value;
    }
    const entry: ApiNode = { class_type: node.type ?? "", inputs };
    if (node.title) entry._meta = { title: node.title };
    prompt[nodeId] = entry;
  }

  if (Object.keys(prompt).length === 0) {
    return { ok: false, error: "没有可执行的节点" };
  }

  // 悬空连线：指向已剔除节点的引用删掉，与前端 graphToPrompt 的收尾一致
  for (const node of Object.values(prompt)) {
    for (const [name, value] of Object.entries(node.inputs)) {
      if (Array.isArray(value) && value.length === 2 && typeof value[0] === "string" && !(value[0] in prompt)) {
        delete node.inputs[name];
      }
    }
  }

  return { ok: true, prompt };
}
