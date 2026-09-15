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
  readField,
  writeField,
  type FieldSpec,
} from "../workflow/form";
import {
  Button,
  Checkbox,
  Empty,
  Notice,
  Panel,
  ProgressBar,
  SectionTitle,
  Select,
  StatusDot,
  TextArea,
  TextInput,
  Toolbar,
  bgSecondary,
  border,
  danger,
  textMuted,
  textSecondary,
  hover,
  FONT_SM,
} from "./ui";

interface GeneratePanelProps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  host: HostController;
  /** 由入口传入的切换回调。 */
  onOpenOrchestrate(): void;
}

export function GeneratePanel(props: GeneratePanelProps): unknown {
  const { ctx, runtime, host, onOpenOrchestrate } = props;
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

  /** 按节点分组，便于用户对上 ComfyUI 里的节点。 */
  const groups = React.useMemo(() => {
    const map = new Map<string, FieldSpec[]>();
    for (const field of visibleFields) {
      const list = map.get(field.nodeId) ?? [];
      list.push(field);
      map.set(field.nodeId, list);
    }
    return Array.from(map.entries());
  }, [visibleFields]);

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
            刷新连接
          </Button>
          {snapshot.settings.processMode === "managed" ? (
            hostSnapshot.running ? (
              <Button tone="danger" onClick={() => void host.stop()}>
                停止服务
              </Button>
            ) : (
              <Button onClick={() => void host.start(runtime)} disabled={hostSnapshot.starting}>
                {hostSnapshot.starting ? "启动中…" : "启动服务"}
              </Button>
            )
          ) : null}
          <Button onClick={onOpenOrchestrate}>编排界面</Button>
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
        <div style={{ padding: "8px 10px 0", display: "flex", flexDirection: "column", gap: 6 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span style={{ fontSize: FONT_SM, color: textSecondary }}>
              运行中 {snapshot.queue.queue_running.length} · 等待 {snapshot.queue.queue_pending.length}
            </span>
            <span style={{ flex: 1 }} />
            <Button tone="danger" onClick={() => void runtime.interrupt()}>
              中断
            </Button>
          </div>
          {snapshot.progress ? (
            <ProgressBar
              value={progressValue}
              label={`${Math.round(progressValue * 100)}%${
                snapshot.progress.node ? ` · 节点 ${snapshot.progress.node}` : ""
              }`}
            />
          ) : null}
          {snapshot.preview ? (
            <img
              src={snapshot.preview}
              alt="生成预览"
              style={{ maxHeight: 200, maxWidth: "100%", objectFit: "contain", borderRadius: 6, alignSelf: "flex-start" }}
            />
          ) : snapshot.progress ? (
            // 有进度却没画面：ComfyUI 默认不发采样预览，说明清楚比让用户对着空白猜好
            <div style={{ fontSize: 11, color: textMuted }}>
              正在生成（未开启采样预览：ComfyUI 需带 --preview-method 启动，可在设置里配置）
            </div>
          ) : null}
        </div>
      ) : null}

      {/* 工作流与参数 */}
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "8px 10px" }}>
        <div style={{ display: "flex", gap: 6, alignItems: "center", marginBottom: 8 }}>
          <Select
            value={activeId}
            onChange={setActiveId}
            disabled={workflows.length === 0}
            options={
              workflows.length === 0
                ? [{ value: "", label: "尚无工作流" }]
                : workflows.map((item) => ({ value: item.id, label: item.name }))
            }
          />
          <Button onClick={() => setImportOpen((open) => !open)}>{importOpen ? "取消导入" : "导入工作流"}</Button>
        </div>

        {importOpen ? (
          <div
            style={{
              border: `1px solid ${border}`,
              borderRadius: 8,
              padding: 10,
              marginBottom: 10,
              background: bgSecondary,
            }}
          >
            <div style={{ fontSize: 11, color: textMuted, lineHeight: 1.6, marginBottom: 6 }}>
              在 ComfyUI 里打开工作流，用「工作流 → 导出（API）」得到 JSON 后粘贴到下面。
              提交的正是这份图，参数表单也由它生成。
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
              <Button onClick={() => fileInputRef.current?.click()}>从文件读取</Button>
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
                if (!file) return;
                setImportError("");
                void file
                  .text()
                  .then((text) => {
                    // 先填进文本框让内容可见可改，仍由用户点导入确认
                    setImportText(text);
                  })
                  .catch((err: unknown) =>
                    setImportError(`读取文件失败：${err instanceof Error ? err.message : String(err)}`),
                  );
              }}
            />
          </div>
        ) : null}

        {!draft ? (
          <Empty hint="导入一个 API 格式工作流后即可调参生成">
            {workflows.length === 0 ? "还没有工作流" : "工作流读取失败，请重新导入"}
          </Empty>
        ) : (
          <>
            {countDisabledNodes(draft) > 0 ? (
              <div style={{ marginBottom: 8 }}>
                <Notice tone="warn">
                  工作流里有被禁用/跳过的节点。API 导出通常已剔除，若生成结果异常请回到 ComfyUI 确认。
                </Notice>
              </div>
            ) : null}

            <div style={{ display: "flex", gap: 6, alignItems: "center", marginBottom: 8 }}>
              <TextInput value={filter} onChange={setFilter} placeholder="筛选参数（如 prompt / seed / 尺寸）" />
              <Button onClick={() => active && setDraft(structuredClone(active.prompt))}>重置</Button>
            </div>

            {groups.length === 0 ? (
              <Empty hint="换个关键词，或点「重置」恢复全部参数">没有匹配的参数</Empty>
            ) : (
              groups.map(([nodeId, nodeFields]) => (
                <div key={nodeId} style={{ marginBottom: 10 }}>
                  <SectionTitle>
                    {nodeFields[0]?.nodeLabel ?? nodeId} · {nodeFields[0]?.classType ?? ""}
                  </SectionTitle>
                  <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
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
                  </div>
                </div>
              ))
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
          {busy ? "提交中…" : "生成"}
        </Button>
        <Button onClick={() => void runtime.freeMemory()} disabled={snapshot.channel === "offline"}>
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
          复制 JSON
        </Button>
        <span style={{ fontSize: 11, color: textMuted }}>{snapshot.results.length} 个结果</span>
      </div>

      {/* 结果画廊 */}
      <div style={{ borderTop: `1px solid ${border}`, background: bgSecondary }}>
        <SectionTitle>结果</SectionTitle>
        {snapshot.results.length === 0 ? (
          <div style={{ padding: "4px 10px 10px", fontSize: 11, color: textMuted }}>
            生成完成后，结果会出现在这里。
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
        {field.input}
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
        <span style={{ fontSize: 11, color: textMuted }} title="该输入由工作流中的连线提供">
          来源：{field.linkFrom}
        </span>
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
          <span style={{ flex: 1, minWidth: 0, fontSize: 10, color: "#fff", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={item.title}>
            {item.savedPath ? `已保存：${item.savedPath}` : item.title}
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
