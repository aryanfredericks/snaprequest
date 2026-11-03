import { describe, expect, it } from "vitest";
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
      { request: { method: "GET", url: "http://a" }, type: "execute", id: 1 },
      { collection: "shop", type: "run", id: 2 },
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
});
