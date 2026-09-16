/**
 * 设置页：连接、进程托管、结果落库与工作流库管理。
 *
 * 逐项改动即落盘（不设保存按钮）：改动都由用户明确操作触发，改完不丢比批量提交更符合直觉。
 */
import React from "react";
import type { AtelyxCtx } from "../ctx";
import { DEFAULT_SETTINGS, saveSettings, type ComfySettings, type ProcessMode } from "../settings";
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
      <Card
        title="连接"
        actions={
          <Button onClick={() => void runtime.connect()} disabled={snapshot.probing}>
            {snapshot.probing ? "连接中…" : "重新连接"}
          </Button>
        }
      >
        <Field label="地址" hint="本机用 127.0.0.1">
          <TextInput value={settings.host} onChange={(value) => patch({ host: value })} placeholder="127.0.0.1" />
        </Field>
        <Field label="端口" hint="托管启动时按此端口传参">
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
        <Field label="接管方式" hint="托管：插件启停进程；外部：只检测连接">
          <Select
            value={settings.processMode}
            onChange={(value) => patch({ processMode: value as ProcessMode })}
            options={[
              { value: "managed", label: "托管（插件启停）" },
              { value: "external", label: "外部（仅检测）" },
            ]}
          />
        </Field>
        <Field label="ComfyUI 目录" hint="托管启动的工作目录。">
          <div style={{ display: "flex", gap: 6 }}>
            <TextInput value={settings.comfyDir} onChange={(value) => patch({ comfyDir: value })} placeholder="选择 ComfyUI 安装目录" />
            <Button onClick={() => void pickDirectory()}>选择</Button>
          </div>
        </Field>
        <Field label="Python 可执行文件" hint="留空用 venv 内 python">
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
        <Field label="附加启动参数" hint="追加在启动命令末尾；端口与跨域由插件注入">
          <TextInput value={settings.extraArgs} onChange={(value) => patch({ extraArgs: value })} placeholder="例如 --lowvram" />
        </Field>
        <Field label="跨域放行" hint="插件与 ComfyUI 通信的前提；关闭后将无法连接">
          <Checkbox checked={settings.autoCors} onChange={(checked) => patch({ autoCors: checked })} label="启动时自动开启跨域放行" />
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
            外部模式：自行启动并带 --enable-cors-header
          </div>
        )}
      </Card>

      <Card title="结果落库">
        <Field label="落库目录" hint="保存到仓库的子目录；留空用附件目录">
          <TextInput
            value={settings.archiveFolder}
            onChange={(value) => patch({ archiveFolder: value })}
            placeholder="留空使用仓库附件目录"
          />
        </Field>
        <Field label="追加入笔记" hint="保存后以 Markdown 追加到当前笔记末尾">
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
            还没有工作流；到生成面板导入「导出（API）」的 JSON
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
        默认：{DEFAULT_SETTINGS.host}:{DEFAULT_SETTINGS.port}
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
