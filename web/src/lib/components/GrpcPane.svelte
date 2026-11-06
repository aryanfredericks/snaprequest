<script lang="ts">
  // The result of a gRPC call: status, response messages, metadata and the connection trace.
  import type { RequestTab } from "../state/app.svelte";
  import Tabs from "./Tabs.svelte";
  import TraceTimeline from "./TraceTimeline.svelte";

  let { tab }: { tab: RequestTab } = $props();
  const call = $derived(tab.grpc);

  let view = $state<"messages" | "metadata" | "trace">("messages");
  let next = $state("");
  let problem = $state<string | null>(null);

  const entries = (m: Record<string, unknown> | null) => Object.entries(m ?? {}).map(([k, v]) => [k, Array.isArray(v) ? v.join(", ") : String(v)] as const);

  function sendNext() {
    problem = null;
    try {
      call.send(JSON.parse(next));
      next = "";
    } catch (e) {
      problem = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<section class="grpc" aria-label="gRPC response" aria-busy={call.running}>
  <div class="status" role="status">
    {#if call.status}
      <strong class={call.status.code === 0 ? "s-2" : "s-err"} data-testid="grpc-status">{call.status.name}</strong>
      {#if call.status.code !== 0}<span>({call.status.code}) {call.status.message}</span>{/if}
      {#if call.durationMs !== null}<span class="muted">{call.durationMs.toFixed(0)} ms</span>{/if}
      <span class="muted">{call.messages.length} message(s)</span>
    {:else if call.running}
      <span class="muted">Calling… {call.messages.length} message(s) so far</span>
    {:else if call.error}
      <span class="s-err">{call.error}</span>
    {:else}
      <span class="muted">Invoke the method to see the response.</span>
    {/if}
  </div>
  {#if call.error && call.status}<p class="s-err" role="alert">{call.error}</p>{/if}
  {#if tab.error}<p class="s-err" role="alert">{tab.error}</p>{/if}
  {#if call.streaming}
    <form class="compose" onsubmit={(e) => (e.preventDefault(), sendNext())}>
      <textarea rows="2" bind:value={next} aria-label="Next message" placeholder={'{ "item": "more" }'}></textarea>
      <div class="actions">
        <button type="submit" class="primary">Send</button>
        <button type="button" onclick={() => call.end()}>End stream</button>
      </div>
    </form>
    {#if problem}<p class="s-err">{problem}</p>{/if}
  {/if}

  <Tabs
    tabs={[
      { id: "messages" as const, label: "Messages", badge: call.messages.length || "" },
      { id: "metadata" as const, label: "Metadata" },
      { id: "trace" as const, label: "Trace", badge: call.trace.length || "" },
    ]}
    selected={view}
    label="gRPC response parts"
    idPrefix="grpc-{tab.id}"
    onselect={(v) => (view = v)}
  />
  <div class="panel" role="tabpanel" id="grpc-{tab.id}-panel" aria-labelledby="grpc-{tab.id}-tab-{view}">
    {#if view === "messages"}
      <ol class="list" aria-label="Response messages">
        {#each call.messages as m (m.id)}
          <li>
            <span class="at">{(m.atMs / 1000).toFixed(3)} s</span>
            <pre>{JSON.stringify(m.message, null, 2)}</pre>
          </li>
        {:else}
          <li class="muted">No messages{call.running ? " yet" : ""}.</li>
        {/each}
      </ol>
    {:else if view === "metadata"}
      {#each [["Headers", call.headers], ["Trailers", call.trailers]] as const as [title, map] (title)}
        <h4>{title}</h4>
        <table aria-label="Response {title.toLowerCase()}">
          <tbody>
            {#each entries(map) as [k, v] (k)}
              <tr><th scope="row">{k}</th><td><code>{v}</code></td></tr>
            {:else}
              <tr><td class="muted">none</td></tr>
            {/each}
          </tbody>
        </table>
      {/each}
    {:else}
      <TraceTimeline trace={call.trace} />
    {/if}
  </div>
</section>

<style>
  .grpc {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    height: 100%;
    min-height: 0;
  }
  .status {
    display: flex;
    gap: 0.8rem;
    align-items: baseline;
    font-size: 0.85rem;
  }
  .compose {
    display: flex;
    gap: 0.4rem;
  }
  .compose textarea {
    flex: 1;
    font-family: var(--mono);
    font-size: 0.82rem;
  }
  .actions {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list li {
    display: grid;
    grid-template-columns: 5rem 1fr;
    gap: 0.4rem;
    border-bottom: 1px solid var(--border);
    padding: 0.2rem 0.3rem;
    font-size: 0.82rem;
  }
  .at {
    color: var(--muted);
    font-family: var(--mono);
  }
  pre {
    margin: 0;
    font-family: var(--mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  h4 {
    margin: 0.4rem 0 0.2rem;
    font-size: 0.85rem;
  }
  table {
    border-collapse: collapse;
    font-size: 0.82rem;
  }
  th {
    text-align: left;
    padding-right: 1rem;
    font-weight: 600;
  }
  code {
    font-family: var(--mono);
  }
</style>
