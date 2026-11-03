<script lang="ts">
  import type { TraceEntry } from "../api/types";
  import { formatMs } from "../model/http";
  import { hops as split, PHASE_NAMES, phaseTotals, totalMs } from "../model/trace";

  interface Props {
    trace: TraceEntry[];
    /** Still running: the timeline grows. */
    live?: boolean;
  }
  let { trace, live = false }: Props = $props();

  const hs = $derived(split(trace));
  const total = $derived(Math.max(totalMs(trace), 0.001));
  const totals = $derived(phaseTotals(hs));
  const pct = (ms: number) => `${(ms / total) * 100}%`;
  const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => ({ f, ms: f * total })));
</script>

<div class="waterfall">
  <div class="legend" aria-hidden="true">
    {#each PHASE_NAMES as p (p)}
      <span><i class="sw p-{p}"></i>{p} {totals[p] ? formatMs(totals[p]) : ""}</span>
    {/each}
    <span class="total">{live ? "running… " : "total "}{formatMs(totalMs(trace))}</span>
  </div>
  <table aria-label="Timing waterfall">
    <thead>
      <tr>
        <th scope="col">Phase</th>
        <th scope="col" class="num">Duration</th>
        <th scope="col" class="axis">
          <span class="sr-only">Timeline</span>
          {#each ticks as t (t.f)}
            <span class="tick" style="left: {t.f * 100}%" aria-hidden="true">{formatMs(t.ms)}</span>
          {/each}
        </th>
      </tr>
    </thead>
    {#each hs as hop, i (i)}
      <tbody>
        {#if hs.length > 1}
          <tr class="hop"><th colspan="3" scope="rowgroup">hop {i + 1}/{hs.length}: {hop.title}</th></tr>
        {/if}
        {#each hop.segments as s, j (j)}
          <tr>
            <th scope="row" class="phase">{s.phase}</th>
            <td class="num">{formatMs(s.end - s.start)}</td>
            <td class="lane">
              <span class="grid" aria-hidden="true"></span>
              <span
                class="bar p-{s.phase}"
                style="left: {pct(s.start)}; width: max(2px, {pct(s.end - s.start)})"
                title="{s.phase}: {formatMs(s.start)} → {formatMs(s.end)}"
              ></span>
              <span class="sr-only">from {formatMs(s.start)} to {formatMs(s.end)}</span>
            </td>
          </tr>
        {/each}
        {#if hop.reused}
          <tr><td></td><td></td><td class="note">connection reused: no DNS, connect or TLS</td></tr>
        {/if}
      </tbody>
    {/each}
  </table>
  {#if hs.length === 0}
    <p class="muted">{live ? "Waiting for the first events…" : "No timing data."}</p>
  {/if}
</div>

<style>
  .waterfall {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    font-size: 0.8rem;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 0.9rem;
    color: var(--fg-soft);
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }
  .total {
    margin-left: auto;
    font-weight: 600;
  }
  .sw {
    display: inline-block;
    width: 0.8rem;
    height: 0.8rem;
    border-radius: 2px;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    table-layout: fixed;
  }
  th,
  td {
    padding: 0.15rem 0.4rem;
    text-align: left;
    font-weight: normal;
  }
  thead th {
    color: var(--muted);
    font-size: 0.72rem;
  }
  th:first-child {
    width: 6rem;
  }
  .num {
    width: 6rem;
    text-align: right;
    font-family: var(--mono);
    padding-right: 0.8rem;
  }
  .axis {
    position: relative;
    height: 1.2rem;
  }
  .tick {
    position: absolute;
    top: 0.2rem;
    transform: translateX(-50%);
    white-space: nowrap;
  }
  /* The timeline obviously starts at 0; its label would crowd the Duration column. */
  .tick:first-child {
    display: none;
  }
  .tick:last-child {
    transform: translateX(-100%);
  }
  .hop th {
    padding-top: 0.6rem;
    font-weight: 600;
    color: var(--fg-soft);
    width: auto;
  }
  .lane {
    position: relative;
    height: 1.4rem;
  }
  .grid {
    position: absolute;
    inset: 0.2rem 0;
    border-left: 1px solid var(--border);
    border-right: 1px solid var(--border);
    background: linear-gradient(to right, transparent calc(25% - 1px), var(--border) 25%, transparent calc(25% + 1px), transparent calc(50% - 1px), var(--border) 50%, transparent calc(50% + 1px), transparent calc(75% - 1px), var(--border) 75%, transparent calc(75% + 1px));
    opacity: 0.6;
  }
  .bar {
    position: absolute;
    top: 0.3rem;
    bottom: 0.3rem;
    border-radius: 2px;
  }
  .note {
    color: var(--muted);
    font-style: italic;
  }
  .p-DNS { background: var(--t-dns); }
  .p-connect { background: var(--t-connect); }
  .p-TLS { background: var(--t-tls); }
  .p-send { background: var(--t-send); }
  .p-wait { background: var(--t-wait); }
  .p-download { background: var(--t-download); }
</style>
