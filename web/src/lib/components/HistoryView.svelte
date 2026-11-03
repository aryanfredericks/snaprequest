<script lang="ts">
  import { app, type HistoryTab } from "../state/app.svelte";
  import { rawText } from "../api/types";
  import { formatMs, statusClass } from "../model/http";
  import CodeEditor from "./CodeEditor.svelte";
  import HeadersTable from "./HeadersTable.svelte";
  import ResultView from "./ResultView.svelte";

  let { tab }: { tab: HistoryTab } = $props();
  const r = $derived(tab.record);
  const body = $derived.by(() => {
    const b = r.request.body;
    if (!b || b.type === "none") return null;
    if (b.type === "json" || b.type === "text") return b.content;
    return JSON.stringify(b, null, 2);
  });
  const failure = $derived(
    r.response ? null : { kind: "execution" as const, message: r.error ?? "failed", trace: [] },
  );
</script>

<div class="hist">
  <section class="req" aria-label="Stored request">
    <div class="head">
      <h2>
        <span class="method m-{r.method.toLowerCase()}">{r.method}</span>
        <span class="url">{r.request.url || r.url}</span>
      </h2>
      <span class="muted small">
        #{r.id} · {new Date(r.created_at).toLocaleString()} · <span class={statusClass(r.status)}>{r.status ?? "failed"}</span> · {formatMs(r.duration_ms)}
        {#if r.collection}· {r.collection}/{r.request_key}{/if}
      </span>
      <span class="actions">
        <button type="button" class="primary" onclick={() => app.rerun(r)} title={r.collection ? "Opens the collection request and sends it as it is now" : "Opens a copy of the request and sends it"}>Re-run</button>
        {#if r.collection && r.request_key}
          <button type="button" onclick={() => app.openRequest(r.collection!, r.request_key!)}>Open request</button>
        {/if}
        <button type="button" class="link danger" onclick={() => app.deleteHistory(r.id)}>Delete</button>
      </span>
    </div>
    <details>
      <summary>Request headers ({r.request.headers?.length ?? 0}){r.request.auth && r.request.auth !== "none" ? ` · ${r.request.auth} auth` : ""}{body ? " · body" : ""}</summary>
      <HeadersTable headers={r.request.headers ?? []} label="Stored request headers" />
      {#if body !== null}<CodeEditor value={body} readonly lang={r.request.body?.type === "json" ? "json" : "text"} label="Stored request body" />{/if}
      <p class="muted small">Credentials are never stored: sensitive headers show <code>&lt;redacted&gt;</code> and auth only its kind.</p>
    </details>
  </section>
  <div class="res">
    <ResultView
      response={r.response}
      {failure}
      name="history-{r.id}"
      idPrefix="hist-{tab.id}"
      label="Stored response"
    />
  </div>
</div>

<style>
  .hist {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.7rem 0.9rem;
    height: 100%;
    min-height: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem 0.9rem;
    align-items: baseline;
  }
  h2 {
    margin: 0;
    font-size: 0.95rem;
    display: flex;
    gap: 0.5rem;
    min-width: 0;
  }
  .method {
    font-family: var(--mono);
  }
  .url {
    font-family: var(--mono);
    font-weight: 500;
    word-break: break-all;
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 0.4rem;
  }
  .danger {
    color: var(--danger);
  }
  details {
    font-size: 0.85rem;
    margin-top: 0.4rem;
  }
  summary {
    cursor: pointer;
    color: var(--fg-soft);
  }
  .small {
    font-size: 0.78rem;
  }
  .res {
    flex: 1;
    min-height: 0;
  }
</style>
