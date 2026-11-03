// The debug view's reading of a trace (the same rules as the CLI's waterfall, crates/snapreq-cli
// src/waterfall.rs): the trace is split into hops at redirects and auth challenges; each phase
// ends at its event and starts `duration` earlier.
import type { Header, HttpVersion, RawBytes, TlsInfo, TraceEntry } from "../api/types";
import { formatBytes, formatMs, PHASES } from "./http";

export type PhaseName = "DNS" | "connect" | "TLS" | "send" | "wait" | "download";
export const PHASE_NAMES: readonly PhaseName[] = ["DNS", "connect", "TLS", "send", "wait", "download"];

export interface Segment {
  phase: PhaseName;
  start: number;
  end: number;
}

export interface Hop {
  /** `301 → https://…`, `401 Digest challenge` or `final response`. */
  title: string;
  /** `redirect`, `auth` or `final`. */
  kind: "redirect" | "auth" | "final";
  segments: Segment[];
  entries: TraceEntry[];
  request?: { method: string; url: string; version: HttpVersion; headers: Header[] };
  response?: { status: number; version: HttpVersion; headers: Header[] };
  remote?: string;
  local?: string;
  reused: boolean;
  dns?: { host: string; addrs: string[] };
  tls?: TlsInfo;
  wire?: { sent: RawBytes; received: RawBytes; truncated: boolean };
  /** Response body bytes as received (before decoding). */
  bodyBytes?: number;
}

const emptyHop = (): Hop => ({ title: "final response", kind: "final", segments: [], entries: [], reused: false });

export function hops(trace: TraceEntry[]): Hop[] {
  const out: Hop[] = [emptyHop()];
  for (const e of trace) {
    const hop = out[out.length - 1]!;
    hop.entries.push(e);
    const seg = (phase: PhaseName, d: number) => hop.segments.push({ phase, start: Math.max(0, e.at_ms - d), end: e.at_ms });
    switch (e.event) {
      case "dns_end":
        seg("DNS", e.duration_ms);
        hop.dns = { host: e.host, addrs: e.addrs };
        break;
      case "connect_end":
        seg("connect", e.duration_ms);
        hop.remote = e.addr;
        hop.local = e.local_addr;
        break;
      case "connection_reused":
        hop.reused = true;
        hop.remote = e.addr;
        break;
      case "tls_end":
        seg("TLS", e.duration_ms);
        hop.tls = e.info;
        break;
      case "request_headers":
        hop.request = { method: e.method, url: e.url, version: e.version, headers: e.headers };
        break;
      case "request_sent":
        seg("send", e.duration_ms);
        break;
      case "response_first_byte":
        seg("wait", e.wait_ms);
        break;
      case "response_headers":
        hop.response = { status: e.status, version: e.version, headers: e.headers };
        break;
      case "response_complete":
        seg("download", e.download_ms);
        hop.bodyBytes = e.body_bytes;
        break;
      case "wire":
        hop.wire = { sent: e.sent, received: e.received, truncated: e.truncated };
        break;
      case "redirect":
        hop.title = `${e.status} → ${e.to}`;
        hop.kind = "redirect";
        out.push(emptyHop());
        break;
      case "auth_challenge":
        hop.title = `${e.status} ${e.scheme} challenge`;
        hop.kind = "auth";
        out.push(emptyHop());
        break;
    }
  }
  return out.filter((h) => h.segments.length || h.request || h.entries.some((e) => e.event === "error"));
}

/** The end of the timeline. */
export const totalMs = (trace: TraceEntry[]) => trace.reduce((m, e) => Math.max(m, e.at_ms), 0);

/** Time spent in each phase over all hops. */
export function phaseTotals(hs: Hop[]): Record<PhaseName, number> {
  const t = { DNS: 0, connect: 0, TLS: 0, send: 0, wait: 0, download: 0 } as Record<PhaseName, number>;
  for (const h of hs) for (const s of h.segments) t[s.phase] += s.end - s.start;
  return t;
}

/** A one-line description of a trace event, like the CLI's verbose output. */
export function describe(e: TraceEntry): { mark: "*" | ">" | "<" | "!"; text: string } {
  switch (e.event) {
    case "dns_start":
      return { mark: "*", text: `resolving ${e.host}` };
    case "dns_end":
      return { mark: "*", text: `resolved ${e.host} → ${e.addrs.join(", ")} (${formatMs(e.duration_ms)})` };
    case "connect_start":
      return { mark: "*", text: `connecting to ${e.addr}` };
    case "connect_end":
      return { mark: "*", text: `connected to ${e.addr} from ${e.local_addr} (${formatMs(e.duration_ms)})` };
    case "connection_reused":
      return { mark: "*", text: `reusing the connection to ${e.addr}` };
    case "tls_start":
      return { mark: "*", text: `TLS handshake with ${e.server_name}` };
    case "tls_end":
      return {
        mark: "*",
        text: `${e.info.version} ${e.info.cipher}${e.info.alpn ? `, ALPN ${e.info.alpn}` : ""} (${formatMs(e.duration_ms)})`,
      };
    case "request_headers":
      return { mark: ">", text: `${e.method} ${e.url} ${e.version} (${e.headers.length} headers)` };
    case "request_sent":
      return { mark: ">", text: `request sent, ${formatBytes(e.body_bytes)} body (${formatMs(e.duration_ms)})` };
    case "response_first_byte":
      return { mark: "<", text: `first byte after ${formatMs(e.wait_ms)}` };
    case "response_headers":
      return { mark: "<", text: `${e.version} ${e.status} (${e.headers.length} headers)` };
    case "response_complete":
      return { mark: "<", text: `${formatBytes(e.body_bytes)} body received (${formatMs(e.download_ms)})` };
    case "wire":
      return { mark: "*", text: `wire capture: ${e.truncated ? "shortened" : "complete"}` };
    case "redirect":
      return { mark: "*", text: `${e.status} redirect: ${e.from} → ${e.to}` };
    case "auth_challenge":
      return { mark: "*", text: `${e.status}: ${e.scheme} challenge, answering` };
    case "info":
      return { mark: "*", text: e.message };
    case "error":
      return { mark: "!", text: `${PHASES[e.phase]}: ${e.message}` };
  }
}

/** Days until a certificate's `not_after` (negative when expired), or null if it does not parse. */
export function daysLeft(notAfter: string, now = Date.now()): number | null {
  const t = Date.parse(notAfter);
  return Number.isNaN(t) ? null : Math.floor((t - now) / 86_400_000);
}
