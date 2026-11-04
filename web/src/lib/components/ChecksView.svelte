<script lang="ts">
  import type { AssertionOutcome, CaptureOutcome } from "../api/types";

  interface Props {
    assertions?: AssertionOutcome[];
    until?: AssertionOutcome[];
    captures?: CaptureOutcome[];
    /** Script `console.log` output. */
    logs?: string[];
  }
  let { assertions = [], until = [], captures = [], logs = [] }: Props = $props();
</script>

<div class="checks">
  {#each [{ title: "Checks", list: assertions }, { title: "Retry until", list: until }] as group (group.title)}
    {#if group.list.length}
      <table aria-label={group.title}>
        <caption>{group.title}: {group.list.filter((a) => a.passed).length} of {group.list.length} passed</caption>
        <tbody>
          {#each group.list as a, i (i)}
            <tr class:fail={!a.passed}>
              <td class="mark" aria-label={a.passed ? "passed" : "failed"}>{a.passed ? "✓" : "✗"}</td>
              <td><code>{a.subject}</code> {a.check}</td>
              <td class="muted">{a.actual !== undefined ? `actual: ${a.actual}` : ""}{a.message ? ` ${a.message}` : ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/each}
  {#if captures.length}
    <table aria-label="Captured values">
      <caption>Captured values</caption>
      <tbody>
        {#each captures as c (c.name)}
          <tr class:fail={!!c.error}>
            <td class="mark">{c.error ? "✗" : c.secret ? "🔒" : "→"}</td>
            <td><code>{c.name}</code></td>
            <td class="v">{c.error ?? c.value ?? "(missing)"}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if logs.length}
    <section aria-label="Script output">
      <h4>Script output</h4>
      <pre class="logs">{logs.join("\n")}</pre>
    </section>
  {/if}
  {#if !assertions.length && !until.length && !captures.length && !logs.length}
    <p class="muted">This request has no checks or captures.</p>
  {/if}
</div>

<style>
  h4 {
    margin: 0 0 0.3rem;
    font-size: 0.82rem;
  }
  .logs {
    margin: 0;
    font-size: 0.8rem;
    white-space: pre-wrap;
    max-height: 16rem;
    overflow: auto;
  }
  .checks {
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.82rem;
  }
  caption {
    text-align: left;
    font-weight: 600;
    padding-bottom: 0.3rem;
  }
  td {
    padding: 0.2rem 0.4rem;
    border-top: 1px solid var(--border);
    vertical-align: top;
  }
  .mark {
    width: 1.5rem;
    color: var(--ok);
    font-weight: 700;
  }
  tr.fail .mark,
  tr.fail .v {
    color: var(--danger);
  }
  .v {
    font-family: var(--mono);
    word-break: break-all;
  }
</style>
