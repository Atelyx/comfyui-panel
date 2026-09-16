/**
 * 编排面板的常驻 iframe。
 *
 * 面板切换时 Atelyx 会卸载视图，直接内嵌的 iframe 随之销毁、每次回来都要重载。
 * 这里把 iframe 放进挂在 document.body 上的固定定位容器：面板可见时把它定位到内容区并显示，
 * 切换走/离线时只隐藏（display:none 不销毁浏览上下文），再回来直接恢复显示，不重载编辑器，
 * 编辑器里未保存的改动也随之保留。
 */
import { bgSecondary } from "./ui";

/** 覆盖层 z-index：压在面板内容之上、宿主弹层之下。 */
const OVERLAY_Z = 30;

interface OrchestrateFrame {
  container: HTMLDivElement;
  iframe: HTMLIFrameElement;
}

let frame: OrchestrateFrame | null = null;
let anchorEl: HTMLElement | null = null;
let observer: ResizeObserver | null = null;
/** 已加载的目标地址：与当前地址相同就重设 src，避免来回切换反复重载。 */
let targetUrl = "";
let loaded = false;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** 让固定定位容器精确覆盖锚点区域（getBoundingClientRect 与 fixed 同为视口坐标）。 */
function syncRect(): void {
  if (!frame || !anchorEl) return;
  const rect = anchorEl.getBoundingClientRect();
  frame.container.style.left = `${rect.left}px`;
  frame.container.style.top = `${rect.top}px`;
  frame.container.style.width = `${rect.width}px`;
  frame.container.style.height = `${rect.height}px`;
}

function ensureFrame(): OrchestrateFrame {
  if (frame) return frame;
  const container = document.createElement("div");
  container.style.position = "fixed";
  container.style.display = "none";
  container.style.zIndex = String(OVERLAY_Z);
  container.style.overflow = "hidden";
  container.style.background = bgSecondary;
  const iframe = document.createElement("iframe");
  iframe.style.width = "100%";
  iframe.style.height = "100%";
  iframe.style.border = "none";
  iframe.title = "ComfyUI 编排界面";
  iframe.addEventListener("load", () => {
    loaded = true;
    notify();
  });
  container.appendChild(iframe);
  document.body.appendChild(container);
  frame = { container, iframe };
  return frame;
}

/** 面板可见时调用：显示常驻 iframe 并让它跟随锚点；目标地址变化时重载。 */
export function attachOrchestrateFrame(anchor: HTMLElement, target: string): () => void {
  const inst = ensureFrame();
  anchorEl = anchor;
  if (targetUrl !== target) {
    targetUrl = target;
    loaded = false;
    notify();
    inst.iframe.src = target;
  }
  inst.container.style.display = "";
  syncRect();
  if (observer) observer.disconnect();
  observer = new ResizeObserver(syncRect);
  observer.observe(anchor);
  window.addEventListener("resize", syncRect);
  // 捕获阶段：面板可能处在可滚动容器里，滚动时锚点会移动
  window.addEventListener("scroll", syncRect, true);
  return () => {
    detachOrchestrateFrame();
  };
}

/** 面板卸载或离线时调用：隐藏但不销毁浏览上下文，下次回来无需重载。 */
export function detachOrchestrateFrame(): void {
  if (observer) {
    observer.disconnect();
    observer = null;
  }
  window.removeEventListener("resize", syncRect);
  window.removeEventListener("scroll", syncRect, true);
  if (frame) frame.container.style.display = "none";
}

/** 用户主动刷新：重载编辑器（会丢弃编辑器内未保存的改动，点刷新即预期如此）。 */
export function reloadOrchestrateFrame(): void {
  if (!frame || !targetUrl) return;
  loaded = false;
  notify();
  frame.iframe.src = targetUrl;
}

export function isOrchestrateFrameLoaded(): boolean {
  return loaded;
}

export function subscribeOrchestrateFrame(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 插件停用时调用：销毁常驻 iframe，释放编辑器资源。 */
export function disposeOrchestrateFrame(): void {
  detachOrchestrateFrame();
  if (frame) {
    frame.iframe.src = "about:blank";
    frame.container.remove();
    frame = null;
  }
  targetUrl = "";
  loaded = false;
  notify();
}
