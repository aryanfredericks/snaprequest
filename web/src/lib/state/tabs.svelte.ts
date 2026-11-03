// The kinds of tabs the main area shows.
import type { ExecuteResult, RequestFile, TraceEntry, VariableInfo } from "../api/types";
import { draftToFile, emptyDraft, sameFile, uid, type Draft } from "../model/draft";
import { sameScope, scopeToFile, type ScopeDraft, type ScopeFile, type VarRow } from "../model/scope";
import type { Route } from "./route";

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

export type Tab = RequestTab | ScopeTab | EnvTab;
