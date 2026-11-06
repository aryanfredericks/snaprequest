// A gRPC call run by the server for a request tab (`GET /api/v1/ws/grpc`, docs/SERVER.md "gRPC").
import { currentWorkspace, sessionMayHaveEnded } from "../api/client";
import type { TraceEntry } from "../api/types";
import { uid } from "../model/draft";
import type { SocketLike } from "./websocket.svelte";

export interface GrpcMessage {
  id: number;
  atMs: number;
  message: unknown;
}

export interface GrpcStatusInfo {
  code: number;
  name: string;
  message: string;
}

export function grpcUrl(loc: { protocol: string; host: string } = location): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/api/v1/ws/grpc`;
}

export class GrpcCall {
  running = $state(false);
  /** The request side is still open (client streaming): more messages can be sent. */
  streaming = $state(false);
  messages = $state<GrpcMessage[]>([]);
  headers = $state<Record<string, unknown> | null>(null);
  trailers = $state<Record<string, unknown> | null>(null);
  status = $state<GrpcStatusInfo | null>(null);
  durationMs = $state<number | null>(null);
  trace = $state<TraceEntry[]>([]);
  error = $state<string | null>(null);
  /** Messages sent after the start (client streaming). */
  sent = $state(0);
  private socket: SocketLike | null = null;

  constructor(private readonly open: () => SocketLike = () => new WebSocket(grpcUrl()) as unknown as SocketLike) {}

  /** Starts a call; `start` names the request, method and first messages (`end: false` keeps
   * the request stream open). */
  start(start: Record<string, unknown>) {
    this.stop();
    this.running = true;
    this.streaming = start.end === false;
    this.messages = [];
    this.headers = null;
    this.trailers = null;
    this.status = null;
    this.durationMs = null;
    this.trace = [];
    this.error = null;
    this.sent = 0;
    const s = this.open();
    this.socket = s;
    s.onopen = () => s.send(JSON.stringify({ ...start, type: "start", workspace: currentWorkspace() }));
    s.onmessage = (ev) => this.receive(String(ev.data));
    s.onerror = () => {
      if (this.socket === s && this.running && !this.status) {
        sessionMayHaveEnded();
        this.error ??= "cannot connect to the server's WebSocket";
      }
    };
    s.onclose = () => {
      if (this.socket !== s) return;
      this.socket = null;
      this.running = false;
      this.streaming = false;
    };
  }

  send(message: unknown) {
    if (!this.streaming) return;
    this.socket?.send(JSON.stringify({ type: "send", message }));
    this.sent++;
  }

  end() {
    this.socket?.send(JSON.stringify({ type: "end" }));
    this.streaming = false;
  }

  cancel() {
    if (this.running) this.socket?.send(JSON.stringify({ type: "cancel" }));
  }

  /** Drops the connection (closing the tab). */
  stop() {
    const s = this.socket;
    this.socket = null;
    s?.close();
    this.running = false;
    this.streaming = false;
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
      case "headers":
        this.headers = msg.metadata as Record<string, unknown>;
        break;
      case "message":
        this.messages.push({ id: uid(), atMs: msg.at_ms as number, message: msg.message });
        break;
      case "status":
        this.status = msg.status as GrpcStatusInfo;
        this.trailers = msg.trailers as Record<string, unknown>;
        this.durationMs = (msg.duration_ms as number | null) ?? null;
        this.running = false;
        this.streaming = false;
        this.socket?.close();
        break;
      case "error": {
        const e = msg.error as { message: string };
        this.error = e.message;
        // Errors before the call ends it; a bad message while streaming does not.
        if (!this.headers) {
          this.running = false;
          this.streaming = false;
          this.socket?.close();
        }
        break;
      }
    }
  }
}
