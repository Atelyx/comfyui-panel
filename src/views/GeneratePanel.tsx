/**
 * 生成面板：选工作流、调参数、提交、看进度、取结果。
 *
 * 三段布局各自滚动，避免长参数表把结果区挤没。
 */
import React from "react";
import type { AtelyxCtx } from "../ctx";
import type { ApiPrompt } from "../comfy/types";
import type { ComfyRuntime, ResultImage } from "../runtime";
import type { HostController } from "../host/controller";
import { archiveImage, appendToCurrentNote } from "../host/archive";
import {
  addWorkflow,
  exportWorkflow,
  listWorkflows,
  loadWorkflow,
  validateApiPrompt,
  type StoredWorkflow,
  type WorkflowSummary,
} from "../workflow/library";
import {
  applySeedMode,
  buildFields,
  coerceFieldValue,
  countDisabledNodes,
  displayValue,
  fieldKey,
  fieldLabel,
  isCommonField,
  readField,
  writeField,
  type FieldSpec,
} from "../workflow/form";
import {
  Button,
  Checkbox,
  Chip,
  CollapseCard,
  CopyIcon,
  Empty,
  FolderIcon,
  ImageIcon,
  Notice,
  Panel,
  PlayIcon,
  RefreshIcon,
  SearchIcon,
  SectionTitle,
  Select,
  SquareIcon,
  StatusDot,
  TextArea,
  TextInput,
  Toolbar,
  UploadIcon,
  ZapIcon,
  accent,
  bgSecondary,
  border,
  danger,
  textMuted,
  textPrimary,
  textSecondary,
  hover,
  FONT_SM,
} from "./ui";

interface GeneratePanelProps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  host: HostController;
}

export function GeneratePanel(props: GeneratePanelProps): unknown {
  const { ctx, runtime, host } = props;
  const snapshot = React.useSyncExternalStore(runtime.subscribe, runtime.getSnapshot);
  const hostSnapshot = React.useSyncExternalStore(host.subscribe, host.getSnapshot);

  const [workflows, setWorkflows] = React.useState<WorkflowSummary[]>([]);
  const [activeId, setActiveId] = React.useState<string>("");
  const [active, setActive] = React.useState<StoredWorkflow | null>(null);
  const [draft, setDraft] = React.useState<ApiPrompt | null>(null);
  const [seedMode, setSeedMode] = React.useState<ReadonlySet<string>>(new Set());
  const [importOpen, setImportOpen] = React.useState(false);
  const [importText, setImportText] = React.useState("");
  const [importError, setImportError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [actionError, setActionError] = React.useState("");
  const [filter, setFilter] = React.useState("");
  const [collapsed, setCollapsed] = React.useState<ReadonlySet<string>>(new Set());
  const [dragOver, setDragOver] = React.useState(false);
  const [previewKey, setPreviewKey] = React.useState<string | null>(null);
  const [savingKey, setSavingKey] = React.useState<string | null>(null);
  const fileInputRef = React.useRef<HTMLInputElement | null>(null);

  const refreshWorkflows = React.useCallback(() => {
    void listWorkflows(ctx).then((items) => {
      setWorkflows(items);
      setActiveId((current) => {
        if (current && items.some((item) => item.id === current)) return current;
        return items[0]?.id ?? "";
      });
    });
  }, [ctx]);

  React.useEffect(() => {
    refreshWorkflows();
  }, [refreshWorkflows]);

  // 草稿是本次提交用的副本，改参数不动库里那份
  React.useEffect(() => {
    if (!activeId) {
      setActive(null);
      setDraft(null);
      return;
    }
    let cancelled = false;
    void loadWorkflow(ctx, activeId).then((loaded) => {
      if (cancelled) return;
      setActive(loaded);
      setDraft(loaded ? structuredClone(loaded.prompt) : null);
      setSeedMode(new Set());
    });
    return () => {
      cancelled = true;
    };
  }, [ctx, activeId]);

  const fields = React.useMemo(
    () => (draft ? buildFields(draft, snapshot.objectInfo) : []),
    [draft, snapshot.objectInfo],
  );

  const visibleFields = React.useMemo(() => {
    const keyword = filter.trim().toLowerCase();
    if (!keyword) return fields;
    return fields.filter(
      (field) =>
        field.input.toLowerCase().includes(keyword) ||
        field.nodeLabel.toLowerCase().includes(keyword) ||
        field.classType.toLowerCase().includes(keyword),
    );
  }, [fields, filter]);

  /** 置顶的常用参数（提示词/种子/采样/尺寸），带节点标题作区分。 */
  const commonFields = React.useMemo(
    () => visibleFields.filter(isCommonField),
    [visibleFields],
  );

  /** 其余字段按节点分组，便于用户对上 ComfyUI 里的节点。 */
  const nodeGroups = React.useMemo(() => {
    const map = new Map<string, FieldSpec[]>();
    for (const field of visibleFields) {
      if (isCommonField(field)) continue;
      const list = map.get(field.nodeId) ?? [];
      list.push(field);
      map.set(field.nodeId, list);
    }
    return Array.from(map.entries());
  }, [visibleFields]);

  // 换工作流时把非常用节点默认折叠，只铺开常用参数；筛选时忽略折叠态
  React.useEffect(() => {
    const ids = new Set<string>();
    for (const field of fields) {
      if (!isCommonField(field)) ids.add(field.nodeId);
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

  const hasFilter = filter.trim().length > 0;

  /** 输出比例与批次：从草稿的尺寸/批次输入读，生成中的占位动画按它布局。 */
  const genSpec = React.useMemo(() => {
    if (!draft) return { aspect: 1, batch: 1 };
    let width: number | null = null;
    let height: number | null = null;
    let batch = 1;
    for (const node of Object.values(draft)) {
      const inputs = node.inputs ?? {};
      for (const [name, value] of Object.entries(inputs)) {
        if (typeof value !== "number") continue;
        if (name === "width" && width === null) width = value;
        else if (name === "height" && height === null) height = value;
        else if (name === "batch_size" && value >= 1) batch = Math.floor(value);
      }
    }
    const aspect = width !== null && height !== null && width > 0 && height > 0 ? width / height : 1;
    return { aspect, batch };
  }, [draft]);

  const setFieldValue = React.useCallback((field: FieldSpec, raw: string | boolean) => {
    setDraft((current) => {
      if (!current) return current;
      return writeField(current, field.nodeId, field.input, coerceFieldValue(field, raw));
    });
  }, []);

  const toggleSeed = React.useCallback((field: FieldSpec) => {
    setSeedMode((current) => {
      const next = new Set(current);
      const key = fieldKey(field);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  /** 读入文件内容到导入框（拖拽与文件选择共用），仍由用户点「导入」确认。 */
  const readJsonFile = React.useCallback((file: File) => {
    setImportError("");
    void file
      .text()
      .then(setImportText)
      .catch((err: unknown) =>
        setImportError(`读取文件失败：${err instanceof Error ? err.message : String(err)}`),
      );
  }, []);

  const doImport = React.useCallback(() => {
    const checked = validateApiPrompt(importText);
    if (!checked.ok || !checked.prompt) {
      setImportError(checked.error ?? "导入失败");
      return;
    }
    setBusy(true);
    void addWorkflow(ctx, checked.prompt)
      .then((stored) => {
        setImportOpen(false);
        setImportText("");
        setImportError("");
        refreshWorkflows();
        setActiveId(stored.id);
      })
      .catch((err: unknown) => setImportError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  }, [ctx, importText, refreshWorkflows]);

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
      setSavingKey(item.key);
      setActionError("");
      try {
        const bytes = await runtime.imageBytes(item.ref);
        const result = await archiveImage(ctx, bytes, item.ref);
        runtime.markSaved(item.key, result.path);
        let appended = false;
        if (runtime.getSnapshot().settings.appendToNote) {
          appended = await appendToCurrentNote(ctx, result.path);
        }
        ctx.notification.notify({
          level: "success",
          message: appended ? `已保存到 ${result.path} 并追加到当前笔记` : `已保存到 ${result.path}`,
        });
      } catch (err) {
        setActionError(err instanceof Error ? err.message : String(err));
      } finally {
        setSavingKey(null);
      }
    },
    [ctx, runtime],
  );

  const runningCount = snapshot.queue.queue_running.length + snapshot.queue.queue_pending.length;
  const progressValue =
    snapshot.progress && snapshot.progress.max > 0 ? snapshot.progress.value / snapshot.progress.max : 0;
  const previewed = previewKey ? snapshot.results.find((item) => item.key === previewKey) ?? null : null;

  return (
    <Panel
      toolbar={
        <Toolbar>
          <StatusDot
            tone={snapshot.channel === "direct" ? "ok" : "bad"}
            label={
              snapshot.channel === "direct" ? "已连接" : snapshot.probing ? "连接中…" : "未连接"
            }
            title={snapshot.channelReason || undefined}
          />
          {snapshot.systemStats?.devices?.[0]?.name ? (
            <span style={{ fontSize: 11, color: textMuted }}>{snapshot.systemStats.devices[0].name}</span>
          ) : null}
          <span style={{ flex: 1 }} />
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
      {snapshot.channelReason ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone={snapshot.channel === "offline" ? "error" : "warn"}>{snapshot.channelReason}</Notice>
        </div>
      ) : null}
      {actionError ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="error" onClose={() => setActionError("")}>
            {actionError}
          </Notice>
        </div>
      ) : null}
      {snapshot.error ? (
        <div style={{ padding: "8px 10px 0" }}>
          <Notice tone="error" onClose={() => runtime.clearError()}>
            {snapshot.error}
          </Notice>
        </div>
      ) : null}

      {/* 队列与进度 */}
      {runningCount > 0 || snapshot.progress ? (
        <div
          style={{
            padding: 10,
            display: "flex",
            flexDirection: "column",
            gap: 8,
            border: `1px solid ${border}`,
            borderRadius: 8,
            background: bgSecondary,
            margin: "0 10px 8px",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span style={{ fontSize: FONT_SM, color: textSecondary }}>
              运行中 {snapshot.queue.queue_running.length} · 等待 {snapshot.queue.queue_pending.length}
            </span>
            <span style={{ flex: 1 }} />
            <Button tone="danger" onClick={() => void runtime.interrupt()}>
              中断
            </Button>
          </div>
          <GenerationGrid
            aspect={genSpec.aspect}
            batch={genSpec.batch}
            percent={snapshot.progress && snapshot.progress.max > 0 ? progressValue * 100 : null}
            node={snapshot.progress?.node ?? ""}
          />
        </div>
      ) : null}

      {/* 工作流与参数 */}
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "8px 0" }}>
        <div style={{ display: "flex", gap: 6, alignItems: "center", padding: "0 10px 8px" }}>
          <Select
            value={activeId}
            onChange={setActiveId}
            disabled={workflows.length === 0}
            options={
              workflows.length === 0
                ? [{ value: "", label: "尚无工作流" }]
                : workflows.map((item) => ({ value: item.id, label: item.name }))
            }
            style={{ maxWidth: 260 }}
          />
          <Button onClick={() => setImportOpen((open) => !open)}>
            {importOpen ? "取消导入" : (
              <>
                <UploadIcon size={12} />
                导入工作流
              </>
            )}
          </Button>
          {draft ? (
            <span style={{ flex: 1, textAlign: "right", fontSize: 11, color: textMuted }}>
              {Object.keys(draft).length} 个节点 · {fields.length} 项参数
            </span>
          ) : null}
        </div>

        {importOpen ? (
          <div
            onDragOver={(e: { preventDefault(): void }) => {
              e.preventDefault();
              setDragOver(true);
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={(e: { preventDefault(): void; dataTransfer: { files?: FileList | null } }) => {
              e.preventDefault();
              setDragOver(false);
              const file = e.dataTransfer.files?.[0];
              if (file) readJsonFile(file);
            }}
            style={{
              border: `1px solid ${dragOver ? accent : border}`,
              borderRadius: 8,
              padding: 10,
              margin: "0 10px 10px",
              background: bgSecondary,
            }}
          >
            <div style={{ fontSize: 11, color: textMuted, lineHeight: 1.6, marginBottom: 6 }}>
              粘贴 ComfyUI「工作流 → 导出（API）」的 JSON，参数表单由它生成。
            </div>
            <TextArea
              value={importText}
              onChange={setImportText}
              placeholder='{"3": {"class_type": "KSampler", "inputs": { ... }}}'
              rows={6}
              mono
            />
            {importError ? (
              <div style={{ marginTop: 6 }}>
                <Notice tone="error">{importError}</Notice>
              </div>
            ) : null}
            <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
              <Button tone="primary" onClick={doImport} disabled={busy || !importText.trim()}>
                导入
              </Button>
              <Button onClick={() => fileInputRef.current?.click()}>
                <FolderIcon size={12} />
                从文件读取
              </Button>
              <span style={{ alignSelf: "center", fontSize: 11, color: textMuted }}>或拖拽 .json 到此处</span>
            </div>
            {/*
              用原生文件选择而不是宿主的系统对话框：对话框只返回路径，而工作流文件通常在仓库外，
              插件的读取通道只服务仓库内相对路径（绝对路径会被拒），拿不到内容。
              原生选择直接给出文件本体，读内容即可，与文件放在哪里无关。
            */}
            <input
              ref={fileInputRef}
              type="file"
              accept=".json,application/json"
              style={{ display: "none" }}
              onChange={(e: { target: { files?: FileList | null; value: string } }) => {
                const file = e.target.files?.[0];
                // 清空以便连续选同一个文件也能再次触发 change
                e.target.value = "";
                if (file) readJsonFile(file);
              }}
            />
          </div>
        ) : null}

        {!draft ? (
          <Empty hint="导入工作流后即可调参生成">
            <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
              <UploadIcon size={22} />
              {workflows.length === 0 ? "还没有工作流" : "工作流读取失败，请重新导入"}
            </span>
          </Empty>
        ) : (
          <>
            {countDisabledNodes(draft) > 0 ? (
              <div style={{ margin: "0 10px 8px" }}>
                <Notice tone="warn">存在被禁用/跳过的节点，结果异常请回 ComfyUI 确认</Notice>
              </div>
            ) : null}

            <div style={{ display: "flex", gap: 6, alignItems: "center", padding: "0 10px 8px" }}>
              <div style={{ position: "relative", flex: 1, minWidth: 0 }}>
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
              <Button onClick={() => active && setDraft(structuredClone(active.prompt))}>重置</Button>
            </div>

            {visibleFields.length === 0 ? (
              <Empty hint="换个关键词或重置筛选">
                <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
                  <SearchIcon size={22} />
                  没有匹配的参数
                </span>
              </Empty>
            ) : (
              <>
                {commonFields.length > 0 ? (
                  <div style={{ marginBottom: 4 }}>
                    <SectionTitle>常用参数</SectionTitle>
                    <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: "0 10px 8px" }}>
                      {commonFields.map((field) => (
                        <FieldRow
                          key={fieldKey(field)}
                          field={field}
                          showNode
                          value={readField(draft, field)}
                          seedChecked={seedMode.has(fieldKey(field))}
                          onToggleSeed={() => toggleSeed(field)}
                          onChange={(raw) => setFieldValue(field, raw)}
                        />
                      ))}
                    </div>
                  </div>
                ) : null}

                {nodeGroups.length > 0 ? (
                  <div style={{ padding: "0 10px" }}>
                    {nodeGroups.map(([nodeId, nodeFields]) => (
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
                    ))}
                  </div>
                ) : null}
              </>
            )}
          </>
        )}
      </div>

      {/* 底部动作条 */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "8px 10px",
          borderTop: `1px solid ${border}`,
          background: bgSecondary,
          flexShrink: 0,
        }}
      >
        <Button tone="primary" onClick={doRun} disabled={!draft || busy || snapshot.channel === "offline"}>
          <PlayIcon size={12} />
          {busy ? "提交中…" : "生成"}
        </Button>
        <Button onClick={() => void runtime.freeMemory()} disabled={snapshot.channel === "offline"}>
          <ZapIcon size={12} />
          释放显存
        </Button>
        <span style={{ flex: 1 }} />
        <Button
          onClick={() => {
            if (!active) return;
            void ctx.clipboard
              .writeText(exportWorkflow(active))
              .then(() => ctx.notification.notify({ level: "success", message: "已复制工作流 JSON 到剪贴板" }))
              .catch((err: unknown) =>
                setActionError(err instanceof Error ? err.message : String(err)),
              );
          }}
          disabled={!active}
          title="复制这份工作流的 API JSON"
        >
          <CopyIcon size={12} />
          复制 JSON
        </Button>
        <span style={{ fontSize: 11, color: textMuted }}>{snapshot.results.length} 个结果</span>
      </div>

      {/* 结果画廊 */}
      <div style={{ borderTop: `1px solid ${border}`, background: bgSecondary }}>
        <SectionTitle>结果</SectionTitle>
        {snapshot.results.length === 0 ? (
          <div
            style={{
              padding: "4px 10px 10px",
              display: "flex",
              alignItems: "center",
              gap: 6,
              fontSize: 11,
              color: textMuted,
            }}
          >
            <ImageIcon size={14} />
            生成后，结果会显示在这里
          </div>
        ) : (
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fill, minmax(96px, 1fr))",
              gap: 6,
              padding: "0 10px 10px",
              maxHeight: 220,
              overflowY: "auto",
            }}
          >
            {snapshot.results.map((item) => (
              <ResultThumb
                key={item.key}
                item={item}
                url={runtime.imageUrl(item.ref)}
                saving={savingKey === item.key}
                onPreview={() => setPreviewKey(item.key)}
                onArchive={() => void doArchive(item)}
              />
            ))}
          </div>
        )}
      </div>

      {previewed ? <PreviewOverlay item={previewed} url={runtime.imageUrl(previewed.ref, false)} onClose={() => setPreviewKey(null)} /> : null}
    </Panel>
  );
}

/** 一行参数：按类型渲染控件；连线输入只读展示来源。 */
function FieldRow(props: {
  field: FieldSpec;
  value: unknown;
  seedChecked: boolean;
  /** 常用参数区需要带上节点名作区分。 */
  showNode?: boolean;
  onToggleSeed: () => void;
  onChange: (raw: string | boolean) => void;
}): unknown {
  const { field, value } = props;
  const label = (
    <div style={{ display: "flex", flexDirection: "column", gap: 1, minWidth: 0 }}>
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
      {props.showNode ? (
        <span
          style={{
            fontSize: 10,
            color: textMuted,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
          title={field.nodeLabel}
        >
          {field.nodeLabel}
        </span>
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

/** 悬停显示操作。 */
function ResultThumb(props: {
  item: ResultImage;
  url: string;
  saving: boolean;
  onPreview: () => void;
  onArchive: () => void;
}): unknown {
  const [isHover, setHover] = React.useState(false);
  const { item } = props;
  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        position: "relative",
        border: `1px solid ${item.failed ? danger : border}`,
        borderRadius: 6,
        overflow: "hidden",
        aspectRatio: "1 / 1",
        background: hover ? "var(--hover)" : "transparent",
      }}
    >
      <img
        src={props.url}
        alt={item.ref.filename}
        loading="lazy"
        onClick={props.onPreview}
        style={{ width: "100%", height: "100%", objectFit: "cover", cursor: "zoom-in", display: "block" }}
      />
      {item.savedPath ? (
        <span
          title={item.savedPath}
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
            gap: 4,
            padding: 4,
            background: "rgba(0,0,0,0.55)",
          }}
        >
          <span style={{ flex: 1, minWidth: 0, fontSize: 10, color: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={item.savedPath ?? item.title}>
            {item.title}
          </span>
          <button
            type="button"
            title="保存到仓库"
            disabled={props.saving}
            onClick={props.onArchive}
            style={{
              border: "none",
              borderRadius: 4,
              background: "rgba(255,255,255,0.9)",
              color: "#111",
              fontSize: 10,
              padding: "2px 6px",
              cursor: props.saving ? "not-allowed" : "pointer",
            }}
          >
            {props.saving ? "保存中" : "保存"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

/** 大图遮罩。 */
function PreviewOverlay(props: { item: ResultImage; url: string; onClose: () => void }): unknown {
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [props]);

  return (
    <div
      onClick={props.onClose}
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
