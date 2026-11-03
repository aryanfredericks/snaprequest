// Client for `GET /api/v1/ws` (docs/SERVER.md, "Live traces and run progress"): one JSON message
// per job; the server answers with `trace` and `progress` events and then a `result` or `error`.
import type { ExecuteResult, RunReport, TraceEntry } from "./types";
import { ApiError } from "./client";

export type ProgressEvent =
  | { event: "request_started"; iteration: number; key: string }
  | {
      event: "request_done";
      iteration: number;
      key: string;
      passed: boolean;
      status: number | null;
      duration_ms: number | null;
    };

export interface JobHandlers {
  onTrace?: (entry: TraceEntry) => void;
  onProgress?: (event: ProgressEvent) => void;
}

type Message =
  | { type: "trace"; id: number; entry: TraceEntry }
  | ({ type: "progress"; id: number } & ProgressEvent)
  | { type: "result"; id: number; result: unknown }
  | { type: "error"; id: number; error: { kind: string; message: string } };

interface Pending extends JobHandlers {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

/** The subset of the browser's WebSocket this client uses (tests pass a fake). */
export interface SocketLike {
  readyState: number;
  send(data: string): void;
  close(): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export function wsUrl(loc: { protocol: string; host: string } = location): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/api/v1/ws`;
}

/** Sends jobs over one lazily opened connection; reconnects when it was closed. */
export class JobClient {
  private socket: SocketLike | null = null;
  private opening: Promise<SocketLike> | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;

  constructor(private readonly open: () => SocketLike = () => new WebSocket(wsUrl()) as unknown as SocketLike) {}

  /** Executes a request (the body of `POST /requests/execute`). */
  execute(body: Record<string, unknown>, handlers: JobHandlers = {}): Promise<ExecuteResult> {
    return this.job("execute", body, handlers) as Promise<ExecuteResult>;
  }

  /** Runs a collection, folder or flow (the body of `POST /runs`). */
  run(body: Record<string, unknown>, handlers: JobHandlers = {}): Promise<RunReport> {
    return this.job("run", body, handlers) as Promise<RunReport>;
  }

  close() {
    this.socket?.close();
  }

  private async job(type: string, body: Record<string, unknown>, handlers: JobHandlers): Promise<unknown> {
    const socket = await this.connect();
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { ...handlers, resolve, reject });
      socket.send(JSON.stringify({ ...body, type, id }));
    });
  }

  private connect(): Promise<SocketLike> {
    if (this.socket && this.socket.readyState === 1) return Promise.resolve(this.socket);
    if (this.opening) return this.opening;
    this.opening = new Promise((resolve, reject) => {
      const s = this.open();
      s.onopen = () => {
        this.socket = s;
        this.opening = null;
        resolve(s);
      };
      s.onerror = () => {
        if (this.opening) {
          this.opening = null;
          reject(new ApiError(0, "connection", "cannot connect to the server's WebSocket"));
        }
      };
      s.onclose = () => {
        if (this.socket === s) this.socket = null;
        for (const [id, p] of this.pending) {
          p.reject(new ApiError(0, "connection", "the connection to the server was lost"));
          this.pending.delete(id);
        }
      };
      s.onmessage = (ev) => this.receive(ev.data);
    });
    return this.opening;
  }

  private receive(data: unknown) {
    let msg: Message;
    try {
      msg = JSON.parse(String(data)) as Message;
    } catch {
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    switch (msg.type) {
      case "trace":
        p.onTrace?.(msg.entry);
        break;
      case "progress": {
        const { type: _t, id: _i, ...event } = msg;
        p.onProgress?.(event as ProgressEvent);
        break;
      }
      case "result":
        this.pending.delete(msg.id);
        p.resolve(msg.result);
        break;
      case "error":
        this.pending.delete(msg.id);
        p.reject(new ApiError(400, msg.error.kind, msg.error.message));
        break;
    }
  }
}

export const jobs = new JobClient();
