/**
 * 远程启停的消息形状与编解码。
 *
 * 单独成模块：帧来自网络，这里的解析是整条能力的信任边界——「哪些字段、怎么核对」
 * 集中在一处，改动只影响本文件，不必在服务逻辑里找散落的类型判断。
 */
/** 频道名：本插件实例之间的私有频道。 */
export const CHANNEL = "comfyui.remote";
/** 协议版本：字段不兼容改动时递增，版本不符的消息一律丢弃。 */
export const PROTOCOL = 1;

/**
 * 请求标识长度上限。
 *
 * reqId 会作为幂等记录的键随命令累积，是这些字段里唯一会留存的一项：放任对端塞超长值，
 * 等于给它一个把内存放大的入口。其余字段只做一次比较或展示，不累计，不设限。
 */
const MAX_REQ_ID_LENGTH = 64;

/** 目标机器上 ComfyUI 的状态。 */
export type RemotePhase = "offline" | "starting" | "ready" | "external";

export interface StatusRequest {
  v: number;
  kind: "status";
  reqId: string;
}

export interface StatusReply {
  v: number;
  kind: "status-reply";
  reqId: string;
  tag: string;
  deviceName: string;
  remoteEnabled: boolean;
  phase: RemotePhase;
  port: number;
  denyReason: string;
}

export interface CommandRequest {
  v: number;
  kind: "start" | "stop";
  reqId: string;
  target: string;
}

/** 回执：受理与结果同形，只差 kind——发起端按 kind 配对到对应的等待者。 */
export interface Reply {
  v: number;
  kind: "accepted" | "result";
  reqId: string;
  ok: boolean;
  message: string;
  phase: RemotePhase;
}

/** 本插件发出去的全部帧。 */
export type Outbound = StatusRequest | StatusReply | CommandRequest | Reply;

/**
 * 解析入站帧。载荷来自网络，一律当不可信输入逐字段核对：缺字段或类型不符即丢弃，
 * 不抛错也不补默认值——猜出来的目标标识会把命令执行到错误的机器上。
 */
export function parseInbound(raw: unknown): Outbound | null {
  if (!isRecord(raw)) return null;
  if (raw.v !== PROTOCOL) return null;
  const reqId = raw.reqId;
  if (typeof reqId !== "string" || !reqId || reqId.length > MAX_REQ_ID_LENGTH) return null;
  const kind = raw.kind;
  if (kind === "status") return { v: PROTOCOL, kind, reqId };
  if (kind === "start" || kind === "stop") {
    if (typeof raw.target !== "string" || !raw.target) return null;
    return { v: PROTOCOL, kind, reqId, target: raw.target };
  }
  if (kind === "accepted" || kind === "result") {
    const phase = parsePhase(raw.phase);
    if (phase === null || typeof raw.ok !== "boolean") return null;
    return {
      v: PROTOCOL,
      kind,
      reqId,
      ok: raw.ok,
      message: typeof raw.message === "string" ? raw.message : "",
      phase,
    };
  }
  if (kind === "status-reply") {
    const phase = parsePhase(raw.phase);
    const port = raw.port;
    if (phase === null) return null;
    if (typeof port !== "number" || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
    if (typeof raw.tag !== "string" || typeof raw.deviceName !== "string") return null;
    if (typeof raw.remoteEnabled !== "boolean" || typeof raw.denyReason !== "string") return null;
    return {
      v: PROTOCOL,
      kind,
      reqId,
      tag: raw.tag,
      deviceName: raw.deviceName,
      remoteEnabled: raw.remoteEnabled,
      phase,
      port,
      denyReason: raw.denyReason,
    };
  }
  return null;
}

export function remoteReply(
  kind: Reply["kind"],
  reqId: string,
  ok: boolean,
  message: string,
  phase: RemotePhase,
): Reply {
  return { v: PROTOCOL, kind, reqId, ok, message, phase };
}

/** 回执必须来自目标成员且配对到同一次请求，键里带上 peerId 与类型才不会串台。 */
export function pendingKey(peerId: number, reqId: string, kind: Reply["kind"]): string {
  return `${peerId}|${reqId}|${kind}`;
}

/** 请求标识只需在本端会话内唯一，用于把回执配回对应的等待者。 */
export function newRequestId(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parsePhase(value: unknown): RemotePhase | null {
  return value === "offline" || value === "starting" || value === "ready" || value === "external" ? value : null;
}
