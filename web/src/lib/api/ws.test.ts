import { describe, expect, it } from "vitest";
import { onUnauthorized } from "./client";
import { JobClient, wsUrl, type SocketLike } from "./ws";

class FakeSocket implements SocketLike {
  readyState = 0;
  sent: Record<string, unknown>[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  accept() {
    this.readyState = 1;
    this.onopen?.({});
  }
  push(msg: unknown) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("JobClient", () => {
  it("routes traces and results by job id", async () => {
    const sockets: FakeSocket[] = [];
    const client = new JobClient(() => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    });
    const traces: string[] = [];
    const a = client.execute({ request: { method: "GET", url: "http://a" } }, { onTrace: (e) => traces.push(e.event) });
    sockets[0]!.accept();
    const progress: string[] = [];
    await tick();
    const b = client.run({ collection: "shop" }, { onProgress: (e) => progress.push(`${e.event} ${e.key}`) });
    await tick();
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.sent).toEqual([
      { request: { method: "GET", url: "http://a" }, type: "execute", id: 1, workspace: "default" },
      { collection: "shop", type: "run", id: 2, workspace: "default" },
    ]);
    sockets[0]!.push({ type: "trace", id: 1, entry: { at_ms: 0, event: "dns_start", host: "a" } });
    sockets[0]!.push({ type: "progress", id: 2, event: "request_started", iteration: 1, key: "health" });
    sockets[0]!.push({ type: "result", id: 2, result: { passed: true } });
    sockets[0]!.push({ type: "error", id: 1, error: { kind: "bad_request", message: "nope" } });
    await expect(b).resolves.toEqual({ passed: true });
    await expect(a).rejects.toMatchObject({ kind: "bad_request", message: "nope" });
    expect(traces).toEqual(["dns_start"]);
    expect(progress).toEqual(["request_started health"]);
  });

  it("passes stream events on and can stop a job", async () => {
    const socket = new FakeSocket();
    const client = new JobClient(() => socket);
    const events: string[] = [];
    const control: { stop?: () => void } = {};
    const job = client.execute({ request: { method: "GET", url: "http://a/events" } }, { onEvent: (e) => events.push(e.data), control });
    socket.accept();
    await tick();
    socket.push({ type: "event", id: 1, event: { event: "message", data: "one", at_ms: 3 } });
    control.stop!();
    expect(socket.sent.at(-1)).toEqual({ type: "cancel", id: 1 });
    socket.push({ type: "result", id: 1, result: { events: [{ event: "message", data: "one" }] } });
    await expect(job).resolves.toMatchObject({ events: [{ data: "one" }] });
    expect(events).toEqual(["one"]);
  });

  it("fails pending jobs when the connection drops, then reconnects", async () => {
    const sockets: FakeSocket[] = [];
    const client = new JobClient(() => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    });
    const a = client.execute({});
    sockets[0]!.accept();
    await tick();
    sockets[0]!.close();
    await expect(a).rejects.toMatchObject({ kind: "connection" });
    const b = client.execute({});
    sockets[1]!.accept();
    await tick();
    sockets[1]!.push({ type: "result", id: 2, result: { history_id: 1 } });
    await expect(b).resolves.toEqual({ history_id: 1 });
  });

  it("builds the URL from the page location", () => {
    expect(wsUrl({ protocol: "https:", host: "x:1" })).toBe("wss://x:1/api/v1/ws");
    expect(wsUrl({ protocol: "http:", host: "localhost" })).toBe("ws://localhost/api/v1/ws");
  });

  it("fails pending jobs when the session ends", async () => {
    let ended = 0;
    onUnauthorized(() => ended++);
    const sockets: FakeSocket[] = [];
    const client = new JobClient(() => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    });
    const job = client.execute({ request: { method: "GET", url: "http://a" } });
    sockets[0]!.accept();
    await tick();
    sockets[0]!.push({ type: "error", id: null, error: { kind: "unauthorized", message: "the session has ended" } });
    await expect(job).rejects.toMatchObject({ status: 401, kind: "unauthorized" });
    expect(ended).toBe(1);
    onUnauthorized(() => {});
  });
});
