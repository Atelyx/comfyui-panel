/**
 * 远程启停：经协作通道让同一协作空间内的另一台机器启停它本机的 ComfyUI。
 *
 * 收发两端放在同一个类里：这是一条协议的两侧，字段与校验共用一套，拆开写容易只改一边而错位。
 * 帧的形状与解析在 `remoteProtocol.ts`。
 *
 * 三条约束决定实现方式：
 * 1. 可达范围 = 同一协作空间。目标只能从 `ctx.collab.peers()` 里选，看不到的机器发不过去。
 * 2. 协作是尽力而为：`sendMessage` 返回「已投递到传输层」，对端在不在、处不处理都无从得知。
 *    因此每条启停命令都要两段回执（受理 + 结果）加超时，界面上的「启动中」不能靠猜。
 * 3. 传输可能重投同一帧。启停会真的拉起进程，重复执行会多出一个抢端口的实例，
 *    所以入站命令按 reqId 去重，重复帧只重放上次回执、不再执行。
 */
import type { AtelyxCtx, CollabMyPeer, CollabPeer } from "../ctx";
import type { ComfySettings } from "../settings";
import type { ComfyRuntime } from "../runtime";
import type { HostController } from "./controller";
import {
  CHANNEL,
  PROTOCOL,
  newRequestId,
  parseInbound,
  pendingKey,
  remoteReply,
  type CommandRequest,
  type Outbound,
  type RemotePhase,
  type Reply,
  type StatusReply,
} from "./remoteProtocol";

/** 状态收集窗口：广播后等回执的时间，局域网内够用。 */
const SCAN_WINDOW_MS = 3000;
/** 等「已受理」回执：对端收到即回，超时说明没响应或没装本插件。 */
const ACCEPT_TIMEOUT_MS = 5000;
/** 等启动结果：对端等待服务就绪最长 180s，留出余量。 */
const START_RESULT_TIMEOUT_MS = 200000;
/** 等停止结果：对端只是一次进程收尾，给足余量即可。 */
const STOP_RESULT_TIMEOUT_MS = 30000;
/** 幂等记录条数：协作是低频操作，留一小段足够挡住重投，不无界增长。 */
const HANDLED_LIMIT = 32;

/** 远程命令的进度。 */
export interface RemoteTask {
  kind: "start" | "stop";
  stage: "sending" | "working" | "done" | "failed";
  message: string;
}

/** 在线成员 + 它对状态请求的回报（未回报说明对端没装或没启用本插件）。 */
export interface RemoteMachine {
  peerId: number;
  nickname: string;
  color: string;
  replied: boolean;
  /** 对端的本机标识：命令的目标匹配用它，不是口令。 */
  tag: string;
  deviceName: string;
  remoteEnabled: boolean;
  phase: RemotePhase;
  /** 对端的监听端口，供本端改连接地址时参考。 */
  port: number;
  /** 非空表示对端当前不受理启动，值是原因。 */
  denyReason: string;
  task: RemoteTask | null;
}

export interface RemoteSnapshot {
  machines: RemoteMachine[];
  scanning: boolean;
  /** 通道不可用之类的整体性问题。 */
  error: string;
}

interface Pending {
  resolve(value: Reply): void;
  reject(error: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

interface HandledEntry {
  accepted: Reply;
  /** 受理后、结果未出时为空（启动等待期间）。 */
  result: Reply | null;
}

export class RemoteControl {
  private readonly ctx: AtelyxCtx;
  private readonly runtime: ComfyRuntime;
  private readonly host: HostController;
  private settings: ComfySettings;
  private readonly listeners = new Set<() => void>();
  private snap: RemoteSnapshot = { machines: [], scanning: false, error: "" };
  /** 每个成员最近一次任务的进度，扫描重建列表时按 peerId 粘回去。 */
  private readonly tasks = new Map<number, RemoteTask>();
  /** 在途请求，键含 peerId 与回执类型，避免回执串台。 */
  private readonly pending = new Map<string, Pending>();
  /** 入站命令的幂等记录：reqId → 已回过的回执。 */
  private readonly handled = new Map<string, HandledEntry>();
  /** 当前状态收集会话；广播后各成员的回执按 peerId 收口到这里。 */
  private collector: { reqId: string; replies: Map<number, StatusReply> } | null = null;
  /** 扫描序号：期间又扫了一次就让新的那次收尾，旧结果不覆盖界面。 */
  private scanSeq = 0;

  constructor(ctx: AtelyxCtx, runtime: ComfyRuntime, host: HostController, settings: ComfySettings) {
    this.ctx = ctx;
    this.runtime = runtime;
    this.host = host;
    this.settings = settings;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): RemoteSnapshot => this.snap;

  applySettings(settings: ComfySettings): void {
    this.settings = settings;
  }

  /** 订阅协作入站消息；返回的撤销函数在插件停用时调用，之后不再受理远程命令。 */
  attach(): () => void {
    const off = this.ctx.events.on("collab:message", (event) => {
      if (event.channel !== CHANNEL) return;
      void this.handleInbound(event.peerId, event.payload);
    });
    return () => {
      off();
      this.collector = null;
      this.abortPending("插件已停用");
    };
  }

  /**
   * 广播状态请求并收口回执，重建机器列表。
   *
   * 用广播而不是逐个单播：本端并不知道谁装了本插件，只能问一遍，回不回的差别就是
   * 「装了并能远程」与「没装/没启用」。低频操作（打开列表、命令结束后各一次），不轮询。
   */
  async scan(): Promise<void> {
    const seq = ++this.scanSeq;
    const peers = this.safePeers();
    const reqId = newRequestId();
    const replies = new Map<number, StatusReply>();
    this.collector = { reqId, replies };
    this.emit({ scanning: true, error: "" });
    const delivered = this.sendAll({ v: PROTOCOL, kind: "status", reqId });
    if (delivered) await delay(SCAN_WINDOW_MS);
    if (seq !== this.scanSeq) return;
    this.collector = null;
    this.forgetAbsent(peers);
    this.emit({
      machines: peers.map((peer) => buildMachine(peer, replies.get(peer.peerId) ?? null, this.tasks)),
      scanning: false,
      error: this.channelError(delivered),
    });
  }

  /** 状态请求发不出去时的原因：先分辨「宿主协作能力面缺失」与「能力面在但通道未连接」。 */
  private channelError(delivered: boolean): string {
    if (delivered) return "";
    let me: CollabMyPeer | null = null;
    try {
      me = this.ctx.collab.myPeer();
    } catch {
      me = null;
    }
    // 意愿声明面与协作连接判定同批加入：缺它说明宿主不会为插件面板建立协作连接
    if (!me || !this.ctx.collab.acquire) {
      return "当前 Atelyx 版本不支持插件的协作通道，请升级 Atelyx 后使用远程启停";
    }
    if (me.peerId === null) {
      return "协作通道未连接：请确认 Atelyx 已开启协作、当前打开的是协作空间内的仓库，稍候重试";
    }
    return "协作通道暂不可用，请稍候重试";
  }

  /** 启动对端；结果一律折进任务状态，不向调用方抛错。 */
  startRemote(machine: RemoteMachine): Promise<void> {
    return this.request("start", machine, START_RESULT_TIMEOUT_MS);
  }

  /** 停止对端。 */
  stopRemote(machine: RemoteMachine): Promise<void> {
    return this.request("stop", machine, STOP_RESULT_TIMEOUT_MS);
  }

  private emit(patch: Partial<RemoteSnapshot>): void {
    this.snap = { ...this.snap, ...patch };
    for (const listener of this.listeners) listener();
  }

  private async request(kind: "start" | "stop", machine: RemoteMachine, resultTimeoutMs: number): Promise<void> {
    const peerId = machine.peerId;
    const reqId = newRequestId();
    try {
      this.setTask(peerId, { kind, stage: "sending", message: "正在发送…" });
      if (!this.sendTo(peerId, { v: PROTOCOL, kind, reqId, target: machine.tag })) {
        this.setTask(peerId, { kind, stage: "failed", message: "协作通道未连接，命令未发出" });
        return;
      }
      const ack = await this.awaitReply(
        pendingKey(peerId, reqId, "accepted"),
        ACCEPT_TIMEOUT_MS,
        "对方未受理（未开启远程启动或已离线）",
      );
      if (!ack.ok) {
        this.setTask(peerId, { kind, stage: "failed", message: ack.message });
        return;
      }
      this.setTask(peerId, { kind, stage: "working", message: ack.message });
      const outcome = await this.awaitReply(
        pendingKey(peerId, reqId, "result"),
        resultTimeoutMs,
        "对方未回报结果，请到对方界面查看",
      );
      this.setTask(peerId, { kind, stage: outcome.ok ? "done" : "failed", message: outcome.message });
    } catch (err) {
      this.setTask(peerId, { kind, stage: "failed", message: describe(err) });
    } finally {
      // 不论成败都重新问一遍：对端的状态可能随这次命令变了（或已经掉线）
      void this.scan();
    }
  }

  private setTask(peerId: number, task: RemoteTask): void {
    this.tasks.set(peerId, task);
    this.emit({
      machines: this.snap.machines.map((machine) => (machine.peerId === peerId ? { ...machine, task } : machine)),
    });
  }

  private awaitReply(key: string, timeoutMs: number, onTimeout: string): Promise<Reply> {
    return new Promise<Reply>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(key);
        reject(new Error(onTimeout));
      }, timeoutMs);
      this.pending.set(key, { resolve, reject, timer });
    });
  }

  private abortPending(message: string): void {
    for (const waiter of this.pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(message));
    }
    this.pending.clear();
  }

  private async handleInbound(peerId: number, raw: unknown): Promise<void> {
    const message = parseInbound(raw);
    if (!message) return;
    if (message.kind === "status") {
      this.sendTo(peerId, this.statusReply(message.reqId));
      return;
    }
    if (message.kind === "status-reply") {
      const collector = this.collector;
      if (collector && collector.reqId === message.reqId) collector.replies.set(peerId, message);
      return;
    }
    if (message.kind === "accepted" || message.kind === "result") {
      this.settle(peerId, message);
      return;
    }
    // 剩下的只可能是命令：Outbound 是闭集，上面已覆盖其余四种
    if (message.kind === "start" || message.kind === "stop") {
      await this.handleCommand(peerId, message);
    }
  }

  private settle(peerId: number, message: Reply): void {
    const key = pendingKey(peerId, message.reqId, message.kind);
    const waiter = this.pending.get(key);
    // 不属于在途请求的回执（已超时、或插件别处发起的）直接丢弃
    if (!waiter) return;
    clearTimeout(waiter.timer);
    this.pending.delete(key);
    waiter.resolve(message);
  }

  private statusReply(reqId: string): StatusReply {
    const me = this.myPeer();
    return {
      v: PROTOCOL,
      kind: "status-reply",
      reqId,
      tag: this.myTag(me),
      deviceName: me?.deviceName ?? "",
      remoteEnabled: this.settings.remoteEnabled,
      phase: this.phase(),
      port: this.settings.port,
      denyReason: this.denyStart(),
    };
  }

  /** 本机 ComfyUI 状态：已连上还分「插件托管」与「外部启动」，后者停止帮不上忙。 */
  private phase(): RemotePhase {
    if (this.runtime.getSnapshot().channel === "direct") {
      return this.host.getSnapshot().running ? "ready" : "external";
    }
    return this.host.getSnapshot().starting ? "starting" : "offline";
  }

  /** 本机不受理启动的原因；空串表示可以受理。停止不受这几个条件限制，见 handleStop。 */
  private denyStart(): string {
    if (!this.settings.remoteEnabled) return "未开启「允许远程启动」";
    if (this.settings.processMode !== "managed") return "接管方式为「外部（仅检测）」，不托管进程";
    if (!this.settings.comfyDir.trim()) return "未配置 ComfyUI 目录";
    return "";
  }

  private async handleCommand(peerId: number, message: CommandRequest): Promise<void> {
    // 标识不符 = 这条命令不是给本机的（单播已按 peerId 定向，这里兜住投递错位）
    if (message.target !== this.myTag(this.myPeer())) return;
    const replay = this.handled.get(message.reqId);
    // 重投的帧只重放回执：再执行一次会多个进程抢同一个端口
    if (replay) {
      this.sendTo(peerId, replay.accepted);
      if (replay.result) this.sendTo(peerId, replay.result);
      return;
    }
    try {
      if (message.kind === "start") await this.handleStart(peerId, message.reqId);
      else await this.handleStop(peerId, message.reqId);
    } catch (err) {
      // 启停走外部 I/O，异常不能往上抛——调用方是 void 的，抛出去发起端会一直等不到结果
      this.settleNow(peerId, message.reqId, false, `对端处理失败：${describe(err)}`);
    }
  }
  private async handleStart(peerId: number, reqId: string): Promise<void> {
    const denial = this.denyStart();
    if (denial) return this.settleNow(peerId, reqId, false, denial);
    const phase = this.phase();
    if (phase === "ready") return this.settleNow(peerId, reqId, true, "本机 ComfyUI 已就绪，无需启动");
    if (phase === "external") return this.settleNow(peerId, reqId, true, "本机 ComfyUI 已在运行（由外部启动）");
    if (phase === "starting") return this.settleNow(peerId, reqId, false, "本机正在启动中，请稍后重试");
    // 进程还在但服务没起来（上一次启动没等到就绪）：再启动也起不来第二个，如实说明
    if (this.host.getSnapshot().running) {
      return this.settleNow(peerId, reqId, false, "本机已有插件托管的进程但服务未就绪，请在对方界面查看运行日志");
    }

    // 就绪可能要几十秒：先回受理，发起端才能区分「对方没响应」与「对方在启动」
    const ack = remoteReply("accepted", reqId, true, "对方已受理，正在启动…", phase);
    // 受理后先占位：启动等待期间若同一帧被重投，走上面的重放分支而不会第二次拉起进程
    this.remember(reqId, ack, null);
    this.sendTo(peerId, ack);

    const ready = await this.host.start(this.runtime, {
      origin: `远程启动（来自 ${this.peerLabel(peerId)}）`,
    });
    const message = ready
      ? "已启动并就绪"
      : this.host.getSnapshot().error || "启动后服务未就绪，请在对方界面查看运行日志";
    const outcome = remoteReply("result", reqId, ready, message, this.phase());
    this.remember(reqId, ack, outcome);
    this.sendTo(peerId, outcome);
  }

  private async handleStop(peerId: number, reqId: string): Promise<void> {
    // 停止不看「允许远程启动」开关：能被远程拉起就得能被远程收掉，
    // 否则对端关掉开关后，它启动的进程就没人能收尾了。外部启动的进程不是插件拉起的，拒绝。
    if (!this.host.getSnapshot().running) {
      const message =
        this.runtime.getSnapshot().channel === "direct"
          ? "本机 ComfyUI 由外部启动，插件无法停止"
          : "本机 ComfyUI 未在运行";
      return this.settleNow(peerId, reqId, false, message);
    }
    const ack = remoteReply("accepted", reqId, true, "对方已受理，正在停止…", this.phase());
    this.sendTo(peerId, ack);
    await this.host.stop();
    const after = this.host.getSnapshot();
    const ok = !after.running && !after.error;
    const message = ok ? "已停止" : after.error || "停止未完成，请在对方界面查看";
    const outcome = remoteReply("result", reqId, ok, message, this.phase());
    this.remember(reqId, ack, outcome);
    this.sendTo(peerId, outcome);
  }

  /** 当场定论的命令（拒绝或无需动作）：受理与结果一次发完，不留中间态。 */
  private settleNow(peerId: number, reqId: string, ok: boolean, message: string): void {
    const phase = this.phase();
    const ack = remoteReply("accepted", reqId, ok, message, phase);
    const outcome = remoteReply("result", reqId, ok, message, phase);
    this.remember(reqId, ack, outcome);
    this.sendTo(peerId, ack);
    this.sendTo(peerId, outcome);
  }

  private remember(reqId: string, accepted: Reply, result: Reply | null): void {
    this.handled.set(reqId, { accepted, result });
    this.trimHandled();
  }

  private trimHandled(): void {
    while (this.handled.size > HANDLED_LIMIT) {
      const oldest = this.handled.keys().next();
      if (oldest.done) return;
      this.handled.delete(oldest.value);
    }
  }

  /** 已不在房间的成员不必再留任务记录。 */
  private forgetAbsent(peers: readonly CollabPeer[]): void {
    const live = new Set(peers.map((peer) => peer.peerId));
    for (const peerId of [...this.tasks.keys()]) {
      if (!live.has(peerId)) this.tasks.delete(peerId);
    }
  }

  /** 本机标识：留空回落到宿主的设备名，保证「目标匹配」始终有个可比对的值。 */
  private myTag(me: CollabMyPeer | null): string {
    const configured = this.settings.remoteTag.trim();
    if (configured) return configured;
    return me?.deviceName || me?.nickname || "未命名机器";
  }

  /** 日志里标明发起方，便于用户事后分辨是谁启动了本机进程。 */
  private peerLabel(peerId: number): string {
    const peer = this.safePeers().find((item) => item.peerId === peerId);
    return peer?.nickname || peer?.deviceName || `成员 ${peerId}`;
  }

  private myPeer(): CollabMyPeer | null {
    try {
      return this.ctx.collab.myPeer();
    } catch {
      return null;
    }
  }

  /** 协作能力未接线或不在协作空间时读取会抛错，这里一律当作「同一协作空间里没别人」。 */
  private safePeers(): CollabPeer[] {
    try {
      return this.ctx.collab.peers();
    } catch {
      return [];
    }
  }

  private sendTo(peerId: number, message: Outbound): boolean {
    try {
      return this.ctx.collab.sendMessage(CHANNEL, message, { to: peerId });
    } catch {
      // 协作能力未接线：消息没发出，由调用方给出可见状态
      return false;
    }
  }

  private sendAll(message: Outbound): boolean {
    try {
      return this.ctx.collab.sendMessage(CHANNEL, message);
    } catch {
      return false;
    }
  }
}

/** 未回报状态的成员也要进列表，故按 peer 建条目、回执缺省时如实标注「未回报」。 */
function buildMachine(
  peer: CollabPeer,
  reply: StatusReply | null,
  tasks: ReadonlyMap<number, RemoteTask>,
): RemoteMachine {
  return {
    peerId: peer.peerId,
    nickname: peer.nickname,
    color: peer.color,
    replied: reply !== null,
    tag: reply?.tag ?? "",
    deviceName: reply?.deviceName || peer.deviceName,
    remoteEnabled: reply?.remoteEnabled ?? false,
    phase: reply?.phase ?? "offline",
    port: reply?.port ?? 0,
    denyReason: reply?.denyReason ?? "",
    task: tasks.get(peer.peerId) ?? null,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
