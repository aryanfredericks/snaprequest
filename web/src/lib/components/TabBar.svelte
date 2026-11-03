<script lang="ts">
  import { app, type Tab } from "../state/app.svelte";

  function close(e: Event, tab: Tab) {
    e.stopPropagation();
    app.close(tab);
  }
  function where(tab: Tab): string {
    switch (tab.kind) {
      case "request":
        return tab.collection ? `${tab.collection}/${tab.key}` : tab.title;
      case "environment":
        return `${tab.collection}: environment ${tab.name}`;
      case "scope":
        return `${tab.collection}${tab.key ? `/${tab.key}` : ""} settings`;
      case "history":
        return `History #${tab.record.id}: ${tab.record.method} ${tab.record.url}`;
      case "diff":
        return `Comparing history #${tab.left.id} and #${tab.right.id}`;
      case "runner":
        return `Runner: ${tab.target.collection}${tab.target.key ? `/${tab.target.key}` : ""}`;
      case "run":
        return `Run #${tab.runId}: ${tab.report.target.name}`;
      case "flow":
        return `Flow ${tab.collection}/${tab.key}`;
    }
  }
</script>

<div class="bar" role="tablist" aria-label="Open requests">
  {#each app.tabs as tab (tab.id)}
    <div class="tab" class:active={tab.id === app.activeId}>
      <button
        type="button"
        role="tab"
        aria-selected={tab.id === app.activeId}
        class="open"
        onclick={() => app.activate(tab)}
        onauxclick={(e) => e.button === 1 && close(e, tab)}
        title={where(tab)}
      >
        {#if tab.kind === "request"}<span class="method m-{tab.draft.method.toLowerCase()}">{tab.draft.method}</span>
        {:else if tab.kind === "environment"}<span class="kind" aria-hidden="true">◎</span>
        {:else if tab.kind === "history"}<span class="kind" aria-hidden="true">↺</span>
        {:else if tab.kind === "diff"}<span class="kind" aria-hidden="true">⇄</span>
        {:else if tab.kind === "flow"}<span class="kind flow" aria-hidden="true">⇢</span>
        {:else if tab.kind === "scope"}<span class="kind" aria-hidden="true">⚙</span>
        {:else}<span class="kind" aria-hidden="true">▶</span>{/if}
        <span class="label">{tab.title}</span>
        {#if tab.dirty}<span class="dirty" aria-label="unsaved changes">●</span>{/if}
      </button>
      <button type="button" class="icon x" aria-label="Close {tab.title}" onclick={(e) => close(e, tab)}>✕</button>
    </div>
  {/each}
  <button type="button" class="icon new" aria-label="New request" title="New request (Alt+N)" onclick={() => app.newRequest()}>＋</button>
</div>

<style>
  .bar {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--border);
    background: var(--bg-soft);
    overflow-x: auto;
    min-height: 2.2rem;
  }
  .tab {
    display: flex;
    align-items: center;
    border-right: 1px solid var(--border);
    max-width: 16rem;
  }
  .tab.active {
    background: var(--bg);
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .open {
    display: flex;
    gap: 0.35rem;
    align-items: center;
    border: none;
    background: none;
    min-width: 0;
    padding: 0.3rem 0.2rem 0.3rem 0.6rem;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .method {
    font-family: var(--mono);
    font-size: 0.68rem;
    font-weight: 700;
  }
  .kind {
    color: var(--muted);
  }
  .dirty {
    color: var(--accent);
    font-size: 0.6rem;
  }
  .x {
    font-size: 0.7rem;
  }
  .new {
    padding: 0 0.7rem;
  }
</style>
