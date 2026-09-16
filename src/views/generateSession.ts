/**
 * 生成面板的会话状态。
 *
 * 面板切换时宿主卸载视图，组件内的选择与参数草稿随之丢失；这里把会话状态放到模块级单例，
 * 面板挂载时从它恢复、变化时写回——切走再回来仍是上次的工作流与参数。
 */
import type { ApiPrompt } from "../comfy/types";
import type { StoredWorkflow } from "../workflow/library";

export interface GenerateSession {
  /** 选中的工作流文件路径。 */
  activeId: string;
  /** 已加载的工作流（含归一化后的 prompt）。 */
  active: StoredWorkflow | null;
  /** 本次提交用的参数草稿。 */
  draft: ApiPrompt | null;
  /** 勾选「每次随机」的字段集合。 */
  seedMode: ReadonlySet<string>;
  /** 草稿相对文件是否被改过（外部改动时决定自动重载还是提示）。 */
  dirty: boolean;
  /** 加载时刻的文件 mtime，用于检测编排界面里的修改。 */
  loadedModified: number;
  filter: string;
}

let session: GenerateSession = emptyGenerateSession();

export function emptyGenerateSession(): GenerateSession {
  return {
    activeId: "",
    active: null,
    draft: null,
    seedMode: new Set(),
    dirty: false,
    loadedModified: -1,
    filter: "",
  };
}

export function getGenerateSession(): GenerateSession {
  return session;
}

/** 把当前面板状态写回会话，供下次挂载恢复。 */
export function saveGenerateSession(next: GenerateSession): void {
  session = next;
}

/** 插件停用时清空，避免重新激活后回到旧草稿。 */
export function resetGenerateSession(): void {
  session = emptyGenerateSession();
}
