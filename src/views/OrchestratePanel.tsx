/**
 * 编排面板：内嵌 ComfyUI 原生的节点编辑器。
 *
 * 为什么内嵌而不是重写：编排是节点图交互（连线、控件、右键菜单、子图），重写必然失真。
 * 内嵌的代价是界面不受插件控制——读不到它的内容，需要数据时仍走它自己的接口。
 *
 * iframe 由 `orchestrateFrame` 常驻管理：切换面板或离线只隐藏不销毁，回来无需重载，
 * 编辑器里的未保存改动随面板切换保留。
 *
 * 服务未连接时不嵌：框架能加载但后端没起来时，用户会看到 ComfyUI 自己的错误页，
 * 不如在这里直说「未连接」并给出启动入口。
 */
import React from "react";
import type { ComfyRuntime } from "../runtime";
import type { HostController } from "../host/controller";
import {
  attachOrchestrateFrame,
  isOrchestrateFrameLoaded,
  reloadOrchestrateFrame,
  subscribeOrchestrateFrame,
} from "./orchestrateFrame";
import { Button, Empty, Panel, Toolbar, StatusDot, border, textMuted } from "./ui";

interface OrchestrateProps {
  runtime: ComfyRuntime;
  host: HostController;
}

export function OrchestratePanel(props: OrchestrateProps): unknown {
  const { runtime, host } = props;
  const snapshot = React.useSyncExternalStore(runtime.subscribe, runtime.getSnapshot);
  const hostSnapshot = React.useSyncExternalStore(host.subscribe, host.getSnapshot);
  const loaded = React.useSyncExternalStore(subscribeOrchestrateFrame, isOrchestrateFrameLoaded);
  const anchorRef = React.useRef<HTMLDivElement | null>(null);

  const target = runtime.baseUrlForUI;
  const offline = snapshot.channel === "offline";

  // 可见且在线时把常驻 iframe 挂到内容区；切换面板或离线时由清理逻辑隐藏
  React.useEffect(() => {
    if (offline || !anchorRef.current) return;
    const anchor = anchorRef.current;
    return attachOrchestrateFrame(anchor, target);
  }, [offline, target]);

  return (
    <Panel
      toolbar={
        <Toolbar>
          <StatusDot
            tone={offline ? "bad" : loaded ? "ok" : "idle"}
            label={offline ? "未连接" : loaded ? "已加载" : "加载中…"}
          />
          <span style={{ flex: 1 }} />
          <Button onClick={() => reloadOrchestrateFrame()} disabled={offline}>
            刷新
          </Button>
        </Toolbar>
      }
    >
      {offline ? (
        <Empty
          hint={
            hostSnapshot.running
              ? "服务已启动，正在等待就绪…"
              : "启动服务后即可编排；也可在设置里改为自行管理"
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
          {/* 空锚点：常驻 iframe 以固定定位覆盖这块区域 */}
          <div ref={anchorRef} style={{ flex: 1, minHeight: 0, position: "relative" }} />
          <div
            style={{
              padding: "4px 10px",
              borderTop: `1px solid ${border}`,
              fontSize: 11,
              color: textMuted,
              flexShrink: 0,
            }}
          >
            界面由 ComfyUI 提供；在此保存的工作流（工作流 → 保存）会自动出现在生成面板的列表里
          </div>
        </>
      )}
    </Panel>
  );
}
