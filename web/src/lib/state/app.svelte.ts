// Application state: collections, the selected environment of each, and the open tabs.
import { api, ApiError, unwrap } from "../api/client";
import type {
  CollectionDoc,
  CollectionListEntry,
  FlowFile,
  HistoryRecord,
  HistorySummary,
  RequestFile,
  RunReport,
  RunSummary,
  VariableInfo,
  VarsTable,
} from "../api/types";
import { jobs } from "../api/ws";
import { draftFromFile, draftToCore, draftToFile, uid } from "../model/draft";
import { draftFromHistory } from "../model/history";
import { flowToFile } from "../model/flow";
import { parseData } from "../model/runs";
import { scopeFromFile, scopeToFile, varsFromTable, varsToTable, type ScopeFile } from "../model/scope";
import { prompt } from "./prompt.svelte";
import { formatRoute } from "./route";
import {
  DiffTab,
  EnvTab,
  FlowTab,
  HistoryTab,
  RequestTab,
  RunnerTab,
  RunTab,
  ScopeTab,
  type RunTarget,
  type Tab,
} from "./tabs.svelte";

export { DiffTab, EnvTab, FlowTab, HistoryTab, RequestTab, RunnerTab, RunTab, ScopeTab, type RunTarget, type Tab };

export interface HistoryFilter {
  url: string;
  method: string;
  /** "", "2" … "5", or "failed". */
  status: string;
  collection: string;
}

export const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export interface Notice {
  id: number;
  kind: "info" | "error";
  text: string;
}

export type ItemKind = "request" | "folder" | "flow";

/** Untyped access for endpoints whose bodies are free-form JSON documents. */
type Doc = Record<string, never>;

class AppState {
  collections = $state<CollectionListEntry[]>([]);
  docs = $state<Record<string, CollectionDoc>>({});
  /** Selected environment per collection (remembered in localStorage). */
  envs = $state<Record<string, string | null>>(loadEnvs());
  tabs = $state<Tab[]>([]);
  activeId = $state<number | null>(null);
  notices = $state<Notice[]>([]);
  /** Bumped whenever something is added to the history (the history list reloads). */
  historyVersion = $state(0);
  /** Bumped whenever a run is stored. */
  runsVersion = $state(0);

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

  /** Runs `f`; failures become an error notice (`what failed: why`) and `null`. */
  private async attempt<T>(what: string, f: () => Promise<T>): Promise<T | null> {
    try {
      return await f();
    } catch (e) {
      this.notify(`${what} failed: ${message(e)}`, "error");
      return null;
    }
  }

  // --- Collections ----------------------------------------------------------------------

  async loadCollections() {
    await this.attempt("Loading collections", async () => {
      const data = unwrap(await api.GET("/api/v1/collections"));
      this.collections = (data.collections ?? []) as CollectionListEntry[];
    });
  }

  async loadCollection(id: string): Promise<CollectionDoc | null> {
    return this.attempt(`Loading ${id}`, async () => {
      const doc = unwrap(
        await api.GET("/api/v1/collections/{id}", { params: { path: { id } } }),
      ) as unknown as CollectionDoc;
      this.docs[id] = doc;
      const env = this.envs[id];
      if (env && !doc.environments.includes(env)) this.selectEnv(id, null);
      return doc;
    });
  }

  /** Reloads a collection and the list (after changes to its files). */
  private async refresh(id: string) {
    await Promise.all([this.loadCollection(id), this.loadCollections()]);
  }

  async createCollection(): Promise<void> {
    const v = await prompt.ask({
      title: "New collection",
      fields: [
        { name: "id", label: "Directory name (id)", pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*", placeholder: "my-api" },
        { name: "name", label: "Display name", required: false, placeholder: "My API" },
      ],
      submit: "Create",
    });
    if (!v) return;
    const ok = await this.attempt("Creating the collection", async () =>
      unwrap(await api.POST("/api/v1/collections", { body: { id: v.id!, name: v.name || null } })),
    );
    if (ok) {
      await this.refresh(v.id!);
      this.notify(`Created ${v.id}`);
    }
  }

  async deleteCollection(id: string) {
    if (!(await prompt.confirm(`Delete ${id}?`, `This removes the collection directory with every request, flow and environment in it.`)))
      return;
    const ok = await this.attempt("Deleting the collection", async () => {
      const r = await api.DELETE("/api/v1/collections/{id}", { params: { path: { id } } });
      if (!r.response.ok) unwrap(r);
      return true;
    });
    if (!ok) return;
    for (const t of [...this.tabs]) if (t.collection === id) this.close(t, true);
    delete this.docs[id];
    await this.loadCollections();
  }

  selectEnv(collection: string, env: string | null) {
    this.envs[collection] = env;
    localStorage.setItem("snapreq.envs", JSON.stringify(this.envs));
    for (const t of this.tabs) if (t.kind === "request" && t.collection === collection) void this.refreshVars(t);
  }

  // --- Tabs -----------------------------------------------------------------------------

  activate(tab: Tab) {
    this.activeId = tab.id;
    const hash = formatRoute(tab.route);
    if (location.hash !== hash) history.replaceState(null, "", hash);
  }

  private open(tab: Tab) {
    this.tabs.push(tab);
    this.activate(tab);
  }

  close(tab: Tab, force = false) {
    if (!force && tab.dirty && !confirm(`Close "${tab.title}" without saving?`)) return;
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
    this.open(tab);
    if (collection) void this.refreshVars(tab);
    return tab;
  }

  async openRequest(collection: string, key: string): Promise<RequestTab | null> {
    const open = this.tabs.find((t) => t.kind === "request" && t.collection === collection && t.key === key);
    if (open) {
      this.activate(open);
      return open as RequestTab;
    }
    return this.attempt(`Opening ${collection}/${key}`, async () => {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } } }),
      ) as unknown as { key: string; file: RequestFile };
      const tab = new RequestTab(collection, data.key);
      tab.draft = draftFromFile(data.file);
      tab.saved = draftToFile(tab.draft);
      this.open(tab);
      void this.refreshVars(tab);
      if (!this.docs[collection]) void this.loadCollection(collection);
      return tab;
    });
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
      // The request's own unsaved [vars] are visible too.
      const own = varsFromTable(tab.draft.rest.vars).map<VariableInfo>((v) => ({
        name: v.name,
        value: v.secret ? null : v.value,
        secret: v.secret,
        has_value: !v.secret,
        scope: "request",
      }));
      tab.vars = [...own, ...data.variables.filter((v) => !own.some((o) => o.name === v.name))];
    } catch {
      tab.vars = [];
    }
  }

  // --- Sending and saving requests --------------------------------------------------------

  async send(tab: Tab) {
    if (tab.kind !== "request" || tab.running) return;
    if (!tab.draft.url.trim()) {
      tab.error = "Enter a URL first.";
      return;
    }
    const body: Record<string, unknown> = { capture_wire: true };
    try {
      if (tab.collection && tab.key) {
        body.collection = tab.collection;
        body.key = tab.key;
        body.file = draftToFile(tab.draft);
        const env = this.envs[tab.collection];
        if (env) body.env = env;
      } else {
        body.request = draftToCore(tab.draft);
      }
    } catch (e) {
      tab.error = message(e);
      return;
    }
    tab.running = true;
    tab.error = null;
    tab.live = [];
    try {
      tab.result = await jobs.execute(body, { onTrace: (e) => tab.live.push(e) });
      this.historyVersion++;
    } catch (e) {
      tab.result = null;
      tab.error = message(e);
    } finally {
      tab.running = false;
    }
  }

  async save(tab: Tab): Promise<boolean> {
    switch (tab.kind) {
      case "request":
        return this.saveRequest(tab);
      case "scope":
        return this.saveScope(tab);
      case "environment":
        return this.saveEnvironment(tab);
      case "flow":
        return this.saveFlow(tab);
      default:
        return false;
    }
  }

  /** Saves a collection request; a scratch request needs `saveAs` first. */
  async saveRequest(tab: RequestTab): Promise<boolean> {
    if (!tab.collection || !tab.key) return false;
    const file = draftToFile(tab.draft);
    const ok = await this.attempt("Saving", async () =>
      unwrap(
        await api.PUT("/api/v1/collections/{id}/requests/{key}", {
          params: { path: { id: tab.collection!, key: tab.key! } },
          body: file as unknown as Doc,
        }),
      ),
    );
    if (!ok) return false;
    tab.saved = file;
    this.notify(`Saved ${tab.key}`);
    void this.loadCollection(tab.collection);
    void this.refreshVars(tab);
    return true;
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
    if (await this.saveRequest(tab)) {
      this.activate(tab);
      void this.loadCollections();
      return true;
    }
    [tab.collection, tab.key] = previous;
    return false;
  }

  // --- History ----------------------------------------------------------------------------

  async listHistory(f: HistoryFilter, before?: number, limit = 100): Promise<HistorySummary[]> {
    const query: Record<string, string | number | boolean> = { limit };
    if (f.url.trim()) query.url = f.url.trim();
    if (f.method) query.method = f.method;
    if (f.status === "failed") query.failed = true;
    else if (f.status) query.class = Number(f.status);
    if (f.collection) query.collection = f.collection;
    if (before !== undefined) query.before = before;
    const data = unwrap(await api.GET("/api/v1/history", { params: { query } }));
    return (data.entries ?? []) as HistorySummary[];
  }

  private async historyRecord(id: number): Promise<HistoryRecord> {
    return unwrap(await api.GET("/api/v1/history/{id}", { params: { path: { id } } })) as unknown as HistoryRecord;
  }

  async openHistory(id: number) {
    const open = this.tabs.find((t) => t.kind === "history" && t.record.id === id);
    if (open) return this.activate(open);
    await this.attempt(`Opening history entry ${id}`, async () => this.open(new HistoryTab(await this.historyRecord(id))));
  }

  async compare(a: number, b: number) {
    const [left, right] = a < b ? [a, b] : [b, a];
    const open = this.tabs.find((t) => t.kind === "diff" && t.left.id === left && t.right.id === right);
    if (open) return this.activate(open);
    await this.attempt("Comparing", async () => {
      const [l, r] = await Promise.all([this.historyRecord(left), this.historyRecord(right)]);
      this.open(new DiffTab(l, r));
    });
  }

  /**
   * Sends a stored request again: a collection request is opened and sent as its file is now;
   * any other request is rebuilt in a new tab (without the credentials, which were never stored).
   */
  async rerun(record: HistoryRecord) {
    if (record.collection && record.request_key) {
      const tab = await this.openRequest(record.collection, record.request_key);
      if (tab) await this.send(tab);
      return;
    }
    const { draft, redacted } = draftFromHistory(record);
    const tab = new RequestTab();
    tab.draft = draft;
    this.open(tab);
    if (redacted.length) {
      this.notify(`Credentials are not kept in history: fill in ${redacted.join(", ")} before sending.`, "error");
      return;
    }
    await this.send(tab);
  }

  async deleteHistory(id: number | null): Promise<boolean> {
    if (id === null && !(await prompt.confirm("Clear the history?", "Every stored request and response is deleted.", "Clear"))) return false;
    const ok = await this.attempt("Deleting history", async () => {
      const r =
        id === null
          ? await api.DELETE("/api/v1/history")
          : await api.DELETE("/api/v1/history/{id}", { params: { path: { id } } });
      if (!r.response.ok) unwrap(r);
      return true;
    });
    if (!ok) return false;
    for (const t of [...this.tabs]) {
      if (t.kind === "history" && (id === null || t.record.id === id)) this.close(t, true);
      if (t.kind === "diff" && (id === null || t.left.id === id || t.right.id === id)) this.close(t, true);
    }
    this.historyVersion++;
    return true;
  }

  // --- Runs -------------------------------------------------------------------------------

  openRunner(target: RunTarget) {
    const tab = new RunnerTab(target, this.envs[target.collection] ?? null);
    this.open(tab);
    if (!this.docs[target.collection]) void this.loadCollection(target.collection);
  }

  async startRun(tab: RunnerTab) {
    if (tab.running) return;
    tab.error = null;
    const body: Record<string, unknown> = {
      collection: tab.target.collection,
      capture_wire: true,
      bail: tab.bail,
      parallel: Math.max(1, tab.parallel),
    };
    if (tab.target.key) body.target = tab.target.key;
    if (tab.env) body.env = tab.env;
    if (tab.delayMs > 0) body.delay_ms = tab.delayMs;
    if (tab.insecure) body.insecure = true;
    if (tab.timeout !== null && tab.timeout >= 0) body.timeout_ms = Math.round(tab.timeout * 1000);
    const vars = Object.fromEntries(tab.vars.filter((v) => v.enabled && v.name.trim()).map((v) => [v.name.trim(), v.value]));
    if (Object.keys(vars).length) body.vars = vars;
    try {
      const rows = parseData(tab.data);
      if (rows.length) body.data = rows;
      else if (tab.iterations > 1) body.iterations = tab.iterations;
    } catch (e) {
      tab.error = `Data: ${message(e)}`;
      return;
    }
    tab.running = true;
    tab.progress = [];
    tab.report = null;
    try {
      const summary = await jobs.run(body, { onProgress: (e) => tab.onProgress(e) });
      tab.report = { ...(await this.runReport(summary.id!)), id: summary.id, collection: tab.target.collection };
      this.activate(tab);
      this.runsVersion++;
      this.historyVersion++;
    } catch (e) {
      tab.error = message(e);
    } finally {
      tab.running = false;
    }
  }

  async runReport(id: number): Promise<RunReport> {
    return unwrap(await api.GET("/api/v1/runs/{id}", { params: { path: { id } } })) as unknown as RunReport;
  }

  async listRuns(limit = 100): Promise<RunSummary[]> {
    const data = unwrap(await api.GET("/api/v1/runs", { params: { query: { limit } } }));
    return (data.runs ?? []) as RunSummary[];
  }

  async openRun(id: number, targetKey?: string) {
    const open = this.tabs.find((t) => (t.kind === "run" && t.runId === id) || (t.kind === "runner" && t.report?.id === id));
    if (open) return this.activate(open);
    await this.attempt(`Opening run ${id}`, async () => {
      const report = await this.runReport(id);
      this.open(new RunTab(id, report, targetKey ? targetKey.split("/")[0]! : null));
    });
  }

  // --- Flows ------------------------------------------------------------------------------

  async openFlow(collection: string, key: string) {
    const open = this.tabs.find((t) => t.kind === "flow" && t.collection === collection && t.key === key);
    if (open) return this.activate(open);
    await this.attempt(`Opening the flow ${key}`, async () => {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/flows/{key}", { params: { path: { id: collection, key } } }),
      ) as unknown as { key: string; file: FlowFile };
      this.open(new FlowTab(collection, data.key, data.file));
      if (!this.docs[collection]) void this.loadCollection(collection);
    });
  }

  async saveFlow(tab: FlowTab): Promise<boolean> {
    const file = flowToFile(tab.draft);
    const ok = await this.attempt("Saving the flow", async () =>
      unwrap(
        await api.PUT("/api/v1/collections/{id}/flows/{key}", {
          params: { path: { id: tab.collection, key: tab.key } },
          body: file as unknown as Doc,
        }),
      ),
    );
    if (!ok) return false;
    tab.saved = JSON.stringify(file);
    this.notify(`Saved ${tab.key}`);
    void this.loadCollection(tab.collection);
    return true;
  }

  async createFlow(collection: string, folder: string) {
    const v = await prompt.ask({
      title: `New flow in ${folder || collection}`,
      fields: [{ name: "name", label: "File name", pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*", placeholder: "checkout", hint: "Saved as <name>.flow.toml" }],
      submit: "Create",
    });
    if (!v) return;
    const key = folder ? `${folder}/${v.name}` : v.name!;
    const file: FlowFile = { name: v.name, step: [{ name: "First step", url: "{{base_url}}/" }] };
    const ok = await this.attempt("Creating the flow", async () =>
      unwrap(await api.PUT("/api/v1/collections/{id}/flows/{key}", { params: { path: { id: collection, key } }, body: file as unknown as Doc })),
    );
    if (!ok) return;
    await this.refresh(collection);
    await this.openFlow(collection, key);
  }

  /**
   * Runs a flow: `all` from the start, `next` the step after the last one run, or the steps
   * `from`–`to` starting from the captured values a previous run left before `from`.
   */
  async runFlow(tab: FlowTab, mode: "all" | "next" | { from: number; to?: number }) {
    if (tab.running !== null) return;
    if (tab.dirty && !(await this.saveFlow(tab))) return;
    const total = tab.draft.steps.length;
    let from = 1;
    let to: number | undefined;
    if (mode === "next") {
      const n = tab.next;
      if (n === null) return;
      from = n;
      to = n;
    } else if (mode !== "all") {
      from = mode.from;
      to = mode.to;
    }
    const body: Record<string, unknown> = { collection: tab.collection, target: tab.key, capture_wire: true };
    const env = this.envs[tab.collection];
    if (env) body.env = env;
    if (from > 1) {
      const run = tab.resumeRun[from];
      if (run === undefined) {
        tab.error = `Run the steps before step ${from} first: their captured values are needed.`;
        return;
      }
      body.from_step = from;
      body.resume = run;
    }
    if (to !== undefined && to < total) body.to_step = to;
    else if (from > 1) body.to_step = total;
    tab.error = null;
    tab.running = from;
    // Results of the steps about to run are replaced: do not show the old ones meanwhile.
    tab.results = Object.fromEntries(Object.entries(tab.results).filter(([n]) => Number(n) < from));
    try {
      const summary = (await jobs.run(body)) as RunReport & { resumable_steps?: number[] };
      const report = await this.runReport(summary.id!);
      const steps = report.iterations[0]?.steps ?? [];
      const kept = Object.fromEntries(Object.entries(tab.results).filter(([n]) => Number(n) < from));
      for (const s of steps) kept[s.number] = s;
      tab.results = kept;
      tab.ran = steps.map((s) => s.number);
      const resumable = summary.resumable_steps ?? [];
      const last = Math.max(from, ...resumable);
      const map = Object.fromEntries(Object.entries(tab.resumeRun).filter(([n]) => Number(n) < from || Number(n) > last));
      for (const n of resumable) map[n] = summary.id!;
      // States after a failure in this run are gone; older ones would not match the new results.
      for (const n of Object.keys(map).map(Number)) if (n > last) delete map[n];
      tab.resumeRun = map;
      const lastRun = steps.at(-1)?.number;
      if (lastRun !== undefined) tab.selected = tab.draft.steps[lastRun - 1]?.id ?? tab.selected;
      this.runsVersion++;
      this.historyVersion++;
    } catch (e) {
      tab.error = message(e);
    } finally {
      tab.running = null;
    }
  }

  // --- Tree changes -----------------------------------------------------------------------

  /** Creates a request named by the user in `folder` and opens it. */
  async createRequest(collection: string, folder: string) {
    const v = await prompt.ask({
      title: `New request in ${folder || collection}`,
      fields: [
        { name: "name", label: "File name", pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*", placeholder: "create-user" },
        { name: "method", label: "Method", type: "select", options: ["GET", "POST", "PUT", "PATCH", "DELETE"].map((m) => ({ value: m, label: m })) },
      ],
      submit: "Create",
    });
    if (!v) return;
    const key = folder ? `${folder}/${v.name}` : v.name!;
    const exists = await api.GET("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } } });
    if (exists.response.ok) {
      this.notify(`${key} already exists`, "error");
      return;
    }
    const file: RequestFile = { url: "{{base_url}}/", ...(v.method !== "GET" ? { method: v.method } : {}) };
    const ok = await this.attempt("Creating the request", async () =>
      unwrap(await api.PUT("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } }, body: file as unknown as Doc })),
    );
    if (!ok) return;
    await this.refresh(collection);
    await this.openRequest(collection, key);
  }

  async createFolder(collection: string, parent: string) {
    const v = await prompt.ask({
      title: `New folder in ${parent || collection}`,
      fields: [{ name: "name", label: "Folder name", pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*", placeholder: "users" }],
      submit: "Create",
    });
    if (!v) return;
    const key = parent ? `${parent}/${v.name}` : v.name!;
    const ok = await this.attempt("Creating the folder", async () =>
      unwrap(await api.PUT("/api/v1/collections/{id}/folders/{key}", { params: { path: { id: collection, key } }, body: {} as Doc })),
    );
    if (ok) await this.refresh(collection);
  }

  /** Moves, renames or reorders an item; open tabs follow it. */
  async move(
    collection: string,
    kind: ItemKind,
    from: string,
    to: string,
    opts: { name?: string; before?: string | null } = {},
  ): Promise<string | null> {
    const body: Record<string, unknown> = { kind, from, to };
    if (opts.name) body.name = opts.name;
    if (opts.before !== undefined) body.place = opts.before === null ? {} : { before: opts.before };
    const res = await this.attempt("Moving", async () =>
      unwrap(await api.POST("/api/v1/collections/{id}/move", { params: { path: { id: collection } }, body: body as never })),
    );
    if (!res) return null;
    const key = String((res as { key?: string }).key ?? from);
    if (key !== from) {
      for (const t of this.tabs) {
        if (t.collection !== collection) continue;
        if (t.kind === "request" && t.key !== null && (t.key === from || (kind === "folder" && t.key.startsWith(`${from}/`)))) {
          t.key = key + t.key.slice(from.length);
        }
        if (t.kind === "scope" && kind === "folder" && (t.key === from || t.key.startsWith(`${from}/`))) {
          t.key = key + t.key.slice(from.length);
        }
      }
      const active = this.active;
      if (active) this.activate(active);
    }
    await this.loadCollection(collection);
    return key;
  }

  async rename(collection: string, kind: ItemKind, key: string) {
    const [parent, name] = splitKey(key);
    const v = await prompt.ask({
      title: `Rename ${name}`,
      fields: [{ name: "name", label: "New file name", value: name, pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*" }],
      submit: "Rename",
    });
    if (v && v.name !== name) await this.move(collection, kind, key, parent, { name: v.name });
  }

  async duplicate(collection: string, key: string) {
    const [parent, name] = splitKey(key);
    const res = await this.attempt("Duplicating", async () => {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } } }),
      ) as unknown as { file: RequestFile };
      const doc = this.docs[collection];
      const taken = new Set(allKeys(doc?.items ?? []));
      let copy = `${name}-copy`;
      for (let i = 2; taken.has(parent ? `${parent}/${copy}` : copy); i++) copy = `${name}-copy-${i}`;
      const newKey = parent ? `${parent}/${copy}` : copy;
      const file = { ...data.file, name: data.file.name ? `${data.file.name} (copy)` : undefined };
      unwrap(await api.PUT("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key: newKey } }, body: file as unknown as Doc }));
      return newKey;
    });
    if (res) {
      await this.refresh(collection);
      await this.openRequest(collection, res);
    }
  }

  async remove(collection: string, kind: ItemKind, key: string) {
    const what = kind === "folder" ? `the folder ${key} and everything in it` : key;
    if (!(await prompt.confirm(`Delete ${key}?`, `This deletes ${what} from the collection's files.`))) return;
    const ok = await this.attempt("Deleting", async () => {
      const r =
        kind === "folder"
          ? await api.DELETE("/api/v1/collections/{id}/folders/{key}", { params: { path: { id: collection, key } } })
          : kind === "flow"
            ? await api.DELETE("/api/v1/collections/{id}/flows/{key}", { params: { path: { id: collection, key } } })
            : await api.DELETE("/api/v1/collections/{id}/requests/{key}", { params: { path: { id: collection, key } } });
      if (!r.response.ok) unwrap(r);
      return true;
    });
    if (!ok) return;
    for (const t of [...this.tabs]) {
      if (t.kind === "history" || t.kind === "diff" || t.kind === "run" || t.kind === "runner" || t.collection !== collection) continue;
      const k = t.kind === "environment" ? null : t.key;
      if (k !== null && (k === key || (kind === "folder" && k.startsWith(`${key}/`)))) this.close(t, true);
    }
    await this.refresh(collection);
  }

  // --- Collection and folder settings -----------------------------------------------------

  async openScope(collection: string, key: string) {
    const open = this.tabs.find((t) => t.kind === "scope" && t.collection === collection && t.key === key);
    if (open) return this.activate(open);
    await this.attempt("Opening the settings", async () => {
      let file: ScopeFile;
      if (key) {
        const data = unwrap(
          await api.GET("/api/v1/collections/{id}/folders/{key}", { params: { path: { id: collection, key } } }),
        ) as unknown as { file: ScopeFile };
        file = data.file;
      } else {
        const doc = (this.docs[collection] ?? (await this.loadCollection(collection)))!;
        file = doc.file as ScopeFile;
      }
      this.open(new ScopeTab(collection, key, scopeFromFile(file, key === "")));
    });
  }

  async saveScope(tab: ScopeTab): Promise<boolean> {
    const file = scopeToFile(tab.draft, tab.isCollection);
    const ok = await this.attempt("Saving the settings", async () =>
      tab.isCollection
        ? unwrap(await api.PUT("/api/v1/collections/{id}", { params: { path: { id: tab.collection } }, body: file as unknown as Doc }))
        : unwrap(
            await api.PUT("/api/v1/collections/{id}/folders/{key}", {
              params: { path: { id: tab.collection, key: tab.key } },
              body: file as unknown as Doc,
            }),
          ),
    );
    if (!ok) return false;
    tab.saved = file;
    this.notify("Saved");
    await this.refresh(tab.collection);
    for (const t of this.tabs) if (t.kind === "request" && t.collection === tab.collection) void this.refreshVars(t);
    return true;
  }

  // --- Environments and secrets -----------------------------------------------------------

  async openEnvironment(collection: string, name: string) {
    const open = this.tabs.find((t) => t.kind === "environment" && t.collection === collection && t.name === name);
    if (open) return this.activate(open);
    await this.attempt(`Opening ${name}`, async () => {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/environments/{name}", { params: { path: { id: collection, name } } }),
      ) as unknown as { vars: VarsTable };
      const tab = new EnvTab(collection, name, varsFromTable(data.vars));
      this.open(tab);
      void this.refreshSecrets(tab);
    });
  }

  async refreshSecrets(tab: EnvTab) {
    try {
      const data = unwrap(
        await api.GET("/api/v1/collections/{id}/variables", { params: { path: { id: tab.collection }, query: { env: tab.name } } }),
      ) as unknown as { variables: VariableInfo[] };
      tab.secretsSet = Object.fromEntries(data.variables.filter((v) => v.secret).map((v) => [v.name, v.has_value]));
    } catch {
      tab.secretsSet = {};
    }
  }

  async createEnvironment(collection: string) {
    const v = await prompt.ask({
      title: "New environment",
      fields: [{ name: "name", label: "Name", pattern: "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*", placeholder: "staging" }],
      submit: "Create",
    });
    if (!v) return;
    const exists = this.docs[collection]?.environments.includes(v.name!);
    if (exists) return this.notify(`${v.name} already exists`, "error");
    const ok = await this.attempt("Creating the environment", async () =>
      unwrap(
        await api.PUT("/api/v1/collections/{id}/environments/{name}", {
          params: { path: { id: collection, name: v.name! } },
          body: {} as Doc,
        }),
      ),
    );
    if (!ok) return;
    await this.loadCollection(collection);
    await this.openEnvironment(collection, v.name!);
  }

  async saveEnvironment(tab: EnvTab): Promise<boolean> {
    const vars = varsToTable(tab.rows) ?? {};
    const ok = await this.attempt("Saving the environment", async () =>
      unwrap(
        await api.PUT("/api/v1/collections/{id}/environments/{name}", {
          params: { path: { id: tab.collection, name: tab.name } },
          body: vars as unknown as Doc,
        }),
      ),
    );
    if (!ok) return false;
    tab.saved = tab.snapshot();
    this.notify(`Saved ${tab.name}`);
    void this.refreshSecrets(tab);
    for (const t of this.tabs) if (t.kind === "request" && t.collection === tab.collection) void this.refreshVars(t);
    return true;
  }

  async deleteEnvironment(collection: string, name: string) {
    if (!(await prompt.confirm(`Delete ${name}?`, `This deletes environments/${name}.toml.`))) return;
    const ok = await this.attempt("Deleting the environment", async () => {
      const r = await api.DELETE("/api/v1/collections/{id}/environments/{name}", { params: { path: { id: collection, name } } });
      if (!r.response.ok) unwrap(r);
      return true;
    });
    if (!ok) return;
    for (const t of [...this.tabs]) if (t.kind === "environment" && t.collection === collection && t.name === name) this.close(t, true);
    if (this.envs[collection] === name) this.selectEnv(collection, null);
    await this.loadCollection(collection);
  }

  /** Stores a secret value (`scope`: an environment name or `shared`); it is never shown again. */
  async setSecret(collection: string, scope: string, name: string): Promise<boolean> {
    const v = await prompt.ask({
      title: `Set the secret ${name}`,
      message:
        scope === "shared"
          ? "Stored in the collection's .secrets/shared.toml (git-ignored) and used by every environment. It is never shown again."
          : `Stored in the collection's .secrets/${scope}.toml (git-ignored). It is never shown again.`,
      fields: [{ name: "value", label: "Value", type: "password" }],
      submit: "Store",
    });
    if (!v) return false;
    const ok = await this.attempt("Storing the secret", async () => {
      const r = await api.PUT("/api/v1/collections/{id}/secrets/{scope}/{name}", {
        params: { path: { id: collection, scope, name } },
        body: { value: v.value! },
      });
      if (!r.response.ok) unwrap(r);
      return true;
    });
    if (!ok) return false;
    this.notify(`Stored ${name}`);
    for (const t of this.tabs) {
      if (t.kind === "request" && t.collection === collection) void this.refreshVars(t);
      if (t.kind === "environment" && t.collection === collection) void this.refreshSecrets(t);
    }
    return true;
  }
}

export function splitKey(key: string): [string, string] {
  const i = key.lastIndexOf("/");
  return i < 0 ? ["", key] : [key.slice(0, i), key.slice(i + 1)];
}

function allKeys(items: CollectionDoc["items"]): string[] {
  return items.flatMap((i) => (i.type === "folder" ? [i.key, ...allKeys(i.items)] : [i.key]));
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
