<script lang="ts">
  import { api, serverVersion, type ServerVersion } from "./lib/api/client";
  import RequestView from "./lib/components/RequestView.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import { app } from "./lib/state/app.svelte";
  import { parseRoute } from "./lib/state/route";

  let version = $state<ServerVersion | null>(null);
  let offline = $state<string | null>(null);
  let view: RequestView | undefined = $state();

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
    else if (r.kind === "new" && !app.tabs.some((t) => t.collection === null)) app.newRequest();
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
      view?.save();
    } else if (e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      app.newRequest();
    } else if (e.altKey && e.key.toLowerCase() === "w" && tab) {
      e.preventDefault();
      if (!tab.dirty || confirm(`Close "${tab.title}" without saving?`)) app.close(tab);
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
        <select value={app.envs[collection] ?? ""} onchange={(e) => app.selectEnv(collection, e.currentTarget.value || null)}>
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
  <Sidebar />
  <main>
    <TabBar />
    {#if tab}
      {#key tab.id}
        <RequestView {tab} bind:this={view} />
      {/key}
    {:else}
      <div class="empty">
        <p>Open a request from a collection, or start a new one.</p>
        <button type="button" class="primary" onclick={() => app.newRequest()}>New request</button>
        <p class="muted small">Shortcuts: Ctrl+Enter send · Ctrl+S save · Alt+N new request · Alt+W close tab</p>
      </div>
    {/if}
  </main>
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
  :global(.sidebar) {
    border-right: 1px solid var(--border);
    background: var(--bg-soft);
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
