/**
 * 设置页：连接、进程托管、结果落库与工作流库管理。
 *
 * 逐项改动即落盘（不设保存按钮）：改动都由用户明确操作触发，改完不丢比批量提交更符合直觉。
 */
import React from "react";
import type { AtelyxCtx } from "../ctx";
import { DEFAULT_SETTINGS, saveSettings, type ComfySettings, type PreviewMethod, type ProcessMode } from "../settings";
import type { ComfyRuntime } from "../runtime";
import type { HostController } from "../host/controller";
import {
  listWorkflows,
  deleteWorkflow,
  renameWorkflow,
  type WorkflowSummary,
} from "../workflow/library";
import { Button, Card, Checkbox, ConfirmButton, Field, Notice, Select, TextArea, TextInput, textMuted, textPrimary, FONT_SM } from "./ui";

interface SettingsProps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  host: HostController;
  /** 回写并重建运行时。 */
  onSettingsChanged(settings: ComfySettings): void;
}

export function SettingsView(props: SettingsProps): unknown {
  const { ctx, runtime, host, onSettingsChanged } = props;
  const snapshot = React.useSyncExternalStore(runtime.subscribe, runtime.getSnapshot);
  const hostSnapshot = React.useSyncExternalStore(host.subscribe, host.getSnapshot);
  const settings = snapshot.settings;
  const [workflows, setWorkflows] = React.useState<WorkflowSummary[]>([]);
  const [renameId, setRenameId] = React.useState<string | null>(null);
  const [renameText, setRenameText] = React.useState("");
  const [dialogError, setDialogError] = React.useState("");

  const refreshWorkflows = React.useCallback(() => {
    void listWorkflows(ctx).then(setWorkflows);
  }, [ctx]);

  React.useEffect(() => {
    refreshWorkflows();
  }, [refreshWorkflows]);

  /** 改一项设置：本地立即生效（UI 无延迟），随后落盘。 */
  const patch = React.useCallback(
    (changes: Partial<ComfySettings>) => {
      const next: ComfySettings = { ...settings, ...changes };
      onSettingsChanged(next);
      void saveSettings(ctx, next);
    },
    [ctx, settings, onSettingsChanged],
  );

  const pickDirectory = React.useCallback(async () => {
    setDialogError("");
    try {
      const dir = await ctx.dialog.pickDirectory();
      if (dir) patch({ comfyDir: dir });
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : String(err));
    }
  }, [ctx, patch]);

  const pickPython = React.useCallback(async () => {
    setDialogError("");
    try {
      // 不设扩展名过滤：Python 可能是 python.exe、conda 环境里的可执行文件或 Unix 下的无扩展名二进制；
      // 过滤器要求每项为非空字符串，塞空串会被宿主判为非法参数而直接抛错（对话框根本不会弹）
      const file = await ctx.dialog.pickFile();
      if (file) patch({ pythonPath: file });
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : String(err));
    }
  }, [ctx, patch]);

  return (
    <div
      style={{
        padding: 16,
        maxHeight: "100%",
        overflowY: "auto",
        color: textPrimary,
        fontSize: FONT_SM,
        boxSizing: "border-box",
      }}
    >
      {snapshot.channelReason ? (
        <div style={{ marginBottom: 12 }}>
          <Notice tone={snapshot.channel === "offline" ? "error" : "warn"}>{snapshot.channelReason}</Notice>
        </div>
      ) : null}

      <Card
        title="连接"
        actions={
          <Button onClick={() => void runtime.connect()} disabled={snapshot.probing}>
            {snapshot.probing ? "连接中…" : "重新连接"}
          </Button>
        }
      >
        <Field label="地址" hint="ComfyUI 所在主机；本机即 127.0.0.1。">
          <TextInput value={settings.host} onChange={(value) => patch({ host: value })} placeholder="127.0.0.1" />
        </Field>
        <Field label="端口" hint="与启动参数共用同一端口（托管启动时会按此端口传参）。">
          <TextInput
            type="number"
            value={String(settings.port)}
            onChange={(value) => {
              const port = Number(value);
              // 只在合法区间落盘，避免中间态（如输入「81」时的过渡值）被写进配置把连接带偏
              if (Number.isInteger(port) && port > 0 && port < 65536) patch({ port });
            }}
          />
        </Field>
        <div style={{ display: "flex", gap: 12, alignItems: "center", fontSize: FONT_SM, color: textMuted }}>
          <span>当前状态：{snapshot.channel === "direct" ? "已连接" : "未连接"}</span>
        </div>
      </Card>

      <Card title="进程">
        <Field
          label="接管方式"
          hint="托管：插件负责启动与停止；外部：进程由你自行管理，插件只检测连接。"
        >
          <Select
            value={settings.processMode}
            onChange={(value) => patch({ processMode: value as ProcessMode })}
            options={[
              { value: "managed", label: "托管：插件启动与停止 ComfyUI" },
              { value: "external", label: "外部：只检测，不启停" },
            ]}
          />
        </Field>
        <Field label="ComfyUI 目录" hint="托管启动的工作目录。">
          <div style={{ display: "flex", gap: 6 }}>
            <TextInput value={settings.comfyDir} onChange={(value) => patch({ comfyDir: value })} placeholder="选择 ComfyUI 安装目录" />
            <Button onClick={() => void pickDirectory()}>选择</Button>
          </div>
        </Field>
        <Field label="Python 可执行文件" hint="留空则用「ComfyUI 目录/venv/Scripts/python.exe」。">
          <div style={{ display: "flex", gap: 6 }}>
            <TextInput value={settings.pythonPath} onChange={(value) => patch({ pythonPath: value })} placeholder="留空使用 venv 内的 python" />
            <Button onClick={() => void pickPython()}>选择</Button>
          </div>
        </Field>
        {dialogError ? (
          <div style={{ marginBottom: 12 }}>
            <Notice tone="error" onClose={() => setDialogError("")}>
              {dialogError}
            </Notice>
          </div>
        ) : null}
        <Field label="附加启动参数" hint="原样追加在启动命令末尾；端口与跨域放行由插件注入。">
          <TextInput value={settings.extraArgs} onChange={(value) => patch({ extraArgs: value })} placeholder="例如 --lowvram" />
        </Field>
        <Field
          label="跨域放行"
          hint="开启后托管启动会带 --enable-cors-header。这是插件与 ComfyUI 通信的前提，关掉后插件将无法连接。"
        >
          <Checkbox checked={settings.autoCors} onChange={(checked) => patch({ autoCors: checked })} label="启动时自动开启跨域放行" />
        </Field>
        <Field
          label="生成中预览"
          hint="ComfyUI 默认不发送采样预览。选一种方式后，托管启动会带上对应参数，生成过程中就能看到画面；选「不预览」则不注入参数（外部启动时需自己加 --preview-method）。"
        >
          <Select
            value={settings.previewMethod}
            onChange={(value) => patch({ previewMethod: value as PreviewMethod })}
            options={[
              { value: "latent2rgb", label: "latent2rgb（最快，推荐）" },
              { value: "taesd", label: "taesd（更清晰，需额外的 VAE 近似模型）" },
              { value: "auto", label: "auto（由 ComfyUI 决定）" },
              { value: "none", label: "不预览" },
            ]}
          />
        </Field>
        {settings.processMode === "managed" ? (
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Button
              tone="primary"
              onClick={() => void host.start(runtime)}
              disabled={hostSnapshot.starting || hostSnapshot.running}
            >
              {hostSnapshot.starting ? "启动中…" : "启动 ComfyUI"}
            </Button>
            <Button tone="danger" onClick={() => void host.stop()} disabled={!hostSnapshot.running}>
              停止 ComfyUI
            </Button>
            <span style={{ fontSize: FONT_SM, color: textMuted }}>
              {hostSnapshot.running ? "插件已启动该进程" : "当前进程不由插件管理"}
            </span>
          </div>
        ) : (
          <div style={{ fontSize: FONT_SM, color: textMuted }}>
            外部模式下插件不启停进程；启动时请自行带上 --enable-cors-header。
          </div>
        )}
      </Card>

      <Card title="结果落库">
        <Field label="落库目录" hint="图片保存进仓库时的子目录；留空则用仓库设置里的附件目录。">
          <TextInput
            value={settings.archiveFolder}
            onChange={(value) => patch({ archiveFolder: value })}
            placeholder="留空使用仓库附件目录"
          />
        </Field>
        <Field label="追加入笔记" hint="保存结果后，把图片以 Markdown 形式追加到当前打开的笔记末尾。">
          <Checkbox
            checked={settings.appendToNote}
            onChange={(checked) => patch({ appendToNote: checked })}
            label="保存结果后追加到当前笔记"
          />
        </Field>
      </Card>

      <Card
        title={`工作流库（${workflows.length}）`}
        actions={
          <div style={{ display: "flex", gap: 6 }}>
            <Button onClick={refreshWorkflows}>刷新</Button>
            <ConfirmButton
              label="全部删除"
              confirmLabel={`确认删除全部 ${workflows.length} 个`}
              disabled={workflows.length === 0}
              onConfirm={() =>
                void (async () => {
                  // 逐项删除以复用「正文与索引一并清理」的既有路径
                  for (const item of workflows) await deleteWorkflow(ctx, item.id);
                  refreshWorkflows();
                })()
              }
            />
          </div>
        }
      >
        {workflows.length === 0 ? (
          <div style={{ color: textMuted, lineHeight: 1.6 }}>
            还没有工作流。在生成面板里点「导入工作流」，粘贴在 ComfyUI 中用「工作流 → 导出（API）」得到的 JSON。
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {workflows.map((item) => (
              <div
                key={item.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "6px 8px",
                  borderRadius: 6,
                  border: `1px solid var(--border)`,
                }}
              >
                {renameId === item.id ? (
                  <>
                    <TextInput value={renameText} onChange={setRenameText} />
                    <Button
                      onClick={() => {
                        void renameWorkflow(ctx, item.id, renameText).then(() => {
                          setRenameId(null);
                          refreshWorkflows();
                        });
                      }}
                    >
                      确定
                    </Button>
                    <Button onClick={() => setRenameId(null)}>取消</Button>
                  </>
                ) : (
                  <>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        {item.name}
                      </div>
                      <div style={{ fontSize: 11, color: textMuted }}>
                        {item.nodeCount} 个节点 · {new Date(item.importedAt).toLocaleString()}
                      </div>
                    </div>
                    <Button
                      onClick={() => {
                        setRenameId(item.id);
                        setRenameText(item.name);
                      }}
                    >
                      重命名
                    </Button>
                    <ConfirmButton
                      label="删除"
                      confirmLabel="确认删除"
                      onConfirm={() => void deleteWorkflow(ctx, item.id).then(refreshWorkflows)}
                    />
                  </>
                )}
              </div>
            ))}
          </div>
        )}
      </Card>

      <div style={{ color: textMuted, fontSize: 11, lineHeight: 1.6 }}>
        默认值（恢复出厂设置时参考）：地址 {DEFAULT_SETTINGS.host}、端口 {DEFAULT_SETTINGS.port}。
      </div>
    </div>
  );
}

/** 面板与设置页共用同一份日志。 */
export function HostLog(props: { host: HostController; rows?: number }): unknown {
  const snapshot = React.useSyncExternalStore(props.host.subscribe, props.host.getSnapshot);
  const rows = props.rows ?? 8;
  if (snapshot.logs.length === 0) return null;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <span style={{ fontSize: 11, color: textMuted }}>运行日志</span>
        <Button onClick={() => props.host.clearLogs()}>清空</Button>
      </div>
      <TextArea
        value={snapshot.logs.slice(-rows).join("\n")}
        onChange={() => undefined}
        rows={rows}
        mono
      />
    </div>
  );
}
