<script lang="ts">
  // Server-Sent Events of a response: live while the stream is read, then the parsed body.
  import type { SseEvent } from "../api/types";

  interface Props {
    events: SseEvent[];
    live?: boolean;
  }
  let { events, live = false }: Props = $props();

  let filter = $state("");
  let type = $state("");
  let expanded = $state<Record<number, boolean>>({});

  const types = $derived([...new Set(events.map((e) => e.event))].sort());
  const shown = $derived(
    events
      .map((e, i) => ({ e, i }))
      .filter(({ e }) => (!type || e.event === type) && (!filter.trim() || e.data.toLowerCase().includes(filter.trim().toLowerCase()))),
  );

  function pretty(data: string): string | null {
    const t = data.trim();
    if (!(t.startsWith("{") || t.startsWith("["))) return null;
    try {
      return JSON.stringify(JSON.parse(t), null, 2);
    } catch {
      return null;
    }
  }
</script>

<div class="events">
  <div class="filters">
    <span class="muted" role="status">{events.length} event(s){live ? ", receiving…" : ""}</span>
    <select bind:value={type} aria-label="Event type">
      <option value="">All types</option>
      {#each types as t (t)}<option value={t}>{t}</option>{/each}
    </select>
    <input type="search" bind:value={filter} placeholder="Filter data" aria-label="Filter events" />
  </div>
  <ol class="list" aria-label="Events">
    {#each shown as { e, i } (i)}
      {@const json = pretty(e.data)}
      <li>
        <span class="at">{e.at_ms !== undefined ? `${(e.at_ms / 1000).toFixed(3)} s` : `#${i + 1}`}</span>
        <span class="type">{e.event}</span>
        <span class="id">{e.id ?? ""}</span>
        <span class="data">
          {#if json && expanded[i]}<pre>{json}</pre>{:else}<code>{e.data}</code>{/if}
        </span>
        {#if json}
          <button type="button" class="link" aria-expanded={!!expanded[i]} onclick={() => (expanded[i] = !expanded[i])}>{expanded[i] ? "raw" : "pretty"}</button>
        {/if}
      </li>
    {:else}
      <li class="empty muted">{live ? "Waiting for events…" : "No events."}</li>
    {/each}
  </ol>
</div>

<style>
  .events {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 0;
  }
  .filters {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .filters input {
    flex: 1;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 0.82rem;
  }
  .list li {
    display: grid;
    grid-template-columns: 5rem 7rem 4rem 1fr auto;
    gap: 0.4rem;
    padding: 0.2rem 0.3rem;
    border-bottom: 1px solid var(--border);
    align-items: start;
  }
  .list li.empty {
    display: block;
  }
  .at,
  .id {
    color: var(--muted);
    font-family: var(--mono);
  }
  .type {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .data {
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
  .data code,
  .data pre {
    font-family: var(--mono);
    margin: 0;
  }
</style>
