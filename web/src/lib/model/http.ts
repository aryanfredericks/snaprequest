// HTTP helpers for showing responses.
import type { Header, Phase } from "../api/types";

/** What was happening in a phase (as the CLI says it). */
export const PHASES: Record<Phase, string> = {
  prepare: "preparing the request",
  dns: "DNS lookup",
  connect: "TCP connect",
  tls: "TLS handshake",
  send: "sending the request",
  wait: "waiting for the response",
  download: "downloading the response",
  redirect: "following a redirect",
  auth: "getting an access token",
};

const STATUS: Record<number, string> = {
  100: "Continue", 101: "Switching Protocols", 102: "Processing", 103: "Early Hints",
  200: "OK", 201: "Created", 202: "Accepted", 203: "Non-Authoritative Information", 204: "No Content",
  205: "Reset Content", 206: "Partial Content", 207: "Multi-Status", 208: "Already Reported", 226: "IM Used",
  300: "Multiple Choices", 301: "Moved Permanently", 302: "Found", 303: "See Other", 304: "Not Modified",
  307: "Temporary Redirect", 308: "Permanent Redirect",
  400: "Bad Request", 401: "Unauthorized", 402: "Payment Required", 403: "Forbidden", 404: "Not Found",
  405: "Method Not Allowed", 406: "Not Acceptable", 407: "Proxy Authentication Required", 408: "Request Timeout",
  409: "Conflict", 410: "Gone", 411: "Length Required", 412: "Precondition Failed", 413: "Content Too Large",
  414: "URI Too Long", 415: "Unsupported Media Type", 416: "Range Not Satisfiable", 417: "Expectation Failed",
  418: "I'm a teapot", 421: "Misdirected Request", 422: "Unprocessable Content", 423: "Locked",
  424: "Failed Dependency", 425: "Too Early", 426: "Upgrade Required", 428: "Precondition Required",
  429: "Too Many Requests", 431: "Request Header Fields Too Large", 451: "Unavailable For Legal Reasons",
  500: "Internal Server Error", 501: "Not Implemented", 502: "Bad Gateway", 503: "Service Unavailable",
  504: "Gateway Timeout", 505: "HTTP Version Not Supported", 506: "Variant Also Negotiates",
  507: "Insufficient Storage", 508: "Loop Detected", 510: "Not Extended", 511: "Network Authentication Required",
};

export const statusText = (code: number) => STATUS[code] ?? "";

/** `s-2` … `s-5`: the CSS class for a status code's colour. */
export const statusClass = (code: number | null | undefined) => (code ? `s-${Math.floor(code / 100)}` : "s-err");

export function header(headers: Header[] | undefined, name: string): string | undefined {
  return headers?.find((h) => h.name.toLowerCase() === name.toLowerCase())?.value;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "–";
  if (ms < 1) return `${ms.toFixed(2)} ms`;
  if (ms < 1000) return `${ms.toFixed(ms < 10 ? 1 : 0)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

export interface Cookie {
  name: string;
  value: string;
  domain?: string;
  path?: string;
  expires?: string;
  maxAge?: string;
  secure: boolean;
  httpOnly: boolean;
  sameSite?: string;
  partitioned: boolean;
}

/** Parses one `Set-Cookie` header value (RFC 6265, leniently). */
export function parseSetCookie(value: string): Cookie | null {
  const [pair, ...attrs] = value.split(";");
  const eq = pair!.indexOf("=");
  if (eq <= 0) return null;
  const c: Cookie = {
    name: pair!.slice(0, eq).trim(),
    value: pair!.slice(eq + 1).trim(),
    secure: false,
    httpOnly: false,
    partitioned: false,
  };
  for (const a of attrs) {
    const i = a.indexOf("=");
    const k = (i < 0 ? a : a.slice(0, i)).trim().toLowerCase();
    const v = i < 0 ? "" : a.slice(i + 1).trim();
    if (k === "domain") c.domain = v;
    else if (k === "path") c.path = v;
    else if (k === "expires") c.expires = v;
    else if (k === "max-age") c.maxAge = v;
    else if (k === "secure") c.secure = true;
    else if (k === "httponly") c.httpOnly = true;
    else if (k === "samesite") c.sameSite = v;
    else if (k === "partitioned") c.partitioned = true;
  }
  return c;
}

export function responseCookies(headers: Header[] | undefined): Cookie[] {
  return (headers ?? [])
    .filter((h) => h.name.toLowerCase() === "set-cookie")
    .map((h) => parseSetCookie(h.value))
    .filter((c): c is Cookie => c !== null);
}

/** Whether a body is text worth showing in an editor (by content type, else by its bytes). */
export function isTextual(contentType: string | undefined, bytes: Uint8Array, utf8: boolean): boolean {
  const ct = (contentType ?? "").toLowerCase();
  if (/^(image|audio|video|font)\/|octet-stream|application\/(pdf|zip|gzip|wasm|x-protobuf|protobuf)/.test(ct)) return false;
  if (ct.startsWith("text/") || /json|xml|javascript|ecmascript|x-www-form-urlencoded|yaml|csv|graphql/.test(ct)) return true;
  if (!utf8) return false;
  // No (useful) content type: text unless it has control characters.
  const sample = bytes.subarray(0, 4096);
  return !sample.some((b) => b < 9 || (b > 13 && b < 32 && b !== 27));
}

/** One line of a hex dump: offset, 16 bytes in hex, and their printable characters. */
export function hexRow(bytes: Uint8Array, row: number): { offset: string; hex: string; text: string } {
  const start = row * 16;
  const chunk = bytes.subarray(start, start + 16);
  const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0"));
  return {
    offset: start.toString(16).padStart(8, "0"),
    hex: [hex.slice(0, 8).join(" "), hex.slice(8).join(" ")].join("  ").padEnd(16 * 3, " "),
    text: Array.from(chunk, (b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : ".")).join(""),
  };
}
