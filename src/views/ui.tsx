/**
 * 界面原语：面板共用的样式与小组件。
 *
 * 样式只用内联 style + 宿主 CSS 变量，不依赖 Tailwind（插件没有构建期的类名提取，
 * Tailwind 类不会生效）；颜色、字号、圆角、动效一律走变量而非硬编码，才能跟随主题
 * 与应用的「字体大小」设置。层级约定：内容区 --bg-primary、工具条 --bg-secondary、
 * 内嵌卡片 --bg-tertiary、浮层 --bg-overlay + --shadow-pop、输入面 --bg-sunken。
 * 状态只用语义色变量，且不只靠颜色表达：StatusPill 的点形、Notice 的文案都是第二重编码。
 */
import React from "react";

/* ===== token 别名：视图从这里取变量串，避免字面量散落 ===== */
export const textPrimary = "var(--text-primary)";
export const textSecondary = "var(--text-secondary)";
export const textMuted = "var(--text-muted)";
export const border = "var(--border)";
export const borderSubtle = "var(--border-subtle)";
export const bgPrimary = "var(--bg-primary)";
export const bgSecondary = "var(--bg-secondary)";
export const bgTertiary = "var(--bg-tertiary)";
export const bgSunken = "var(--bg-sunken)";
export const hover = "var(--hover)";
export const accent = "var(--accent)";
export const accentFg = "var(--accent-fg)";
export const accentSoft = "var(--accent-soft)";
export const danger = "var(--danger)";
export const success = "var(--success)";
export const warning = "var(--warning)";

/** 字阶（rem）：micro 11 / caption 12 / ui 13 / body 14，随应用字体大小设置缩放。 */
export const FONT_MICRO = "var(--fs-micro)";
export const FONT_CAPTION = "var(--fs-caption)";
export const FONT_UI = "var(--fs-ui)";
export const FONT_BODY = "var(--fs-body)";

/** 状态变化统一时长与曲线；无限循环动画（占位格流光/呼吸）不归入此档。 */
export const TRANSITION_FAST = "var(--dur-fast) var(--ease)";
export const TRANSITION_BASE = "var(--dur-base) var(--ease)";

/** 浮层通用外观：宿主 PopupLayer 同款——overlay 底 + 细边 + pop 投影（投影只给浮起元素）。 */
export const FLOAT_STYLE = {
  background: "var(--bg-overlay)",
  border: `1px solid ${border}`,
  borderRadius: "var(--radius-md)",
  boxShadow: "var(--shadow-pop)",
};

/** 滚动区类名与滚动条样式：宿主主题可能隐藏滚动条，滚动区需要自带；
 *  thumb 走 --scrollbar-* 变量随主题取色。 */
export const SCROLL_LIST_CLASS = "cf-scroll-list";
export const SCROLLBAR_CSS = `
.cf-scroll-list::-webkit-scrollbar { width: 10px; height: 10px; }
.cf-scroll-list::-webkit-scrollbar-track { background: transparent; }
.cf-scroll-list::-webkit-scrollbar-thumb { background: var(--scrollbar-thumb); border-radius: 5px; }
.cf-scroll-list::-webkit-scrollbar-thumb:hover { background: var(--scrollbar-thumb-hover); }
`;

/** 图标基座（lucide 的 24×24 线性风格）；不引图标库，图标即几条 path。 */
function Svg(props: { size?: number; className?: string; children?: unknown }): unknown {
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
      className={props.className}
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

export function PlayIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M6 4l13 8-13 8V4z" />
    </Svg>
  );
}

export function SquareIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <rect x="6" y="6" width="12" height="12" rx="1" />
    </Svg>
  );
}

export function RefreshIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M21 12a9 9 0 1 1-2.64-6.36" />
      <path d="M21 3v5h-5" />
    </Svg>
  );
}

export function UploadIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <path d="M17 8l-5-5-5 5" />
      <path d="M12 3v12" />
    </Svg>
  );
}

export function SaveIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z" />
      <path d="M17 21v-8H7v8" />
      <path d="M7 3v5h8" />
    </Svg>
  );
}

export function CopyIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <rect x="9" y="9" width="13" height="13" rx="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </Svg>
  );
}

export function SearchIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <circle cx="11" cy="11" r="8" />
      <path d="M21 21l-4.35-4.35" />
    </Svg>
  );
}

export function ChevronDownIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M6 9l6 6 6-6" />
    </Svg>
  );
}

export function ChevronRightIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M9 6l6 6-6 6" />
    </Svg>
  );
}

export function ImageIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <circle cx="8.5" cy="8.5" r="1.5" />
      <path d="M21 15l-5-5L5 21" />
    </Svg>
  );
}

export function ZapIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
    </Svg>
  );
}

/**
 * 生成图像（图片框 + 四角生成星 + 山线）：生图语义的图标，设置页 tab 等宿主小尺寸位用。
 * 生成星按宿主 tab 图标契约接 className（宿主渲染时带布局类）。
 */
export function ImageSparkIcon(props: { size?: number; className?: string }): unknown {
  return (
    <Svg size={props.size} className={props.className}>
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <path d="M8.5 5.3Q9.4 7.6 11.7 8.5Q9.4 9.4 8.5 11.7Q7.6 9.4 5.3 8.5Q7.6 7.6 8.5 5.3Z" />
      <path d="M21 15l-5-5L5 21" />
    </Svg>
  );
}

/** 面板外壳：铺满可用空间，自身不滚动（滚动交内容子区）。 */
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
        fontSize: FONT_UI,
      }}
    >
      {props.toolbar}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>{props.children}</div>
    </div>
  );
}

/** 面板头工具条：状态在左，动作在右；secondary 底 + 1px 底边与内容区分层。 */
export function Toolbar(props: { children: unknown }): unknown {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        flexWrap: "wrap",
        padding: "8px 12px",
        borderBottom: `1px solid ${border}`,
        background: bgSecondary,
        flexShrink: 0,
      }}
    >
      {props.children}
    </div>
  );
}

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
type ButtonSize = "sm" | "md";

/** 控件高固定（sm 24 / md 28），字号变化不撑开布局。 */
const BUTTON_SIZE: Record<ButtonSize, { height: number; padding: string; font: string; gap: number }> = {
  sm: { height: 24, padding: "0 8px", font: FONT_MICRO, gap: 4 },
  md: { height: 28, padding: "0 12px", font: FONT_UI, gap: 6 },
};

/** 变体只给底色/文字色/hover 反馈；强调色留给需引起注意的动作。 */
const BUTTON_VARIANT: Record<
  ButtonVariant,
  { background: string; color: string; hoverBackground?: string; hoverColor?: string }
> = {
  primary: { background: accent, color: accentFg, hoverBackground: "var(--accent-hover)" },
  secondary: { background: bgTertiary, color: textPrimary, hoverBackground: hover },
  ghost: { background: "transparent", color: textSecondary, hoverBackground: hover, hoverColor: textPrimary },
  danger: {
    background: "transparent",
    color: danger,
    hoverBackground: "color-mix(in srgb, var(--danger) 12%, transparent)",
  },
};

export function Button(props: {
  children?: unknown;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** 受控高亮（弹层触发钮展开时）：底色 --hover，优先于变体底色。 */
  active?: boolean;
  style?: Record<string, unknown>;
}): unknown {
  const [isHover, setHover] = React.useState(false);
  const [isFocusVisible, setFocusVisible] = React.useState(false);
  const size = BUTTON_SIZE[props.size ?? "md"];
  const v = BUTTON_VARIANT[props.variant ?? "secondary"];
  const enabled = !props.disabled;
  const background = props.active ? hover : isHover && enabled ? (v.hoverBackground ?? v.background) : v.background;
  const color = isHover && enabled ? (v.hoverColor ?? v.color) : v.color;
  return (
    <button
      type="button"
      title={props.title}
      disabled={props.disabled}
      onClick={props.disabled ? undefined : props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={(e: { target: EventTarget | null }) =>
        setFocusVisible(e.target instanceof HTMLElement && e.target.matches(":focus-visible"))
      }
      onBlur={() => setFocusVisible(false)}
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        gap: size.gap,
        height: size.height,
        padding: size.padding,
        borderRadius: "var(--radius-sm)",
        fontSize: size.font,
        fontWeight: 500,
        fontFamily: "inherit",
        cursor: props.disabled ? "default" : "pointer",
        opacity: props.disabled ? 0.4 : 1,
        border: "none",
        background,
        color,
        whiteSpace: "nowrap",
        flexShrink: 0,
        outline: "none",
        boxShadow: isFocusVisible && enabled ? "var(--focus-ring)" : undefined,
        transition: `background ${TRANSITION_FAST}, color ${TRANSITION_FAST}`,
        ...props.style,
      }}
    >
      {props.children}
    </button>
  );
}

/**
 * 状态语义胶囊：色 + 点形 + 文字三重编码——ok/warn 圆点、bad/idle 方点，
 * 文字与点同色（语义变量），不辨色也能读出状态。
 */
export function StatusPill(props: { tone: "ok" | "warn" | "bad" | "idle"; label: string; title?: string }): unknown {
  const color =
    props.tone === "ok" ? success : props.tone === "warn" ? warning : props.tone === "bad" ? danger : textMuted;
  const round = props.tone === "ok" || props.tone === "warn";
  return (
    <span
      title={props.title}
      style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: FONT_MICRO, flexShrink: 0 }}
    >
      <span style={{ width: 6, height: 6, borderRadius: round ? 3 : 1, background: color, flexShrink: 0 }} />
      <span style={{ color }}>{props.label}</span>
    </span>
  );
}

/** 进度条（0..1）：4px 细条，轨道下沉、填充走强调色。 */
export function ProgressBar(props: { value: number; label?: string }): unknown {
  const pct = Math.max(0, Math.min(1, props.value));
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ height: 4, borderRadius: 2, background: "var(--bg-sunken)", overflow: "hidden" }}>
        <div
          style={{
            width: `${pct * 100}%`,
            height: "100%",
            background: accent,
            transition: `width ${TRANSITION_BASE}`,
          }}
        />
      </div>
      {props.label ? <div style={{ fontSize: FONT_MICRO, color: textMuted }}>{props.label}</div> : null}
    </div>
  );
}

const NOTICE_TONE: Record<"info" | "warn" | "error", { color: string; borderColor: string; background: string }> = {
  info: { color: textSecondary, borderColor: border, background: bgTertiary },
  warn: {
    color: warning,
    borderColor: "color-mix(in srgb, var(--warning) 32%, transparent)",
    background: "color-mix(in srgb, var(--warning) 10%, transparent)",
  },
  error: {
    color: danger,
    borderColor: "color-mix(in srgb, var(--danger) 32%, transparent)",
    background: "color-mix(in srgb, var(--danger) 10%, transparent)",
  },
};

/** 提示条：语义色只上边与底（color-mix 低占比），文字承载具体原因。 */
export function Notice(props: { tone: "info" | "warn" | "error"; children: unknown; onClose?: () => void }): unknown {
  const t = NOTICE_TONE[props.tone];
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 8,
        padding: "8px 10px",
        borderRadius: "var(--radius-sm)",
        border: `1px solid ${t.borderColor}`,
        background: t.background,
        fontSize: FONT_CAPTION,
        color: t.color,
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
            color: "inherit",
            cursor: "pointer",
            padding: 0,
            flexShrink: 0,
          }}
        >
          <CloseIcon size={14} />
        </button>
      ) : null}
    </div>
  );
}

const CHIP_TONE: Record<"default" | "ok" | "warn", { color: string; borderColor: string; background: string }> = {
  default: { color: textSecondary, borderColor: border, background: bgTertiary },
  ok: {
    color: success,
    borderColor: "color-mix(in srgb, var(--success) 32%, transparent)",
    background: "color-mix(in srgb, var(--success) 12%, transparent)",
  },
  warn: {
    color: warning,
    borderColor: "color-mix(in srgb, var(--warning) 32%, transparent)",
    background: "color-mix(in srgb, var(--warning) 12%, transparent)",
  },
};

/** 低调小徽标：展示状态或来源这类次要信息，不抢输入行的空间。 */
export function Chip(props: {
  children: unknown;
  title?: string;
  tone?: "default" | "ok" | "warn";
}): unknown {
  const t = CHIP_TONE[props.tone ?? "default"];
  return (
    <span
      title={props.title}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        maxWidth: "100%",
        padding: "2px 6px",
        borderRadius: "var(--radius-xs)",
        border: `1px solid ${t.borderColor}`,
        background: t.background,
        fontSize: FONT_MICRO,
        lineHeight: 1.2,
        color: t.color,
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis",
        flexShrink: 0,
      }}
    >
      {props.children}
    </span>
  );
}

/** 可折叠区块（内嵌卡片）：标题栏可点击展开/收起，children 仅在展开时渲染。 */
export function CollapseCard(props: {
  title: unknown;
  subtitle?: unknown;
  badge?: unknown;
  open: boolean;
  onToggle: () => void;
  children: unknown;
}): unknown {
  return (
    <div
      style={{
        border: `1px solid ${border}`,
        borderRadius: "var(--radius-sm)",
        background: bgTertiary,
        marginBottom: 8,
        overflow: "hidden",
      }}
    >
      <button
        type="button"
        onClick={props.onToggle}
        title={props.open ? "收起" : "展开"}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          width: "100%",
          boxSizing: "border-box",
          padding: "6px 8px",
          border: "none",
          background: "transparent",
          color: textPrimary,
          fontSize: FONT_CAPTION,
          fontWeight: 500,
          textAlign: "left",
          cursor: "pointer",
        }}
      >
        {props.open ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
        <span
          style={{
            flexShrink: 0,
            maxWidth: "45%",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {props.title}
        </span>
        {props.subtitle ? (
          <span
            style={{
              flex: 1,
              minWidth: 0,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              fontSize: FONT_MICRO,
              fontWeight: 400,
              color: textMuted,
            }}
          >
            {props.subtitle}
          </span>
        ) : (
          <span style={{ flex: 1 }} />
        )}
        {props.badge ? (
          <span
            style={{
              flexShrink: 0,
              fontSize: FONT_MICRO,
              color: textSecondary,
              border: `1px solid ${border}`,
              borderRadius: "var(--radius-xs)",
              padding: "1px 6px",
              lineHeight: 1.2,
            }}
          >
            {props.badge}
          </span>
        ) : null}
      </button>
      {props.open ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 6,
            padding: 8,
            borderTop: `1px solid ${borderSubtle}`,
          }}
        >
          {props.children}
        </div>
      ) : null}
    </div>
  );
}

/** 空态：图标块 + 标题 + 说明 + 行动；说明承载「为什么空、下一步做什么」。 */
export function Empty(props: { icon?: unknown; title: unknown; description?: unknown; action?: unknown }): unknown {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 8,
        padding: "40px 24px",
        textAlign: "center",
      }}
    >
      {props.icon ? (
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: "var(--radius-md)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            background: bgTertiary,
            color: textMuted,
            flexShrink: 0,
          }}
        >
          {props.icon}
        </div>
      ) : null}
      <div style={{ fontSize: FONT_BODY, fontWeight: 500, color: textPrimary }}>{props.title}</div>
      {props.description ? (
        <div style={{ fontSize: FONT_UI, color: textMuted, lineHeight: 1.6, maxWidth: "42ch" }}>
          {props.description}
        </div>
      ) : null}
      {props.action ? <div style={{ marginTop: 4 }}>{props.action}</div> : null}
    </div>
  );
}

const FIELD_BASE: Record<string, unknown> = {
  width: "100%",
  boxSizing: "border-box",
  padding: "4px 8px",
  borderRadius: "var(--radius-sm)",
  border: `1px solid var(--input-border)`,
  background: "var(--input-bg)",
  color: textPrimary,
  fontSize: FONT_UI,
  fontFamily: "inherit",
  outline: "none",
  transition: `border-color ${TRANSITION_FAST}`,
};

export function TextInput(props: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: string;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  title?: string;
  style?: Record<string, unknown>;
}): unknown {
  const [focused, setFocused] = React.useState(false);
  return (
    <input
      className="cf-field"
      type={props.type ?? "text"}
      value={props.value}
      placeholder={props.placeholder}
      min={props.min}
      max={props.max}
      step={props.step}
      disabled={props.disabled}
      title={props.title}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        ...FIELD_BASE,
        borderColor: focused ? accent : "var(--input-border)",
        boxShadow: focused ? "var(--focus-ring)" : undefined,
        cursor: props.disabled ? "default" : undefined,
        opacity: props.disabled ? 0.5 : 1,
        ...(props.style ?? {}),
      }}
    />
  );
}

/** 自适应高度文本域：内容变化时在 min/max 高度间自动伸缩；到顶后内部滚动，不可手动调整大小。 */
export function AutoTextArea(props: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  minHeight?: number;
  maxHeight?: number;
  style?: Record<string, unknown>;
}): unknown {
  const ref = React.useRef<HTMLTextAreaElement | null>(null);
  const fit = React.useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const min = props.minHeight ?? 48;
    const max = props.maxHeight ?? 200;
    el.style.height = "0px";
    el.style.height = `${Math.max(min, Math.min(max, el.scrollHeight))}px`;
    el.style.overflowY = el.scrollHeight > max ? "auto" : "hidden";
  }, [props.minHeight, props.maxHeight]);
  React.useEffect(fit, [fit, props.value]);
  return (
    <textarea
      className="cf-field"
      ref={ref}
      value={props.value}
      placeholder={props.placeholder}
      disabled={props.disabled}
      rows={1}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        ...FIELD_BASE,
        border: "none",
        background: "transparent",
        lineHeight: 1.5,
        opacity: props.disabled ? 0.5 : 1,
        resize: "none",
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
  disabled?: boolean;
  style?: Record<string, unknown>;
}): unknown {
  return (
    <textarea
      className="cf-field"
      value={props.value}
      placeholder={props.placeholder}
      rows={props.rows ?? 3}
      disabled={props.disabled}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        ...FIELD_BASE,
        fontFamily: props.mono ? "var(--font-mono)" : "inherit",
        lineHeight: 1.5,
        resize: "vertical",
        opacity: props.disabled ? 0.5 : 1,
        ...(props.style ?? {}),
      }}
    />
  );
}

export function Select(props: {
  value: string;
  onChange: (value: string) => void;
  options: Array<{ value: string; label: string }>;
  disabled?: boolean;
  title?: string;
  style?: Record<string, unknown>;
}): unknown {
  return (
    <select
      className="cf-field"
      value={props.value}
      disabled={props.disabled}
      title={props.title}
      onChange={(e: { target: { value: string } }) => props.onChange(e.target.value)}
      style={{
        ...FIELD_BASE,
        // select 的按钮区不吃底部 padding（顶部 4px 照算），FIELD_BASE 的竖直 padding 会把文字压低
        padding: "0 8px",
        height: 24,
        cursor: props.disabled ? "default" : "pointer",
        opacity: props.disabled ? 0.5 : 1,
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
        gap: 8,
        fontSize: FONT_UI,
        color: textPrimary,
        cursor: props.disabled ? "default" : "pointer",
        opacity: props.disabled ? 0.5 : 1,
      }}
    >
      <input
        type="checkbox"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e: { target: { checked: boolean } }) => props.onChange(e.target.checked)}
        style={{ accentColor: accent, width: 14, height: 14, cursor: "inherit", flexShrink: 0 }}
      />
      {props.label}
    </label>
  );
}

export function Field(props: { label: string; hint?: unknown; children: unknown }): unknown {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 12 }}>
      <div style={{ fontSize: FONT_CAPTION, color: textPrimary }}>{props.label}</div>
      {props.hint ? (
        <div style={{ fontSize: FONT_MICRO, color: textMuted, lineHeight: 1.5 }}>{props.hint}</div>
      ) : null}
      {props.children}
    </div>
  );
}

/** 设置区块卡：内嵌卡片档（tertiary 底 + 细边），标题与右侧行动同排。 */
export function Card(props: { title: string; children: unknown; actions?: unknown }): unknown {
  return (
    <div
      style={{
        border: `1px solid ${border}`,
        borderRadius: "var(--radius-sm)",
        background: bgTertiary,
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
        <div style={{ fontSize: FONT_UI, color: textPrimary, fontWeight: 600 }}>{props.title}</div>
        {props.actions}
      </div>
      {props.children}
    </div>
  );
}

export function DownloadIcon(props: { size?: number }): unknown {
  return (
    <Svg size={props.size}>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <path d="M7 10l5 5 5-5" />
      <path d="M12 15V3" />
    </Svg>
  );
}

/** 右键菜单容器：fixed 定位 + 实测尺寸视口钳制 + 点击外部关闭（Esc 归调用方，便于区分菜单与弹层两层）。 */
export function ContextMenu(props: {
  x: number;
  y: number;
  onClose: () => void;
  children: unknown;
}): unknown {
  const { x, y, onClose } = props;
  const ref = React.useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = React.useState({ x, y });
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setPos({
      x: Math.max(4, Math.min(x, window.innerWidth - el.offsetWidth - 4)),
      y: Math.max(4, Math.min(y, window.innerHeight - el.offsetHeight - 4)),
    });
  }, [x, y]);
  React.useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [onClose]);
  return (
    <div
      ref={ref}
      style={{
        position: "fixed",
        left: pos.x,
        top: pos.y,
        zIndex: 70,
        minWidth: 150,
        padding: "4px 0",
        ...FLOAT_STYLE,
      }}
      // 菜单内再右键不落到遮罩上，避免菜单在原地反复重开
      onContextMenu={(e: { preventDefault(): void; stopPropagation(): void }) => {
        e.preventDefault();
        e.stopPropagation();
      }}
    >
      {props.children}
    </div>
  );
}

/** 右键菜单项：图标 + 文案，悬停出中性底。 */
export function ContextMenuItem(props: { onClick: () => void; children: unknown }): unknown {
  const [isHover, setHover] = React.useState(false);
  return (
    <button
      type="button"
      onClick={props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        width: "100%",
        padding: "5px 12px",
        border: "none",
        background: isHover ? hover : "transparent",
        color: isHover ? textPrimary : textSecondary,
        fontSize: FONT_CAPTION,
        fontFamily: "inherit",
        textAlign: "left",
        cursor: "pointer",
      }}
    >
      {props.children}
    </button>
  );
}
