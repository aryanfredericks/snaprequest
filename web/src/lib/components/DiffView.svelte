<script lang="ts">
  import { MergeView } from "@codemirror/merge";
  import { EditorState } from "@codemirror/state";
  import { EditorView } from "@codemirror/view";
  import { app, type DiffTab } from "../state/app.svelte";
  import { theme } from "../editor/cm";
  import { comparableBody, comparableHeaders } from "../model/history";
  import { formatMs, statusClass } from "../model/http";
  import type { HistoryRecord, Timings } from "../api/types";
  import Tabs from "./Tabs.svelte";

  let { tab }: { tab: DiffTab } = $props();
  type Part = "body" | "headers" | "timing";
  let part = $state<Part>("body");
  let host: HTMLDivElement | undefined = $state();

  const text = (r: HistoryRecord, p: Part) =>
    p === "body" ? comparableBody(r.response?.body, r.response?.headers) : comparableHeaders(r.response?.headers);

  $effect(() => {
    if (!host || part === "timing") return;
    const ext = [theme, EditorView.lineWrapping, EditorState.readOnly.of(true), EditorView.editable.of(false)];
    const view = new MergeView({
      parent: host,
      a: { doc: text(tab.left, part), extensions: [...ext, EditorView.contentAttributes.of({ "aria-label": `#${tab.left.id} ${part}` })] },
      b: { doc: text(tab.right, part), extensions: [...ext, EditorView.contentAttributes.of({ "aria-label": `#${tab.right.id} ${part}` })] },
      collapseUnchanged: { margin: 3, minSize: 6 },
      gutter: true,
    });
    return () => view.destroy();
  });

  const phases: { key: keyof Timings; label: string }[] = [
    { key: "dns_ms", label: "DNS" },
    { key: "connect_ms", label: "connect" },
    { key: "tls_ms", label: "TLS" },
    { key: "send_ms", label: "send" },
    { key: "wait_ms", label: "wait" },
    { key: "download_ms", label: "download" },
    { key: "total_ms", label: "total" },
  ];
  const delta = (a: number | null | undefined, b: number | null | undefined) =>
    a === null || a === undefined || b === null || b === undefined ? "" : `${b - a >= 0 ? "+" : ""}${formatMs(b - a)}`;
</script>

<section class="diff" aria-label="Compare responses">
  <div class="sides">
    {#each [tab.left, tab.right] as r (r.id)}
      <div class="side">
        <button type="button" class="link" onclick={() => app.openHistory(r.id)}>#{r.id}</button>
        <span class="method m-{r.method.toLowerCase()}">{r.method}</span>
        <span class="url" title={r.url}>{r.url}</span>
        <strong class={statusClass(r.status)}>{r.status ?? "failed"}</strong>
        <span class="muted">{new Date(r.created_at).toLocaleTimeString()}</span>
      </div>
    {/each}
  </div>
  <Tabs
    tabs={[{ id: "body", label: "Body" }, { id: "headers", label: "Headers" }, { id: "timing", label: "Timing" }]}
    selected={part}
    label="Compare"
    idPrefix="diff-{tab.id}"
    onselect={(p) => (part = p)}
  />
  <div class="panel" role="tabpanel" id="diff-{tab.id}-panel" aria-labelledby="diff-{tab.id}-tab-{part}">
    {#if part === "timing"}
      <table aria-label="Timing comparison">
        <thead><tr><th>Phase</th><th>#{tab.left.id}</th><th>#{tab.right.id}</th><th>Change</th></tr></thead>
        <tbody>
          {#each phases as p (p.key)}
            {@const a = tab.left.response?.timings[p.key]}
            {@const b = tab.right.response?.timings[p.key]}
            <tr class:total={p.key === "total_ms"}>
              <th scope="row">{p.label}</th><td>{formatMs(a)}</td><td>{formatMs(b)}</td><td>{delta(a, b)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="muted small">{part === "body" ? "JSON bodies are pretty-printed first." : "Header names are lower-cased and sorted."} Unchanged stretches are folded.</p>
      {#key part}<div class="merge" bind:this={host}></div>{/key}
    {/if}
  </div>
</section>

<style>
  .diff {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.7rem 0.9rem;
    height: 100%;
    min-height: 0;
  }
  .sides {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.8rem;
  }
  .side {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    min-width: 0;
    font-size: 0.85rem;
  }
  .method {
    font-family: var(--mono);
    font-weight: 700;
    font-size: 0.75rem;
  }
  .url {
    font-family: var(--mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .merge {
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .merge :global(.cm-mergeView) {
    max-height: none;
  }
  .merge :global(.cm-changedLine) {
    background: var(--accent-bg);
  }
  .merge :global(.cm-deletedChunk) {
    background: var(--danger-bg);
  }
  .small {
    font-size: 0.78rem;
    margin: 0 0 0.3rem;
  }
  table {
    border-collapse: collapse;
    font-size: 0.85rem;
    min-width: 28rem;
  }
  th,
  td {
    text-align: right;
    padding: 0.25rem 0.8rem;
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
  }
  th:first-child {
    text-align: left;
    font-family: inherit;
  }
  .total {
    font-weight: 700;
  }
</style>
