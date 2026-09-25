/**
 * 生成历史的持久化：记录存插件键值存储（单键整表），图片副本存插件私有目录。
 *
 * ComfyUI 的队列与历史都在服务进程内存里，预览节点的输出还随服务重启整目录删除；
 * 记录与图片副本因此都在折入画廊的当下落盘，服务或应用重启后仍可回看。
 * 实现收 `ctx`，runtime 只依赖 {@link HistoryPersist} 接口。
 */
import type { AtelyxCtx } from "./ctx";
import { bytesToBase64 } from "./comfy/socket";
import type { ResultImage } from "./runtime";

/** 画廊保留上限，也是持久化记录条数上限。 */
export const HISTORY_LIMIT = 200;

const HISTORY_KEY = "history";
const ORIGIN_KEY = "originId";
/** 私有目录里的副本子目录（写盘时宿主自动补齐）。 */
const COPY_DIR = "history";

/** runtime 依赖的持久化面。 */
export interface HistoryPersist {
  /** 恢复持久化记录；损坏数据逐条丢弃，不抛错。 */
  loadHistory(): Promise<ResultImage[]>;
  saveHistory(items: ResultImage[]): Promise<void>;
  /** 本机实例 id：无存值时生成新 id 并持久化。 */
  loadOriginId(): Promise<string>;
  /** 图片字节写入副本目录，写失败抛错。 */
  saveImageCopy(name: string, bytes: Uint8Array): Promise<void>;
  /** 读副本为 dataURL，副本缺失或读取失败抛错。 */
  readImageCopy(name: string): Promise<string>;
  /** 删除副本文件；孤立副本删除失败不影响功能，内部消化。 */
  deleteImageCopy(name: string): Promise<void>;
}

/** 副本文件名：promptId 是服务端 uuid（hex，唯一），同任务的图按序号区分。 */
export function copyName(promptId: string, index: number, fileName: string): string {
  const dot = fileName.lastIndexOf(".");
  const ext = dot > 0 ? fileName.slice(dot) : ".png";
  return `${promptId}-${index}${ext}`;
}

/** 逐条校验磁盘记录：形状不对的条目丢弃，不让脏数据把画廊带崩。 */
function coerceResults(raw: unknown): ResultImage[] {
  if (!Array.isArray(raw)) return [];
  const items: ResultImage[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const e = entry as Record<string, unknown>;
    const ref = e.ref as Record<string, unknown> | undefined;
    if (typeof e.key !== "string" || typeof e.promptId !== "string") continue;
    if (!ref || typeof ref.filename !== "string") continue;
    items.push({
      key: e.key,
      promptId: e.promptId,
      ref: {
        filename: ref.filename,
        subfolder: typeof ref.subfolder === "string" ? ref.subfolder : "",
        type: typeof ref.type === "string" ? ref.type : "output",
      },
      title: typeof e.title === "string" ? e.title : "",
      promptText: typeof e.promptText === "string" ? e.promptText : "",
      createdAt: typeof e.createdAt === "number" ? e.createdAt : 0,
      failed: e.failed === true,
      savedPath: typeof e.savedPath === "string" ? e.savedPath : undefined,
      local: typeof e.local === "string" ? e.local : undefined,
    });
  }
  return items;
}

/** 与服务端会话 id 同形的随机串。 */
function randomId(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function historyPersist(ctx: AtelyxCtx): HistoryPersist {
  // privateDir() 取一次即可（安装期内不变），复制路径拼接用
  let privateRoot: string | null = null;
  const copyPath = async (name: string): Promise<string> => {
    if (privateRoot === null) privateRoot = await ctx.fs.privateDir();
    return `${privateRoot}/${COPY_DIR}/${name}`;
  };

  return {
    async loadHistory(): Promise<ResultImage[]> {
      try {
        return coerceResults(await ctx.storage.get(HISTORY_KEY));
      } catch {
        return [];
      }
    },

    async saveHistory(items: ResultImage[]): Promise<void> {
      await ctx.storage.set(HISTORY_KEY, items);
    },

    async loadOriginId(): Promise<string> {
      try {
        const saved = await ctx.storage.get(ORIGIN_KEY);
        if (typeof saved === "string" && saved) return saved;
      } catch {
        // 读取失败按无值处理，下面生成新 id
      }
      const id = randomId(8);
      try {
        await ctx.storage.set(ORIGIN_KEY, id);
      } catch {
        // 持久化失败时本次会话仍用该 id；代价是重启后旧记录可能被视为其他机器的（不常见）
      }
      return id;
    },

    async saveImageCopy(name: string, bytes: Uint8Array): Promise<void> {
      const result = await ctx.fs.writeFileBase64(await copyPath(name), bytesToBase64(bytes));
      if (!result.ok) throw new Error(result.summary);
    },

    async readImageCopy(name: string): Promise<string> {
      return ctx.fs.readFileDataUrl(await copyPath(name));
    },

    async deleteImageCopy(name: string): Promise<void> {
      try {
        await ctx.fs.deleteFile(await copyPath(name));
      } catch {
        // 副本已不存在或被占用都不影响功能
      }
    },
  };
}
