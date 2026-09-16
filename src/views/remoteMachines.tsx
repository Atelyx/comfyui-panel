/**
 * 远程启停入口：工具栏上的按钮 + 在线机器列表。
 *
 * 列表就是同房间的成员，能不能远程启停看对方回执（未回执 = 对端没装或没启用本插件）。
 * 只在弹层打开期间刷新，关掉即停——别的成员启停服务不是高频事件，没必要常驻轮询。
 *
 * 启停后本端的连接地址不会自动切到对方：地址由用户在设置里填（宿主不提供对端 IP），
 * 这里只把对方的端口与标识摆出来，省得来回问。
 */
import React from "react";
import type { RemoteControl, RemoteMachine } from "../host/remote";
import { Button, Notice, StatusDot, bgSecondary, border, textMuted, textPrimary, FONT_SM } from "./ui";

/** 弹层打开期间的状态刷新间隔；对方启停后几秒内列表跟上即可。 */
const REFRESH_MS = 10000;

export function RemoteEntry(props: { remote: RemoteControl }): unknown {
  const { remote } = props;
  const snapshot = React.useSyncExternalStore(remote.subscribe, remote.getSnapshot);
  const [open, setOpen] = React.useState(false);
  const rootRef = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    if (!open) return;
    void remote.scan();
    const timer = setInterval(() => void remote.scan(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [open, remote]);

  // 点弹层外面收起
  React.useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent): void => {
      const root = rootRef.current;
      if (root && event.target instanceof Node && root.contains(event.target)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const remoteCount = snapshot.machines.filter((machine) => machine.replied && machine.remoteEnabled).length;

  return (
    <div ref={rootRef} style={{ position: "relative" }}>
      <Button onClick={() => setOpen((current) => !current)} active={open} title="经协作通道启停同房间其他机器上的 ComfyUI">
        远程
        {remoteCount > 0 ? ` ${remoteCount}` : ""}
      </Button>
      {open ? (
        <div
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 4px)",
            zIndex: 50,
            width: 340,
            padding: 10,
            display: "flex",
            flexDirection: "column",
            gap: 8,
            borderRadius: 8,
            border: `1px solid ${border}`,
            background: bgSecondary,
            boxShadow: "0 10px 32px rgba(0,0,0,0.22)",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <StatusDot
              tone={snapshot.scanning ? "idle" : snapshot.error ? "bad" : "ok"}
              label={snapshot.scanning ? "查询中…" : snapshot.error ? "通道不可用" : "协作通道"}
            />
            <span style={{ flex: 1 }} />
            <Button onClick={() => void remote.scan()} disabled={snapshot.scanning}>
              刷新
            </Button>
          </div>

          {snapshot.error ? <Notice tone="warn">{snapshot.error}</Notice> : null}

          {snapshot.machines.length === 0 ? (
            <div style={{ fontSize: 11, color: textMuted, lineHeight: 1.6 }}>
              同房间里没有其他成员。远程启停要求两台机器都启用协作、连同一个中转，并打开同一个仓库。
            </div>
          ) : (
            snapshot.machines.map((machine) => <MachineRow key={machine.peerId} machine={machine} remote={remote} />)
          )}
        </div>
      ) : null}
    </div>
  );
}

function MachineRow(props: { machine: RemoteMachine; remote: RemoteControl }): unknown {
  const { machine, remote } = props;
  const task = machine.task;
  const busy = task !== null && (task.stage === "sending" || task.stage === "working");
  const canStart = machine.replied && !busy && machine.phase === "offline" && machine.denyReason === "";
  const canStop = machine.replied && !busy && machine.phase === "ready";

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, paddingTop: 6, borderTop: `1px solid ${border}` }}>
      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: 4,
            flexShrink: 0,
            background: machine.color || textMuted,
          }}
        />
        <span style={{ fontSize: FONT_SM, color: textPrimary, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {machine.nickname}
        </span>
        <span style={{ flex: 1 }} />
        <StatusDot tone={phaseTone(machine)} label={phaseLabel(machine)} />
      </div>

      <div style={{ fontSize: 11, color: textMuted, lineHeight: 1.5 }}>{describeMachine(machine)}</div>

      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <Button
          onClick={() => void remote.startRemote(machine)}
          disabled={!canStart}
          tone="primary"
          title={startHint(machine)}
        >
          {busy && task?.kind === "start" ? "启动中…" : "启动"}
        </Button>
        <Button
          onClick={() => void remote.stopRemote(machine)}
          disabled={!canStop}
          tone="danger"
          title={stopHint(machine)}
        >
          {busy && task?.kind === "stop" ? "停止中…" : "停止"}
        </Button>
        {machine.port > 0 ? (
          <span style={{ fontSize: 11, color: textMuted }}>端口 {machine.port}</span>
        ) : null}
      </div>

      {task ? (
        <div style={{ fontSize: 11, color: task.stage === "failed" ? "#e5534b" : textMuted, lineHeight: 1.5 }}>
          {task.message}
        </div>
      ) : null}
    </div>
  );
}

function phaseLabel(machine: RemoteMachine): string {
  if (!machine.replied) return "未响应";
  if (machine.phase === "ready") return "已就绪";
  if (machine.phase === "starting") return "启动中";
  if (machine.phase === "external") return "外部运行中";
  return "未运行";
}

function phaseTone(machine: RemoteMachine): "ok" | "warn" | "bad" | "idle" {
  if (!machine.replied) return "idle";
  if (machine.phase === "ready") return "ok";
  if (machine.phase === "starting" || machine.phase === "external") return "warn";
  return "idle";
}

/** 对方的状态一行说清：设备名 + 标识 + 为什么不能启动。 */
function describeMachine(machine: RemoteMachine): string {
  if (!machine.replied) return `${machine.deviceName || "未知设备"}：未响应，需在对端启用本插件与协作`;
  const parts = [machine.deviceName || "未知设备"];
  if (machine.tag) parts.push(`标识 ${machine.tag}`);
  if (machine.phase === "ready") parts.push("连接地址用它的局域网地址与本端口");
  else if (machine.phase === "external") parts.push("进程由对端自行启动，无法远程停止");
  // 已在运行或正在启动时，开关状态与能不能启动无关，说了只会跟状态标签打架
  else if (machine.denyReason) parts.push(`不可远程启动：${machine.denyReason}`);
  return parts.join(" · ");
}

function startHint(machine: RemoteMachine): string {
  if (!machine.replied) return "对端未响应，无法发送命令";
  if (machine.denyReason) return machine.denyReason;
  if (machine.phase !== "offline") return "对端已在运行或正在启动";
  return "在对端托管启动 ComfyUI，并开放局域网监听";
}

function stopHint(machine: RemoteMachine): string {
  if (!machine.replied) return "对端未响应，无法发送命令";
  if (machine.phase === "external") return "对端进程不是插件托管的，无法远程停止";
  if (machine.phase !== "ready") return "对端当前没有在运行的托管进程";
  return "停止对端由插件托管的 ComfyUI 进程";
}
