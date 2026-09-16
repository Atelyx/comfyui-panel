/**
 * 工作流文件源：把 ComfyUI 用户目录（`user/default/workflows/`）当作工作流库的唯一来源。
 *
 * 所有操作经 `/userdata` HTTP 接口，不碰插件本地存储——文件由 ComfyUI 编辑器与插件共享，
 * 一方保存另一方立刻可见。`path` 是相对当前用户目录的路径（如 `workflows/xx.json`）。
 */
import type { ComfyClient } from "../comfy/client";
import type { UserDataEntry } from "../comfy/types";

/** ComfyUI 工作流文件所在的用户目录子路径。 */
export const WORKFLOWS_DIR = "workflows";

/** 列表项：mtime 由服务端给出，用于探测外部改动（编排界面保存后秒级感知）。 */
export interface WorkflowFileInfo {
  /** 相对用户目录的路径（`workflows/xx.json`），也用作工作流 id。 */
  path: string;
  /** 文件名（不含目录，含 .json 后缀）。 */
  name: string;
  /** 毫秒时间戳。 */
  modified: number;
  size: number;
}

/** ComfyUI 内部的收藏索引文件，不是工作流，列表里不展示。 */
const INTERNAL_FILES = new Set([".index.json"]);

function isWorkflowEntry(entry: UserDataEntry): boolean {
  if (typeof entry.path !== "string") return false;
  if (INTERNAL_FILES.has(entry.path)) return false;
  if (!entry.path.toLowerCase().endsWith(".json")) return false;
  // 隐藏文件（点点开头的项）不展示
  const name = entry.path.slice(entry.path.lastIndexOf("/") + 1);
  return !name.startsWith(".");
}

/** 列表返回的 path 相对 `dir` 参数（无 workflows/ 前缀），这里补成完整的用户相对路径。 */
export function toWorkflowFile(entry: UserDataEntry): WorkflowFileInfo {
  return {
    path: `${WORKFLOWS_DIR}/${entry.path}`,
    name: entry.path.slice(entry.path.lastIndexOf("/") + 1),
    modified: typeof entry.modified === "number" ? entry.modified : 0,
    size: typeof entry.size === "number" ? entry.size : 0,
  };
}

/** 列目录并按文件名排序（稳定展示，不随服务端返回顺序跳变）。 */
export async function listWorkflowFiles(client: ComfyClient): Promise<WorkflowFileInfo[]> {
  const entries = await client.listUserDir(WORKFLOWS_DIR);
  return entries
    .filter(isWorkflowEntry)
    .map(toWorkflowFile)
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** 读文件原文。 */
export function readWorkflowFile(client: ComfyClient, path: string): Promise<string> {
  return client.readUserFile(path);
}

/** 写入（覆盖）文件；ComfyUI 侧父目录自动创建。 */
export function writeWorkflowFile(client: ComfyClient, path: string, content: string): Promise<void> {
  return client.writeUserFile(path, content);
}

/** 展示名：去目录与 .json 后缀。 */
export function workflowDisplayName(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  return name.toLowerCase().endsWith(".json") ? name.slice(0, -5) : name;
}

/**
 * 对比两次列表快照，返回发生变化的条目（新增/修改/删除）。
 * 只有 mtime 与 size 参与比较，不读文件正文。
 */
export function diffWorkflows(
  previous: Map<string, WorkflowFileInfo>,
  next: WorkflowFileInfo[],
): { changed: WorkflowFileInfo[]; removed: string[] } {
  const nextMap = new Map(next.map((item) => [item.path, item]));
  const changed: WorkflowFileInfo[] = [];
  const removed: string[] = [];
  for (const [path, item] of nextMap) {
    const prev = previous.get(path);
    if (!prev || prev.modified !== item.modified || prev.size !== item.size) changed.push(item);
  }
  for (const path of previous.keys()) {
    if (!nextMap.has(path)) removed.push(path);
  }
  return { changed, removed };
}
