/**
 * 结果落库：把生成的图片存进当前仓库，可选追加到当前笔记。
 *
 * 为什么分「写临时件 → 导入附件」两步：插件的文件写入只收文本，没有二进制通道；
 * 而附件导入是Atelyx既有能力（附件目录、重名规则、引用语义都由它保持一致），复用比自造落位规则稳。
 * 两步都经原始命令逃生舱调用，因为附件能力未被封装成常规服务。
 *
 * 落库后固定清理临时件：临时区随仓库同步，留下等于撑大用户的同步盘。
 */
import type { AtelyxCtx } from "../ctx";
import { bytesToBase64 } from "../comfy/socket";
import type { ImageRef } from "../comfy/types";

/** 临时区归组键（Atelyx据此派生定长目录名，无路径语义）。 */
const TEMP_GROUP = "comfyui";

export interface ArchiveResult {
  /** 仓库内相对路径。 */
  path: string;
  /** 未开启追加时为 undefined。 */
  appended?: boolean;
}

/** 收敛成不含路径段的安全叶子名。 */
function safeFileName(ref: ImageRef): string {
  const leaf = ref.filename.split(/[\\/]/).pop() ?? ref.filename;
  const cleaned = leaf.replace(/[\\/:*?"<>|\u0000]/g, "_").replace(/^\.+/, "").trim();
  return cleaned || `comfyui-${Date.now()}.png`;
}

/** 加前缀以便与用户自己的图片区分。 */
function archiveName(ref: ImageRef): string {
  const base = safeFileName(ref);
  return `comfyui-${base}`;
}

/** 字节由调用方取好——取图要求直连通道，而通道可用性判断在运行时层。 */
export async function archiveImage(
  ctx: AtelyxCtx,
  bytes: Uint8Array,
  ref: ImageRef,
): Promise<ArchiveResult> {
  const fileName = archiveName(ref);
  const base64Data = bytesToBase64(bytes);

  // 写入仓库内的临时区（路径边界由Atelyx校验）
  const tempRef = (await ctx.native.invoke("write_temp_attachment", {
    canvasId: TEMP_GROUP,
    fileName,
    base64Data,
  })) as string;
  if (!tempRef) throw new Error("临时文件写入失败");

  try {
    // 复制进附件目录，沿用用户配置的附件文件夹与重名规则
    const imported = (await ctx.native.invoke("import_vault_attachment", {
      rel: tempRef,
      fileName,
    })) as { file?: string } | undefined;
    const path = imported?.file;
    if (!path) throw new Error("附件导入失败");
    return { path };
  } finally {
    // 无论成败都清掉临时件：它只是过程产物
    try {
      await ctx.vault.deleteFile(tempRef);
    } catch {
      // 清理失败不影响落库结果
    }
  }
}

/** 笔记服务可选（提供者被停用即跳过）。用相对路径引用，换机器或换仓库根目录后仍能解析。 */
export async function appendToCurrentNote(ctx: AtelyxCtx, repoPath: string): Promise<boolean> {
  const note = ctx.services.get("note") as
    | { currentFile(): string | null; write(content: string): Promise<void>; read(file?: string): Promise<string> }
    | undefined;
  if (!note) return false;
  const file = note.currentFile();
  if (!file) return false;
  const current = await note.read();
  const separator = current.endsWith("\n") || current.length === 0 ? "" : "\n";
  await note.write(`${current}${separator}\n![](${repoPath})\n`);
  return true;
}
