// A WebSocket connection opened by the server for a request tab (`GET /api/v1/ws/websocket`,
// docs/SERVER.md "WebSocket sessions"): the server connects to the target and relays messages.
import { currentWorkspace, sessionMayHaveEnded } from "../api/client";
import type { CoreResponse, TraceEntry, WsMessage } from "../api/types";
import { uid } from "../model/draft";

export type WsStatus = "idle" | "connecting" | "open" | "closed";

export interface WsLogEntry {
  id: number;
  direction: "sent" | "received";
  /** Milliseconds since the connection opened. */
  atMs: number;
  message: WsMessage;
}

/** The subset of the browser's WebSocket used here (tests pass a fake). */
export interface SocketLike {
  readyState: number;
  send(data: string): void;
  close(): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export function sessionUrl(loc: { protocol: string; host: string } = location): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/api/v1/ws/websocket`;
}

/** Most messages kept in the log; older ones are dropped. */
export const LOG_LIMIT = 5000;

/** Payload size in bytes (binary data is base64). */
export function messageSize(m: WsMessage): number {
  switch (m.kind) {
    case "text":
      return new TextEncoder().encode(m.data).length;
    case "close":
      return (m.reason ?? "").length + 2;
    default:
      return Math.floor((m.data.length * 3) / 4) - (m.data.endsWith("==") ? 2 : m.data.endsWith("=") ? 1 : 0);
  }
}

/** Text of a message for the log, binary shown as hex. */
export function messageText(m: WsMessage): string {
  switch (m.kind) {
    case "text":
      return m.data;
    case "close":
      return `close ${m.code ?? "-"}${m.reason ? ` ${m.reason}` : ""}`;
    default: {
      let bytes: number[] = [];
      try {
        bytes = Array.from(atob(m.data), (c) => c.charCodeAt(0));
      } catch {
        return m.data;
      }
      const hex = bytes.slice(0, 256).map((b) => b.toString(16).padStart(2, "0")).join(" ");
      return bytes.length > 256 ? `${hex} …` : hex;
    }
  }
}

export class WsSession {
  status = $state<WsStatus>("idle");
  log = $state<WsLogEntry[]>([]);
  /** Trace events of the handshake. */
  trace = $state<TraceEntry[]>([]);
  handshake = $state<CoreResponse | null>(null);
  protocol = $state<string | null>(null);
  /** Names of the request file's saved messages (resolved by the server). */
  saved = $state<string[]>([]);
  error = $state<string | null>(null);
  closeInfo = $state<string | null>(null);
  private socket: SocketLike | null = null;

  constructor(private readonly open: () => SocketLike = () => new WebSocket(sessionUrl()) as unknown as SocketLike) {}

  /** Opens a session; `connect` is the first message (`request`, or `collection` and `key`). */
  connect(connect: Record<string, unknown>) {
    this.disconnect();
    this.status = "connecting";
    this.trace = [];
    this.handshake = null;
    this.protocol = null;
    this.saved = [];
    this.error = null;
    this.closeInfo = null;
    const s = this.open();
    this.socket = s;
    s.onopen = () => s.send(JSON.stringify({ ...connect, type: "connect", workspace: currentWorkspace() }));
    s.onmessage = (ev) => this.receive(String(ev.data));
    s.onerror = () => {
      if (this.socket !== s) return;
      if (this.status === "connecting") {
        sessionMayHaveEnded();
        this.error = "cannot connect to the server's WebSocket";
      }
    };
    s.onclose = () => {
      if (this.socket !== s) return;
      this.socket = null;
      if (this.status !== "closed") {
        this.status = "closed";
        this.closeInfo ??= "the connection to the server ended";
      }
    };
  }

  send(message: WsMessage) {
    this.socket?.send(JSON.stringify({ type: "send", message }));
  }

  sendSaved(name: string) {
    this.socket?.send(JSON.stringify({ type: "send", name }));
  }

  close() {
    if (this.status === "open") this.socket?.send(JSON.stringify({ type: "close" }));
    else this.disconnect();
  }

  /** Drops the connection without the closing handshake (closing the tab). */
  disconnect() {
    const s = this.socket;
    this.socket = null;
    s?.close();
    if (this.status === "open" || this.status === "connecting") this.status = "closed";
  }

  clear() {
    this.log = [];
  }

  private receive(text: string) {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(text) as Record<string, unknown>;
    } catch {
      return;
    }
    switch (msg.type) {
      case "trace":
        this.trace.push(msg.entry as TraceEntry);
        break;
      case "open":
        this.status = "open";
        this.handshake = msg.handshake as CoreResponse;
        this.protocol = (msg.protocol as string | null) ?? null;
        this.saved = (msg.messages as string[]) ?? [];
        break;
      case "message": {
        const entry: WsLogEntry = {
          id: uid(),
          direction: msg.direction as WsLogEntry["direction"],
          atMs: msg.at_ms as number,
          message: msg.message as WsMessage,
        };
        this.log.push(entry);
        if (this.log.length > LOG_LIMIT) this.log.splice(0, this.log.length - LOG_LIMIT);
        break;
      }
      case "closed": {
        const by = msg.by === "client" ? "closed" : "closed by the server";
        const code = msg.code === null || msg.code === undefined ? "" : ` (${String(msg.code)}${msg.reason ? ` ${String(msg.reason)}` : ""})`;
        this.closeInfo = `${by}${code}`;
        this.status = "closed";
        this.socket?.close();
        break;
      }
      case "error": {
        const e = msg.error as { message: string; trace?: TraceEntry[] };
        this.error = e.message;
        if (e.trace) this.trace = e.trace;
        if (this.status !== "open") {
          this.status = "closed";
          this.socket?.close();
        }
        break;
      }
    }
  }
}
