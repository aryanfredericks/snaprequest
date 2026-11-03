<script lang="ts">
  import type { TreeItem, VariableInfo } from "../api/types";
  import type { StepDraft } from "../model/flow";
  import { HEADER_NAMES } from "../model/headers";
  import AuthEditor from "./AuthEditor.svelte";
  import BodyEditor from "./BodyEditor.svelte";
  import CodeEditor from "./CodeEditor.svelte";
  import JsonField from "./JsonField.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";
  import MethodSelect from "./MethodSelect.svelte";
  import SettingsEditor from "./SettingsEditor.svelte";
  import Tabs from "./Tabs.svelte";

  interface Props {
    step: StepDraft;
    number: number;
    items: TreeItem[];
    vars: VariableInfo[] | null;
  }
  let { step, number, items, vars }: Props = $props();

  type Part = "params" | "headers" | "body" | "auth" | "checks" | "retry" | "settings";
  let part = $state<Part>("checks");
  const d = $derived(step.draft);
  const requests = $derived(flatten(items));
  const parts = $derived([
    ...(step.kind === "inline"
      ? [
          { id: "params" as const, label: "Params", badge: d.query.length },
          { id: "headers" as const, label: "Headers", badge: d.headers.length },
          { id: "body" as const, label: "Body", badge: d.body.type === "none" ? "" : d.body.type },
          { id: "auth" as const, label: "Auth", badge: d.auth.type === "inherit" ? "" : d.auth.type },
        ]
      : []),
    { id: "checks" as const, label: "Checks, captures & vars" },
    { id: "retry" as const, label: "Retry & delay", badge: step.retry ? `×${step.retry.max ?? 10}` : "" },
    ...(step.kind === "inline" ? [{ id: "settings" as const, label: "Settings" }] : []),
  ]);
  $effect.pre(() => {
    if (!parts.some((p) => p.id === part)) part = "checks";
  });

  function flatten(list: TreeItem[]): { key: string; label: string }[] {
    return list.flatMap((i) => (i.type === "folder" ? flatten(i.items) : [{ key: i.key, label: `${i.method} ${i.key}` }]));
  }
</script>

<div class="step">
  <div class="top">
    <label class="name">Step {number} name <input bind:value={step.name} placeholder="optional" /></label>
    <div class="kind" role="radiogroup" aria-label="Step {number} kind">
      <label><input type="radio" bind:group={step.kind} value="request" /> Collection request</label>
      <label><input type="radio" bind:group={step.kind} value="inline" /> Inline request</label>
    </div>
  </div>
  {#if step.kind === "request"}
    <label class="req">Request
      <select bind:value={step.request}>
        <option value="" disabled>choose a request</option>
        {#each requests as r (r.key)}<option value={r.key}>{r.label}</option>{/each}
      </select>
    </label>
    <p class="muted small">The request is sent as its file says; checks and captures below are added to its own.</p>
  {:else}
    <div class="line">
      <MethodSelect bind:value={d.method} label="Step {number} method" />
      <CodeEditor value={d.url} singleLine highlightVars {vars} label="Step {number} URL" onchange={(v) => (d.url = v)} class="url" />
    </div>
  {/if}
  <Tabs tabs={parts} selected={part} label="Step {number} parts" idPrefix="step-{step.id}" onselect={(p) => (part = p)} />
  <div class="panel" role="tabpanel" id="step-{step.id}-panel" aria-labelledby="step-{step.id}-tab-{part}">
    {#key step.id}
      {#if part === "params"}
        <KeyValueEditor bind:rows={d.query} label="Step {number} query" {vars} />
      {:else if part === "headers"}
        <KeyValueEditor bind:rows={d.headers} label="Step {number} headers" {vars} names={HEADER_NAMES} />
      {:else if part === "body"}
        <BodyEditor body={d.body} {vars} inCollection />
      {:else if part === "auth"}
        <AuthEditor auth={d.auth} {vars} inCollection />
      {:else if part === "settings"}
        <SettingsEditor settings={d.settings} inCollection />
      {:else if part === "retry"}
        <div class="retry">
          <label class="check">
            <input type="checkbox" checked={!!step.retry} onchange={(e) => (step.retry = e.currentTarget.checked ? { max: 10, every_ms: 1000 } : null)} />
            Send again until the checks pass
          </label>
          {#if step.retry}
            <label>Attempts <input type="number" min="1" bind:value={step.retry.max} /></label>
            <label>Pause between attempts (ms) <input type="number" min="0" bind:value={step.retry.every_ms} /></label>
            <span class="lbl">Until (other checks than the step's own; empty: the step's checks)</span>
            <JsonField value={step.retry.until} label="Step {number} retry until" placeholder={'{ "json": { "$.state": "ready" } }'} onchange={(v) => step.retry && (step.retry.until = v as Record<string, unknown> | undefined)} />
          {/if}
          <label>Wait before the step (ms)
            <input type="number" min="0" value={step.delayMs ?? ""} oninput={(e) => (step.delayMs = e.currentTarget.value ? Math.max(0, Number(e.currentTarget.value)) : null)} />
          </label>
        </div>
      {:else}
        <div class="checks">
          <span class="lbl">Checks (<code>assert</code>)</span>
          <JsonField value={d.rest.assert} label="Step {number} checks" placeholder={'{ "status": 201, "json": { "$.id": { "type": "integer" } } }'} onchange={(v) => (d.rest.assert = v as Record<string, unknown> | undefined)} />
          <span class="lbl">Captures (<code>capture</code>): values for later steps</span>
          <JsonField value={d.rest.capture} label="Step {number} captures" placeholder={'{ "order_id": "$.id" }'} onchange={(v) => (d.rest.capture = v as Record<string, unknown> | undefined)} />
          <span class="lbl">Step variables (<code>vars</code>)</span>
          <JsonField value={d.rest.vars} label="Step {number} variables" placeholder={'{ "item": "sku-1" }'} onchange={(v) => (d.rest.vars = v as typeof d.rest.vars)} />
        </div>
      {/if}
    {/key}
  </div>
</div>

<style>
  .step {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .top {
    display: flex;
    gap: 1.2rem;
    align-items: flex-end;
    flex-wrap: wrap;
  }
  .name,
  .req,
  .retry label:not(.check) {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.78rem;
    color: var(--muted);
  }
  .name input {
    width: 18rem;
  }
  .kind {
    display: flex;
    gap: 1rem;
    font-size: 0.85rem;
  }
  .line {
    display: flex;
    gap: 0.4rem;
  }
  .line :global(.url) {
    flex: 1;
  }
  .retry,
  .checks {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    max-width: 44rem;
  }
  .retry input[type="number"] {
    width: 9rem;
  }
  .lbl {
    font-size: 0.78rem;
    color: var(--muted);
    margin-top: 0.3rem;
  }
  .small {
    font-size: 0.78rem;
    margin: 0;
  }
</style>
