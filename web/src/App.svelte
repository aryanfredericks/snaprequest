<script lang="ts">
  import { api, serverVersion, type ServerVersion } from "./lib/api/client";
  import DiffView from "./lib/components/DiffView.svelte";
  import EnvView from "./lib/components/EnvView.svelte";
  import HistoryPanel from "./lib/components/HistoryPanel.svelte";
  import HistoryView from "./lib/components/HistoryView.svelte";
  import PromptDialog from "./lib/components/PromptDialog.svelte";
  import RequestView from "./lib/components/RequestView.svelte";
  import ScopeView from "./lib/components/ScopeView.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import { app } from "./lib/state/app.svelte";
  import { parseRoute } from "./lib/state/route";

  let version = $state<ServerVersion | null>(null);
  let offline = $state<string | null>(null);
  let view: RequestView | undefined = $state();
  type Side = "collections" | "history";
  let side = $state<Side>((localStorage.getItem("snapreq.side") as Side | null) ?? "collections");
  $effect(() => localStorage.setItem("snapreq.side", side));

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
    else if (r.kind === "new" && !app.tabs.some((t) => t.kind === "request" && t.collection === null)) app.newRequest();
  }

  const tab = $derived(app.active);
  const collection = $derived(tab?.collection ?? null);
  const doc = $derived(collection ? app.docs[collection] : null);

  function keys(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key === "Enter" && tab) {
      e.preventDefault();
      void app.send(tab);
    } else if (mod && e.key.toLowerCase() === "s" && tab) {
      e.preventDefault();
      if (tab.kind === "request") view?.save();
      else void app.save(tab);
    } else if (e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      app.newRequest();
    } else if (e.altKey && e.key.toLowerCase() === "w" && tab) {
      e.preventDefault();
      app.close(tab);
    }
  }
</script>

<svelte:window onkeydown={keys} />

<div class="app">
  <header>
    <h1><span class="logo" aria-hidden="true">⚡</span> SnapRequest</h1>
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
  </header>
  <aside class="side">
    <div class="switch" role="tablist" aria-label="Sidebar">
      {#each [{ id: "collections", label: "Collections" }, { id: "history", label: "History" }] as s (s.id)}
        <button type="button" role="tab" aria-selected={side === s.id} aria-controls="side-panel" onclick={() => (side = s.id as Side)}>{s.label}</button>
      {/each}
    </div>
    <div class="side-panel" id="side-panel" role="tabpanel">
      {#if side === "collections"}<Sidebar />{:else}<HistoryPanel />{/if}
    </div>
  </aside>
  <main>
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
        {:else}
          <DiffView {tab} />
        {/if}
      {/key}
    {:else}
      <div class="empty">
        <p>Open a request from a collection, or start a new one.</p>
        <button type="button" class="primary" onclick={() => app.newRequest()}>New request</button>
        <p class="muted small">Shortcuts: Ctrl+Enter send · Ctrl+S save · Alt+N new request · Alt+W close tab</p>
      </div>
    {/if}
  </main>
  <PromptDialog />
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
