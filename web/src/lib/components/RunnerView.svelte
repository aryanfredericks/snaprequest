<script lang="ts">
  import { app, type RunnerTab } from "../state/app.svelte";
  import { formatMs, statusClass } from "../model/http";
  import { parseData } from "../model/runs";
  import KeyValueEditor from "./KeyValueEditor.svelte";
  import ReportView from "./ReportView.svelte";

  let { tab }: { tab: RunnerTab } = $props();
  const doc = $derived(app.docs[tab.target.collection]);
  const rows = $derived.by(() => {
    try {
      return { rows: parseData(tab.data), error: null };
    } catch (e) {
      return { rows: [], error: e instanceof Error ? e.message : String(e) };
    }
  });
  const done = $derived(tab.progress.filter((p) => p.done).length);
  const failed = $derived(tab.progress.filter((p) => p.done && !p.passed).length);

  async function loadFile(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    if (file) tab.data = await file.text();
  }
</script>

<section class="runner" aria-label="Runner">
  <div class="head">
    <h2>Run {tab.target.kind} <code>{tab.target.key || doc?.file.name || tab.target.collection}</code></h2>
    <span class="muted">{tab.target.collection}</span>
    <button type="button" class="primary" onclick={() => app.startRun(tab)} disabled={tab.running || !!rows.error}>
      {tab.running ? "Running…" : tab.report ? "Run again" : "Run"}
    </button>
  </div>
  <form class="options" onsubmit={(e) => (e.preventDefault(), app.startRun(tab))}>
    <label>Environment
      <select bind:value={tab.env}>
        <option value={null}>none</option>
        {#each doc?.environments ?? [] as env (env)}<option value={env}>{env}</option>{/each}
      </select>
    </label>
    <label>Iterations
      <input type="number" min="1" bind:value={tab.iterations} disabled={rows.rows.length > 0} />
    </label>
    <label>Parallel iterations <input type="number" min="1" bind:value={tab.parallel} /></label>
    <label>Delay between requests (ms) <input type="number" min="0" bind:value={tab.delayMs} /></label>
    <label class="check"><input type="checkbox" bind:checked={tab.bail} /> Stop at the first failure</label>
  </form>
  <details class="more">
    <summary>Data ({rows.rows.length ? `${rows.rows.length} rows: one iteration each` : "none"}) and variables</summary>
    <div class="cols">
      <div>
        <label for="runner-data-{tab.id}">CSV with a header row, or a JSON array of objects</label>
        <textarea id="runner-data-{tab.id}" bind:value={tab.data} rows="6" placeholder={"user,password\nann,secret1\nbob,secret2"}></textarea>
        <input type="file" accept=".csv,.json,text/csv,application/json" aria-label="Load data from a file" onchange={loadFile} />
        {#if rows.error}<p class="s-err small" role="alert">{rows.error}</p>{/if}
      </div>
      <div>
        <span class="lbl">Variables for this run (highest precedence)</span>
        <KeyValueEditor bind:rows={tab.vars} label="Run variables" />
      </div>
    </div>
  </details>
  {#if tab.error}<p class="s-err" role="alert">{tab.error}</p>{/if}

  {#if tab.running || (tab.progress.length && !tab.report)}
    <div class="progress" role="status" aria-live="polite">
      <p>{done} requests done{failed ? `, ${failed} failed` : ""}{tab.running ? "…" : ""}</p>
      <ol aria-label="Progress">
        {#each tab.progress as p, i (i)}
          <li>
            <span class="it">#{p.iteration}</span>
            <span class="mark">{p.done ? (p.passed ? "✓" : "✗") : "…"}</span>
            <span>{p.key}</span>
            {#if p.done}<span class={statusClass(p.status)}>{p.status ?? "error"}</span> <span class="muted">{formatMs(p.durationMs)}</span>{/if}
          </li>
        {/each}
      </ol>
    </div>
  {/if}
  {#if tab.report?.id}
    <ReportView report={tab.report} runId={tab.report.id} idPrefix="runner-{tab.id}" />
  {/if}
</section>

<style>
  .runner {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    padding: 0.8rem 1rem;
    overflow: auto;
    height: 100%;
  }
  .head {
    display: flex;
    gap: 0.9rem;
    align-items: baseline;
  }
  h2 {
    margin: 0;
    font-size: 1.05rem;
  }
  .head button {
    margin-left: auto;
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem 1.2rem;
    align-items: flex-end;
  }
  .options label:not(.check) {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.78rem;
    color: var(--muted);
  }
  .options input[type="number"] {
    width: 7rem;
  }
  .more summary {
    cursor: pointer;
    font-size: 0.85rem;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
    margin-top: 0.5rem;
    font-size: 0.8rem;
  }
  .cols textarea {
    width: 100%;
    font-family: var(--mono);
    margin: 0.3rem 0;
  }
  .lbl {
    color: var(--muted);
  }
  .progress ol {
    list-style: none;
    padding: 0;
    margin: 0;
    font-size: 0.82rem;
    max-height: 20rem;
    overflow: auto;
  }
  .progress li {
    display: flex;
    gap: 0.6rem;
    padding: 0.1rem 0;
  }
  .it {
    color: var(--muted);
    width: 2.5rem;
  }
  .small {
    font-size: 0.8rem;
  }
</style>
