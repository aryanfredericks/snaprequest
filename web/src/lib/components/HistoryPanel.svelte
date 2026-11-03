<script lang="ts">
  import type { HistorySummary } from "../api/types";
  import { app, message, type HistoryFilter } from "../state/app.svelte";
  import { formatMs, statusClass } from "../model/http";
  import { timeAgo } from "../model/history";
  import { METHODS } from "../model/draft";
  import VirtualList from "./VirtualList.svelte";

  let filter = $state<HistoryFilter>({ url: "", method: "", status: "", collection: "" });
  let entries = $state<HistorySummary[]>([]);
  let loading = $state(false);
  let done = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<number[]>([]);
  let now = $state(Date.now());
  const PAGE = 100;

  async function load(reset: boolean) {
    if (loading) return;
    loading = true;
    error = null;
    try {
      const before = reset ? undefined : entries.at(-1)?.id;
      const page = await app.listHistory(filter, before, PAGE);
      entries = reset ? page : [...entries, ...page];
      done = page.length < PAGE;
      now = Date.now();
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  // Reload on filter changes (debounced while typing) and after new requests.
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    void app.historyVersion;
    const f = $state.snapshot(filter);
    void f;
    clearTimeout(timer);
    timer = setTimeout(() => void load(true), 200);
    return () => clearTimeout(timer);
  });

  function toggle(id: number) {
    selected = selected.includes(id) ? selected.filter((x) => x !== id) : [...selected.slice(-1), id];
  }

  function more() {
    if (!done && !loading) void load(false);
  }
</script>

<div class="history">
  <div class="filters">
    <input type="search" bind:value={filter.url} placeholder="Search URLs" aria-label="Search history by URL" />
    <div class="row">
      <select bind:value={filter.method} aria-label="Method filter">
        <option value="">any method</option>
        {#each METHODS as m (m)}<option value={m}>{m}</option>{/each}
      </select>
      <select bind:value={filter.status} aria-label="Status filter">
        <option value="">any status</option>
        <option value="2">2xx</option>
        <option value="3">3xx</option>
        <option value="4">4xx</option>
        <option value="5">5xx</option>
        <option value="failed">failed</option>
      </select>
      <select bind:value={filter.collection} aria-label="Collection filter">
        <option value="">anywhere</option>
        {#each app.collections as c (c.id)}<option value={c.id}>{c.name ?? c.id}</option>{/each}
      </select>
    </div>
    <div class="row actions">
      <button type="button" disabled={selected.length !== 2} onclick={() => app.compare(selected[0]!, selected[1]!)} title="Select two entries to compare their responses">
        Compare{selected.length ? ` (${selected.length}/2)` : ""}
      </button>
      <button type="button" class="link" onclick={async () => (await app.deleteHistory(null)) && (selected = [])}>Clear all</button>
    </div>
  </div>
  {#if error}<p class="s-err small" role="alert">{error}</p>{/if}
  <div class="list">
    {#if entries.length === 0 && !loading}
      <p class="muted small">{filter.url || filter.method || filter.status || filter.collection ? "Nothing matches." : "Requests you send appear here."}</p>
    {:else}
      <VirtualList count={entries.length} rowHeight={46} label="History" onend={more}>
        {#snippet row(i)}
          {@const h = entries[i]!}
          <div class="entry" class:sel={selected.includes(h.id)}>
            <input type="checkbox" checked={selected.includes(h.id)} onchange={() => toggle(h.id)} aria-label="Select #{h.id} for comparing" />
            <button type="button" class="open" onclick={() => app.openHistory(h.id)} title={h.url}>
              <span class="top">
                <span class="method m-{h.method.toLowerCase()}">{h.method}</span>
                <span class="status {statusClass(h.status)}">{h.status ?? "failed"}</span>
                <span class="url">{h.url.replace(/^\w+:\/\//, "")}</span>
              </span>
              <span class="meta muted">
                #{h.id} · {timeAgo(h.created_at, now)} · {formatMs(h.duration_ms)}{h.collection ? ` · ${h.collection}/${h.request_key}` : ""}
              </span>
            </button>
          </div>
        {/snippet}
      </VirtualList>
    {/if}
  </div>
  {#if loading}<p class="muted small" role="status">Loading…</p>{/if}
</div>

<style>
  .history {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 0;
    flex: 1;
  }
  .filters {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .row {
    display: flex;
    gap: 0.3rem;
  }
  .row select {
    flex: 1;
    min-width: 0;
    padding: 0.2rem;
    font-size: 0.75rem;
  }
  .actions {
    justify-content: space-between;
  }
  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    height: 100%;
    border-bottom: 1px solid var(--border);
    white-space: normal;
  }
  .entry.sel {
    background: var(--accent-bg);
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    text-align: left;
    background: none;
    border: none;
    padding: 0.2rem 0.2rem;
  }
  .open:hover {
    background: var(--bg-hover);
  }
  .top {
    display: flex;
    gap: 0.35rem;
    align-items: baseline;
    min-width: 0;
  }
  .method {
    font-family: var(--mono);
    font-size: 0.68rem;
    font-weight: 700;
  }
  .status {
    font-family: var(--mono);
    font-size: 0.72rem;
    font-weight: 700;
  }
  .url {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.8rem;
  }
  .meta {
    font-size: 0.7rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .small {
    font-size: 0.8rem;
    margin: 0.3rem;
  }
</style>
