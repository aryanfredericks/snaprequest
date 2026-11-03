<script lang="ts" generics="T extends string">
  // An accessible tab strip (WAI-ARIA tabs pattern: arrow keys, Home/End).
  interface Props {
    tabs: { id: T; label: string; badge?: string | number }[];
    selected: T;
    label: string;
    idPrefix: string;
    onselect: (id: T) => void;
  }
  let { tabs, selected, label, idPrefix, onselect }: Props = $props();
  let list: HTMLDivElement;

  function key(e: KeyboardEvent) {
    const i = tabs.findIndex((t) => t.id === selected);
    let next = -1;
    if (e.key === "ArrowRight") next = (i + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    if (next < 0) return;
    e.preventDefault();
    onselect(tabs[next]!.id);
    list.querySelectorAll<HTMLButtonElement>("[role=tab]")[next]?.focus();
  }
</script>

<div class="tabs" role="tablist" aria-label={label} bind:this={list} tabindex="-1" onkeydown={key}>
  {#each tabs as t (t.id)}
    <button
      type="button"
      role="tab"
      id="{idPrefix}-tab-{t.id}"
      aria-selected={t.id === selected}
      aria-controls="{idPrefix}-panel"
      tabindex={t.id === selected ? 0 : -1}
      onclick={() => onselect(t.id)}
    >
      {t.label}
      {#if t.badge !== undefined && t.badge !== "" && t.badge !== 0}<span class="badge">{t.badge}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .tabs {
    display: flex;
    gap: 0.1rem;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
    flex-shrink: 0;
  }
  button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--muted);
    padding: 0.4rem 0.7rem;
    cursor: pointer;
    white-space: nowrap;
    font: inherit;
    font-size: 0.85rem;
  }
  button:hover {
    color: var(--fg);
  }
  button[aria-selected="true"] {
    color: var(--fg);
    border-bottom-color: var(--accent);
  }
  .badge {
    margin-left: 0.3rem;
    font-size: 0.7rem;
    padding: 0 0.35rem;
    border-radius: 1rem;
    background: var(--bg-hover);
    color: var(--fg-soft);
  }
</style>
