/**
 * 界面原语：面板共用的样式与小组件。
 *
 * 样式只用内联 style + 宿主的 CSS 变量，不依赖 Tailwind（插件没有构建期的类名提取，
 * Tailwind 类不会生效）；颜色走变量而非硬编码，才能跟随用户主题变化。
 */
import React from "react";

export const textPrimary = "var(--text-primary)";
export const textSecondary = "var(--text-secondary)";
export const textMuted = "var(--text-muted)";
export const border = "var(--border)";
export const bgPrimary = "var(--bg-primary)";
export const bgSecondary = "var(--bg-secondary)";
export const hover = "var(--hover)";
export const accent = "var(--accent)";
export const accentFg = "var(--accent-fg)";
export const danger = "#e5534b";

export const FONT_SM = 12;
export const FONT_MD = 13;

/** 图标基座（lucide 的 24×24 线性风格）；不引图标库，图标即几条 path。 */
function Svg(props: { size?: number; children?: unknown }): unknown {
  return (
    <svg
      width={props.size ?? 14}
      height={props.size ?? 14}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ flexShrink: 0 }}
    >
      {props.children}
    </svg>
  );
}

export function CloseIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M18 6L6 18" />
      <path d="M6 6l12 12" />
    </Svg>
  );
}

/** 面板外壳：铺满可用空间，自身负责滚动。 */
export function Panel(props: { children: unknown; toolbar?: unknown }): unknown {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        minHeight: 0,
        background: bgPrimary,
        color: textPrimary,
        fontSize: FONT_MD,
      }}
    >
      {props.toolbar}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>{props.children}</div>
    </div>
  );
}

/** 顶部工具条：状态在左，动作在右。 */
export function Toolbar(props: { children: unknown }): unknown {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        flexWrap: "wrap",
        padding: "8px 10px",
        borderBottom: `1px solid ${border}`,
        background: bgSecondary,
        flexShrink: 0,
      }}
    >
      {props.children}
    </div>
  );
}

interface ButtonProps {
  children: unknown;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  primary?: boolean;
  /** 危险动作（如中断、停止）。 */
  tone?: "default" | "primary" | "danger";
  active?: boolean;
}

export function Button(props: ButtonProps): unknown {
  const [isHover, setHover] = React.useState(false);
  const tone = props.tone ?? (props.primary ? "primary" : "default");
  const background = tone === "primary" ? accent : tone === "danger" ? danger : props.active ? hover : "transparent";
  const color = tone === "primary" ? accentFg : tone === "danger" ? "#fff" : textPrimary;
  return (
    <button
      type="button"
      title={props.title}
      disabled={props.disabled}
      onClick={props.disabled ? undefined : props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "5px 10px",
        borderRadius: 6,
        fontSize: FONT_SM,
        fontFamily: "inherit",
        cursor: props.disabled ? "not-allowed" : "pointer",
        opacity: props.disabled ? 0.5 : 1,
        border: `1px solid ${tone === "default" ? border : "transparent"}`,
        background: props.disabled ? "transparent" : tone === "default" && isHover ? hover : background,
        color,
        whiteSpace: "nowrap",
      }}
    >
      {props.children}
    </button>
  );
}

export function StatusDot(props: { tone: "ok" | "warn" | "bad" | "idle"; label: string; title?: string }): unknown {
  const color =
    props.tone === "ok" ? "#3fb950" : props.tone === "warn" ? "#d29922" : props.tone === "bad" ? danger : textMuted;
  return (
    <span title={props.title} style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: FONT_SM }}>
      <span style={{ width: 8, height: 8, borderRadius: 4, background: color, flexShrink: 0 }} />
      <span style={{ color: textSecondary }}>{props.label}</span>
    </span>
  );
}

/** 进度条（0..1）。 */
export function ProgressBar(props: { value: number; label?: string }): unknown {
  const pct = Math.max(0, Math.min(1, props.value));
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ height: 6, borderRadius: 3, background: hover, overflow: "hidden" }}>
        <div style={{ width: `${pct * 100}%`, height: "100%", background: accent, transition: "width 120ms linear" }} />
      </div>
      {props.label ? <div style={{ fontSize: 11, color: textMuted }}>{props.label}</div> : null}
    </div>
  );
}

export function Notice(props: { tone: "info" | "warn" | "error"; children: unknown; onClose?: () => void }): unknown {
  const color = props.tone === "error" ? danger : props.tone === "warn" ? "#d29922" : textSecondary;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 8,
        padding: "8px 10px",
        borderRadius: 6,
        border: `1px solid ${color}`,
        background: bgSecondary,
        fontSize: FONT_SM,
        color: textSecondary,
        lineHeight: 1.5,
      }}
    >
      <div style={{ flex: 1, minWidth: 0, wordBreak: "break-word" }}>{props.children}</div>
      {props.onClose ? (
        <button
          type="button"
          onClick={props.onClose}
          title="关闭"
          style={{
            display: "inline-flex",
            border: "none",
            background: "transparent",
            color: textMuted,
            cursor: "pointer",
            padding: 0,
          }}
        >
          <CloseIcon size={14} />
        </button>
      ) : null}
    </div>
  );
}

export function SectionTitle(props: { children: unknown; right?: unknown }): unknown {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 8,
        padding: "6px 10px",
        fontSize: 11,
        letterSpacing: 0.4,
        textTransform: "uppercase",
        color: textMuted,
        flexShrink: 0,
      }}
    >
      <span>{props.children}</span>
      {props.right}
    </div>
  );
}

export function Empty(props: { children: unknown; hint?: unknown }): unknown {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 8,
        padding: 24,
        textAlign: "center",
        color: textMuted,
        fontSize: FONT_SM,
        lineHeight: 1.6,
      }}
    >
      <div>{props.children}</div>
      {props.hint ? <div style={{ fontSize: 11 }}>{props.hint}</div> : null}
    </div>
  );
}

export function TextInput(props: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: string;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  style?: Record<string, unknown>;
}): unknown {
  return (
    <input
      type={props.type ?? "text"}
      value={props.value}
      placeholder={props.placeholder}
      min={props.min}
      max={props.max}
      step={props.step}
      disabled={props.disabled}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        width: "100%",
        boxSizing: "border-box",
        padding: "5px 8px",
        borderRadius: 6,
        border: `1px solid ${border}`,
        background: bgPrimary,
        color: textPrimary,
        fontSize: FONT_SM,
        fontFamily: "inherit",
        outline: "none",
        ...(props.style ?? {}),
      }}
    />
  );
}

export function TextArea(props: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  rows?: number;
  mono?: boolean;
}): unknown {
  return (
    <textarea
      value={props.value}
      placeholder={props.placeholder}
      rows={props.rows ?? 3}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        width: "100%",
        boxSizing: "border-box",
        padding: "6px 8px",
        borderRadius: 6,
        border: `1px solid ${border}`,
        background: bgPrimary,
        color: textPrimary,
        fontSize: FONT_SM,
        fontFamily: props.mono ? "ui-monospace, SFMono-Regular, Menlo, monospace" : "inherit",
        lineHeight: 1.5,
        resize: "vertical",
        outline: "none",
      }}
    />
  );
}

export function Select(props: {
  value: string;
  onChange: (value: string) => void;
  options: Array<{ value: string; label: string }>;
  disabled?: boolean;
  style?: Record<string, unknown>;
}): unknown {
  return (
    <select
      value={props.value}
      disabled={props.disabled}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        width: "100%",
        boxSizing: "border-box",
        padding: "5px 6px",
        borderRadius: 6,
        border: `1px solid ${border}`,
        background: bgPrimary,
        color: textPrimary,
        fontSize: FONT_SM,
        fontFamily: "inherit",
        outline: "none",
        ...(props.style ?? {}),
      }}
    >
      {props.options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

export function Checkbox(props: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: unknown;
  disabled?: boolean;
  title?: string;
}): unknown {
  return (
    <label
      title={props.title}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        fontSize: FONT_SM,
        color: textSecondary,
        cursor: props.disabled ? "not-allowed" : "pointer",
        opacity: props.disabled ? 0.6 : 1,
      }}
    >
      <input
        type="checkbox"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e: { target: { checked: boolean } }) => props.onChange(e.target.checked)}
        style={{ accentColor: accent, cursor: "inherit" }}
      />
      {props.label}
    </label>
  );
}

/** 破坏性动作的二次确认：就地展开，不用系统弹窗（应用内统一走界面反馈）。 */
export function ConfirmButton(props: {
  label: string;
  confirmLabel: string;
  onConfirm: () => void;
  disabled?: boolean;
}): unknown {
  const [asking, setAsking] = React.useState(false);
  if (!asking) {
    return <Button disabled={props.disabled} onClick={() => setAsking(true)}>{props.label}</Button>;
  }
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
      <Button
        tone="danger"
        onClick={() => {
          setAsking(false);
          props.onConfirm();
        }}
      >
        {props.confirmLabel}
      </Button>
      <Button onClick={() => setAsking(false)}>取消</Button>
    </span>
  );
}

export function Field(props: { label: string; hint?: unknown; children: unknown }): unknown {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 12 }}>
      <div style={{ fontSize: FONT_SM, color: textPrimary }}>{props.label}</div>
      {props.hint ? <div style={{ fontSize: 11, color: textMuted, lineHeight: 1.5 }}>{props.hint}</div> : null}
      {props.children}
    </div>
  );
}

export function Card(props: { title: string; children: unknown; actions?: unknown }): unknown {
  return (
    <div
      style={{
        border: `1px solid ${border}`,
        borderRadius: 8,
        background: bgSecondary,
        padding: 12,
        marginBottom: 12,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          marginBottom: 10,
        }}
      >
        <div style={{ fontSize: FONT_MD, color: textPrimary, fontWeight: 600 }}>{props.title}</div>
        {props.actions}
      </div>
      {props.children}
    </div>
  );
}
