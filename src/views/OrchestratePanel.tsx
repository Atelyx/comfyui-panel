/**
 * 编排面板：内嵌 ComfyUI 原生的节点编辑器。
 *
 * 为什么内嵌而不是重写：编排是节点图交互（连线、控件、右键菜单、子图），重写必然失真。
 * 内嵌的代价是界面不受插件控制——读不到它的内容，需要数据时仍走它自己的接口。
 *
 * 服务未连接时不嵌：框架能加载但后端没起来时，用户会看到 ComfyUI 自己的错误页，
 * 不如在这里直说「未连接」并给出启动入口。
 */
import React from "react";
import type { ComfyRuntime } from "../runtime";
import type { HostController } from "../host/controller";
import { Button, Empty, Panel, Toolbar, StatusDot, border, bgSecondary, textMuted } from "./ui";

interface OrchestrateProps {
  runtime: ComfyRuntime;
  host: HostController;
  onOpenGenerate(): void;
}

export function OrchestratePanel(props: OrchestrateProps): unknown {
  const { runtime, host, onOpenGenerate } = props;
  const snapshot = React.useSyncExternalStore(runtime.subscribe, runtime.getSnapshot);
  const hostSnapshot = React.useSyncExternalStore(host.subscribe, host.getSnapshot);
  const [reloadKey, setReloadKey] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);

  const target = runtime.baseUrlForUI;
  const offline = snapshot.channel === "offline";

  return (
    <Panel
      toolbar={
        <Toolbar>
          <StatusDot
            tone={offline ? "bad" : loaded ? "ok" : "idle"}
            label={offline ? "未连接" : loaded ? "已加载" : "加载中…"}
          />
          <span style={{ flex: 1 }} />
          <Button onClick={() => { setLoaded(false); setReloadKey((key) => key + 1); }} disabled={offline}>
            刷新
          </Button>
          <Button onClick={() => void host.openExternal(target)}>在浏览器打开</Button>
          <Button onClick={onOpenGenerate}>返回生成</Button>
        </Toolbar>
      }
    >
      {offline ? (
        <Empty
          hint={
            hostSnapshot.running
              ? "服务已启动，正在等待就绪…"
              : "启动 ComfyUI 服务后即可在此编排；也可在设置里改为自行管理进程"
          }
        >
          <span style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 10 }}>
            <span>未连接到 ComfyUI</span>
            {hostSnapshot.running || snapshot.probing ? null : (
              <Button tone="primary" onClick={() => void host.start(runtime)} disabled={hostSnapshot.starting}>
                {hostSnapshot.starting ? "启动中…" : "启动 ComfyUI"}
              </Button>
            )}
          </span>
        </Empty>
      ) : (
        <>
          <iframe
            key={reloadKey}
            src={target}
            title="ComfyUI 编排界面"
            onLoad={() => setLoaded(true)}
            style={{ flex: 1, minHeight: 0, width: "100%", border: "none", background: bgSecondary }}
          />
          <div
            style={{
              padding: "4px 10px",
              borderTop: `1px solid ${border}`,
              fontSize: 11,
              color: textMuted,
              flexShrink: 0,
            }}
          >
            此界面由 ComfyUI 自己提供，插件读不到它的内容；导入工作流请在 ComfyUI 内用「导出（API）」。
          </div>
        </>
      )}
    </Panel>
  );
}
