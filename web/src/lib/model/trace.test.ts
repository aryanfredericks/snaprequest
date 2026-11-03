import { describe as group, expect, it } from "vitest";
import type { TraceEntry } from "../api/types";
import { daysLeft, describe, hops, phaseTotals, totalMs } from "./trace";

const wire = { sent: { encoding: "utf8" as const, data: "GET / HTTP/1.1\r\n\r\n" }, received: { encoding: "utf8" as const, data: "HTTP/1.1 302 Found\r\n\r\n" }, truncated: false };

const trace: TraceEntry[] = [
  { at_ms: 0, event: "dns_start", host: "a.test" },
  { at_ms: 5, event: "dns_end", host: "a.test", addrs: ["10.0.0.1"], duration_ms: 5 },
  { at_ms: 5, event: "connect_start", addr: "10.0.0.1:80" },
  { at_ms: 8, event: "connect_end", addr: "10.0.0.1:80", local_addr: "10.0.0.9:5000", duration_ms: 3 },
  { at_ms: 8, event: "request_headers", method: "GET", url: "http://a.test/old", version: "HTTP/1.1", headers: [] },
  { at_ms: 9, event: "request_sent", body_bytes: 0, duration_ms: 1 },
  { at_ms: 20, event: "response_first_byte", wait_ms: 11 },
  { at_ms: 20, event: "response_headers", status: 302, version: "HTTP/1.1", headers: [] },
  { at_ms: 21, event: "response_complete", body_bytes: 0, download_ms: 1 },
  { at_ms: 21, event: "wire", ...wire },
  { at_ms: 21, event: "redirect", status: 302, from: "http://a.test/old", to: "http://a.test/new" },
  { at_ms: 21, event: "connection_reused", addr: "10.0.0.1:80" },
  { at_ms: 21, event: "request_headers", method: "GET", url: "http://a.test/new", version: "HTTP/1.1", headers: [] },
  { at_ms: 22, event: "request_sent", body_bytes: 0, duration_ms: 1 },
  { at_ms: 30, event: "response_first_byte", wait_ms: 8 },
  { at_ms: 30, event: "response_headers", status: 200, version: "HTTP/1.1", headers: [] },
  { at_ms: 34, event: "response_complete", body_bytes: 120, download_ms: 4 },
];

group("trace", () => {
  it("splits hops at redirects and positions phases", () => {
    const hs = hops(trace);
    expect(hs.map((h) => [h.title, h.kind, h.response?.status, h.reused])).toEqual([
      ["302 → http://a.test/new", "redirect", 302, false],
      ["final response", "final", 200, true],
    ]);
    expect(hs[0]!.segments).toEqual([
      { phase: "DNS", start: 0, end: 5 },
      { phase: "connect", start: 5, end: 8 },
      { phase: "send", start: 8, end: 9 },
      { phase: "wait", start: 9, end: 20 },
      { phase: "download", start: 20, end: 21 },
    ]);
    expect(hs[0]!.wire).toEqual(wire);
    expect(hs[1]!.wire).toBeUndefined();
    expect(hs[0]!.remote).toBe("10.0.0.1:80");
    expect(totalMs(trace)).toBe(34);
    expect(phaseTotals(hs)).toEqual({ DNS: 5, connect: 3, TLS: 0, send: 2, wait: 19, download: 5 });
  });

  it("describes events and failures", () => {
    expect(describe(trace[1]!)).toEqual({ mark: "*", text: "resolved a.test → 10.0.0.1 (5.0 ms)" });
    expect(describe({ at_ms: 3, event: "error", phase: "connect", message: "refused" })).toEqual({ mark: "!", text: "TCP connect: refused" });
    const failed = hops([
      { at_ms: 0, event: "connect_start", addr: "127.0.0.1:1" },
      { at_ms: 1, event: "error", phase: "connect", message: "refused" },
    ]);
    expect(failed).toHaveLength(1);
  });

  it("counts certificate days", () => {
    expect(daysLeft("2026-01-11T00:00:00Z", Date.parse("2026-01-01T00:00:00Z"))).toBe(10);
    expect(daysLeft("nope")).toBeNull();
  });
});
