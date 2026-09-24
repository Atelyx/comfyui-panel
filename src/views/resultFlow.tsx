/**
 * 结果记录流：生成面板的主体区域。
 *
 * 记录按「旧→新」纵向排列：新生成从底部冒出、把旧的顶到上方，顶部溢出部分以渐变
 * 遮罩过渡；贴底时新记录自动滚到最新，往上翻历史不打扰。运行中的任务以占位记录
 * 展示（规格与提示词标题都取队列里该任务的真实 prompt，不依赖草稿），完成后按
 * promptId 分组为「提示词标题 + 时间 + 若干张图」，标题可一键复制，每张图可预览、
 * 保存到仓库。
 */
import React from "react";
import type { AtelyxCtx } from "../ctx";
import type { ProgressPayload, QueueItem } from "../comfy/types";
import type { ComfyRuntime, ResultImage } from "../runtime";
import { parseAspectPair, promptTextOf } from "../workflow/form";
import {
  ContextMenu,
  ContextMenuItem,
  CopyIcon,
  DownloadIcon,
  SCROLL_LIST_CLASS,
  accent,
  bgPrimary,
  bgSecondary,
  border,
  danger,
  Empty,
  FONT_SM,
  ImageIcon,
  SquareIcon,
  textMuted,
  textPrimary,
  Button,
} from "./ui";

interface RecordFlowProps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  results: ResultImage[];
  /** 队列快照（queue_running / queue_pending）。 */
  running: QueueItem[];
  pending: QueueItem[];
  progress: ProgressPayload | null;
  offline: boolean;
  /** 协作空间仓库不可落库：保存入口禁用并说明原因。 */
  saveDisabled: boolean;
  savingKey: string | null;
  onArchive: (item: ResultImage) => void;
  onInterrupt: () => void;
}

/** 结果记录流：渲染空态、运行中占位记录与已完成的分组记录。 */
export function RecordFlow(props: RecordFlowProps): unknown {
  const [previewKey, setPreviewKey] = React.useState<string | null>(null);
  const runningCount = props.running.length + props.pending.length;
  const previewed = previewKey ? props.results.find((item) => item.key === previewKey) ?? null : null;
  const listRef = React.useRef<HTMLDivElement | null>(null);
  /** 视图是否贴底：贴底时新记录自动滚到最新，翻历史时不打扰。 */
  const pinnedRef = React.useRef(true);
  /** 内容溢出时才需要顶部阴影遮罩。 */
  const [topFade, setTopFade] = React.useState(false);

  const records = React.useMemo(() => {
    const map = new Map<string, ResultImage[]>();
    const order: string[] = [];
    for (const item of props.results) {
      let list = map.get(item.promptId);
      if (!list) {
        list = [];
        map.set(item.promptId, list);
        order.push(item.promptId);
      }
      list.push(item);
    }
    // 结果列表最近在前；展示改为旧→新，最新记录沉在底部把旧的顶上去
    return order.reverse().map((promptId) => ({ promptId, items: map.get(promptId) ?? [] }));
  }, [props.results]);

  const syncView = React.useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    setTopFade(el.scrollHeight > el.clientHeight + 4);
    if (pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, []);

  React.useEffect(() => {
    syncView();
  }, [syncView, props.results, props.progress, props.running, props.pending]);

  const onScroll = React.useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    setTopFade(el.scrollHeight > el.clientHeight + 4);
  }, []);

  /** 预览右键「复制图片」：字节走直连通道，成败以 boolean 回给遮罩就地提示。 */
  const copyPreviewImage = React.useCallback(
    async (item: ResultImage): Promise<boolean> => {
      try {
        const dataUrl = await props.runtime.imageDataUrl(item.ref);
        await props.ctx.clipboard.copyImage(dataUrl);
        return true;
      } catch {
        return false;
      }
    },
    [props.ctx, props.runtime],
  );

  /** 复制某批的完整提示词：成败都弹宿主通知，记录流内不另设错误区。 */
  const copyPrompt = React.useCallback(
    (text: string) => {
      void props.ctx.clipboard
        .writeText(text)
        .then(() => props.ctx.notification.notify({ level: "success", message: "已复制提示词" }))
        .catch(() => props.ctx.notification.notify({ level: "error", message: "复制提示词失败，请重试" }));
    },
    [props.ctx],
  );

  /** 预览右键「下载图片」：宿主命令落系统 Downloads，重名由宿主自动加序号。 */
  const downloadPreviewImage = React.useCallback(
    async (item: ResultImage): Promise<boolean> => {
      try {
        const dataUrl = await props.runtime.imageDataUrl(item.ref);
        await props.ctx.native.invoke("save_image_to_downloads", {
          fileName: item.ref.filename,
          dataUrl,
        });
        return true;
      } catch {
        return false;
      }
    },
    [props.ctx, props.runtime],
  );

  return (
    <div
      style={{
        position: "relative",
        flex: 1,
        /** 高级参数展开时生成坞可能占掉大部分高度，结果流至少保留这一条可见。 */
        minHeight: 120,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <div
        ref={listRef}
        onScroll={onScroll}
        className={SCROLL_LIST_CLASS}
        style={{
          flex: 1,
          minHeight: 0,
          overflowY: "auto",
          padding: 10,
        }}
      >
        {props.offline ? (
          <div style={{ height: "100%", display: "flex", flexDirection: "column" }}>
            <Empty hint="连接后展示生成结果">
              <ImageIcon size={22} />
              未连接
            </Empty>
          </div>
        ) : records.length === 0 && runningCount === 0 ? (
          <div style={{ height: "100%", display: "flex", flexDirection: "column" }}>
            <Empty hint="输入提示词并生成，结果会按每次生成分组展示">
              <ImageIcon size={22} />
              还没有结果
            </Empty>
          </div>
        ) : (
          <>
            {records.map((record) => {
              const failed = record.items.some((item) => item.failed);
              const lead = record.items[0];
              const promptText = lead?.promptText ?? "";
              return (
                <RecordCard
                  key={record.promptId}
                  title={promptText || lead?.title || "未命名任务"}
                  time={lead?.createdAt}
                  failed={failed}
                  action={
                    promptText ? (
                      <CopyPromptButton text={promptText} onCopy={copyPrompt} />
                    ) : null
                  }
                >
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))",
                      gap: 8,
                    }}
                  >
                    {record.items.map((item) => (
                      <RecordImage
                        key={item.key}
                        item={item}
                        url={props.runtime.imageUrl(item.ref)}
                        saving={props.savingKey === item.key}
                        saveDisabled={props.saveDisabled}
                        onPreview={() => setPreviewKey(item.key)}
                        onArchive={() => props.onArchive(item)}
                      />
                    ))}
                  </div>
                </RecordCard>
              );
            })}
            {runningCount > 0 || props.progress ? (
              <RunningRecord
                running={props.running}
                pending={props.pending}
                progress={props.progress}
                onInterrupt={props.onInterrupt}
                onCopyPrompt={copyPrompt}
              />
            ) : null}
          </>
        )}

        {previewed ? (
          <PreviewOverlay
            item={previewed}
            url={props.runtime.imageUrl(previewed.ref, false)}
            onClose={() => setPreviewKey(null)}
            onCopyImage={() => copyPreviewImage(previewed)}
            onDownloadImage={() => downloadPreviewImage(previewed)}
          />
        ) : null}
      </div>
      {topFade ? (
        <div
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            right: 0,
            height: 36,
            pointerEvents: "none",
            zIndex: 2,
            background: `linear-gradient(to bottom, ${bgPrimary}, transparent)`,
          }}
        />
      ) : null}
    </div>
  );
}

/** 运行中的占位记录：标题行（提示词 + 运行/等待计数 + 复制 + 中断）与按输出规格排布的占位动画。 */
function RunningRecord(props: {
  running: QueueItem[];
  pending: QueueItem[];
  progress: ProgressPayload | null;
  onInterrupt: () => void;
  onCopyPrompt: (text: string) => void;
}): unknown {
  const prompt = props.running[0]?.[2];
  const spec = promptSpec(prompt);
  const promptText = promptTextOf(prompt);
  const percent =
    props.progress && props.progress.max > 0 ? (props.progress.value / props.progress.max) * 100 : null;
  const queued = props.running.length + props.pending.length;
  return (
    <RecordCard
      title={promptText || "生成中"}
      action={
        <>
          {queued > 0 ? (
            <span style={{ fontSize: 10, color: textMuted, flexShrink: 0 }}>
              {`运行 ${props.running.length} · 等待 ${props.pending.length}`}
            </span>
          ) : null}
          {promptText ? <CopyPromptButton text={promptText} onCopy={props.onCopyPrompt} /> : null}
          <Button tone="danger" onClick={props.onInterrupt} title="中断当前执行">
            <SquareIcon size={12} />
            中断
          </Button>
        </>
      }
    >
      <GenerationGrid
        aspect={spec.aspect}
        batch={spec.batch}
        percent={percent}
        node={props.progress?.node ?? ""}
      />
    </RecordCard>
  );
}

/** 标题行的复制按钮：复制该批完整提示词，常驻小图标、悬停提亮。 */
function CopyPromptButton(props: { text: string; onCopy: (text: string) => void }): unknown {
  const [isHover, setHover] = React.useState(false);
  return (
    <button
      type="button"
      title="复制提示词"
      onClick={() => props.onCopy(props.text)}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "inline-flex",
        alignItems: "center",
        border: "none",
        background: "transparent",
        padding: 2,
        color: isHover ? textPrimary : textMuted,
        cursor: "pointer",
        flexShrink: 0,
      }}
    >
      <CopyIcon size={12} />
    </button>
  );
}

/** 单条记录卡：标题行（标题/时间/失败标记/右侧动作）+ 内容。块级流里用下边距分隔，不参与弹性压缩。 */
function RecordCard(props: {
  title: string;
  time?: number;
  failed?: boolean;
  action?: unknown;
  children: unknown;
}): unknown {
  return (
    <div
      style={{
        marginBottom: 10,
        border: `1px solid ${border}`,
        borderRadius: 8,
        background: bgSecondary,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "6px 10px",
          borderBottom: `1px solid ${border}`,
          background: bgPrimary,
        }}
      >
        <span
          style={{
            flex: 1,
            minWidth: 0,
            fontSize: FONT_SM,
            color: textPrimary,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
          title={props.title}
        >
          {props.title}
        </span>
        {props.failed ? (
          <span style={{ fontSize: 10, color: danger, flexShrink: 0 }} title="任务执行出错，图中可能缺失">
            出错
          </span>
        ) : null}
        {props.time !== undefined ? (
          <span style={{ fontSize: 10, color: textMuted, flexShrink: 0 }}>{formatTime(props.time)}</span>
        ) : null}
        {props.action}
      </div>
      <div style={{ padding: 10 }}>{props.children}</div>
    </div>
  );
}

/** 从队列项的真实 prompt 读输出规格：与提交时的草稿无关，展示的是排队那一刻的尺寸。 */
function promptSpec(prompt: unknown): { aspect: number; batch: number } {
  let width: number | null = null;
  let height: number | null = null;
  let aspect: number | null = null;
  let batch = 1;
  if (prompt && typeof prompt === "object") {
    for (const node of Object.values(
      prompt as Record<string, { class_type?: string; inputs?: Record<string, unknown> }>,
    )) {
      const inputs = node?.inputs ?? {};
      // 分辨率选择器节点没有可写的宽高数字，输出比例从 aspect_ratio 值里取
      if (node?.class_type === "ResolutionSelector" && typeof inputs.aspect_ratio === "string") {
        const pair = parseAspectPair(inputs.aspect_ratio);
        if (pair) aspect = pair.w / pair.h;
      }
      for (const [name, value] of Object.entries(inputs)) {
        if (typeof value !== "number") continue;
        if (name === "width" && width === null) width = value;
        else if (name === "height" && height === null) height = value;
        else if (name === "batch_size" && value >= 1) batch = Math.floor(value);
      }
    }
  }
  if (width !== null && height !== null && width > 0 && height > 0) {
    return { aspect: width / height, batch };
  }
  return { aspect: aspect ?? 1, batch };
}

function formatTime(ts: number): string {
  const date = new Date(ts);
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 结果图格：按图片真实宽高比展示（加载后测量），悬停显示保存操作。 */
function RecordImage(props: {
  item: ResultImage;
  url: string;
  saving: boolean;
  saveDisabled: boolean;
  onPreview: () => void;
  onArchive: () => void;
}): unknown {
  const [isHover, setHover] = React.useState(false);
  const [ratio, setRatio] = React.useState<number | null>(null);
  const aspect = ratio === null ? "1 / 1" : String(clampRatio(ratio));
  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        position: "relative",
        borderRadius: 6,
        overflow: "hidden",
        aspectRatio: aspect,
        maxHeight: 320,
        border: `1px solid ${props.item.failed ? danger : border}`,
        background: "var(--hover)",
      }}
    >
      <img
        src={props.url}
        alt={props.item.ref.filename}
        loading="lazy"
        onClick={props.onPreview}
        onLoad={(e: { currentTarget: HTMLImageElement }) => {
          const target = e.currentTarget;
          if (target.naturalWidth > 0 && target.naturalHeight > 0) {
            setRatio(target.naturalWidth / target.naturalHeight);
          }
        }}
        style={{
          width: "100%",
          height: "100%",
          objectFit: "cover",
          display: "block",
          cursor: "zoom-in",
        }}
      />
      {props.item.savedPath ? (
        <span
          title={props.item.savedPath}
          style={{
            position: "absolute",
            top: 4,
            right: 4,
            fontSize: 10,
            lineHeight: 1.6,
            padding: "0 5px",
            borderRadius: 4,
            background: "rgba(0,0,0,0.6)",
            color: "#fff",
          }}
        >
          已保存 ✓
        </span>
      ) : null}
      {isHover ? (
        <div
          style={{
            position: "absolute",
            inset: "auto 0 0 0",
            display: "flex",
            alignItems: "center",
            gap: 4,
            padding: 4,
            background: "rgba(0,0,0,0.55)",
          }}
        >
          <span
            style={{
              flex: 1,
              minWidth: 0,
              fontSize: 10,
              color: "#fff",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
            title={props.item.ref.filename}
          >
            {props.item.ref.filename}
          </span>
          <button
            type="button"
            title={props.saveDisabled ? "协作空间仓库暂不支持保存图片" : "保存到仓库"}
            disabled={props.saving || props.saveDisabled}
            onClick={props.onArchive}
            style={{
              border: "none",
              borderRadius: 4,
              background: "rgba(255,255,255,0.9)",
              color: "#111",
              fontSize: 10,
              padding: "2px 6px",
              cursor: props.saving || props.saveDisabled ? "not-allowed" : "pointer",
            }}
          >
            {props.saving ? "保存中" : "保存"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

/** 展示比例收敛到合理区间，避免超宽图被压成一线、超高图撑满整屏。 */
function clampRatio(ratio: number): number {
  return Math.max(0.6, Math.min(2.4, ratio));
}

/** 操作结果提示自动消失时长。 */
const NOTICE_MS = 2500;

/**
 * 大图遮罩：点击遮罩或 Esc 关闭；右键（图上或遮罩空白处）弹「复制图片 / 下载图片」，
 * 动作成败以 boolean 返回、在遮罩底部就地提示。菜单开着时 Esc 只关菜单不连关预览。
 */
function PreviewOverlay(props: {
  item: ResultImage;
  url: string;
  onClose: () => void;
  onCopyImage: () => Promise<boolean>;
  onDownloadImage: () => Promise<boolean>;
}): unknown {
  const [menu, setMenu] = React.useState<{ x: number; y: number } | null>(null);
  const [notice, setNotice] = React.useState<{ text: string; kind: "success" | "error" } | null>(null);

  React.useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (menu) setMenu(null);
        else props.onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu, props]);

  /** 执行菜单动作：关菜单 → 等结果 → 就地提示。 */
  const runAction = React.useCallback(
    (action: () => Promise<boolean>, okText: string, failText: string) => {
      setMenu(null);
      void action()
        .then((ok) => setNotice({ text: ok ? okText : failText, kind: ok ? "success" : "error" }))
        .catch(() => setNotice({ text: failText, kind: "error" }));
    },
    [],
  );

  return (
    <div
      onClick={props.onClose}
      onContextMenu={(e: { preventDefault(): void; clientX: number; clientY: number }) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 60,
        background: "rgba(0,0,0,0.78)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 24,
        cursor: "zoom-out",
      }}
    >
      <img
        src={props.url}
        alt={props.item.ref.filename}
        style={{ maxWidth: "100%", maxHeight: "100%", objectFit: "contain", borderRadius: 6 }}
      />
      {menu ? (
        <ContextMenu x={menu.x} y={menu.y} onClose={() => setMenu(null)}>
          <ContextMenuItem
            onClick={() => runAction(props.onCopyImage, "已复制到剪贴板", "复制图片失败，请重试")}
          >
            <CopyIcon size={13} />
            复制图片
          </ContextMenuItem>
          <ContextMenuItem
            onClick={() => runAction(props.onDownloadImage, "已保存到 Downloads", "下载图片失败，请重试")}
          >
            <DownloadIcon size={13} />
            下载图片
          </ContextMenuItem>
        </ContextMenu>
      ) : null}
      {notice ? (
        <div
          style={{
            position: "absolute",
            bottom: 32,
            left: "50%",
            transform: "translateX(-50%)",
            padding: "6px 12px",
            borderRadius: 6,
            fontSize: FONT_SM,
            background: "rgba(0,0,0,0.75)",
            color: notice.kind === "success" ? "#4ade80" : "#f87171",
          }}
        >
          {notice.text}
        </div>
      ) : null}
    </div>
  );
}

/** 生成动画关键帧（随组件的 `<style>` 挂载/卸载，不落样式文件）。 */
const GEN_ANIMATION_CSS = `
@keyframes gen-scan {
  0% { top: -30%; }
  100% { top: 130%; }
}
@keyframes gen-shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
@keyframes gen-breathe {
  0%, 100% { opacity: 0.35; }
  50% { opacity: 1; }
}
`;

/** 占位格尺寸：单张稍大，多张缩成小格；比例取输出图比例。 */
function genCellSize(aspect: number, count: number): { width: number; height: number } {
  const clamp = (value: number, lo: number, hi: number): number => Math.max(lo, Math.min(hi, value));
  if (count <= 1) {
    const height = clamp(260 / aspect, 90, 200);
    return { width: clamp(height * aspect, 120, 260), height };
  }
  const height = clamp(140 / aspect, 56, 120);
  return { width: clamp(height * aspect, 72, 140), height };
}

/**
 * 生成中的占位动画：按输出图比例画圆角矩形，格数随批次（batch_size）展开。
 * 有进度时按完成度分三态——已生成（✓）、生成中（流光 + 扫描线 + 居中进度）、待生成（呼吸）；
 * 尚无进度（排队中）时全部以呼吸点示意。
 */
function GenerationGrid(props: {
  aspect: number;
  batch: number;
  /** 整批共用的采样进度（0~100），无数据时为 null（排队中）。 */
  percent: number | null;
  /** 当前执行节点名。 */
  node: string;
}): unknown {
  const MAX_CELLS = 12;
  const shown = Math.min(props.batch, MAX_CELLS);
  const size = genCellSize(props.aspect, shown);
  const pct = props.percent === null ? null : Math.round(props.percent);
  const activeIndex = pct === null ? -1 : Math.min(shown - 1, Math.max(0, Math.floor((pct / 100) * shown)));
  const fontPct = size.height > 120 ? 22 : size.height > 80 ? 16 : 13;

  const renderCell = (index: number): unknown => {
    const state =
      pct === null ? "queued" : index < activeIndex ? "done" : index === activeIndex ? "active" : "pending";
    return (
      <div
        key={index}
        style={{
          position: "relative",
          width: size.width,
          height: size.height,
          maxWidth: "100%",
          borderRadius: 10,
          overflow: "hidden",
          boxSizing: "border-box",
          border: state === "active" || state === "done" ? `1px solid ${accent}` : `1px solid ${border}`,
          background:
            state === "done"
              ? "color-mix(in srgb, var(--accent) 16%, transparent)"
              : state === "active"
                ? "color-mix(in srgb, var(--accent) 6%, transparent)"
                : bgSecondary,
        }}
      >
        {state === "queued" ? (
          <span
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              animation: "gen-breathe 1.6s ease-in-out infinite",
            }}
          >
            <span style={{ width: 5, height: 5, borderRadius: 3, background: textMuted }} />
          </span>
        ) : null}
        {state === "done" ? (
          <span
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: accent,
              fontSize: Math.max(12, Math.round(fontPct * 0.75)),
            }}
          >
            ✓
          </span>
        ) : null}
        {state === "active" ? (
          <>
            <div
              style={{
                position: "absolute",
                inset: 0,
                background:
                  "linear-gradient(115deg, transparent 30%, rgba(255,255,255,0.09) 50%, transparent 70%)",
                backgroundSize: "200% 100%",
                animation: "gen-shimmer 2.4s linear infinite",
              }}
            />
            <div
              style={{
                position: "absolute",
                left: 0,
                right: 0,
                height: 2,
                top: 0,
                background: "linear-gradient(90deg, transparent, var(--accent), transparent)",
                animation: "gen-scan 1.8s ease-in-out infinite",
              }}
            />
            <div
              style={{
                position: "absolute",
                inset: 0,
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
                gap: 2,
                color: textPrimary,
              }}
            >
              <span style={{ fontSize: fontPct, fontWeight: 600, fontVariantNumeric: "tabular-nums" }}>
                {pct}%
              </span>
              {props.node ? (
                <span
                  style={{
                    fontSize: 9,
                    color: textMuted,
                    maxWidth: "92%",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  节点 {props.node}
                </span>
              ) : null}
            </div>
          </>
        ) : null}
        {state === "pending" ? (
          <span
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: textMuted,
              fontSize: 9,
            }}
          >
            待生成
          </span>
        ) : null}
      </div>
    );
  };

  return (
    <>
      <style>{GEN_ANIMATION_CSS}</style>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, maxWidth: "100%" }}>
        {Array.from({ length: shown }, (_, index) => renderCell(index))}
        {props.batch > MAX_CELLS ? (
          <span style={{ alignSelf: "center", fontSize: 11, color: textMuted }}>
            …共 {props.batch} 张
          </span>
        ) : null}
      </div>
    </>
  );
}
