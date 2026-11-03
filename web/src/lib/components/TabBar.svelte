<script lang="ts">
  import { app, type Tab } from "../state/app.svelte";

  function close(e: Event, tab: Tab) {
    e.stopPropagation();
    if (tab.dirty && !confirm(`Close "${tab.title}" without saving?`)) return;
    app.close(tab);
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
        title={tab.collection ? `${tab.collection}/${tab.key}` : tab.title}
      >
        {#if tab.kind === "request"}<span class="method m-{tab.draft.method.toLowerCase()}">{tab.draft.method}</span>{/if}
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
