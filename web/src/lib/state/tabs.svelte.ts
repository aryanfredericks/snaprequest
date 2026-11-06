// The kinds of tabs the main area shows.
import type {
  ExecuteResult,
  FlowFile,
  HistoryRecord,
  RequestFile,
  RunReport,
  RunStep,
  SseEvent,
  TraceEntry,
  VariableInfo,
} from "../api/types";
import type { ProgressEvent } from "../api/ws";
import { flowFromFile, flowToFile, type FlowDraft } from "../model/flow";
import { row, type Row } from "../model/draft";
import { draftToFile, emptyDraft, sameFile, uid, type Draft } from "../model/draft";
import { sameScope, scopeToFile, type ScopeDraft, type ScopeFile, type VarRow } from "../model/scope";
import type { Route } from "./route";
import { WsSession } from "./websocket.svelte";

/** A request being edited: a collection request (`collection` + `key`) or a scratch request. */
export class RequestTab {
  readonly kind = "request" as const;
  readonly id = uid();
  collection = $state<string | null>(null);
  key = $state<string | null>(null);
  draft = $state<Draft>(emptyDraft());
  /** The file as last loaded or saved (null: never saved). */
  saved = $state<RequestFile | null>(null);
  result = $state<ExecuteResult | null>(null);
  /** Trace events of the running request, as they arrive. */
  live = $state<TraceEntry[]>([]);
  running = $state(false);
  error = $state<string | null>(null);
  /** Variables visible to this request (null: outside a collection). */
  vars = $state<VariableInfo[] | null>(null);
  /** Server-Sent Events of the running request, as they arrive. */
  liveEvents = $state<SseEvent[]>([]);
  /** Stops the running request (null when nothing runs). */
  stop = $state<(() => void) | null>(null);
  /** A GraphQL schema fetched by introspection, and the state of fetching it. */
  graphql = $state<{ schema: unknown; sdl: string } | null>(null);
  schemaState = $state<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  /** The connection of a WebSocket request. */
  readonly ws = new WsSession();

  constructor(collection: string | null = null, key: string | null = null) {
    this.collection = collection;
    this.key = key;
    this.draft = emptyDraft(collection !== null);
  }

  get title(): string {
    return this.draft.name.trim() || this.key?.split("/").pop() || this.draft.url.trim() || "New request";
  }

  get dirty(): boolean {
    if (!this.saved) return this.collection !== null || this.draft.url.trim() !== "";
    return !sameFile(draftToFile(this.draft), this.saved);
  }

  get route(): Route {
    return this.collection && this.key
      ? { kind: "request", collection: this.collection, key: this.key }
      : { kind: "new" };
  }
}

/** The settings of a collection (`key` = "") or a folder. */
export class ScopeTab {
  readonly kind = "scope" as const;
  readonly id = uid();
  collection: string;
  key = $state("");
  draft = $state<ScopeDraft>(null as unknown as ScopeDraft);
  saved = $state<ScopeFile>({});

  constructor(collection: string, key: string, draft: ScopeDraft) {
    this.collection = collection;
    this.key = key;
    this.draft = draft;
    this.saved = scopeToFile(draft, key === "");
  }

  get isCollection() {
    return this.key === "";
  }

  get title(): string {
    return `${this.draft.name || this.key.split("/").pop() || this.collection} settings`;
  }

  get dirty(): boolean {
    return !sameScope(scopeToFile(this.draft, this.isCollection), this.saved);
  }

  get route(): Route {
    return this.key
      ? { kind: "folder", collection: this.collection, key: this.key }
      : { kind: "collection", collection: this.collection };
  }
}

/** An environment file. */
export class EnvTab {
  readonly kind = "environment" as const;
  readonly id = uid();
  collection: string;
  name: string;
  rows = $state<VarRow[]>([]);
  saved = $state("");
  /** Whether each secret has a value (from the variables endpoint). */
  secretsSet = $state<Record<string, boolean>>({});

  constructor(collection: string, name: string, rows: VarRow[]) {
    this.collection = collection;
    this.name = name;
    this.rows = rows;
    this.saved = this.snapshot();
  }

  snapshot(): string {
    return JSON.stringify(this.rows.filter((r) => r.name.trim()).map((r) => [r.name.trim(), r.secret ? null : r.value]));
  }

  get title(): string {
    return `${this.name} (environment)`;
  }

  get dirty(): boolean {
    return this.snapshot() !== this.saved;
  }

  get route(): Route {
    return { kind: "environment", collection: this.collection, name: this.name };
  }
}

/** A stored request and its response. */
export class HistoryTab {
  readonly kind = "history" as const;
  readonly id = uid();
  readonly dirty = false;
  readonly collection: string | null;
  record: HistoryRecord;

  constructor(record: HistoryRecord) {
    this.record = record;
    this.collection = record.collection;
  }

  get title(): string {
    return `#${this.record.id} ${this.record.method} ${this.record.url.replace(/^\w+:\/\/[^/]+/, "") || "/"}`;
  }

  get route(): Route {
    return { kind: "history", id: this.record.id };
  }
}

/** Two stored responses side by side. */
export class DiffTab {
  readonly kind = "diff" as const;
  readonly id = uid();
  readonly dirty = false;
  readonly collection = null;
  left: HistoryRecord;
  right: HistoryRecord;

  constructor(left: HistoryRecord, right: HistoryRecord) {
    this.left = left;
    this.right = right;
  }

  get title(): string {
    return `#${this.left.id} ↔ #${this.right.id}`;
  }

  get route(): Route {
    return { kind: "compare", a: this.left.id, b: this.right.id };
  }
}

/** What a runner runs: a whole collection, a folder or a flow. */
export interface RunTarget {
  collection: string;
  kind: "collection" | "folder" | "flow";
  /** Folder or flow key ("" for the collection). */
  key: string;
}

export interface ProgressRow {
  iteration: number;
  key: string;
  done: boolean;
  passed?: boolean;
  status?: number | null;
  durationMs?: number | null;
}

/** A configured run of a collection, folder or flow, with its live progress and report. */
export class RunnerTab {
  readonly kind = "runner" as const;
  readonly id = uid();
  readonly dirty = false;
  target: RunTarget;
  env = $state<string | null>(null);
  iterations = $state(1);
  /** CSV or JSON rows for a data-driven run. */
  data = $state("");
  bail = $state(false);
  delayMs = $state(0);
  parallel = $state(1);
  insecure = $state(false);
  /** Seconds per request; empty: the requests' own settings. */
  timeout = $state<number | null>(null);
  vars = $state<Row[]>([row()]);
  running = $state(false);
  progress = $state<ProgressRow[]>([]);
  error = $state<string | null>(null);
  report = $state<RunReport | null>(null);

  constructor(target: RunTarget, env: string | null) {
    this.target = target;
    this.env = env;
  }

  get collection() {
    return this.target.collection;
  }

  get title(): string {
    return `Run ${this.target.key || this.target.collection}`;
  }

  get route(): Route {
    return this.report?.id ? { kind: "run", id: this.report.id } : { kind: "home" };
  }

  onProgress(e: ProgressEvent) {
    if (e.event === "request_started") {
      this.progress.push({ iteration: e.iteration, key: e.key, done: false });
    } else {
      const r = this.progress.findLast((p) => p.iteration === e.iteration && p.key === e.key && !p.done);
      if (r) Object.assign(r, { done: true, passed: e.passed, status: e.status, durationMs: e.duration_ms });
    }
  }
}

/** A stored run report. */
export class RunTab {
  readonly kind = "run" as const;
  readonly id = uid();
  readonly dirty = false;
  runId: number;
  collection: string | null;
  report: RunReport;

  constructor(runId: number, report: RunReport, collection: string | null) {
    this.runId = runId;
    this.report = report;
    this.collection = collection;
  }

  get title(): string {
    return `Run #${this.runId} ${this.report.target.name}`;
  }

  get route(): Route {
    return { kind: "run", id: this.runId };
  }
}

/** A flow being edited and run step by step. */
export class FlowTab {
  readonly kind = "flow" as const;
  readonly id = uid();
  collection: string;
  key = $state("");
  draft = $state<FlowDraft>(null as unknown as FlowDraft);
  saved = $state("");
  selected = $state<number | null>(null);
  /** For each step that can be resumed, the run whose saved state before it to use. */
  resumeRun = $state<Record<number, number>>({});
  /** Detailed results by step number (from the latest runs). */
  results = $state<Record<number, RunStep>>({});
  /** Step numbers of the latest run, in order. */
  ran = $state<number[]>([]);
  running = $state<number | null>(null);
  error = $state<string | null>(null);

  constructor(collection: string, key: string, file: FlowFile) {
    this.collection = collection;
    this.key = key;
    this.draft = flowFromFile(file);
    this.saved = JSON.stringify(flowToFile(this.draft));
  }

  get title(): string {
    return this.draft.name || this.key.split("/").pop() || this.key;
  }

  get dirty(): boolean {
    return JSON.stringify(flowToFile(this.draft)) !== this.saved;
  }

  get route(): Route {
    return { kind: "flow", collection: this.collection, key: this.key };
  }

  /** The step that "Step" runs next (1-based), or null at the end. */
  get next(): number | null {
    const done = Object.keys(this.results).map(Number);
    if (!done.length) return 1;
    const last = Math.max(...done);
    const failed = !this.results[last]!.passed;
    if (failed) return null;
    return last < this.draft.steps.length ? last + 1 : null;
  }
}

export type Tab = RequestTab | ScopeTab | EnvTab | HistoryTab | DiffTab | RunnerTab | RunTab | FlowTab;
