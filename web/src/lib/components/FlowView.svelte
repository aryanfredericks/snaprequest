<script lang="ts">
  import type { VariableInfo } from "../api/types";
  import { app, type FlowTab } from "../state/app.svelte";
  import { api, unwrap } from "../api/client";
  import { newStep, stepTitle, type StepDraft } from "../model/flow";
  import { formatMs, statusClass } from "../model/http";
  import { capturedAfter } from "../model/runs";
  import StepDetail from "./StepDetail.svelte";
  import StepEditor from "./StepEditor.svelte";
  import VarsEditor from "./VarsEditor.svelte";

  let { tab }: { tab: FlowTab } = $props();
  const d = $derived(tab.draft);
  const doc = $derived(app.docs[tab.collection]);
  let view = $state<"edit" | "result">("edit");
  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);
  let serverVars = $state<VariableInfo[]>([]);

  const selectedIndex = $derived(d.steps.findIndex((s) => s.id === tab.selected));
  const step = $derived(selectedIndex >= 0 ? d.steps[selectedIndex]! : null);
  const result = $derived(selectedIndex >= 0 ? (tab.results[selectedIndex + 1] ?? null) : null);
  const allResults = $derived(Object.values(tab.results).sort((a, b) => a.number - b.number));

  // Variables the steps see: the collection's (with the environment), the flow's, and every
  // value a step captures.
  $effect(() => {
    const env = app.envs[tab.collection] ?? undefined;
    api
      .GET("/api/v1/collections/{id}/variables", { params: { path: { id: tab.collection }, query: { key: tab.key, env } } })
      .then((r) => (serverVars = (unwrap(r) as unknown as { variables: VariableInfo[] }).variables))
      .catch(() => (serverVars = []));
  });
  const vars = $derived.by((): VariableInfo[] => {
    const out: VariableInfo[] = [];
    for (const s of d.steps)
      for (const name of Object.keys(s.draft.rest.capture ?? {}))
        out.push({ name, value: null, secret: false, has_value: true, scope: "captured by a step" });
    for (const v of d.vars)
      if (v.name.trim()) out.push({ name: v.name.trim(), value: v.secret ? null : v.value, secret: v.secret, has_value: !v.secret, scope: "flow" });
    return [...out, ...serverVars.filter((v) => !out.some((o) => o.name === v.name))];
  });

  // The selection follows result changes (a run selects its last step).
  $effect(() => {
    if (tab.selected === null && d.steps.length) tab.selected = d.steps[0]!.id;
  });

  function select(s: StepDraft, show: "edit" | "result" = view) {
    tab.selected = s.id;
    view = show;
  }

  function move(from: number, to: number) {
    if (to < 0 || to >= d.steps.length || from === to) return;
    const [s] = d.steps.splice(from, 1);
    d.steps.splice(to, 0, s!);
    // Results and saved states belong to step numbers that no longer mean the same steps.
    tab.results = {};
    tab.resumeRun = {};
  }

  function add(kind: StepDraft["kind"]) {
    const s = newStep(kind);
    d.steps.push(s);
    select(s, "edit");
  }

  function remove(i: number) {
    d.steps.splice(i, 1);
    tab.results = {};
    tab.resumeRun = {};
  }

  function status(n: number): { mark: string; cls: string } {
    const r = tab.results[n];
    if (tab.running !== null && n >= tab.running && !r) return { mark: "…", cls: "muted" };
    if (!r) return { mark: "", cls: "" };
    return r.passed ? { mark: "✓", cls: "s-2" } : { mark: "✗", cls: "s-err" };
  }

  async function runFrom(n: number) {
    await app.runFlow(tab, { from: n });
    view = "result";
  }
</script>

<section class="flow" aria-label="Flow {tab.key}">
  <div class="head">
    <input class="title" bind:value={d.name} placeholder={tab.key} aria-label="Flow name" />
    <span class="muted">{tab.collection} / {tab.key}.flow.toml</span>
    <span class="actions">
      <button type="button" class="primary" onclick={async () => ((await app.runFlow(tab, "all")), (view = "result"))} disabled={tab.running !== null || !d.steps.length}>
        Run all
      </button>
      <button type="button" onclick={async () => ((await app.runFlow(tab, "next")), (view = "result"))} disabled={tab.running !== null || tab.next === null} title="Run the next step with the values captured so far">
        Step{tab.next !== null && Object.keys(tab.results).length ? ` (${tab.next})` : ""}
      </button>
      <button type="button" onclick={() => ((tab.results = {}), (tab.resumeRun = {}))} disabled={!Object.keys(tab.results).length}>Reset</button>
      <button type="button" onclick={() => app.save(tab)} disabled={!tab.dirty}>{tab.dirty ? "Save*" : "Saved"}</button>
    </span>
  </div>
  <input class="desc" bind:value={d.description} placeholder="Description" aria-label="Flow description" />
  {#if tab.error}<p class="s-err" role="alert">{tab.error}</p>{/if}
  {#if tab.running !== null}<p class="muted" role="status">Running from step {tab.running}…</p>{/if}

  <div class="body">
    <div class="steps">
      <ol aria-label="Steps">
        {#each d.steps as s, i (s.id)}
          {@const st = status(i + 1)}
          {@const r = tab.results[i + 1]}
          <li
            class:sel={tab.selected === s.id}
            class:drop={dropAt === i && dragFrom !== null && dragFrom !== i}
            draggable="true"
            ondragstart={() => (dragFrom = i)}
            ondragover={(e) => (e.preventDefault(), (dropAt = i))}
            ondragleave={() => (dropAt = null)}
            ondrop={(e) => {
              e.preventDefault();
              if (dragFrom !== null) move(dragFrom, i);
              dragFrom = null;
              dropAt = null;
            }}
            ondragend={() => ((dragFrom = null), (dropAt = null))}
          >
            <button type="button" class="pick" onclick={() => select(s)} aria-label="Step {i + 1}: {stepTitle(s)}{st.mark === '✓' ? ', passed' : st.mark === '✗' ? ', failed' : ''}">
              <span class="num">{i + 1}</span>
              <span class="st {st.cls}" aria-hidden="true">{st.mark}</span>
              <span class="t">{stepTitle(s)}</span>
              {#if r?.response}<span class="code {statusClass(r.response.status)}">{r.response.status}</span>
                <span class="muted ms">{formatMs(r.response.duration_ms)}</span>{/if}
            </button>
            <span class="tools">
              <button type="button" class="icon" title="Run from this step" aria-label="Run from step {i + 1}" disabled={tab.running !== null || (i > 0 && tab.resumeRun[i + 1] === undefined)} onclick={() => runFrom(i + 1)}>▶</button>
              <button type="button" class="icon" aria-label="Move step {i + 1} up" disabled={i === 0} onclick={() => move(i, i - 1)}>↑</button>
              <button type="button" class="icon" aria-label="Move step {i + 1} down" disabled={i === d.steps.length - 1} onclick={() => move(i, i + 1)}>↓</button>
              <button type="button" class="icon" aria-label="Remove step {i + 1}" onclick={() => remove(i)}>✕</button>
            </span>
          </li>
        {/each}
      </ol>
      <div class="add">
        <button type="button" onclick={() => add("request")}>+ Collection request</button>
        <button type="button" onclick={() => add("inline")}>+ Inline request</button>
      </div>
      <details class="flowvars">
        <summary>Flow variables ({d.vars.length})</summary>
        <VarsEditor bind:rows={d.vars} label="Flow variables" secretHint="Defaults for every step; captured values and step variables override them." />
      </details>
      <p class="muted small">▶ runs from a step with the values the steps before it captured in the last run. Drag steps or use ↑ ↓ to reorder.</p>
    </div>

    <div class="detail">
      {#if step}
        <div class="switch" role="radiogroup" aria-label="Step view">
          <button type="button" role="radio" aria-checked={view === "edit"} class:on={view === "edit"} onclick={() => (view = "edit")}>Edit</button>
          <button type="button" role="radio" aria-checked={view === "result"} class:on={view === "result"} onclick={() => (view = "result")} disabled={!result}>Result</button>
        </div>
        {#if view === "result" && result}
          {@const captured = capturedAfter(allResults, result.number)}
          <div class="after">
            <h4>Variables after step {result.number}</h4>
            {#if captured.length}
              <table aria-label="Captured values after step {result.number}">
                <tbody>
                  {#each captured as c (c.name)}
                    <tr><td><code>{c.name}</code></td><td class="v">{c.secret ? "🔒 " : ""}{c.value}</td></tr>
                  {/each}
                </tbody>
              </table>
            {:else}
              <p class="muted small">Nothing captured yet.</p>
            {/if}
          </div>
          <StepDetail step={result} idPrefix="flow-{tab.id}" />
        {:else}
          <StepEditor {step} number={selectedIndex + 1} items={doc?.items ?? []} {vars} />
        {/if}
      {:else}
        <p class="muted">Add a step to start.</p>
      {/if}
    </div>
  </div>
</section>

<style>
  .flow {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.7rem 0.9rem;
    height: 100%;
    min-height: 0;
  }
  .head {
    display: flex;
    gap: 0.8rem;
    align-items: baseline;
  }
  .title {
    font-weight: 600;
    font-size: 1rem;
    border-color: transparent;
    background: transparent;
  }
  .title:hover,
  .title:focus,
  .desc:hover,
  .desc:focus {
    border-color: var(--border);
  }
  .desc {
    border-color: transparent;
    background: transparent;
    color: var(--fg-soft);
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 0.4rem;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(16rem, 22rem) 1fr;
    gap: 1rem;
  }
  .steps {
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: flex;
    align-items: center;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    margin-bottom: 0.3rem;
    background: var(--bg);
  }
  li.sel {
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  li.drop {
    border-top: 3px solid var(--accent);
  }
  .pick {
    flex: 1;
    min-width: 0;
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    background: none;
    border: none;
    text-align: left;
    padding: 0.4rem 0.5rem;
    cursor: grab;
  }
  .num {
    color: var(--muted);
    font-family: var(--mono);
    font-size: 0.75rem;
  }
  .st {
    width: 0.8rem;
    font-weight: 700;
  }
  .t {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .code {
    font-family: var(--mono);
    font-size: 0.72rem;
    font-weight: 700;
  }
  .ms {
    font-size: 0.7rem;
  }
  .tools {
    display: flex;
  }
  .tools .icon {
    padding: 0.1rem 0.3rem;
  }
  .add {
    display: flex;
    gap: 0.4rem;
  }
  .flowvars summary {
    cursor: pointer;
    font-size: 0.85rem;
  }
  .detail {
    min-width: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .switch {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    align-self: flex-start;
    overflow: hidden;
  }
  .switch button {
    border: none;
    border-radius: 0;
    background: transparent;
    padding: 0.2rem 0.8rem;
  }
  .switch button.on {
    background: var(--accent-bg);
    font-weight: 600;
  }
  .after h4 {
    margin: 0 0 0.3rem;
    font-size: 0.85rem;
  }
  .after table {
    border-collapse: collapse;
    font-size: 0.82rem;
  }
  .after td {
    padding: 0.15rem 0.6rem 0.15rem 0;
    border-bottom: 1px solid var(--border);
  }
  .v {
    font-family: var(--mono);
    word-break: break-all;
  }
  .small {
    font-size: 0.78rem;
    margin: 0;
  }
</style>
