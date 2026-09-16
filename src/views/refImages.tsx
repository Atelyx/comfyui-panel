/**
 * 参考图栈：绑定工作流里的 LoadImage 节点。
 *
 * 一个槽位对应一个未连线的 LoadImage 节点：上传图片到 ComfyUI input 目录并把文件名写进
 * 该节点的 image 字段，或从 input 目录已有图里选（候选来自节点定义声明的文件清单）。
 * 工作流没有 LoadImage 节点时整体隐藏；连线输入是只读拓扑，不在这里出现。
 */
import React from "react";
import type { ApiPrompt, ObjectInfoMap } from "../comfy/types";
import type { ComfyRuntime } from "../runtime";
import { readField, type FieldSpec } from "../workflow/form";
import { border, Button, Select, UploadIcon, textMuted, bgSecondary } from "./ui";

interface RefImageStackProps {
  fields: FieldSpec[];
  draft: ApiPrompt | null;
  objectInfo: ObjectInfoMap | null;
  runtime: ComfyRuntime;
  offline: boolean;
  /** 值写回草稿（置 dirty 并只拷贝该字段路径）；「从已有图选择」同步触发。 */
  onChange: (field: FieldSpec, value: string) => void;
  /** 上传并写字段：调用方负责校验工作流未切换，失败抛错由槽位展示。 */
  onUpload: (field: FieldSpec, file: File) => Promise<void>;
  /** 上传/写字段失败的用户可见原因。 */
  onError: (message: string) => void;
}

/** 一次最多平铺展示的槽位数，超出折叠成「+N」。 */
const MAX_VISIBLE = 5;

export function RefImageStack(props: RefImageStackProps): unknown {
  const [expanded, setExpanded] = React.useState(false);
  const slots = React.useMemo(
    () => props.fields.filter((f) => f.classType === "LoadImage" && f.input === "image" && !f.connected),
    [props.fields],
  );
  const options = loadImageOptions(props.objectInfo);
  /**
   * 「从已有图选择」的候选并入各槽位当前文件名：节点定义连接时取一次不再刷新，
   * 刚上传的文件若不并入就选不中自己的值，下拉会显示空白。
   */
  const allOptions = React.useMemo(() => {
    const set = new Set(options);
    if (props.draft) {
      for (const slot of slots) {
        const value = String(readField(props.draft, slot) ?? "");
        if (value) set.add(value);
      }
    }
    return Array.from(set);
  }, [options, slots, props.draft]);
  if (slots.length === 0) return null;

  const visible = expanded ? slots : slots.slice(0, MAX_VISIBLE);
  const extra = slots.length - visible.length;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4, flexShrink: 0 }}>
      <span style={{ fontSize: 10, color: textMuted }}>参考图</span>
      <div style={{ display: "flex", alignItems: "center" }}>
        {visible.map((slot, index) => {
          const filename = props.draft ? String(readField(props.draft, slot) ?? "") : "";
          return (
            <div key={slot.nodeId} style={{ marginLeft: index === 0 ? 0 : -10 }}>
              <RefSlot
                field={slot}
                filename={filename}
                thumbUrl={filename ? props.runtime.imageUrl({ filename, subfolder: "", type: "input" }) : null}
                options={allOptions}
                disabled={props.offline}
                onChange={(value) => props.onChange(slot, value)}
                onUpload={(file) => props.onUpload(slot, file)}
                onError={props.onError}
              />
            </div>
          );
        })}
        {extra > 0 ? (
          <button
            type="button"
            onClick={() => setExpanded(true)}
            title={`展开其余 ${extra} 个参考图`}
            style={{
              width: 30,
              height: 64,
              marginLeft: -8,
              border: `1px dashed ${border}`,
              borderRadius: 6,
              background: "var(--hover)",
              color: textMuted,
              fontSize: 10,
              cursor: "pointer",
            }}
          >
            +{extra}
          </button>
        ) : null}
      </div>
    </div>
  );
}

/** 单个 LoadImage 槽位：缩略图 + 悬停面板（选已有图 / 上传 / 清空）。 */
function RefSlot(props: {
  field: FieldSpec;
  filename: string;
  thumbUrl: string | null;
  options: string[];
  disabled: boolean;
  onChange: (value: string) => void;
  onUpload: (file: File) => Promise<void>;
  onError: (message: string) => void;
}): unknown {
  const fileRef = React.useRef<HTMLInputElement | null>(null);
  const overlayRef = React.useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = React.useState(false);
  const [uploading, setUploading] = React.useState(false);

  const pickFile = (): void => fileRef.current?.click();

  /** 从缩略图移向悬停面板的间隙不熄灭：relatedTarget 落在面板内即视为仍在悬停。 */
  const leaveThumb = (e: { relatedTarget: unknown }): void => {
    const next = e.relatedTarget;
    if (next instanceof Node && overlayRef.current && overlayRef.current.contains(next)) return;
    setHover(false);
  };

  const handleFile = (e: { target: { files: FileList | null; value: string } }): void => {
    const file = e.target.files?.[0];
    if (!file || props.disabled) return;
    setUploading(true);
    props
      .onUpload(file)
      .catch((err: unknown) => props.onError(err instanceof Error ? err.message : String(err)))
      .finally(() => setUploading(false));
    e.target.value = "";
  };

  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      <div
        onMouseEnter={() => setHover(true)}
        onMouseLeave={leaveThumb}
        onClick={props.disabled ? undefined : pickFile}
        title={props.disabled ? "未连接，无法上传" : props.filename ? "点击替换参考图" : "点击上传参考图"}
        style={{
          width: 48,
          height: 64,
          borderRadius: 6,
          border: `1px dashed ${props.thumbUrl ? "transparent" : border}`,
          background: props.thumbUrl ? "transparent" : "var(--hover)",
          overflow: "hidden",
          position: "relative",
          cursor: props.disabled ? "not-allowed" : "pointer",
          transform: hover
            ? "translateY(-6px) scale(1.1) rotate(0deg)"
            : `rotate(${props.field.nodeId.charCodeAt(0) % 2 === 0 ? -2.5 : 2.5}deg)`,
          transition: "transform 0.15s ease",
          zIndex: hover ? 2 : 1,
        }}
      >
        {props.thumbUrl ? (
          <img
            src={props.thumbUrl}
            alt={props.filename}
            style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }}
          />
        ) : (
          <span
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: textMuted,
            }}
          >
            <UploadIcon size={16} />
          </span>
        )}
      </div>

      {hover ? (
        <div
          ref={overlayRef}
          onMouseEnter={() => setHover(true)}
          onMouseLeave={() => setHover(false)}
          style={{
            position: "absolute",
            top: "100%",
            left: 0,
            width: 170,
            zIndex: 30,
            padding: 6,
            display: "flex",
            flexDirection: "column",
            gap: 4,
            borderRadius: 6,
            border: `1px solid ${border}`,
            background: bgSecondary,
            boxShadow: "0 8px 24px rgba(0,0,0,0.18)",
          }}
        >
          {props.options.length > 0 ? (
            <Select
              value={props.filename}
              onChange={props.onChange}
              options={[{ value: "", label: "从已有图选择…" }, ...props.options.map((option) => ({ value: option, label: option }))]}
            />
          ) : (
            <span style={{ fontSize: 10, color: textMuted }}>节点定义加载后可选择已有图</span>
          )}
          <span style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <Button disabled={props.disabled || uploading} onClick={pickFile}>
              {uploading ? "上传中…" : props.filename ? "替换" : "上传"}
            </Button>
            {props.filename ? <Button onClick={() => props.onChange("")}>清空</Button> : null}
          </span>
        </div>
      ) : null}

      <input
        ref={fileRef}
        type="file"
        accept="image/*"
        style={{ display: "none" }}
        onChange={handleFile}
      />
    </div>
  );
}

/**
 * LoadImage 的 image 字段声明为 `["COMBO", { image: [...], upload: [...] }]`，
 * 文件清单在 `image` 键下；buildFields 的组合解析只认 `options` 键，取这里才能拿到真实候选。
 */
function loadImageOptions(objectInfo: ObjectInfoMap | null): string[] {
  const schema = objectInfo?.["LoadImage"]?.input?.required?.image;
  if (!Array.isArray(schema) || schema.length < 2) return [];
  const meta = schema[1];
  const list = meta && typeof meta === "object" ? (meta as { image?: unknown }).image : undefined;
  return Array.isArray(list) ? list.map(String) : [];
}
