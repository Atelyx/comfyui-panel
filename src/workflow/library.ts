/**
 * 工作流库：文件即工作流，库的清单就是 ComfyUI 用户目录里的工作流文件。
 *
 * 列表来自 `/userdata` 目录接口；读文件时按格式分派——API 格式直接校验使用，
 * UI 格式经 `convertUiToApi` 转换（规则照搬 ComfyUI 前端）。转换失败的文件仍可列出，
 * 由界面提示用户在编排界面打开，不产出错位的 prompt。
 */
import type { ComfyClient } from "../comfy/client";
import type { ApiPrompt } from "../comfy/types";
import { isApiFormat, convertUiToApi, applyDraftToUi } from "./convert";
import {
  listWorkflowFiles,
  readWorkflowFile,
  writeWorkflowFile,
  type WorkflowFileInfo,
} from "./files";

/** 加载结果：转换/校验失败时给出可展示的原因。 */
export type LoadWorkflowResult =
  | { ok: true; workflow: StoredWorkflow }
  | { ok: false; error: string };

/** 参数写回结果。 */
export type SaveParamsResult = { ok: true } | { ok: false; error: string };

/** 已加载的工作流：prompt 是可直接提交的 API 格式。 */
export interface StoredWorkflow {
  /** 文件相对路径（`workflows/xx.json`）。 */
  id: string;
  name: string;
  /** 加载时刻的 mtime，界面据此判断文件是否在编排界面被改过。 */
  modified: number;
  prompt: ApiPrompt;
}

/** 校验结果：不合法时指出哪里不对。 */
export interface ValidationResult {
  ok: boolean;
  error?: string;
  prompt?: ApiPrompt;
}

/** 列表（含 mtime），文件名排序。 */
export async function listWorkflows(client: ComfyClient): Promise<WorkflowFileInfo[]> {
  return listWorkflowFiles(client);
}

/**
 * 读文件并归一化出可提交的 prompt。
 * `file` 来自列表（含 mtime，回填到结果供界面做外部修改检测）；
 * 文件内容是 API 格式则校验后直接使用，是 UI 格式则转换。
 */
export async function loadWorkflow(client: ComfyClient, file: WorkflowFileInfo): Promise<LoadWorkflowResult> {
  let raw: unknown;
  try {
    const text = await readWorkflowFile(client, file.path);
    try {
      raw = JSON.parse(text);
    } catch (err) {
      return { ok: false, error: `文件不是合法 JSON：${err instanceof Error ? err.message : String(err)}` };
    }
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }

  let prompt: ApiPrompt;
  if (isApiFormat(raw)) {
    const checked = validateApiPrompt(raw);
    if (!checked.ok || !checked.prompt) {
      return { ok: false, error: checked.error ?? "API 格式校验失败" };
    }
    prompt = checked.prompt;
  } else {
    const converted = convertUiToApi(raw);
    if (!converted.ok) return { ok: false, error: converted.error };
    prompt = converted.prompt;
  }

  return {
    ok: true,
    workflow: {
      id: file.path,
      name: file.name.replace(/\.json$/i, ""),
      modified: file.modified,
      prompt,
    },
  };
}

/**
 * 把参数草稿写回工作流文件。
 * UI 格式只覆盖 widget 值（保持节点结构与连线）；API 格式整体替换为草稿。
 */
export async function saveWorkflowParams(
  client: ComfyClient,
  file: WorkflowFileInfo,
  prompt: ApiPrompt,
): Promise<SaveParamsResult> {
  let text: string;
  try {
    text = await readWorkflowFile(client, file.path);
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    return { ok: false, error: `文件不是合法 JSON：${err instanceof Error ? err.message : String(err)}` };
  }
  try {
    let content: string;
    if (isApiFormat(parsed)) {
      content = JSON.stringify(prompt, null, 2);
    } else {
      const applied = applyDraftToUi(parsed, prompt);
      if (!applied.ok) return { ok: false, error: applied.error };
      content = JSON.stringify(applied.workflow, null, 2);
    }
    await writeWorkflowFile(client, file.path, content);
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  return { ok: true };
}

/** 导出为可直接分享的 JSON 文本。 */
export function exportWorkflow(workflow: StoredWorkflow): string {
  return JSON.stringify(workflow.prompt, null, 2);
}

/**
 * 校验并归一化一份 API 格式工作流。
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
      ...(entry._meta ? { _meta: entry._meta as ApiPrompt[string]["_meta"] } : {}),
    };
  }
  if (withClassType === 0) {
    return { ok: false, error: "没有任何带 class_type 的节点——看起来不是 API 格式工作流" };
  }
  return { ok: true, prompt };
}
