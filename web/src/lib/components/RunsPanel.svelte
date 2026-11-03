<script lang="ts">
  import type { RunSummary } from "../api/types";
  import { app, message } from "../state/app.svelte";
  import { timeAgo } from "../model/history";

  let runs = $state<RunSummary[]>([]);
  let error = $state<string | null>(null);
  let now = $state(Date.now());

  $effect(() => {
    void app.runsVersion;
    app
      .listRuns()
      .then((r) => {
        runs = r;
        now = Date.now();
        error = null;
      })
      .catch((e: unknown) => (error = message(e)));
  });
</script>

<div class="runs">
  {#if error}<p class="s-err small" role="alert">{error}</p>{/if}
  {#if runs.length === 0}
    <p class="muted small">Runs of collections, folders and flows appear here. Start one from a collection's menu.</p>
  {/if}
  <ul aria-label="Runs">
    {#each runs as r (r.id)}
      <li>
        <button type="button" onclick={() => app.openRun(r.id, r.target_key)}>
          <span class={r.passed ? "s-2" : "s-err"} aria-label={r.passed ? "passed" : "failed"}>{r.passed ? "✓" : "✗"}</span>
          <span class="name">{r.name}</span>
          <span class="meta muted">#{r.id} · {r.kind} · {r.target_key} · {timeAgo(r.created_at, now)}</span>
        </button>
      </li>
    {/each}
  </ul>
</div>

<style>
  .runs {
    overflow: auto;
    flex: 1;
    min-height: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  button {
    display: grid;
    grid-template-columns: 1.2rem 1fr;
    gap: 0 0.3rem;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    padding: 0.35rem 0.2rem;
  }
  button:hover {
    background: var(--bg-hover);
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    grid-column: 2;
    font-size: 0.72rem;
  }
  .small {
    font-size: 0.8rem;
    margin: 0.3rem;
  }
</style>
