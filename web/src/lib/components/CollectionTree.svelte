<script lang="ts">
  import type { TreeItem } from "../api/types";
  import { app } from "../state/app.svelte";
  import Self from "./CollectionTree.svelte";

  interface Props {
    collection: string;
    items: TreeItem[];
    depth?: number;
  }
  let { collection, items, depth = 1 }: Props = $props();
  let open = $state<Record<string, boolean>>({});
  const active = $derived(app.active);
</script>

<ul role={depth === 1 ? "group" : "group"}>
  {#each items as item (item.key)}
    <li role="treeitem" aria-level={depth + 1} aria-expanded={item.type === "folder" ? !!open[item.key] : undefined} aria-selected={item.type === "request" && active?.collection === collection && active?.key === item.key}>
      {#if item.type === "folder"}
        <button type="button" class="row" style="padding-left: {depth * 0.8}rem" onclick={() => (open[item.key] = !open[item.key])}>
          <span class="caret" aria-hidden="true">{open[item.key] ? "▾" : "▸"}</span> 📁 {item.name}
        </button>
        {#if open[item.key]}
          <Self {collection} items={item.items} depth={depth + 1} />
        {/if}
      {:else}
        <button
          type="button"
          class="row"
          class:current={active?.collection === collection && active?.key === item.key}
          style="padding-left: {depth * 0.8 + 0.9}rem"
          onclick={() => app.openRequest(collection, item.key)}
        >
          <span class="method m-{item.method.toLowerCase()}">{item.method}</span> {item.name}
        </button>
      {/if}
    </li>
  {/each}
</ul>

<style>
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
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .current {
    background: var(--accent-bg);
  }
  .caret {
    width: 0.8rem;
    color: var(--muted);
  }
  .method {
    font-family: var(--mono);
    font-size: 0.68rem;
    font-weight: 700;
    min-width: 2.6rem;
  }
</style>
