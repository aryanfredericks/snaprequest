<script lang="ts">
  import type { TraceEntry } from "../api/types";
  import { describe } from "../model/trace";
  import HeadersTable from "./HeadersTable.svelte";

  let { trace }: { trace: TraceEntry[] } = $props();
  let open = $state<Record<number, boolean>>({});
</script>

<ol class="timeline" aria-label="Trace events">
  {#each trace as e, i (i)}
    {@const d = describe(e)}
    {@const headers = e.event === "request_headers" || e.event === "response_headers" ? e.headers : null}
    <li class="m-{d.mark === '!' ? 'err' : d.mark === '>' ? 'out' : d.mark === '<' ? 'in' : 'info'}">
      <span class="at">{e.at_ms.toFixed(2).padStart(8, " ")} ms</span>
      <span class="mark" aria-hidden="true">{d.mark}</span>
      <span class="text">
        {d.text}
        {#if headers?.length}
          <button type="button" class="link" aria-expanded={!!open[i]} onclick={() => (open[i] = !open[i])}>{open[i] ? "hide headers" : "headers"}</button>
        {/if}
      </span>
      {#if headers && open[i]}
        <div class="headers"><HeadersTable {headers} label={e.event === "request_headers" ? "Request headers sent" : "Response headers received"} /></div>
      {/if}
    </li>
  {/each}
</ol>

<style>
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    font-family: var(--mono);
    font-size: 0.78rem;
  }
  li {
    display: grid;
    grid-template-columns: 7.5rem 1rem 1fr;
    gap: 0.3rem;
    padding: 0.1rem 0;
    border-bottom: 1px dashed var(--border);
  }
  .at {
    color: var(--muted);
    white-space: pre;
    text-align: right;
  }
  .m-out .mark {
    color: var(--accent);
  }
  .m-in .mark {
    color: var(--ok);
  }
  .m-err {
    color: var(--danger);
  }
  .headers {
    grid-column: 3;
    font-family: system-ui, sans-serif;
  }
  .link {
    font-size: 0.75rem;
  }
</style>
