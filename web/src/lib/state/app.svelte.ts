// Application state: collections, the selected environment of each, and the open tabs.
import { api, ApiError, unwrap } from "../api/client";
import type {
  CollectionDoc,
  CollectionListEntry,
  ExecuteResult,
  RequestFile,
  TraceEntry,
  VariableInfo,
} from "../api/types";
import { jobs } from "../api/ws";
import { draftFromFile, draftToCore, draftToFile, emptyDraft, sameFile, uid, type Draft } from "../model/draft";
import { formatRoute, type Route } from "./route";

export const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

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

export type Tab = RequestTab;

export interface Notice {
  id: number;
  kind: "info" | "error";
  text: string;
}

class AppState {
  collections = $state<CollectionListEntry[]>([]);
  docs = $state<Record<string, CollectionDoc>>({});
  /** Selected environment per collection (remembered in localStorage). */
  envs = $state<Record<string, string | null>>(loadEnvs());
  tabs = $state<Tab[]>([]);
  activeId = $state<number | null>(null);
  notices = $state<Notice[]>([]);

  get active(): Tab | null {
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  notify(text: string, kind: Notice["kind"] = "info") {
    const n = { id: uid(), kind, text };
    this.notices.push(n);
    setTimeout(() => this.dismiss(n.id), kind === "error" ? 8000 : 3000);
  }

  dismiss(id: number) {
    this.notices = this.notices.filter((n) => n.id !== id);
  }

  // --- Collections ----------------------------------------------------------------------

  async loadCollections() {
    try {
      const data = unwrap(await api.GET("/api/v1/collections"));
      this.collections = (data.collections ?? []) as CollectionListEntry[];
    } catch (e) {
      this.notify(`Loading collections failed: ${message(e)}`, "error");
    }
  }

  async loadCollection(id: string): Promise<CollectionDoc | null> {
    try {
      const doc = unwrap(
        await api.GET("/api/v1/collections/{id}", { params: { path: { id } } }),
      ) as unknown as CollectionDoc;
      this.docs[id] = doc;
      const env = this.envs[id];
      if (env && !doc.environments.includes(env)) this.selectEnv(id, null);
      return doc;
    } catch (e) {
      this.notify(`Loading ${id} failed: ${message(e)}`, "error");
      return null;
    }
  }

  selectEnv(collection: string, env: string | null) {
    this.envs[collection] = env;
    localStorage.setItem("snapreq.envs", JSON.stringify(this.envs));
    for (const t of this.tabs) if (t.collection === collection) void this.refreshVars(t);
  }

  // --- Tabs -----------------------------------------------------------------------------

  activate(tab: Tab) {
    this.activeId = tab.id;
    const hash = formatRoute(tab.route);
    if (location.hash !== hash) history.replaceState(null, "", hash);
  }

  close(tab: Tab) {
    const i = this.tabs.indexOf(tab);
    if (i < 0) return;
    this.tabs.splice(i, 1);
    if (this.activeId === tab.id) {
      const next = this.tabs[Math.min(i, this.tabs.length - 1)];
      if (next) this.activate(next);
      else {
        this.activeId = null;
        history.replaceState(null, "", "#/");
      }
    }
  }

  newRequest(collection: string | null = null): RequestTab {
    const tab = new RequestTab(collection);
    this.tabs.push(tab);
    this.activate(tab);
    if (collection) void this.refreshVars(tab);
    return tab;
  }

  async openRequest(collection: string, key: string): Promise<RequestTab | null> {
    const open = this.tabs.find((t) => t.kind === "request" && t.collection === collection && t.key === key);
    if (open) {
      this.activate(open);
      return open;
    }
    try {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } } }),
      ) as unknown as { key: string; file: RequestFile };
      const tab = new RequestTab(collection, data.key);
      tab.draft = draftFromFile(data.file);
      tab.saved = draftToFile(tab.draft);
      this.tabs.push(tab);
      this.activate(tab);
      void this.refreshVars(tab);
      if (!this.docs[collection]) void this.loadCollection(collection);
      return tab;
    } catch (e) {
      this.notify(`Opening ${collection}/${key} failed: ${message(e)}`, "error");
      return null;
    }
  }

  async refreshVars(tab: RequestTab) {
    if (!tab.collection) {
      tab.vars = null;
      return;
    }
    const env = this.envs[tab.collection] ?? undefined;
    try {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/variables", {
          params: { path: { id: tab.collection }, query: { key: tab.key ?? undefined, env } },
        }),
      ) as unknown as { variables: VariableInfo[] };
      // Variables the request file itself declares are visible even before saving.
      tab.vars = data.variables;
    } catch {
      tab.vars = [];
    }
  }

  // --- Sending and saving ---------------------------------------------------------------

  async send(tab: RequestTab) {
    if (tab.running) return;
    if (!tab.draft.url.trim()) {
      tab.error = "Enter a URL first.";
      return;
    }
    tab.running = true;
    tab.error = null;
    tab.live = [];
    const body: Record<string, unknown> = { capture_wire: true };
    if (tab.collection && tab.key) {
      body.collection = tab.collection;
      body.key = tab.key;
      body.file = draftToFile(tab.draft);
      const env = this.envs[tab.collection];
      if (env) body.env = env;
    } else {
      body.request = draftToCore(tab.draft);
    }
    try {
      tab.result = await jobs.execute(body, { onTrace: (e) => tab.live.push(e) });
    } catch (e) {
      tab.result = null;
      tab.error = message(e);
    } finally {
      tab.running = false;
    }
  }

  /** Saves a collection request; a scratch request needs `saveAs` first. */
  async save(tab: RequestTab): Promise<boolean> {
    if (!tab.collection || !tab.key) return false;
    const file = draftToFile(tab.draft);
    try {
      unwrap(
        await api.PUT("/api/v1/collections/{id}/requests/{key}", {
          params: { path: { id: tab.collection, key: tab.key } },
          body: file as unknown as Record<string, never>,
        }),
      );
      tab.saved = file;
      this.notify(`Saved ${tab.key}`);
      void this.loadCollection(tab.collection);
      void this.refreshVars(tab);
      return true;
    } catch (e) {
      this.notify(`Saving failed: ${message(e)}`, "error");
      return false;
    }
  }

  /** Saves the tab's request as `key` in `collection` (refusing to overwrite another one). */
  async saveAs(tab: RequestTab, collection: string, key: string): Promise<boolean> {
    key = key.trim().replace(/^\/+|\/+$/g, "").replace(/\.toml$/, "");
    const exists = await api.GET("/api/v1/collections/{id}/requests/{key}", {
      params: { path: { id: collection, key } },
    });
    if (exists.response.ok && !(tab.collection === collection && tab.key === key)) {
      this.notify(`${collection}/${key} already exists`, "error");
      return false;
    }
    const previous = [tab.collection, tab.key] as const;
    tab.collection = collection;
    tab.key = key;
    if (await this.save(tab)) {
      this.activate(tab);
      return true;
    }
    [tab.collection, tab.key] = previous;
    return false;
  }
}

function loadEnvs(): Record<string, string | null> {
  try {
    return JSON.parse(localStorage.getItem("snapreq.envs") ?? "{}") as Record<string, string | null>;
  } catch {
    return {};
  }
}

export const app = new AppState();
export { ApiError };
