// Collection runs: data files for data-driven runs, and reading run reports.
import type { CaptureOutcome, RunIteration, RunStep } from "../api/types";
import type { ShownResponse } from "../components/ResultView.svelte";

/** Parses CSV (header row, RFC 4180 quoting) or JSON (an array of objects) into data rows. */
export function parseData(text: string): Record<string, string>[] {
  const t = text.trim();
  if (!t) return [];
  if (t.startsWith("[") || t.startsWith("{")) {
    const v: unknown = JSON.parse(t);
    if (!Array.isArray(v) || !v.every((r) => r && typeof r === "object" && !Array.isArray(r))) {
      throw new Error("JSON data must be an array of objects");
    }
    return (v as Record<string, unknown>[]).map((r) =>
      Object.fromEntries(Object.entries(r).map(([k, x]) => [k, typeof x === "string" ? x : JSON.stringify(x)])),
    );
  }
  const rows = csv(t);
  const [head, ...body] = rows;
  if (!head || !head.length) return [];
  return body
    .filter((r) => r.some((c) => c !== ""))
    .map((r, i) => {
      if (r.length !== head.length) throw new Error(`CSV row ${i + 2} has ${r.length} fields, the header has ${head.length}`);
      return Object.fromEntries(head.map((h, j) => [h.trim(), r[j] ?? ""]));
    });
}

function csv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === "") quoted = true;
    else if (c === ",") {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else field += c;
  }
  if (quoted) throw new Error("CSV: a quoted field is not closed");
  row.push(field);
  rows.push(row);
  return rows;
}

/** A report step's response in the shape the response view shows. */
export function stepResponse(s: RunStep): ShownResponse | null {
  const r = s.response;
  if (!r) return null;
  const body = r.body ?? "";
  return {
    status: r.status,
    version: r.version ?? "HTTP/1.1",
    url: s.request?.url ?? "",
    headers: r.headers ?? [],
    body: { encoding: "utf8", data: body },
    timings: r.timings ?? {
      dns_ms: null,
      connect_ms: null,
      tls_ms: null,
      send_ms: null,
      wait_ms: null,
      download_ms: null,
      total_ms: r.duration_ms,
    },
    sizes: {
      request_headers: 0,
      request_body: 0,
      response_headers: 0,
      response_body: body.length,
      response_body_decoded: body.length,
    },
    trace: r.trace ?? [],
  };
}

/** Captured values after each step (latest value of each name), for "variables after step N". */
export function capturedAfter(steps: RunStep[], upTo: number): CaptureOutcome[] {
  const out = new Map<string, CaptureOutcome>();
  for (const s of steps) {
    if (s.number > upTo) continue;
    for (const c of s.captures) if (!c.error) out.set(c.name, c);
  }
  return [...out.values()];
}

/** Steps, checks and failures of a report, for the summary line. */
export function reportCounts(iterations: RunIteration[]) {
  let steps = 0;
  let failed = 0;
  let skipped = 0;
  let checks = 0;
  for (const it of iterations) {
    skipped += it.skipped;
    for (const s of it.steps) {
      steps++;
      if (!s.passed) failed++;
      checks += s.assertions.length;
    }
  }
  return { steps, failed, skipped, checks };
}
