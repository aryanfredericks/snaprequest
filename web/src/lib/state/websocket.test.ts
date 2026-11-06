import { describe, expect, it } from "vitest";
import { messageSize, messageText, WsSession, type SocketLike } from "./websocket.svelte";

class FakeSocket implements SocketLike {
  readyState = 0;
  sent: unknown[] = [];
  closed = false;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closed = true;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(msg: unknown) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

describe("WsSession", () => {
  it("connects, logs messages and closes", () => {
    const fake = new FakeSocket();
    const s = new WsSession(() => fake);
    s.connect({ request: { method: "GET", url: "ws://x" } });
    expect(s.status).toBe("connecting");
    fake.open();
    expect(fake.sent[0]).toMatchObject({ type: "connect", request: { url: "ws://x" }, workspace: "default" });
    fake.receive({ type: "trace", entry: { at_ms: 1, event: "info", message: "x" } });
    fake.receive({ type: "open", handshake: { status: 101 }, protocol: "chat", messages: ["join"] });
    expect(s.status).toBe("open");
    expect(s.protocol).toBe("chat");
    expect(s.saved).toEqual(["join"]);
    expect(s.trace).toHaveLength(1);

    s.send({ kind: "text", data: "hi" });
    s.sendSaved("join");
    expect(fake.sent.slice(1)).toEqual([
      { type: "send", message: { kind: "text", data: "hi" } },
      { type: "send", name: "join" },
    ]);
    fake.receive({ type: "message", direction: "received", at_ms: 5, message: { kind: "text", data: "hi" } });
    expect(s.log).toHaveLength(1);
    // An error while open (an unknown saved message) keeps the connection.
    fake.receive({ type: "error", error: { kind: "bad_request", message: "no saved message `x`" } });
    expect(s.status).toBe("open");
    expect(s.error).toContain("no saved message");

    s.close();
    expect(fake.sent.at(-1)).toEqual({ type: "close" });
    fake.receive({ type: "closed", by: "server", code: 1000, reason: "" });
    expect(s.status).toBe("closed");
    expect(s.closeInfo).toBe("closed by the server (1000)");
  });

  it("a failed handshake ends with the error and its trace", () => {
    const fake = new FakeSocket();
    const s = new WsSession(() => fake);
    s.connect({ request: { method: "GET", url: "ws://x" } });
    fake.open();
    fake.receive({ type: "error", error: { kind: "execution", message: "answered 404", trace: [{ at_ms: 0, event: "info", message: "t" }] } });
    expect(s.status).toBe("closed");
    expect(s.error).toBe("answered 404");
    expect(s.trace).toHaveLength(1);
  });
});

describe("message display", () => {
  it("shows binary as hex and counts bytes", () => {
    expect(messageText({ kind: "binary", data: "AAEC" })).toBe("00 01 02");
    expect(messageSize({ kind: "binary", data: "AAEC" })).toBe(3);
    expect(messageSize({ kind: "binary", data: "AAE=" })).toBe(2);
    expect(messageSize({ kind: "text", data: "é" })).toBe(2);
    expect(messageText({ kind: "close", code: 1000, reason: "bye" })).toBe("close 1000 bye");
  });
});
