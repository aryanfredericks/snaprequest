<script lang="ts">
  import type { TreeItem } from "../api/types";
  import { app, splitKey } from "../state/app.svelte";
  import { canDrop, nodeId, tree, type DropTarget } from "../state/tree.svelte";
  import { prompt } from "../state/prompt.svelte";
  import ItemMenu, { type MenuItem } from "./ItemMenu.svelte";
  import Self from "./CollectionTree.svelte";

  interface Props {
    collection: string;
    items: TreeItem[];
    /** Key of the folder holding `items` ("" at the root). */
    parent?: string;
    depth?: number;
  }
  let { collection, items, parent = "", depth = 1 }: Props = $props();

  const active = $derived(app.active);
  let drop = $state<{ key: string; mode: "before" | "into" } | null>(null);

  const name = (key: string) => splitKey(key)[1];
  const isOpen = (key: string) => !!tree.expanded[nodeId(collection, key)];
  const toggle = (key: string) => (tree.expanded[nodeId(collection, key)] = !isOpen(key));

  function folders(list: TreeItem[], out: string[] = []): string[] {
    for (const i of list) if (i.type === "folder") (out.push(i.key), folders(i.items, out));
    return out;
  }

  function menu(item: TreeItem, index: number): MenuItem[] {
    const kind = item.type;
    const prev = items[index - 1];
    const afterNext = items[index + 2];
    const common: MenuItem[] = [
      { label: "Move up", disabled: !prev, action: () => app.move(collection, kind, item.key, parent, { before: prev ? name(prev.key) : null }) },
      {
        label: "Move down",
        disabled: index === items.length - 1,
        action: () => app.move(collection, kind, item.key, parent, { before: afterNext ? name(afterNext.key) : null }),
      },
      { label: "Move to folder…", action: () => moveTo(item) },
      { label: "Rename…", action: () => app.rename(collection, kind, item.key) },
    ];
    if (item.type === "folder") {
      return [
        { label: "New request…", action: () => app.createRequest(collection, item.key) },
        { label: "New folder…", action: () => app.createFolder(collection, item.key) },
        { label: "Folder settings", action: () => app.openScope(collection, item.key) },
        ...common,
        { label: "Delete…", danger: true, action: () => app.remove(collection, "folder", item.key) },
      ];
    }
    return [
      { label: "Open", action: () => app.openRequest(collection, item.key) },
      { label: "Duplicate", action: () => app.duplicate(collection, item.key) },
      ...common,
      { label: "Delete…", danger: true, action: () => app.remove(collection, "request", item.key) },
    ];
  }

  async function moveTo(item: TreeItem) {
    const doc = app.docs[collection];
    const all = ["", ...folders(doc?.items ?? [])].filter(
      (f) => canDrop({ collection, kind: item.type, key: item.key }, collection, f) && f !== parent,
    );
    if (!all.length) return app.notify("There is no other folder to move it to.");
    const v = await prompt.ask({
      title: `Move ${name(item.key)}`,
      fields: [{ name: "to", label: "Folder", type: "select", options: all.map((f) => ({ value: f, label: f || "(collection root)" })) }],
      submit: "Move",
    });
    if (v) await app.move(collection, item.type, item.key, v.to ?? "", { before: null });
  }

  // --- Drag and drop ------------------------------------------------------------------

  function target(item: TreeItem, e: DragEvent): (DropTarget & { mode: "before" | "into" }) | null {
    const el = e.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    const top = e.clientY - r.top < r.height * 0.3;
    if (item.type === "folder" && !top) return { to: item.key, before: null, mode: "into" };
    return { to: parent, before: name(item.key), mode: "before" };
  }

  function over(item: TreeItem, e: DragEvent) {
    const t = target(item, e);
    const d = tree.dragging;
    if (!t || !canDrop(d, collection, t.to) || d?.key === item.key) {
      drop = null;
      return;
    }
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    drop = { key: item.key, mode: t.mode };
  }

  async function dropOn(item: TreeItem, e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    const t = target(item, e);
    const d = tree.dragging;
    drop = null;
    tree.dragging = null;
    if (!t || !d || !canDrop(d, collection, t.to) || d.key === item.key) return;
    if (t.mode === "into") tree.expanded[nodeId(collection, t.to)] = true;
    await app.move(collection, d.kind, d.key, t.to, { before: t.before });
  }

  function start(item: TreeItem, e: DragEvent) {
    tree.dragging = { collection, kind: item.type, key: item.key };
    e.dataTransfer?.setData("text/plain", item.key);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }
</script>

<ul role="group">
  {#each items as item, i (item.key)}
    {@const current = item.type === "request" && active?.kind === "request" && active.collection === collection && active.key === item.key}
    <li
      role="treeitem"
      aria-level={depth + 1}
      aria-expanded={item.type === "folder" ? isOpen(item.key) : undefined}
      aria-selected={current}
    >
      <div
        class="row-wrap"
        class:current
        class:drop-before={drop?.key === item.key && drop.mode === "before"}
        class:drop-into={drop?.key === item.key && drop.mode === "into"}
        role="presentation"
        draggable="true"
        ondragstart={(e) => start(item, e)}
        ondragend={() => ((tree.dragging = null), (drop = null))}
        ondragover={(e) => over(item, e)}
        ondragleave={() => (drop = null)}
        ondrop={(e) => dropOn(item, e)}
      >
        {#if item.type === "folder"}
          <button
            type="button"
            class="row"
            data-tree-row
            aria-expanded={isOpen(item.key)}
            style="padding-left: {depth * 0.8}rem"
            onclick={() => toggle(item.key)}
          >
            <span class="caret" aria-hidden="true">{isOpen(item.key) ? "▾" : "▸"}</span>
            <span aria-hidden="true">📁</span>
            {item.name}
          </button>
        {:else}
          <button
            type="button"
            class="row"
            data-tree-row
            style="padding-left: {depth * 0.8 + 0.9}rem"
            onclick={() => app.openRequest(collection, item.key)}
          >
            <span class="method m-{item.method.toLowerCase()}">{item.method}</span>
            {item.name}
          </button>
        {/if}
        <ItemMenu items={menu(item, i)} label="Actions for {item.name}" />
      </div>
      {#if item.type === "folder" && isOpen(item.key)}
        <Self {collection} items={item.items} parent={item.key} depth={depth + 1} />
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
  .row-wrap {
    display: flex;
    align-items: center;
    border-radius: 4px;
    border-top: 2px solid transparent;
  }
  .row-wrap:hover {
    background: var(--bg-hover);
  }
  .current {
    background: var(--accent-bg);
  }
  .drop-before {
    border-top-color: var(--accent);
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
    padding: 0.2rem 0.4rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
