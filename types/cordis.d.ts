/**
 * 本地校验用的最小类型桩（宿主 `@atelyx/cordis` 类型面的等价替身）。
 *
 * 插件入口在实际加载时由宿主转译，`import type { Context }` 会被擦除；本文件只让
 * `tsc --noEmit` 能在插件仓库内独立运行——不参与插件运行时，也不进入发布产物。
 * 插件真正使用的服务面在 `src/ctx.ts` 里定型。
 */
export interface Context {}
