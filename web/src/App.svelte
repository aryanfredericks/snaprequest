<script lang="ts">
  import { api, serverVersion, type ServerVersion } from "./lib/api/client";
  import AccountDialog from "./lib/components/AccountDialog.svelte";
  import WorkspaceDialog from "./lib/components/WorkspaceDialog.svelte";
  import { workspaces } from "./lib/state/workspaces.svelte";
  import { auth } from "./lib/state/auth.svelte";
  import DiffView from "./lib/components/DiffView.svelte";
  import EnvView from "./lib/components/EnvView.svelte";
  import FlowView from "./lib/components/FlowView.svelte";
  import ReportView from "./lib/components/ReportView.svelte";
  import RunnerView from "./lib/components/RunnerView.svelte";
  import RunsPanel from "./lib/components/RunsPanel.svelte";
  import HistoryPanel from "./lib/components/HistoryPanel.svelte";
  import HistoryView from "./lib/components/HistoryView.svelte";
  import PromptDialog from "./lib/components/PromptDialog.svelte";
  import RequestView from "./lib/components/RequestView.svelte";
  import ScopeView from "./lib/components/ScopeView.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import { app } from "./lib/state/app.svelte";
  import { parseRoute } from "./lib/state/route";
  import { label, match, SHORTCUTS } from "./lib/state/shortcuts";
  import { theme, type ThemeChoice } from "./lib/state/theme.svelte";

  let version = $state<ServerVersion | null>(null);
  let offline = $state<string | null>(null);
  let view: RequestView | undefined = $state();
  type Side = "collections" | "history" | "runs";
  let side = $state<Side>((localStorage.getItem("snapreq.side") as Side | null) ?? "collections");
  $effect(() => localStorage.setItem("snapreq.side", side));
  let help: HTMLDialogElement;
  let account: AccountDialog | undefined = $state();
  let wsDialog: WorkspaceDialog | undefined = $state();
  let mainEl: HTMLElement;

  $effect(() => {
    serverVersion(api)
      .then((v) => (version = v))
      .catch((e: unknown) => (offline = e instanceof Error ? e.message : String(e)));
    void openRoute(location.hash);
    const onHash = () => void openRoute(location.hash);
    addEventListener("hashchange", onHash);
    return () => removeEventListener("hashchange", onHash);
  });

  async function openRoute(hash: string) {
    const r = parseRoute(hash);
    if (r.kind === "request") await app.openRequest(r.collection, r.key);
    else if (r.kind === "environment") await app.openEnvironment(r.collection, r.name);
    else if (r.kind === "folder") await app.openScope(r.collection, r.key);
    else if (r.kind === "collection") await app.openScope(r.collection, "");
    else if (r.kind === "history") await app.openHistory(r.id);
    else if (r.kind === "compare") await app.compare(r.a, r.b);
    else if (r.kind === "flow") await app.openFlow(r.collection, r.key);
    else if (r.kind === "run") await app.openRun(r.id);
    else if (r.kind === "new" && !app.tabs.some((t) => t.kind === "request" && t.collection === null)) app.newRequest();
  }

  const tab = $derived(app.active);
  const collection = $derived(tab?.collection ?? null);
  const doc = $derived(collection ? app.docs[collection] : null);

  function keys(e: KeyboardEvent) {
    const el = e.target as HTMLElement | null;
    const typing = !!el?.closest("input, textarea, select, [contenteditable=true], .cm-editor");
    const action = match({ ...pick(e), typing });
    if (!action) return;
    // Ctrl+S and Alt shortcuts are taken by the app wherever the focus is; ? only outside fields.
    e.preventDefault();
    switch (action) {
      case "send":
        if (tab?.kind === "runner") void app.startRun(tab);
        else if (tab?.kind === "flow") void app.runFlow(tab, "all");
        else if (tab) void app.send(tab);
        break;
      case "save":
        if (tab?.kind === "request") view?.save();
        else if (tab) void app.save(tab);
        break;
      case "new":
        app.newRequest();
        break;
      case "close":
        if (tab) app.close(tab);
        break;
      case "prev-tab":
      case "next-tab": {
        const i = app.tabs.findIndex((t) => t.id === app.activeId);
        const n = app.tabs.length;
        if (n) app.activate(app.tabs[(i + (action === "next-tab" ? 1 : -1) + n) % n]!);
        break;
      }
      case "focus-url":
        document.querySelector<HTMLElement>("[aria-label='URL'][contenteditable]")?.focus();
        break;
      case "side-collections":
      case "side-history":
      case "side-runs":
        side = action.slice(5) as Side;
        break;
      case "help":
        help.showModal();
        break;
    }
  }

  function pick(e: KeyboardEvent) {
    return { key: e.key, code: e.code, ctrlKey: e.ctrlKey, metaKey: e.metaKey, altKey: e.altKey, shiftKey: e.shiftKey };
  }
</script>

<svelte:window onkeydown={keys} />

<div class="app">
  <button type="button" class="skip" onclick={() => mainEl.focus()}>Skip to the main area</button>
  <header>
    <h1><span class="logo" aria-hidden="true">⚡</span> SnapRequest</h1>
    <span class="ws">
      <select aria-label="Workspace" value={workspaces.id} onchange={(e) => workspaces.switch(e.currentTarget.value)}>
        {#each workspaces.list as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
      </select>
      <button type="button" class="icon" aria-label="Workspace settings" title="Members, roles and new workspaces" onclick={() => wsDialog?.open()}>⚙</button>
      {#if workspaces.current?.role === "viewer"}<span class="badge" title="Viewers read and run collections but cannot change them">read-only</span>{/if}
    </span>
    {#if collection && doc}
      <label class="env">
        Environment
        <select aria-label="Environment" value={app.envs[collection] ?? ""} onchange={(e) => app.selectEnv(collection, e.currentTarget.value || null)}>
          <option value="">none</option>
          {#each doc.environments as env (env)}<option value={env}>{env}</option>{/each}
        </select>
      </label>
    {/if}
    <span class="status" aria-live="polite">
      {#if version}
        server {version.version}
      {:else if offline}
        <span class="s-err">server unreachable: {offline}</span>
      {/if}
    </span>
    <label class="theme">
      <span class="sr-only">Theme</span>
      <select aria-label="Theme" value={theme.choice} onchange={(e) => theme.set(e.currentTarget.value as ThemeChoice)}>
        <option value="system">System theme</option>
        <option value="light">Light</option>
        <option value="dark">Dark</option>
      </select>
    </label>
    <button type="button" class="icon" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" onclick={() => help.showModal()}>⌨</button>
    {#if auth.user}
      <button type="button" class="user" title="Account, API tokens and log out" onclick={() => account?.open()}>
        <span aria-hidden="true">👤</span> {auth.user.username}
      </button>
    {/if}
  </header>
  <aside class="side">
    <div class="switch" role="tablist" aria-label="Sidebar">
      {#each [{ id: "collections", label: "Collections" }, { id: "history", label: "History" }, { id: "runs", label: "Runs" }] as s (s.id)}
        <button type="button" role="tab" aria-selected={side === s.id} aria-controls="side-panel" onclick={() => (side = s.id as Side)}>{s.label}</button>
      {/each}
    </div>
    <div class="side-panel" id="side-panel" role="tabpanel">
      {#if side === "collections"}<Sidebar />{:else if side === "history"}<HistoryPanel />{:else}<RunsPanel />{/if}
    </div>
  </aside>
  <main bind:this={mainEl} tabindex="-1" aria-label="Main area">
    <TabBar />
    {#if tab}
      {#key tab.id}
        {#if tab.kind === "request"}
          <RequestView {tab} bind:this={view} />
        {:else if tab.kind === "scope"}
          <ScopeView {tab} />
        {:else if tab.kind === "environment"}
          <EnvView {tab} />
        {:else if tab.kind === "history"}
          <HistoryView {tab} />
        {:else if tab.kind === "diff"}
          <DiffView {tab} />
        {:else if tab.kind === "runner"}
          <RunnerView {tab} />
        {:else if tab.kind === "flow"}
          <FlowView {tab} />
        {:else}
          <div class="run-tab"><ReportView report={tab.report} runId={tab.runId} idPrefix="run-{tab.id}" /></div>
        {/if}
      {/key}
    {:else}
      <div class="empty">
        <p>Open a request from a collection, or start a new one.</p>
        <button type="button" class="primary" onclick={() => app.newRequest()}>New request</button>
        <p class="muted small">Press <kbd>?</kbd> for the keyboard shortcuts.</p>
      </div>
    {/if}
  </main>
  <PromptDialog />
  {#if auth.user}<AccountDialog bind:this={account} />{/if}
  <WorkspaceDialog bind:this={wsDialog} />
  <dialog bind:this={help} aria-labelledby="help-title" class="help">
    <h2 id="help-title">Keyboard shortcuts</h2>
    <table>
      <tbody>
        {#each SHORTCUTS as sc (sc.action)}
          <tr><td><kbd>{label(sc.keys)}</kbd></td><td>{sc.does}</td></tr>
        {/each}
        <tr><td><kbd>Ctrl+F</kbd></td><td>Search in an editor or response body</td></tr>
        <tr><td><kbd>↑ ↓ ← →</kbd></td><td>Move in the collections list and between tabs of a panel</td></tr>
      </tbody>
    </table>
    <form method="dialog"><button type="submit" class="primary">Close</button></form>
  </dialog>
  <div class="notices" aria-live="polite">
    {#each app.notices as n (n.id)}
      <div class="notice {n.kind}" role={n.kind === "error" ? "alert" : "status"}>
        {n.text}
        <button type="button" class="icon" aria-label="Dismiss" onclick={() => app.dismiss(n.id)}>✕</button>
      </div>
    {/each}
  </div>
</div>

<style>
  .app {
    display: grid;
    grid-template-columns: 17rem 1fr;
    grid-template-rows: auto 1fr;
    height: 100%;
  }
  header {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 1.2rem;
    padding: 0.4rem 0.9rem;
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 1rem;
  }
  .logo {
    color: var(--accent);
  }
  .env {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    font-size: 0.8rem;
    color: var(--muted);
  }
  .skip {
    position: absolute;
    left: -100vw;
    top: 0.3rem;
    z-index: 30;
  }
  .skip:focus {
    left: 0.5rem;
  }
  .theme select {
    font-size: 0.78rem;
    padding: 0.15rem 0.3rem;
  }
  main:focus {
    outline: none;
  }
  .help table {
    border-collapse: collapse;
    margin: 0.5rem 0 1rem;
    font-size: 0.88rem;
  }
  .help td {
    padding: 0.3rem 0.8rem 0.3rem 0;
    border-bottom: 1px solid var(--border);
  }
  .help h2 {
    margin: 0;
    font-size: 1rem;
  }
  .help form {
    display: flex;
    justify-content: flex-end;
  }
  kbd {
    font-family: var(--mono);
    font-size: 0.8rem;
    padding: 0.05rem 0.35rem;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 4px;
    background: var(--bg-soft);
  }
  .ws {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }
  .ws select {
    font-size: 0.8rem;
    padding: 0.15rem 0.3rem;
  }
  .badge {
    font-size: 0.72rem;
    padding: 0.05rem 0.4rem;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--warn);
  }
  .user {
    font-size: 0.8rem;
    padding: 0.15rem 0.5rem;
  }
  .status {
    margin-left: auto;
    color: var(--muted);
    font-size: 0.8rem;
  }
  .side {
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--bg-soft);
    min-height: 0;
  }
  .switch {
    display: flex;
    gap: 0.2rem;
    padding: 0.4rem 0.5rem 0;
    border-bottom: 1px solid var(--border);
  }
  .switch button {
    flex: 1;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    color: var(--muted);
    padding: 0.3rem;
  }
  .switch button[aria-selected="true"] {
    color: var(--fg);
    border-bottom-color: var(--accent);
    font-weight: 600;
  }
  .side-panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 0.5rem;
  }
  .side-panel > :global(.sidebar) {
    padding: 0;
    flex: 1;
    min-height: 0;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .run-tab {
    padding: 0.8rem 1rem;
    overflow: auto;
    height: 100%;
  }
  main > :global(.view) {
    flex: 1;
  }
  .empty {
    margin: auto;
    text-align: center;
  }
  .small {
    font-size: 0.8rem;
  }
  .notices {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    z-index: 10;
  }
  .notice {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    padding: 0.5rem 0.7rem;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--bg-soft);
    box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
    font-size: 0.85rem;
  }
  .notice.error {
    border-color: var(--danger);
    color: var(--danger);
  }
</style>
