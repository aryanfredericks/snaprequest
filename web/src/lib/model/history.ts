// Turning stored history entries back into editable requests, and helpers for listing and
// comparing them.
import type { CoreBody, Header, HistoryRecord, RawBytes } from "../api/types";
import { rawText } from "../api/types";
import { emptyBody, emptyDraft, part, row, type BodyDraft, type Draft } from "./draft";
import { formatJson } from "./format";
import { header } from "./http";

/** The value history stores instead of credentials. */
export const REDACTED = "<redacted>";

function bodyFromCore(b: CoreBody | null | undefined): BodyDraft {
  const out = emptyBody();
  if (!b) return out;
  out.type = b.type;
  switch (b.type) {
    case "text":
      out.content = b.content;
      out.contentType = b.content_type ?? "";
      break;
    case "json":
      out.content = b.content;
      break;
    case "form":
      out.fields = b.fields.map((f) => row(f.name, f.value, f.enabled !== false));
      break;
    case "multipart":
      out.parts = b.parts.map((p) =>
        p.kind === "file"
          ? part({ name: p.name, kind: "file", file: p.path, filename: p.filename ?? "", contentType: p.content_type ?? "", enabled: p.enabled !== false })
          : part({ name: p.name, kind: "text", value: p.value, enabled: p.enabled !== false }),
      );
      break;
    case "file":
      out.file = b.path;
      out.contentType = b.content_type ?? "";
      break;
    case "graphql":
      out.query = b.query;
      out.variables = b.variables ?? "";
      out.operationName = b.operation_name ?? "";
      break;
  }
  return out;
}

/**
 * A scratch request rebuilt from a stored one. Credentials were never stored: headers whose
 * value was redacted come back disabled, and `redacted` lists them.
 */
export function draftFromHistory(r: HistoryRecord): { draft: Draft; redacted: string[] } {
  const d = emptyDraft(false);
  d.method = r.request.method || r.method;
  d.url = r.request.url || r.url;
  const redacted: string[] = [];
  d.headers = (r.request.headers ?? []).map((h) => {
    const hidden = h.value === REDACTED || h.value.includes(REDACTED);
    if (hidden) redacted.push(h.name);
    return row(h.name, hidden ? "" : h.value, !hidden);
  });
  d.body = bodyFromCore(r.request.body as CoreBody | null | undefined);
  const auth = r.request.auth;
  if (auth && auth !== "none") redacted.push(`${auth} auth`);
  return { draft: d, redacted };
}

/** "3 min ago", "yesterday", or a date. */
export function timeAgo(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 90 * 60) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 20 * 3600) return `${Math.round(s / 3600)} h ago`;
  if (s < 36 * 3600) return "yesterday";
  return new Date(ms).toLocaleDateString();
}

/** A body for comparing: pretty JSON when it is JSON, else the text. */
export function comparableBody(body: RawBytes | null | undefined, headers: Header[] | undefined): string {
  const text = rawText(body);
  const ct = header(headers, "content-type") ?? "";
  return /json/i.test(ct) || /^\s*[[{]/.test(text) ? formatJson(text) : text;
}

/** Headers as sorted `name: value` lines (dates and request ids differ every time; sorting keeps the rest aligned). */
export function comparableHeaders(headers: Header[] | undefined): string {
  return (headers ?? [])
    .map((h) => `${h.name.toLowerCase()}: ${h.value}`)
    .sort()
    .join("\n");
}
