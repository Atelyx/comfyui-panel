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
import { RemoteControl } from "./host/remote";
import { GeneratePanel } from "./views/GeneratePanel";
import { resetGenerateSession } from "./views/generateSession";
import { OrchestratePanel } from "./views/OrchestratePanel";
import { disposeOrchestrateFrame } from "./views/orchestrateFrame";
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
  remote: RemoteControl;
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
      deps.remote.applySettings(next);
    }
  };

  // 协作入站订阅的撤销函数：远程命令会真的拉起进程，插件停用后必须不再受理
  let detachRemote: (() => void) | null = null;

  void loadSettings(pluginCtx)
    .then((loaded) => {
      settings = loaded;
      const runtime = new ComfyRuntime(loaded);
      const host = new HostController(pluginCtx, loaded);
      const remote = new RemoteControl(pluginCtx, runtime, host, loaded);
      deps = {
        ctx: pluginCtx,
        runtime,
        host,
        remote,
        onSettingsChanged,
      };
      detachRemote = remote.attach();
      notify();
      // 连接由插件持有而非面板持有：两个面板共用同一个运行时，若把断开挂在某个面板的
      // 清理里，切到另一个面板就会把连接掐断（表现为「明明连上了却显示未连接」）。
      const connected = runtime.connect();
      // 随应用启动：先探测再决定——服务已在（如外部启动）时再拉进程只会抢端口失败退出。
      if (loaded.autoStart && loaded.processMode === "managed") {
        void connected.then(() => {
          if (runtime.getSnapshot().channel !== "direct") void host.start(runtime);
        });
      }
    })
    .catch((err: unknown) => {
      pluginCtx.notification.notify({
        level: "warning",
        message: `设置读取失败，已用默认值启动：${err instanceof Error ? err.message : String(err)}`,
      });
    });

  /** 面板外壳：铺满可用空间并自持布局。生成与编排各自是独立视图，切换交给Atelyx的面板标签。 */
  function PanelShell(props: { children: unknown }): unknown {
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
        {props.children}
      </div>
    );
  }

  function InitPlaceholder(): unknown {
    return (
      <div style={{ height: "100%", display: "flex", alignItems: "center", justifyContent: "center", color: textMuted, fontSize: FONT_SM }}>
        正在初始化…
      </div>
    );
  }

  function GenerateView(): unknown {
    const ready = React.useSyncExternalStore(subscribe, getDeps);
    if (!ready) return <InitPlaceholder />;
    return (
      <PanelShell>
        <GeneratePanel ctx={ready.ctx} runtime={ready.runtime} host={ready.host} remote={ready.remote} />
      </PanelShell>
    );
  }

  function OrchestrateView(): unknown {
    const ready = React.useSyncExternalStore(subscribe, getDeps);
    if (!ready) return <InitPlaceholder />;
    return (
      <PanelShell>
        <OrchestratePanel runtime={ready.runtime} host={ready.host} />
      </PanelShell>
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
    // 当前仓库身份（null = 协作空间仓库）：决定结果能否保存。宿主在插件装载完成后
    // 才广播 vault:switch，这里订阅时事件必然未发过，收不到事件的场景按 unknown 保留直调通道。
    const offVault = pluginCtx.events.on("vault:switch", (p) => {
      deps?.runtime.setVaultRoot(p.root);
    });
    return () => {
      offVault();
    };
  });

  pluginCtx.effect(() => {
    // 进程收尾由 Atelyx 统一负责（插件停用/卸载/应用退出都会结束本插件启动的进程树）
    const offPanel = pluginCtx.slots.registerView({
      kind: VIEW_KIND,
      label: "ComfyUI",
      component: GenerateView,
    });
    const offOrchestrate = pluginCtx.slots.registerView({
      kind: `${VIEW_KIND}.orchestrate`,
      label: "ComfyUI 编排",
      component: OrchestrateView,
    });
    const offSetting = pluginCtx.slots.registerSetting({
      key: SETTING_KEY,
      label: "ComfyUI",
      component: ComfySettings,
    });
    return () => {
      offPanel();
      offOrchestrate();
      offSetting();
      disposeOrchestrateFrame();
      resetGenerateSession();
      detachRemote?.();
      detachRemote = null;
      deps?.runtime.disconnect();
    };
  });

  pluginCtx.effect(() => {
    // 声明本插件需要协作通道：宿主只为有活跃声明的窗口维持协作连接，远程启停的收发全靠它。
    // 旧版宿主没有声明面：降级为不声明，远程启停会提示升级宿主。
    if (!pluginCtx.collab.acquire) return;
    const release = pluginCtx.collab.acquire();
    return () => release();
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
