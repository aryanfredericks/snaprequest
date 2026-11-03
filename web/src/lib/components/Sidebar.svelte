<script lang="ts">
  import { app } from "../state/app.svelte";
  import { canDrop, nodeId, tree } from "../state/tree.svelte";
  import CollectionTree from "./CollectionTree.svelte";
  import ItemMenu, { type MenuItem } from "./ItemMenu.svelte";

  let dropRoot = $state<string | null>(null);

  $effect(() => {
    void app.loadCollections();
  });

  async function toggle(id: string) {
    const key = nodeId(id);
    tree.expanded[key] = !tree.expanded[key];
    if (tree.expanded[key] && !app.docs[id]) await app.loadCollection(id);
  }

  // Collections opened in an earlier visit (or by a link) are loaded again.
  $effect(() => {
    for (const c of app.collections) if (tree.expanded[nodeId(c.id)] && !app.docs[c.id]) void app.loadCollection(c.id);
  });

  function collectionMenu(id: string): MenuItem[] {
    return [
      { label: "New request…", action: () => app.createRequest(id, "") },
      { label: "New folder…", action: () => app.createFolder(id, "") },
      { label: "New environment…", action: () => app.createEnvironment(id) },
      { label: "Collection settings", action: () => app.openScope(id, "") },
      { label: "Reload from disk", action: () => app.loadCollection(id) },
      { label: "Delete collection…", danger: true, action: () => app.deleteCollection(id) },
    ];
  }

  function envMenu(id: string, env: string): MenuItem[] {
    return [
      { label: "Edit", action: () => app.openEnvironment(id, env) },
      { label: app.envs[id] === env ? "Stop using" : "Use", action: () => app.selectEnv(id, app.envs[id] === env ? null : env) },
      { label: "Delete…", danger: true, action: () => app.deleteEnvironment(id, env) },
    ];
  }

  /** Arrow keys move between rows; Right/Left open and close folders (tree pattern). */
  function keys(e: KeyboardEvent) {
    const el = document.activeElement as HTMLElement | null;
    if (!el?.matches("[data-tree-row]")) return;
    const rows = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("[data-tree-row]")];
    const i = rows.indexOf(el);
    const expanded = el.getAttribute("aria-expanded");
    if (e.key === "ArrowDown") rows[i + 1]?.focus();
    else if (e.key === "ArrowUp") rows[i - 1]?.focus();
    else if (e.key === "Home") rows[0]?.focus();
    else if (e.key === "End") rows[rows.length - 1]?.focus();
    else if (e.key === "ArrowRight" && expanded === "false") el.click();
    else if (e.key === "ArrowLeft" && expanded === "true") el.click();
    else return;
    e.preventDefault();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<nav class="sidebar" aria-label="Collections" onkeydown={keys}>
  <div class="head">
    <h2>Collections</h2>
    <span>
      <button type="button" class="icon" title="New collection" aria-label="New collection" onclick={() => app.createCollection()}>⊞</button>
      <button type="button" class="icon" title="New request (Alt+N)" aria-label="New request" onclick={() => app.newRequest()}>＋</button>
    </span>
  </div>
  {#if app.collections.length === 0}
    <p class="muted small">No collections in the workspace yet.</p>
  {/if}
  <ul role="tree" aria-label="Collections">
    {#each app.collections as c (c.id)}
      {@const open = !!tree.expanded[nodeId(c.id)]}
      {@const doc = app.docs[c.id]}
      <li role="treeitem" aria-level={1} aria-expanded={open} aria-selected={false}>
        <div
          class="row-wrap"
          class:drop-into={dropRoot === c.id}
          role="presentation"
          ondragover={(e) => {
            if (canDrop(tree.dragging, c.id, "")) {
              e.preventDefault();
              dropRoot = c.id;
            }
          }}
          ondragleave={() => (dropRoot = null)}
          ondrop={async (e) => {
            e.preventDefault();
            dropRoot = null;
            const d = tree.dragging;
            tree.dragging = null;
            if (d && canDrop(d, c.id, "")) await app.move(c.id, d.kind, d.key, "", { before: null });
          }}
        >
          <button type="button" class="row coll" data-tree-row aria-expanded={open} onclick={() => toggle(c.id)} title={c.error ?? c.description ?? ""}>
            <span class="caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
            {c.name ?? c.id}
            {#if c.error}<span class="s-err" title={c.error}>⚠</span>{/if}
          </button>
          <ItemMenu items={collectionMenu(c.id)} label="Actions for {c.name ?? c.id}" />
        </div>
        {#if open && doc}
          <CollectionTree collection={c.id} items={doc.items} />
          {#if doc.flows.length}
            <h3 class="group">Flows</h3>
            <ul role="group">
              {#each doc.flows as f (f.key)}
                <li role="treeitem" aria-level={2} aria-selected={false}>
                  <div class="row-wrap">
                    <button type="button" class="row sub" data-tree-row title="{f.key}: {f.steps} steps" onclick={() => app.notify("The flow editor arrives with P7.7.")}>
                      <span class="flow" aria-hidden="true">⇢</span> {f.name}
                    </button>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
          <h3 class="group">
            Environments
            <button type="button" class="icon" aria-label="New environment in {c.name ?? c.id}" title="New environment" onclick={() => app.createEnvironment(c.id)}>＋</button>
          </h3>
          <ul role="group">
            {#each doc.environments as env (env)}
              <li role="treeitem" aria-level={2} aria-selected={app.envs[c.id] === env}>
                <div class="row-wrap">
                  <button type="button" class="row sub" data-tree-row onclick={() => app.openEnvironment(c.id, env)}>
                    <span class="dot" class:on={app.envs[c.id] === env} aria-hidden="true">●</span>
                    {env}
                    {#if app.envs[c.id] === env}<span class="muted small-inline">(in use)</span>{/if}
                  </button>
                  <ItemMenu items={envMenu(c.id, env)} label="Actions for environment {env}" />
                </div>
              </li>
            {:else}
              <li class="muted small none">none</li>
            {/each}
          </ul>
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
  .group {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    margin: 0.5rem 0 0.1rem 1rem;
    font-weight: 600;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row-wrap {
    display: flex;
    align-items: center;
    border-radius: 4px;
  }
  .row-wrap:hover {
    background: var(--bg-hover);
  }
  .drop-into {
    background: var(--accent-bg);
    outline: 1px dashed var(--accent);
  }
  .row {
    flex: 1;
    display: flex;
    gap: 0.35rem;
    align-items: center;
    min-width: 0;
    text-align: left;
    background: none;
    border: none;
    padding: 0.25rem 0.4rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    padding-left: 1.7rem;
  }
  .coll {
    font-weight: 600;
  }
  .caret {
    width: 0.8rem;
    color: var(--muted);
  }
  .flow {
    color: var(--accent);
  }
  .dot {
    font-size: 0.55rem;
    color: var(--border);
  }
  .dot.on {
    color: var(--ok);
  }
  .small {
    font-size: 0.8rem;
    margin: 0.3rem;
  }
  .small-inline {
    font-size: 0.75rem;
  }
  .none {
    padding-left: 1.7rem;
  }
</style>
