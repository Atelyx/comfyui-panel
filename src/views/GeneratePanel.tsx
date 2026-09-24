/**
 * 生成面板：选工作流、底部生成坞调参与提交、顶部结果记录流。
 *
 * 工作流来自 ComfyUI 用户目录（`user/default/workflows`）：列表由运行时轮询 `/userdata`
 * 保持最新——编排界面里保存的修改几秒内会出现在这里。选中文件被外部改动时：
 * 本地没改过参数就自动重载，改过则提示手动重载，不覆盖用户正在调的参数。
 *
 * 布局：结果记录流占据主体（顶部，按每次生成分组）；底部是生成坞——参考图（绑定工作流里
 * 的 LoadImage 节点）+ 大提示词输入 + 圆形生成按钮、常用参数条（只取未连线字段，缺哪个
 * 隐藏哪个）、高级参数折叠区（节点分组全量表单）。未加载工作流时退化为空态提示。
 */
import React from "react";
import type { AtelyxCtx } from "../ctx";
import type { ApiPrompt } from "../comfy/types";
import type { ComfyRuntime, ResultImage } from "../runtime";
import type { HostController } from "../host/controller";
import type { RemoteControl } from "../host/remote";
import { archiveImage } from "../host/archive";
import { exportWorkflow, type StoredWorkflow } from "../workflow/library";
import { workflowDisplayName } from "../workflow/files";
import { getGenerateSession, saveGenerateSession } from "./generateSession";
import {
  applySeedMode,
  buildFields,
  coerceFieldValue,
  countDisabledNodes,
  defaultSeedKeys,
  displayValue,
  fieldKey,
  fieldLabel,
  isCommonField,
  isPromptInput,
  parseAspectPair,
  readField,
  writeField,
  type FieldSpec,
} from "../workflow/form";
import { RecordFlow } from "./resultFlow";
import { RefImageStack } from "./refImages";
import { RemoteEntry } from "./remoteMachines";
import {
  Button,
  Checkbox,
  Chip,
  ChevronDownIcon,
  ChevronRightIcon,
  CollapseCard,
  CopyIcon,
  Empty,
  Notice,
  Panel,
  PlayIcon,
  RefreshIcon,
  SearchIcon,
  Select,
  SquareIcon,
  StatusDot,
  AutoTextArea,
  SCROLL_LIST_CLASS,
  SCROLLBAR_CSS,
  TextArea,
  TextInput,
  Toolbar,
  UploadIcon,
  ZapIcon,
  accent,
  accentFg,
  bgPrimary,
  bgSecondary,
  border,
  hover,
  textMuted,
  textPrimary,
  textSecondary,
  FONT_SM,
} from "./ui";

interface GeneratePanelProps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  host: HostController;
  remote: RemoteControl;
}

/** 输出比例预设（仅比例）：配合「分辨率」一起算出宽高。 */
const RATIO_PRESETS: Array<{ label: string; w: number; h: number }> = [
  { label: "1:1", w: 1, h: 1 },
  { label: "2:3", w: 2, h: 3 },
  { label: "3:2", w: 3, h: 2 },
  { label: "3:4", w: 3, h: 4 },
  { label: "4:3", w: 4, h: 3 },
  { label: "9:16", w: 9, h: 16 },
  { label: "16:9", w: 16, h: 9 },
  { label: "21:9", w: 21, h: 9 },
];

/** 分辨率（百万像素）预设。 */
const MEGAPIXEL_OPTIONS = ["0.5", "1", "1.5", "2", "4"];

const BATCH_OPTIONS = ["1", "2", "3", "4"];

export function GeneratePanel(props: GeneratePanelProps): unknown {
  const { ctx, runtime, host, remote } = props;
  const snapshot = React.useSyncExternalStore(runtime.subscribe, runtime.getSnapshot);
  const hostSnapshot = React.useSyncExternalStore(host.subscribe, host.getSnapshot);

  const workflows = snapshot.workflows;
  const session = getGenerateSession();
  const [activeId, setActiveId] = React.useState<string>(() => session.activeId);
  /** 上传是异步的，回调落地前可能已切换工作流；用 ref 记住"提交上传时的工作流"。 */
  const activeIdRef = React.useRef(activeId);
  activeIdRef.current = activeId;
  const [active, setActive] = React.useState<StoredWorkflow | null>(() => session.active);
  const [draft, setDraft] = React.useState<ApiPrompt | null>(() => session.draft);
  const [seedMode, setSeedMode] = React.useState<ReadonlySet<string>>(() => session.seedMode);
  const [loadedModified, setLoadedModified] = React.useState<number>(() => session.loadedModified);
  /** 参数被改过就不再自动重载，避免覆盖正在调的值。 */
  const [dirty, setDirty] = React.useState<boolean>(() => session.dirty);
  /** 选中的文件在编排界面被改过、而本地又有改动时的提示条。 */
  const [fileUpdated, setFileUpdated] = React.useState(false);
  /** 读取/转换失败的原因（如子图无法转换）。 */
  const [loadError, setLoadError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [actionError, setActionError] = React.useState("");
  const [filter, setFilter] = React.useState<string>(() => session.filter);
  const [advancedOpen, setAdvancedOpen] = React.useState<boolean>(() => session.advancedOpen);
  const [promptFieldKey, setPromptFieldKey] = React.useState<string>(() => session.promptFieldKey);
  const [collapsed, setCollapsed] = React.useState<ReadonlySet<string>>(new Set());
  const [savingKey, setSavingKey] = React.useState<string | null>(null);

  const offline = snapshot.channel === "offline";

  // 切走再回来时恢复上次的选择与草稿：状态变化即写回会话存储
  React.useEffect(() => {
    saveGenerateSession({
      activeId,
      active,
      draft,
      seedMode,
      dirty,
      loadedModified,
      filter,
      advancedOpen,
      promptFieldKey,
    });
  }, [activeId, active, draft, seedMode, dirty, loadedModified, filter, advancedOpen, promptFieldKey]);

  // 选中文件：首次加载、切换、或该文件被外部改动时重载
  React.useEffect(() => {
    if (!activeId) {
      if (snapshot.workflows.length > 0) {
        setActiveId(snapshot.workflows[0].path);
      } else {
        setActive(null);
        setDraft(null);
        setLoadError("");
        setFileUpdated(false);
        setDirty(false);
      }
      return;
    }
    const file = snapshot.workflows.find((item) => item.path === activeId);
    if (!file) {
      // 选中的文件已被删除：清空并回到自动选择
      setActive(null);
      setDraft(null);
      setLoadError("");
      setFileUpdated(false);
      setDirty(false);
      setActiveId(snapshot.workflows[0]?.path ?? "");
      return;
    }
    // 已加载且文件没变：什么都不做（列表刷新时也不重载，避免丢草稿）
    if (active && active.id === activeId && loadedModified === file.modified) return;
    if (active && active.id === activeId && loadedModified !== file.modified) {
      if (dirty) {
        setFileUpdated(true);
        return;
      }
      // 无本地改动，静默重载新内容
    }
    let cancelled = false;
    setLoadError("");
    void runtime.loadWorkflow(file).then((result) => {
      if (cancelled) return;
      if (result.ok) {
        setActive(result.workflow);
        setDraft(structuredClone(result.workflow.prompt));
        setSeedMode(defaultSeedKeys(result.workflow.prompt));
        setLoadedModified(result.workflow.modified);
        setDirty(false);
        setFileUpdated(false);
      } else {
        setActive(null);
        setDraft(null);
        setLoadError(result.error);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [activeId, snapshot.workflows, active, loadedModified, dirty, runtime]);

  /** 手动重载：放弃本地参数改动，读回文件当前内容。 */
  const reloadActive = React.useCallback(() => {
    setDirty(false);
    setFileUpdated(false);
    setActive(null);
  }, []);

  const fields = React.useMemo(
    () => (draft ? buildFields(draft, snapshot.objectInfo) : []),
    [draft, snapshot.objectInfo],
  );

  /** 高级表单排除未连线的 LoadImage 的 image/upload——它们由参考图栈接管，列表解析拿不到真实候选；连线的仍以只读来源展示。 */
  const formFields = React.useMemo(
    () => fields.filter((f) => !(f.classType === "LoadImage" && !f.connected && (f.input === "image" || f.input === "upload"))),
    [fields],
  );

  const visibleFields = React.useMemo(() => {
    const keyword = filter.trim().toLowerCase();
    if (!keyword) return formFields;
    return formFields.filter(
      (field) =>
        field.input.toLowerCase().includes(keyword) ||
        field.nodeLabel.toLowerCase().includes(keyword) ||
        field.classType.toLowerCase().includes(keyword),
    );
  }, [formFields, filter]);

  /**
   * 按节点分组（含全部字段）。卡片内常用参数排前；含常用参数的节点排前且默认展开，
   * 其余折叠——重要参数无需滚动即可见，又不重复展示同一节点。
   */
  const nodeGroups = React.useMemo(() => {
    const map = new Map<string, FieldSpec[]>();
    for (const field of visibleFields) {
      const list = map.get(field.nodeId) ?? [];
      list.push(field);
      map.set(field.nodeId, list);
    }
    const groups = Array.from(map.entries());
    for (const [, list] of groups) {
      list.sort((a, b) => Number(isCommonField(b)) - Number(isCommonField(a)));
    }
    const hasCommon = (group: [string, FieldSpec[]]): boolean => group[1].some(isCommonField);
    return groups.sort((a, b) => Number(hasCommon(b)) - Number(hasCommon(a)));
  }, [visibleFields]);

  // 换工作流时：不含常用参数的节点默认折叠，含常用参数的铺开；筛选时忽略折叠态
  React.useEffect(() => {
    const hasCommon = new Set<string>();
    for (const field of fields) {
      if (isCommonField(field)) hasCommon.add(field.nodeId);
    }
    const ids = new Set<string>();
    for (const field of fields) {
      if (!hasCommon.has(field.nodeId)) ids.add(field.nodeId);
    }
    setCollapsed(ids);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId]);

  const toggleCollapsed = React.useCallback((nodeId: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  }, []);

  /**
   * 常用参数锚点：只认未连线字段（连线输入由工作流拓扑决定，改了等于改结构），
   * 每个锚点缺了就把对应控件隐藏，整体退化为纯高级表单。
   */
  const unconnected = (input: string): FieldSpec | undefined =>
    fields.find((f) => f.input === input && !f.connected);
  /**
   * 主提示词字段：优先用户在高级参数里指定的（`nodeId.input` 精确匹配）；
   * 未指定时自动取第一个未连线提示词字段（按节点迭代顺序）。
   */
  const candidatePrompts = fields.filter((f) => !f.connected && isPromptInput(f.input));
  const promptField =
    candidatePrompts.find((f) => fieldKey(f) === promptFieldKey) ?? candidatePrompts[0];
  /** 指定的字段在当前工作流已不存在（如切换工作流）时回落到自动。 */
  const promptSelectValue = candidatePrompts.some((f) => fieldKey(f) === promptFieldKey)
    ? promptFieldKey
    : "";
  /**
   * 输出宽高的兜底来源：优先取「空潜像」节点的宽高（同一节点、都未连线），
   * 找不到才退到任意同时含未连线宽高的节点。
   */
  const latentSize = React.useMemo(() => {
    const both = (nodeId: string): { width: FieldSpec; height: FieldSpec } | null => {
      const width = fields.find((f) => f.nodeId === nodeId && f.input === "width" && !f.connected);
      const height = fields.find((f) => f.nodeId === nodeId && f.input === "height" && !f.connected);
      return width && height ? { width, height } : null;
    };
    const isLatent = (nodeId: string): boolean =>
      fields.some((f) => f.nodeId === nodeId && f.classType.startsWith("EmptyLatentImage"));
    const nodeIds = [...new Set(fields.map((f) => f.nodeId))];
    nodeIds.sort((a, b) => Number(isLatent(b)) - Number(isLatent(a)));
    for (const nodeId of nodeIds) {
      const hit = both(nodeId);
      if (hit) return hit;
    }
    return null;
  }, [fields]);

  const widthField = latentSize?.width;
  const heightField = latentSize?.height;
  /**
   * 比例优先读分辨率选择器节点：这类工作流没有可写的宽高字段，输出尺寸由该节点的
   * aspect_ratio 与 megapixels 决定；取不到才退回「空潜像宽高」的通用实现。
   */
  const ratioField = fields.find(
    (f) => f.classType === "ResolutionSelector" && f.input === "aspect_ratio" && !f.connected,
  );
  const megapixelsField = fields.find(
    (f) => f.classType === "ResolutionSelector" && f.input === "megapixels" && !f.connected,
  );
  const multipleField = fields.find(
    (f) => f.classType === "ResolutionSelector" && f.input === "multiple" && !f.connected,
  );
  const batchField = unconnected("batch_size");
  const stepsField = unconnected("steps");
  const cfgField = unconnected("cfg");

  const promptValue = promptField && draft ? String(readField(draft, promptField) ?? "") : "";
  /** 当前宽高与换算出的百万像素（兜底路径用它算比例/分辨率）。 */
  const sizeW = widthField && draft ? Number(readField(draft, widthField) ?? 0) : 0;
  const sizeH = heightField && draft ? Number(readField(draft, heightField) ?? 0) : 0;
  const activeRatio =
    sizeW > 0 && sizeH > 0
      ? RATIO_PRESETS.find((p) => Math.abs(sizeW / sizeH - p.w / p.h) < 0.02)?.label ?? ""
      : "";
  const fallbackMp = sizeW > 0 && sizeH > 0 ? (sizeW * sizeH) / (1024 * 1024) : 1;
  const selectorMp = megapixelsField && draft ? Number(readField(draft, megapixelsField) ?? 1) : 1;
  /** 分辨率选择器的取整基数（multiple 字段），换算宽高的提示用它保持一致。 */
  const selectorMultiple = multipleField && draft ? Number(readField(draft, multipleField) ?? 8) : 8;
  const batchValue = batchField && draft ? String(readField(draft, batchField) ?? "1") : "1";

  const setFieldValue = React.useCallback((field: FieldSpec, raw: string | boolean) => {
    setDirty(true);
    setDraft((current) => {
      if (!current) return current;
      return writeField(current, field.nodeId, field.input, coerceFieldValue(field, raw));
    });
  }, []);

  const setPrompt = React.useCallback(
    (value: string) => {
      if (!promptField) return;
      setDirty(true);
      setDraft((current) =>
        current ? writeField(current, promptField.nodeId, promptField.input, value) : current,
      );
    },
    [promptField],
  );

  /** 兜底路径：把算出的宽高写进空潜像节点。 */
  const writeSize = React.useCallback(
    (w: number, h: number) => {
      if (!widthField || !heightField) return;
      setDirty(true);
      setDraft((current) => {
        if (!current) return current;
        const withWidth = writeField(current, widthField.nodeId, widthField.input, w);
        return writeField(withWidth, heightField.nodeId, heightField.input, h);
      });
    },
    [widthField, heightField],
  );

  /** 兜底路径：按当前分辨率换算宽高后写入。 */
  const applyRatioFallback = React.useCallback(
    (label: string) => {
      const preset = RATIO_PRESETS.find((p) => p.label === label);
      if (!preset) return;
      const size = sizeForMp(preset.w, preset.h, fallbackMp, 8);
      writeSize(size.w, size.h);
    },
    [fallbackMp, writeSize],
  );

  /** 兜底路径：按当前比例换算宽高后写入。 */
  const applyResolutionFallback = React.useCallback(
    (value: string) => {
      const mp = Number(value);
      if (!Number.isFinite(mp) || mp <= 0) return;
      const wr = sizeW > 0 ? sizeW : 1;
      const hr = sizeH > 0 ? sizeH : 1;
      const size = sizeForMp(wr, hr, mp, 8);
      writeSize(size.w, size.h);
    },
    [sizeW, sizeH, writeSize],
  );

  /** 尺寸选择面板：比例/分辨率/张数合成一个按钮，各段选项按当前路径取数。 */
  const sizeSections: SizeSection[] = [];
  const sizeDisplay: string[] = [];
  let sizeTitle: string | undefined;
  if (draft) {
    const selector =
      ratioField && (ratioField.options?.length ?? 0) > 0 ? ratioField : undefined;
    if (selector) {
      const selectorRatio = String(readField(draft, selector) ?? "");
      sizeSections.push({
        label: "比例",
        value: selectorRatio,
        options: (selector.options ?? []).map((option) => ({ value: option, label: aspectLabel(option) })),
        onChange: (value) => setFieldValue(selector, value),
      });
      sizeDisplay.push(aspectLabel(selectorRatio) || "?");
      if (megapixelsField) {
        const pair = parseAspectPair(selectorRatio);
        sizeSections.push({
          label: "分辨率",
          value: mpKey(selectorMp),
          options: mpSizeOptions(selectorMp, pair, selectorMultiple),
          onChange: (value) => setFieldValue(megapixelsField, value),
        });
        sizeDisplay.push(`${mpKey(selectorMp)}K`);
        const size = pair ? sizeForMp(pair.w, pair.h, selectorMp, selectorMultiple) : null;
        sizeTitle = size ? `${size.w}:${size.h}` : undefined;
      }
    } else if (widthField && heightField) {
      const pair = sizeW > 0 && sizeH > 0 ? { w: sizeW, h: sizeH } : null;
      sizeSections.push({
        label: "比例",
        value: activeRatio,
        options: RATIO_PRESETS.map((preset) => ({ value: preset.label, label: preset.label })),
        onChange: applyRatioFallback,
      });
      sizeSections.push({
        label: "分辨率",
        value: mpKey(fallbackMp),
        options: mpSizeOptions(fallbackMp, pair, 8),
        onChange: applyResolutionFallback,
      });
      sizeDisplay.push(activeRatio || (pair ? `${pair.w}:${pair.h}` : "?"));
      sizeDisplay.push(`${mpKey(fallbackMp)}K`);
      sizeTitle = pair ? `${pair.w}:${pair.h}` : undefined;
    }
    if (batchField) {
      sizeSections.push({
        label: "张数",
        value: batchValue,
        options: batchSegments(batchValue),
        onChange: (value) => setFieldValue(batchField, value),
      });
      sizeDisplay.push(batchValue);
    }
  }

  /** 参考图上传：落地时若已切换工作流，不把文件名写进新草稿的同名节点。 */
  const uploadRefImage = React.useCallback(
    (field: FieldSpec, file: File): Promise<void> => {
      const startedOn = activeIdRef.current;
      return runtime.uploadImage(file).then((ref) => {
        if (activeIdRef.current !== startedOn) return;
        setFieldValue(field, ref.filename);
      });
    },
    [runtime, setFieldValue],
  );

  const toggleSeed = React.useCallback((field: FieldSpec) => {
    setSeedMode((current) => {
      const next = new Set(current);
      const key = fieldKey(field);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  /** 把当前参数草稿写回工作流文件；写完后静默重载读回（内容与草稿一致）。 */
  const doSaveParams = React.useCallback(() => {
    if (!active || !draft) return;
    const file = workflows.find((item) => item.path === active.id);
    if (!file) return;
    setBusy(true);
    setActionError("");
    void runtime
      .saveWorkflowParams(file, draft)
      .then((result) => {
        if (!result.ok) {
          setActionError(result.error);
          return;
        }
        setDirty(false);
        setFileUpdated(false);
        ctx.notification.notify({ level: "success", message: `已保存参数到「${active.name}」` });
      })
      .catch((err: unknown) => setActionError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  }, [active, draft, workflows, runtime, ctx]);

  const doRun = React.useCallback(() => {
    if (!draft || !active) return;
    setActionError("");
    setBusy(true);
    const payload = applySeedMode(draft, seedMode);
    void runtime
      .run({ prompt: payload, title: active.name })
      .catch((err: unknown) => setActionError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  }, [draft, active, seedMode, runtime]);

  const doArchive = React.useCallback(
    async (item: ResultImage) => {
      if (runtime.getSnapshot().vaultKind === "space") {
        setActionError("协作空间仓库暂不支持保存图片：请切换到个人仓库后再保存");
        return;
      }
      setSavingKey(item.key);
      setActionError("");
      try {
        const bytes = await runtime.imageBytes(item.ref);
        const result = await archiveImage(ctx, bytes, item.ref);
        runtime.markSaved(item.key, result.path);
        ctx.notification.notify({
          level: "success",
          message: `已保存到 ${result.path}`,
        });
      } catch (err) {
        setActionError(err instanceof Error ? err.message : String(err));
      } finally {
        setSavingKey(null);
      }
    },
    [ctx, runtime],
  );

  const copyJson = React.useCallback(() => {
    if (!active) return;
    void ctx.clipboard
      .writeText(exportWorkflow(active))
      .then(() => ctx.notification.notify({ level: "success", message: "已复制工作流 JSON 到剪贴板" }))
      .catch((err: unknown) => setActionError(err instanceof Error ? err.message : String(err)));
  }, [active, ctx]);

  const hasFilter = filter.trim().length > 0;

  return (
    <Panel
      toolbar={
        <Toolbar>
          <StatusDot
            tone={snapshot.channel === "direct" ? "ok" : "bad"}
            label={
              snapshot.channel === "direct" ? "已连接" : snapshot.probing ? "连接中…" : "未连接"
            }
          />
          {snapshot.systemStats?.devices?.[0]?.name ? (
            <span style={{ fontSize: 11, color: textMuted }}>{snapshot.systemStats.devices[0].name}</span>
          ) : null}
          <Select
            value={activeId}
            onChange={setActiveId}
            disabled={workflows.length === 0 || offline}
            options={
              workflows.length === 0
                ? [{ value: "", label: "尚无工作流" }]
                : workflows.map((item) => ({ value: item.path, label: workflowDisplayName(item.path) }))
            }
            style={{ maxWidth: 240, flex: 1 }}
          />
          <span style={{ flex: 1 }} />
          <RemoteEntry remote={remote} />
          <Button onClick={() => void runtime.freeMemory()} disabled={offline} title="释放模型/显存，不影响队列">
            <ZapIcon size={12} />
            释放显存
          </Button>
          <Button onClick={() => void runtime.connect()} disabled={snapshot.probing}>
            <RefreshIcon size={12} />
            刷新连接
          </Button>
          {snapshot.settings.processMode === "managed" ? (
            hostSnapshot.running ? (
              <Button tone="danger" onClick={() => void host.stop()}>
                <SquareIcon size={12} />
                停止服务
              </Button>
            ) : (
              <Button onClick={() => void host.start(runtime)} disabled={hostSnapshot.starting}>
                <PlayIcon size={12} />
                {hostSnapshot.starting ? "启动中…" : "启动服务"}
              </Button>
            )
          ) : null}
        </Toolbar>
      }
    >
      {actionError ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="error" onClose={() => setActionError("")}>
            {actionError}
          </Notice>
        </div>
      ) : null}
      <style>{SCROLLBAR_CSS}</style>
      {snapshot.error ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="error" onClose={() => runtime.clearError()}>
            {snapshot.error}
          </Notice>
        </div>
      ) : null}
      {fileUpdated ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="warn" onClose={() => setFileUpdated(false)}>
            工作流文件已在编排界面更新；你正在调参的改动会被保留
            <span style={{ display: "inline-flex", marginLeft: 8 }}>
              <Button onClick={reloadActive}>重新载入</Button>
            </span>
          </Notice>
        </div>
      ) : null}
      {loadError ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="error" onClose={() => setLoadError("")}>
            {loadError}；该工作流请在编排界面打开
          </Notice>
        </div>
      ) : null}

      {!draft ? (
        <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
          {offline ? (
            <Empty hint="连接后自动发现 ComfyUI 目录里的工作流">
              <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                <UploadIcon size={22} />
                未连接
              </span>
            </Empty>
          ) : snapshot.workflowsError ? (
            <Empty hint="工作流列表来自 ComfyUI 目录接口">
              <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                <UploadIcon size={22} />
                {snapshot.workflowsError}
              </span>
            </Empty>
          ) : workflows.length === 0 ? (
            <Empty hint="在编排界面保存的工作流会自动出现在这里">
              <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                <UploadIcon size={22} />
                还没有工作流
              </span>
            </Empty>
          ) : loadError ? (
            <Empty hint="该工作流请在编排界面打开">
              <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                <UploadIcon size={22} />
                无法在生成面板使用
              </span>
            </Empty>
          ) : (
            <Empty hint="文件读取失败">
              <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                <UploadIcon size={22} />
                读取失败
              </span>
            </Empty>
          )}
        </div>
      ) : (
        <>
          <RecordFlow
            ctx={ctx}
            runtime={runtime}
            results={snapshot.results}
            running={snapshot.queue.queue_running}
            pending={snapshot.queue.queue_pending}
            progress={snapshot.progress}
            offline={offline}
            saveDisabled={snapshot.vaultKind === "space"}
            savingKey={savingKey}
            onArchive={(item) => void doArchive(item)}
            onInterrupt={() => void runtime.interrupt()}
          />

          {/* 底部生成坞：高度吃紧时自身让出空间给记录流，高级区内部滚动 */}
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              minHeight: 0,
              flexShrink: 1,
              borderTop: `1px solid ${border}`,
              background: bgSecondary,
              paddingBottom: 8,
            }}
          >
            <div
              style={{
                display: "flex",
                alignItems: "stretch",
                gap: 10,
                margin: 8,
                padding: 10,
                borderRadius: 14,
                border: `1px solid ${border}`,
                background: bgPrimary,
                boxShadow: "0 2px 10px rgba(0,0,0,0.06)",
                flexShrink: 0,
              }}
            >
              <RefImageStack
                fields={fields}
                draft={draft}
                objectInfo={snapshot.objectInfo}
                runtime={runtime}
                offline={offline}
                onChange={(field, value) => setFieldValue(field, value)}
                onUpload={uploadRefImage}
                onError={setActionError}
              />
              <AutoTextArea
                value={promptValue}
                onChange={setPrompt}
                placeholder={
                  promptField
                    ? "描述你想生成的内容…"
                    : "该工作流没有提示词字段，请到高级参数或编排界面调整"
                }
                disabled={!promptField}
                minHeight={60}
                maxHeight={160}
                style={{ flex: 1, minWidth: 0 }}
              />
              <button
                type="button"
                onClick={doRun}
                disabled={!draft || busy || offline}
                title={busy ? "提交中…" : "生成"}
                style={{
                  alignSelf: "center",
                  width: 44,
                  height: 44,
                  borderRadius: 22,
                  border: "none",
                  flexShrink: 0,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  background: !draft || busy || offline ? hover : accent,
                  color: accentFg,
                  cursor: !draft || busy || offline ? "not-allowed" : "pointer",
                }}
              >
                <PlayIcon size={18} />
              </button>
            </div>

            {/* 常用参数条 */}
            <div
              style={{
                display: "flex",
                flexWrap: "wrap",
                gap: "6px 12px",
                alignItems: "flex-start",
                padding: "0 10px 8px",
                flexShrink: 0,
              }}
            >
              {sizeSections.length > 0 ? (
                <SizePicker
                  display={sizeDisplay.join(" · ")}
                  title={sizeTitle}
                  disabled={offline}
                  sections={sizeSections}
                />
              ) : null}
              {stepsField ? (
                <ParamControl label="步数">
                  <NumberControl field={stepsField} value={readField(draft, stepsField)} onChange={(raw) => setFieldValue(stepsField, raw)} />
                </ParamControl>
              ) : null}
              {cfgField ? (
                <ParamControl label="CFG">
                  <NumberControl field={cfgField} value={readField(draft, cfgField)} onChange={(raw) => setFieldValue(cfgField, raw)} />
                </ParamControl>
              ) : null}
            </div>

            {/* 高级参数折叠区：展开时占满生成坞剩余高度，内容超高内部滚动 */}
            <div style={{ display: "flex", flexDirection: "column", minHeight: 0, flex: 1, padding: "0 10px" }}>
              <div
                style={{
                  display: "flex",
                  flexDirection: "column",
                  minHeight: 0,
                  flex: 1,
                  border: `1px solid ${border}`,
                  borderRadius: 8,
                  background: bgPrimary,
                  overflow: "hidden",
                }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "6px 10px", flexShrink: 0 }}>
                  <button
                    type="button"
                    onClick={() => setAdvancedOpen(!advancedOpen)}
                    style={{
                      display: "inline-flex",
                      alignItems: "center",
                      gap: 4,
                      border: "none",
                      background: "transparent",
                      color: textPrimary,
                      fontSize: FONT_SM,
                      cursor: "pointer",
                      padding: "2px 0",
                    }}
                  >
                    {advancedOpen ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
                    高级参数
                  </button>
                  <span style={{ fontSize: 10, color: textMuted }}>
                    {Object.keys(draft).length} 节点 · {fields.length} 项参数
                  </span>
                  <span style={{ flex: 1 }} />
                  <Button onClick={copyJson} disabled={!active} title="复制这份工作流的 API JSON">
                    <CopyIcon size={12} />
                    复制 JSON
                  </Button>
                  <Button
                    onClick={doSaveParams}
                    disabled={!active || !draft || busy || offline}
                    title="把当前参数写回工作流文件"
                  >
                    保存参数
                  </Button>
                  <Button
                    onClick={reloadActive}
                    disabled={!activeId || offline}
                    title="放弃本地参数改动，读回工作流文件里的参数"
                  >
                    重新加载
                  </Button>
                </div>
                {advancedOpen ? (
                  <div
                    className={SCROLL_LIST_CLASS}
                    style={{
                      padding: "8px 10px 10px",
                      borderTop: `1px solid ${border}`,
                      flex: 1,
                      minHeight: 0,
                      overflowY: "auto",
                    }}
                  >
                    {candidatePrompts.length > 1 || promptFieldKey ? (
                      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
                        <span style={{ fontSize: FONT_SM, color: textSecondary, flexShrink: 0 }}>主提示词</span>
                        <Select
                          value={promptSelectValue}
                          onChange={setPromptFieldKey}
                          options={[
                            { value: "", label: "自动（第一个提示词字段）" },
                            ...candidatePrompts.map((f) => ({
                              value: fieldKey(f),
                              label: `${f.nodeLabel} · ${fieldLabel(f)}`,
                            })),
                          ]}
                          title="底部大输入框绑定的提示词字段；有多个提示词节点时手动指定"
                        />
                      </div>
                    ) : null}
                    {countDisabledNodes(draft) > 0 ? (
                      <div style={{ marginBottom: 8 }}>
                        <Notice tone="warn">存在被禁用/跳过的节点，结果异常请回 ComfyUI 确认</Notice>
                      </div>
                    ) : null}
                    <div style={{ position: "relative", marginBottom: 8 }}>
                      <span
                        style={{
                          position: "absolute",
                          left: 8,
                          top: "50%",
                          transform: "translateY(-50%)",
                          display: "inline-flex",
                          color: textMuted,
                          pointerEvents: "none",
                        }}
                      >
                        <SearchIcon size={12} />
                      </span>
                      <TextInput
                        value={filter}
                        onChange={setFilter}
                        placeholder="筛选参数…"
                        title="按参数名、节点名或节点类型筛选"
                        style={{ paddingLeft: 26 }}
                      />
                    </div>
                    {visibleFields.length === 0 ? (
                      <div style={{ padding: "16px 0", textAlign: "center", fontSize: FONT_SM, color: textMuted }}>
                        没有匹配的参数
                      </div>
                    ) : (
                      nodeGroups.map(([nodeId, nodeFields]) => (
                        <CollapseCard
                          key={nodeId}
                          title={nodeFields[0]?.nodeLabel ?? nodeId}
                          subtitle={nodeFields[0]?.classType ?? ""}
                          badge={`${nodeFields.length} 字段`}
                          open={hasFilter || !collapsed.has(nodeId)}
                          onToggle={() => toggleCollapsed(nodeId)}
                        >
                          {nodeFields.map((field) => (
                            <FieldRow
                              key={fieldKey(field)}
                              field={field}
                              value={readField(draft, field)}
                              seedChecked={seedMode.has(fieldKey(field))}
                              onToggleSeed={() => toggleSeed(field)}
                              onChange={(raw) => setFieldValue(field, raw)}
                            />
                          ))}
                        </CollapseCard>
                      ))
                    )}
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        </>
      )}
    </Panel>
  );
}

/** 分辨率选择器的候选值带注释（如 "1:1 (Square)"），宫格只展示比例部分。 */
function aspectLabel(option: string): string {
  const paren = option.indexOf(" (");
  return paren > 0 ? option.slice(0, paren) : option;
}

/** 由比例与百万像素算宽高：就近取整到 multiple 的倍数（与分辨率选择器节点同式）。 */
function sizeForMp(wr: number, hr: number, mp: number, multiple = 8): { w: number; h: number } {
  const total = (mp > 0 ? mp : 1) * 1024 * 1024;
  const scale = Math.sqrt(total / (wr * hr));
  const snap = (n: number): number => Math.max(multiple, Math.round(n / multiple) * multiple);
  return { w: snap(wr * scale), h: snap(hr * scale) };
}

/** 百万像素的数字键：整数不带小数点（1 → "1"，1.5 → "1.5"），与预设值对齐。 */
function mpKey(mp: number): string {
  return String(Number(mp.toFixed(1)));
}

/** 分辨率分段：当前值不在预设里时追加为一项，保证选中态可见。 */
function megapixelSegments(current: number): Array<{ value: string; label: string }> {
  const cur = mpKey(current);
  const list = MEGAPIXEL_OPTIONS.includes(cur) ? MEGAPIXEL_OPTIONS : [...MEGAPIXEL_OPTIONS, cur];
  return list.map((item) => ({ value: item, label: `${item}K` }));
}

/** 分辨率选项：按当前比例与取整基数附带换算出的宽高提示。 */
function mpSizeOptions(
  mp: number,
  pair: { w: number; h: number } | null,
  multiple: number,
): Array<{ value: string; label: string; title?: string }> {
  return megapixelSegments(mp).map((option) => {
    const size = pair ? sizeForMp(pair.w, pair.h, Number(option.value), multiple) : null;
    return size ? { ...option, title: `${size.w}:${size.h}` } : option;
  });
}

/** 主参数条里的一个带标签控件。 */
function ParamControl(props: { label: string; children: unknown }): unknown {
  return (
    <span style={{ display: "flex", flexDirection: "column", gap: 2, fontSize: 10, color: textMuted }}>
      <span>{props.label}</span>
      {props.children}
    </span>
  );
}

/** 尺寸选择面板的一类选项（比例/分辨率/张数各一段）。 */
interface SizeSection {
  label: string;
  value: string;
  options: Array<{ value: string; label: string; title?: string }>;
  onChange: (value: string) => void;
}

/** 尺寸选择：比例/分辨率/张数合成一个按钮，点击弹出分层选择面板。 */
function SizePicker(props: {
  display: string;
  disabled?: boolean;
  title?: string;
  sections: SizeSection[];
}): unknown {
  const [open, setOpen] = React.useState(false);
  const [openUp, setOpenUp] = React.useState(false);
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const panelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  /** 按按钮与面板的实际尺寸决定弹出方向：下方放不下就翻到上方（视口钳制）。 */
  const measure = React.useCallback(() => {
    const root = rootRef.current;
    const panel = panelRef.current;
    if (!root || !panel) return;
    const rect = root.getBoundingClientRect();
    const height = panel.offsetHeight;
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    setOpenUp(spaceBelow < height + 8 && spaceAbove > spaceBelow);
  }, []);

  React.useLayoutEffect(() => {
    if (open) measure();
  }, [open, measure]);

  React.useEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    const observer = panel ? new ResizeObserver(measure) : null;
    if (observer && panel) observer.observe(panel);
    window.addEventListener("resize", measure);
    return () => {
      if (observer) observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [open, measure]);

  return (
    <div
      ref={rootRef}
      style={{
        position: "relative",
        display: "flex",
        flexDirection: "column",
        gap: 2,
        fontSize: 10,
        color: textMuted,
      }}
    >
      <span>尺寸</span>
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        disabled={props.disabled}
        title={props.title}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 4,
          padding: "4px 10px",
          borderRadius: 6,
          border: `1px solid ${border}`,
          background: open ? hover : "transparent",
          color: textPrimary,
          fontSize: FONT_SM,
          fontFamily: "inherit",
          cursor: props.disabled ? "not-allowed" : "pointer",
          opacity: props.disabled ? 0.5 : 1,
          whiteSpace: "nowrap",
        }}
      >
        {props.display}
      </button>
      {open ? (
        <div
          ref={panelRef}
          style={{
            position: "absolute",
            left: 0,
            ...(openUp ? { bottom: "calc(100% + 4px)" } : { top: "calc(100% + 4px)" }),
            zIndex: 50,
            minWidth: 280,
            padding: 10,
            display: "flex",
            flexDirection: "column",
            gap: 10,
            borderRadius: 8,
            border: `1px solid ${border}`,
            background: bgSecondary,
            boxShadow: "0 10px 32px rgba(0,0,0,0.22)",
          }}
        >
          {props.sections.map((section) => (
            <div key={section.label} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <span style={{ fontSize: 10, color: textMuted }}>{section.label}</span>
              <OptionRow options={section.options} value={section.value} onChange={section.onChange} />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** 面板内的一行选项按钮（自动换行，选项可带 title 提示）。 */
function OptionRow(props: {
  options: Array<{ value: string; label: string; title?: string }>;
  value: string;
  onChange: (value: string) => void;
}): unknown {
  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
      {props.options.map((option) => {
        const active = option.value === props.value;
        return (
          <button
            key={option.value}
            type="button"
            title={option.title}
            onClick={() => props.onChange(option.value)}
            style={{
              border: `1px solid ${active ? accent : border}`,
              borderRadius: 6,
              padding: "3px 8px",
              fontSize: FONT_SM,
              fontFamily: "inherit",
              background: active ? accent : "transparent",
              color: active ? accentFg : textSecondary,
              cursor: "pointer",
              whiteSpace: "nowrap",
            }}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/** 数字字段的窄输入框（步数/CFG/降噪/种子共用）。 */
function NumberControl(props: {
  field: FieldSpec;
  value: unknown;
  disabled?: boolean;
  onChange: (raw: string) => void;
}): unknown {
  return (
    <TextInput
      type="number"
      value={displayValue(props.value)}
      onChange={props.onChange}
      min={props.field.min}
      max={props.field.max}
      step={props.field.step ?? (props.field.kind === "int" ? 1 : 0.01)}
      disabled={props.disabled}
      style={{ width: 72 }}
    />
  );
}

/** 张数分段：当前值不在预设里时追加为一项，保证选中态可见。 */
function batchSegments(value: string): Array<{ value: string; label: string }> {
  const list = BATCH_OPTIONS.includes(value) ? BATCH_OPTIONS : [...BATCH_OPTIONS, value];
  return list.map((item) => ({ value: item, label: item }));
}

/** 一行参数：按类型渲染控件；连线输入只读展示来源。 */
function FieldRow(props: {
  field: FieldSpec;
  value: unknown;
  seedChecked: boolean;
  onToggleSeed: () => void;
  onChange: (raw: string | boolean) => void;
}): unknown {
  const { field, value } = props;
  const label = (
    <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
      <span
        style={{
          fontSize: 11,
          color: textSecondary,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
        title={`${field.nodeId}.${field.input}`}
      >
        {fieldLabel(field)}
      </span>
      {field.isSeed ? (
        <Checkbox checked={props.seedChecked} onChange={props.onToggleSeed} label="每次随机" title="每次生成时换一个新种子" />
      ) : null}
    </div>
  );

  if (field.connected) {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <div style={{ width: 180, flexShrink: 0 }}>{label}</div>
        <Chip title="由工作流的连线提供">来源：{field.linkFrom}</Chip>
      </div>
    );
  }

  const disabled = field.isSeed && props.seedChecked;
  let control: unknown;
  if (field.kind === "boolean") {
    control = (
      <Checkbox
        checked={value === true}
        onChange={(checked) => props.onChange(checked)}
        label={value === true ? "开启" : "关闭"}
      />
    );
  } else if (field.kind === "combo" && field.options) {
    control = (
      <Select
        value={displayValue(value)}
        onChange={props.onChange}
        options={field.options.map((option) => ({
          value: option,
          label: field.optionLabels?.[option] ?? option,
        }))}
      />
    );
  } else if (field.kind === "int" || field.kind === "float") {
    control = (
      <TextInput
        type="number"
        value={displayValue(value)}
        onChange={props.onChange}
        min={field.min}
        max={field.max}
        step={field.step ?? (field.kind === "int" ? 1 : 0.01)}
        disabled={disabled}
      />
    );
  } else if (field.kind === "json") {
    control = <TextArea value={displayValue(value)} onChange={props.onChange} rows={2} mono />;
  } else {
    control = field.multiline ? (
      <TextArea value={displayValue(value)} onChange={props.onChange} rows={4} />
    ) : (
      <TextInput value={displayValue(value)} onChange={props.onChange} disabled={disabled} />
    );
  }

  return (
    <div style={{ display: "flex", alignItems: "flex-start", gap: 8 }}>
      <div style={{ width: 180, flexShrink: 0, paddingTop: 4 }}>{label}</div>
      <div style={{ flex: 1, minWidth: 0 }}>{control}</div>
    </div>
  );
}
