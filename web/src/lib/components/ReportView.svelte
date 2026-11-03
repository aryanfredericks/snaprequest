<script lang="ts">
  import type { RunReport, RunStep } from "../api/types";
  import { formatMs, statusClass } from "../model/http";
  import { reportCounts } from "../model/runs";
  import StepDetail from "./StepDetail.svelte";

  interface Props {
    report: RunReport;
    runId: number;
    idPrefix: string;
  }
  let { report, runId, idPrefix }: Props = $props();
  let selected = $state<{ it: number; step: number } | null>(null);
  const counts = $derived(reportCounts(report.iterations));
  const chosen = $derived.by((): RunStep | null => {
    if (!selected) return null;
    const it = report.iterations.find((i) => i.number === selected!.it);
    return it?.steps.find((s) => s.number === selected!.step) ?? null;
  });
  const many = $derived(report.iterations.length > 1);
</script>

<section class="report" aria-label="Run report">
  <div class="summary" role="status" aria-label="Run summary">
    <strong class={report.passed ? "s-2" : "s-err"}>{report.passed ? "Passed" : "Failed"}</strong>
    <span>{report.target.kind} <code>{report.target.key || report.target.name}</code></span>
    <span>{counts.steps - counts.failed} of {counts.steps} requests passed{counts.skipped ? `, ${counts.skipped} skipped` : ""}</span>
    {#if many}<span>{report.iterations.filter((i) => i.passed).length} of {report.iterations.length} iterations passed</span>{/if}
    <span class="downloads">
      Report:
      <a href="/api/v1/runs/{runId}/report/html" download>HTML</a>
      <a href="/api/v1/runs/{runId}/report/junit" download>JUnit</a>
      <a href="/api/v1/runs/{runId}/report/json" download>JSON</a>
    </span>
  </div>
  {#each report.iterations as it (it.number)}
    <div class="iteration">
      {#if many}
        <h3>
          <span class={it.passed ? "s-2" : "s-err"}>{it.passed ? "✓" : "✗"}</span>
          Iteration {it.number}
          {#if it.data}<span class="muted data">{Object.entries(it.data).map(([k, v]) => `${k}=${v}`).join(", ")}</span>{/if}
        </h3>
      {/if}
      <table aria-label={many ? `Iteration ${it.number}` : "Steps"}>
        <thead><tr><th class="n">#</th><th>Step</th><th>Request</th><th>Status</th><th class="num">Time</th><th>Checks</th></tr></thead>
        <tbody>
          {#each it.steps as s (s.number)}
            {@const failedChecks = s.assertions.filter((a) => !a.passed).length}
            <tr class:sel={selected?.it === it.number && selected.step === s.number} class:fail={!s.passed}>
              <td class="n">{s.number}</td>
              <td>
                <button type="button" class="link" onclick={() => (selected = { it: it.number, step: s.number })} aria-label="Show step {s.number} {s.title}">
                  {s.passed ? "✓" : "✗"} {s.title}
                </button>
                {#if s.attempts > 1}<span class="muted"> ×{s.attempts}</span>{/if}
              </td>
              <td class="req">{s.request ? `${s.request.method} ${s.request.url}` : ""}</td>
              <td class={statusClass(s.response?.status)}>{s.response?.status ?? (s.error ? "error" : "")}</td>
              <td class="num">{formatMs(s.response?.duration_ms)}</td>
              <td>{s.assertions.length ? (failedChecks ? `${failedChecks} of ${s.assertions.length} failed` : `${s.assertions.length} passed`) : ""}{s.error ? s.error.message : ""}</td>
            </tr>
          {/each}
          {#if it.skipped}
            <tr><td></td><td colspan="5" class="muted">{it.skipped} not run (an earlier step failed)</td></tr>
          {/if}
        </tbody>
      </table>
    </div>
  {/each}
  {#if chosen && selected}
    <StepDetail step={chosen} idPrefix="{idPrefix}-{selected.it}" />
  {:else}
    <p class="muted small">Select a step to see its response, checks, waterfall and wire log.</p>
  {/if}
</section>

<style>
  .report {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1.2rem;
    align-items: baseline;
    font-size: 0.88rem;
  }
  .downloads {
    margin-left: auto;
    display: flex;
    gap: 0.6rem;
  }
  a {
    color: var(--accent);
  }
  h3 {
    font-size: 0.85rem;
    margin: 0.3rem 0;
    display: flex;
    gap: 0.5rem;
  }
  .data {
    font-weight: normal;
    font-family: var(--mono);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--muted);
    font-size: 0.75rem;
    padding: 0.2rem 0.4rem;
  }
  td {
    padding: 0.2rem 0.4rem;
    border-top: 1px solid var(--border);
  }
  .n {
    width: 2rem;
    color: var(--muted);
  }
  .num {
    text-align: right;
    font-family: var(--mono);
  }
  .req {
    font-family: var(--mono);
    font-size: 0.75rem;
    max-width: 22rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  tr.sel {
    background: var(--accent-bg);
  }
  tr.fail .link {
    color: var(--danger);
  }
  .link {
    padding: 0;
    text-align: left;
  }
  .small {
    font-size: 0.8rem;
  }
</style>
