/**
 * 本地校验用的 `react` 类型桩（宿主运行时提供全局 React，本文件只让 `tsc --noEmit` 能独立跑）。
 *
 * 运行时不参与：宿主打包时把 `react` / `react/jsx-runtime` 接到宿主的全局 React 上，
 * 本文件不进入发布产物。
 */
declare module "react" {
  export type ReactNode = unknown;
  export type Key = string | number;

  export interface CSSProperties {
    [key: string]: string | number | undefined;
  }

  export interface SyntheticEvent<T = unknown> {
    target: T;
    preventDefault(): void;
    stopPropagation(): void;
  }

  export interface ChangeEvent<T = unknown> extends SyntheticEvent<T> {
    target: T & { value: string; checked: boolean; files?: FileList | null };
  }

  export interface MouseEvent<T = unknown> extends SyntheticEvent<T> {
    clientX: number;
    clientY: number;
    button: number;
  }

  export type SetStateAction<S> = S | ((prev: S) => S);

  export function createElement(
    type: unknown,
    props?: Record<string, unknown> | null,
    ...children: unknown[]
  ): unknown;
  export function cloneElement(element: unknown, props?: Record<string, unknown>): unknown;
  export function useState<S>(init: S | (() => S)): [S, (value: SetStateAction<S>) => void];
  export function useEffect(effect: () => void | (() => void), deps?: readonly unknown[]): void;
  export function useLayoutEffect(effect: () => void | (() => void), deps?: readonly unknown[]): void;
  export function useMemo<T>(fn: () => T, deps: readonly unknown[]): T;
  export function useCallback<T extends (...args: never[]) => unknown>(fn: T, deps: readonly unknown[]): T;
  export function useRef<T>(init: T): { current: T };
  export function useReducer<S, A>(reducer: (state: S, action: A) => S, init: S): [S, (action: A) => void];
  export function useContext<T>(context: unknown): T;
  export function useSyncExternalStore<T>(
    subscribe: (onStoreChange: () => void) => () => void,
    getSnapshot: () => T,
    getServerSnapshot?: () => T,
  ): T;
  export function memo<T>(component: T): T;
  export function forwardRef<T>(render: T): T;
  export function createContext<T>(defaultValue: T): unknown;
  export function useId(): string;

  export const Fragment: unknown;
  export const StrictMode: unknown;
  export const Suspense: unknown;

  const React: {
    createElement: typeof createElement;
    cloneElement: typeof cloneElement;
    useState: typeof useState;
    useEffect: typeof useEffect;
    useLayoutEffect: typeof useLayoutEffect;
    useMemo: typeof useMemo;
    useCallback: typeof useCallback;
    useRef: typeof useRef;
    useReducer: typeof useReducer;
    useContext: typeof useContext;
    useSyncExternalStore: typeof useSyncExternalStore;
    memo: typeof memo;
    forwardRef: typeof forwardRef;
    createContext: typeof createContext;
    useId: typeof useId;
    Fragment: unknown;
    StrictMode: unknown;
    Suspense: unknown;
  };
  export default React;
}

/**
 * 全局 JSX 命名空间（`jsx: "react"` 经典运行时用；宿主注入模块把 JSX 转出
 * `React.createElement` 调用）。
 */
declare namespace JSX {
  type Element = unknown;
  interface ElementAttributesProperty {}
  interface ElementChildrenAttribute {
    children: unknown;
  }
  interface IntrinsicAttributes {
    key?: string | number;
  }
  interface IntrinsicElements {
    [elemName: string]: {
      [prop: string]: unknown;
      key?: string | number;
      children?: unknown;
      ref?: unknown;
    };
  }
}
