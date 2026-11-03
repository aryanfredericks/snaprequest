<script lang="ts">
  import type { TraceEntry } from "../api/types";
  import { formatBytes, statusClass } from "../model/http";
  import { hops as split } from "../model/trace";
  import TlsDetails from "./TlsDetails.svelte";
  import TraceTimeline from "./TraceTimeline.svelte";
  import Tabs from "./Tabs.svelte";
  import Waterfall from "./Waterfall.svelte";
  import WireLog from "./WireLog.svelte";

  interface Props {
    trace: TraceEntry[];
    live?: boolean;
    idPrefix: string;
  }
  let { trace, live = false, idPrefix }: Props = $props();

  type Part = "waterfall" | "timeline" | "connection" | "tls" | "wire";
  let part = $state<Part>("waterfall");
  const hs = $derived(split(trace));
  let hopIndex = $state(-1);
  const hop = $derived(hs[hopIndex < 0 || hopIndex >= hs.length ? hs.length - 1 : hopIndex]);
  const anyTls = $derived(hs.some((h) => h.tls));
  const anyWire = $derived(hs.some((h) => h.wire));
  const parts = $derived([
    { id: "waterfall" as const, label: "Waterfall" },
    { id: "timeline" as const, label: "Events", badge: trace.length },
    { id: "connection" as const, label: hs.length > 1 ? "Redirects & connection" : "Connection" },
    ...(anyTls ? [{ id: "tls" as const, label: "TLS" }] : []),
    ...(anyWire ? [{ id: "wire" as const, label: "Wire log" }] : []),
  ]);
  $effect.pre(() => {
    if (!parts.some((p) => p.id === part)) part = "waterfall";
  });
  const tlsHost = $derived(
    hop?.entries.find((e): e is Extract<TraceEntry, { event: "tls_start" }> => e.event === "tls_start")?.server_name,
  );
</script>

<div class="debug">
  <Tabs tabs={parts} selected={part} label="Debug views" {idPrefix} onselect={(p) => (part = p)} />
  <div class="panel" role="tabpanel" id="{idPrefix}-panel" aria-labelledby="{idPrefix}-tab-{part}">
    {#if (part === "tls" || part === "wire") && hs.length > 1}
      <label class="hopsel">
        Hop
        <select bind:value={hopIndex}>
          <option value={-1}>final ({hs.length}/{hs.length})</option>
          {#each hs as h, i (i)}<option value={i}>{i + 1}: {h.title}</option>{/each}
        </select>
      </label>
    {/if}
    {#if part === "waterfall"}
      <Waterfall {trace} {live} />
    {:else if part === "timeline"}
      <TraceTimeline {trace} />
    {:else if part === "connection"}
      <ol class="hops" aria-label="Hops">
        {#each hs as h, i (i)}
          <li>
            <div class="hop-title">
              {#if h.response}<strong class={statusClass(h.response.status)}>{h.response.status}</strong>{/if}
              <span class="method m-{h.request?.method.toLowerCase() ?? 'other'}">{h.request?.method ?? ""}</span>
              <span class="url">{h.request?.url ?? ""}</span>
              {#if h.kind !== "final"}<span class="muted">→ {h.title}</span>{/if}
            </div>
            <dl>
              {#if h.dns}<dt>DNS</dt><dd>{h.dns.host} → {h.dns.addrs.join(", ")}</dd>{/if}
              <dt>Connection</dt>
              <dd>{h.remote ? `${h.remote}${h.local ? ` from ${h.local}` : ""}` : "not connected"}{h.reused ? " (reused)" : ""}</dd>
              {#if h.request}<dt>Protocol</dt><dd>{h.response?.version ?? h.request.version}{h.tls ? ` over ${h.tls.version}` : ""}</dd>{/if}
              {#if h.bodyBytes !== undefined}<dt>Body</dt><dd>{formatBytes(h.bodyBytes)} received</dd>{/if}
            </dl>
          </li>
        {/each}
      </ol>
    {:else if part === "tls" && hop?.tls}
      <TlsDetails info={hop.tls} host={tlsHost} />
    {:else if part === "tls"}
      <p class="muted">This hop did not use TLS{hop?.reused ? " handshake (the connection was reused)" : ""}.</p>
    {:else if part === "wire" && hop?.wire}
      <WireLog sent={hop.wire.sent} received={hop.wire.received} truncated={hop.wire.truncated} />
    {:else if part === "wire"}
      <p class="muted">No wire capture for this hop (HTTP/2 exchanges are not captured).</p>
    {/if}
  </div>
</div>

<style>
  .debug {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 0;
  }
  .panel {
    padding-top: 0.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .hopsel {
    font-size: 0.8rem;
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .hops {
    margin: 0;
    padding-left: 1.2rem;
    font-size: 0.82rem;
  }
  .hops li {
    padding-bottom: 0.6rem;
  }
  .hop-title {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
  }
  .method {
    font-family: var(--mono);
    font-weight: 700;
    font-size: 0.75rem;
  }
  .url {
    font-family: var(--mono);
    word-break: break-all;
  }
  dl {
    display: grid;
    grid-template-columns: 7rem 1fr;
    gap: 0.15rem 0.6rem;
    margin: 0.3rem 0 0;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    font-family: var(--mono);
    font-size: 0.78rem;
  }
</style>
