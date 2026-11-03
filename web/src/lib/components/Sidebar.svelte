<script lang="ts">
  import { app } from "../state/app.svelte";
  import CollectionTree from "./CollectionTree.svelte";

  let open = $state<Record<string, boolean>>({});

  $effect(() => {
    void app.loadCollections();
  });

  async function toggle(id: string) {
    open[id] = !open[id];
    if (open[id] && !app.docs[id]) await app.loadCollection(id);
  }
</script>

<nav class="sidebar" aria-label="Collections">
  <div class="head">
    <h2>Collections</h2>
    <button type="button" class="icon" title="New request (Alt+N)" aria-label="New request" onclick={() => app.newRequest()}>＋</button>
  </div>
  {#if app.collections.length === 0}
    <p class="muted small">No collections in the workspace yet.</p>
  {/if}
  <ul role="tree" aria-label="Collections">
    {#each app.collections as c (c.id)}
      <li role="treeitem" aria-level={1} aria-expanded={!!open[c.id]} aria-selected={false}>
        <button type="button" class="row coll" onclick={() => toggle(c.id)} title={c.error ?? c.description ?? ""}>
          <span class="caret" aria-hidden="true">{open[c.id] ? "▾" : "▸"}</span>
          {c.name ?? c.id}
          {#if c.error}<span class="s-err" title={c.error}>⚠</span>{/if}
        </button>
        {#if open[c.id] && app.docs[c.id]}
          <CollectionTree collection={c.id} items={app.docs[c.id]!.items} />
        {/if}
      </li>
    {/each}
  </ul>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.5rem;
    overflow: auto;
    font-size: 0.85rem;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h2 {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    margin: 0.2rem 0.3rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    gap: 0.35rem;
    align-items: center;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    padding: 0.25rem 0.4rem;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .coll {
    font-weight: 600;
  }
  .caret {
    width: 0.8rem;
    color: var(--muted);
  }
  .small {
    font-size: 0.8rem;
    margin: 0.3rem;
  }
</style>
