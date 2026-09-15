/**
 * 插件入口：装配面板、设置页与命令。
 *
 * 运行时与状态中心在 `apply` 期间建立一次，界面消费其快照，因此切面板不会断连接；
 * 停用时注册项与订阅随上下文一起撤销。
 *
 * 面板切换用组件内的局部状态：同一视图可能被放进多个面板甚至独立窗口，全局状态会互相干扰。
 */
import React from "react";
import type { AtelyxCtx } from "./ctx";
import { loadSettings, DEFAULT_SETTINGS, type ComfySettings } from "./settings";
import { ComfyRuntime } from "./runtime";
import { HostController } from "./host/controller";
import { GeneratePanel } from "./views/GeneratePanel";
import { OrchestratePanel } from "./views/OrchestratePanel";
import { SettingsView } from "./views/Settings";
import { bgSecondary, border, FONT_SM, textMuted } from "./views/ui";

/** 与清单里的 name 一致。 */
const PLUGIN_ID = "com.atelyx.comfyui-panel";

const VIEW_KIND = PLUGIN_ID;
const SETTING_KEY = `${PLUGIN_ID}.settings`;

/** 三者就绪后才可渲染。 */
interface PanelDeps {
  ctx: AtelyxCtx;
  runtime: ComfyRuntime;
  host: HostController;
  onSettingsChanged(next: ComfySettings): void;
}

export default function apply(pluginCtx: AtelyxCtx): void {

  // 设置要先读出来才能建运行时（地址与端口决定传输层）。读取是异步的，
  // 故设一个就绪后通知订阅者的小状态：未就绪时显示初始化中，而不是闪空白。
  let deps: PanelDeps | null = null;
  const listeners = new Set<() => void>();
  const subscribe = (listener: () => void): (() => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  const getDeps = (): PanelDeps | null => deps;
  const notify = (): void => {
    for (const listener of listeners) listener();
  };

  let settings: ComfySettings = { ...DEFAULT_SETTINGS };
  const onSettingsChanged = (next: ComfySettings): void => {
    settings = next;
    if (deps) {
      deps.runtime.applySettings(next);
      deps.host.applySettings(next);
    }
  };

  void loadSettings(pluginCtx)
    .then((loaded) => {
      settings = loaded;
      const runtime = new ComfyRuntime(loaded);
      deps = {
        ctx: pluginCtx,
        runtime,
        host: new HostController(pluginCtx, loaded),
        onSettingsChanged,
      };
      notify();
      // 连接由插件持有而非面板持有：两个面板共用同一个运行时，若把断开挂在某个面板的
      // 清理里，切到另一个面板就会把连接掐断（表现为「明明连上了却显示未连接」）。
      void runtime.connect();
    })
    .catch((err: unknown) => {
      pluginCtx.notification.notify({
        level: "warning",
        message: `设置读取失败，已用默认值启动：${err instanceof Error ? err.message : String(err)}`,
      });
    });

/** 内部分「生成」与「编排」两个标签。 */
  function ComfyPanel(props: { initial: "generate" | "orchestrate" }): unknown {
    const ready = React.useSyncExternalStore(subscribe, getDeps);
    const [mode, setMode] = React.useState<"generate" | "orchestrate">(props.initial);

    if (!ready) {
      return (
        <div style={{ height: "100%", display: "flex", alignItems: "center", justifyContent: "center", color: textMuted, fontSize: FONT_SM }}>
          正在初始化…
        </div>
      );
    }

    return (
      <div
        style={{
          height: "100%",
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          border: `1px solid ${border}`,
          boxSizing: "border-box",
          background: bgSecondary,
        }}
      >
        {mode === "generate" ? (
          <GeneratePanel
            ctx={ready.ctx}
            runtime={ready.runtime}
            host={ready.host}
            onOpenOrchestrate={() => setMode("orchestrate")}
          />
        ) : (
          <OrchestratePanel
            runtime={ready.runtime}
            host={ready.host}
            onOpenGenerate={() => setMode("generate")}
          />
        )}
      </div>
    );
  }

  function ComfySettings(): unknown {
    const ready = React.useSyncExternalStore(subscribe, getDeps);
    if (!ready) {
      return <div style={{ padding: 16, color: textMuted, fontSize: FONT_SM }}>正在初始化…</div>;
    }
    return (
      <SettingsView
        ctx={ready.ctx}
        runtime={ready.runtime}
        host={ready.host}
        onSettingsChanged={ready.onSettingsChanged}
      />
    );
  }

  pluginCtx.effect(() => {
    // 关窗兜底：应用退出时宿主不结束插件进程（它只保证停用/卸载时收尾），而子进程不会随
    // 父进程消失，留下的是占着端口与显存的孤儿。宿主侧若改为进程随应用退出（如 Windows
    // Job Object）即可移除本段——那时它是多余的。
    const onHide = (): void => deps?.host.killOnShutdown();
    window.addEventListener("pagehide", onHide);
    window.addEventListener("beforeunload", onHide);

    const offPanel = pluginCtx.slots.registerView({
      kind: VIEW_KIND,
      label: "ComfyUI",
      component: () => <ComfyPanel initial="generate" />,
    });
    const offOrchestrate = pluginCtx.slots.registerView({
      kind: `${VIEW_KIND}.orchestrate`,
      label: "ComfyUI 编排",
      component: () => <ComfyPanel initial="orchestrate" />,
    });
    const offSetting = pluginCtx.slots.registerSetting({
      key: SETTING_KEY,
      label: "ComfyUI",
      component: ComfySettings,
    });
    return () => {
      window.removeEventListener("pagehide", onHide);
      window.removeEventListener("beforeunload", onHide);
      offPanel();
      offOrchestrate();
      offSetting();
      deps?.runtime.disconnect();
    };
  });

  pluginCtx.effect(() => {
    const offConnect = pluginCtx.slots.registerCommand({
      id: `${PLUGIN_ID}.connect`,
      label: "ComfyUI：连接 / 刷新",
      run: () => void deps?.runtime.connect(),
    });
    const offStart = pluginCtx.slots.registerCommand({
      id: `${PLUGIN_ID}.start`,
      label: "ComfyUI：启动服务",
      run: () => {
        if (deps && settings?.processMode === "managed") void deps.host.start(deps.runtime);
      },
    });
    const offStop = pluginCtx.slots.registerCommand({
      id: `${PLUGIN_ID}.stop`,
      label: "ComfyUI：停止服务",
      run: () => void deps?.host.stop(),
    });
    const offInterrupt = pluginCtx.slots.registerCommand({
      id: `${PLUGIN_ID}.interrupt`,
      label: "ComfyUI：中断当前任务",
      run: () => void deps?.runtime.interrupt(),
    });
    return () => {
      offConnect();
      offStart();
      offStop();
      offInterrupt();
    };
  });
}
