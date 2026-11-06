import { describe, expect, it } from "vitest";
import { draftFromFile, draftToFile, isGrpc, isWebSocket } from "../model/draft";
import { GrpcCall } from "./grpc.svelte";
import type { SocketLike } from "./websocket.svelte";

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
    this.onclose?.({});
  }
  push(msg: unknown) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

describe("GrpcCall", () => {
  it("streams messages and ends with a status", () => {
    const fake = new FakeSocket();
    const c = new GrpcCall(() => fake);
    c.start({ request: { method: "GET", url: "grpc://x" }, method: "a.B/C", messages: ["{}"], end: false });
    fake.onopen?.({});
    expect(fake.sent[0]).toMatchObject({ type: "start", method: "a.B/C", end: false, workspace: "default" });
    expect(c.running && c.streaming).toBe(true);
    fake.push({ type: "headers", metadata: { "x-a": "1" } });
    fake.push({ type: "message", at_ms: 3, message: { item: "A" } });
    c.send({ item: "b" });
    c.end();
    expect(fake.sent.slice(1)).toEqual([{ type: "send", message: { item: "b" } }, { type: "end" }]);
    fake.push({ type: "status", status: { code: 0, name: "OK", message: "" }, trailers: {}, duration_ms: 9 });
    expect(c.running).toBe(false);
    expect(c.messages).toHaveLength(1);
    expect(c.status?.name).toBe("OK");
    expect(c.durationMs).toBe(9);
  });

  it("an error before the call ends it", () => {
    const fake = new FakeSocket();
    const c = new GrpcCall(() => fake);
    c.start({ request: { method: "GET", url: "grpc://x" } });
    fake.onopen?.({});
    fake.push({ type: "error", error: { kind: "invalid", message: "no service" } });
    expect(c.running).toBe(false);
    expect(c.error).toBe("no service");
  });
});

describe("gRPC drafts", () => {
  it("are recognized and keep their table, without a method", () => {
    const d = draftFromFile({ url: "{{host}}", grpc: { method: "a.B/C", message: '{"id": 1}' } });
    expect(isGrpc(d)).toBe(true);
    expect(isWebSocket(d)).toBe(false);
    d.method = "POST";
    expect(draftToFile(d)).toEqual({ url: "{{host}}", grpc: { method: "a.B/C", message: '{"id": 1}' } });
    expect(isGrpc(draftFromFile({ url: "grpcs://api:443" }))).toBe(true);
  });
});
