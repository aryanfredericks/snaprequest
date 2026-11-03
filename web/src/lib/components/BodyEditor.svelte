<script lang="ts">
  import { untrack } from "svelte";
  import type { VariableInfo } from "../api/types";
  import { formatJson } from "../model/format";
  import { BODY_TYPES, part, type BodyDraft, type BodyType } from "../model/draft";
  import { CONTENT_TYPES } from "../model/headers";
  import CodeEditor from "./CodeEditor.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";

  interface Props {
    body: BodyDraft;
    vars?: VariableInfo[] | null;
    /** Paths are relative to the request file (collections) or the server's directory. */
    inCollection: boolean;
  }
  let { body, vars = null, inCollection }: Props = $props();

  const labels: Record<BodyType, string> = {
    none: "None",
    json: "JSON",
    text: "Text",
    form: "Form (urlencoded)",
    multipart: "Multipart",
    file: "File",
  };
  const pathHint = $derived(
    inCollection ? "path relative to the request file" : "path on the server",
  );
  let fromFile = $state(false);
  // Decided when another body is shown (another tab), not while typing the path.
  $effect.pre(() => {
    const b = body;
    untrack(() => (fromFile = b.file !== "" && (b.type === "json" || b.type === "text")));
  });
  let jsonError = $derived.by(() => {
    if (body.type !== "json" || fromFile || !body.content.trim()) return null;
    // Placeholders are valid in values; replace them before checking the JSON.
    try {
      JSON.parse(body.content.replace(/\{\{[^{}]*\}\}/g, "0"));
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  });
</script>

<div class="body">
  <div class="types" role="radiogroup" aria-label="Body type">
    {#each BODY_TYPES as t (t)}
      <label><input type="radio" name="body-type" value={t} bind:group={body.type} /> {labels[t]}</label>
    {/each}
  </div>

  {#if body.type === "json" || body.type === "text"}
    <div class="row">
      <label class="check"><input type="checkbox" bind:checked={fromFile} onchange={() => !fromFile && (body.file = "")} /> From a file</label>
      {#if body.type === "json" && !fromFile}
        <button type="button" onclick={() => (body.content = formatJson(body.content))} disabled={!!jsonError}>Format</button>
      {/if}
      {#if body.type === "text"}
        <label class="ct">Content type <input bind:value={body.contentType} list="body-content-types" placeholder="text/plain; charset=utf-8" /></label>
      {/if}
    </div>
    {#if fromFile}
      <label class="field">File <input bind:value={body.file} placeholder={pathHint} /></label>
    {:else}
      <CodeEditor
        value={body.content}
        lang={body.type === "json" ? "json" : "text"}
        highlightVars
        {vars}
        label="Request body"
        onchange={(v) => (body.content = v)}
        class="grow"
      />
      {#if jsonError}<p class="warn" role="status">Not valid JSON: {jsonError}</p>{/if}
    {/if}
  {:else if body.type === "form"}
    <KeyValueEditor bind:rows={body.fields} label="Form fields" {vars} />
  {:else if body.type === "multipart"}
    <table aria-label="Multipart parts">
      <thead><tr><th class="on"><span class="sr-only">Enabled</span></th><th>Name</th><th>Kind</th><th>Value or file</th><th>File name / type</th><th class="on"></th></tr></thead>
      <tbody>
        {#each body.parts as p, i (p.id)}
          <tr class:off={!p.enabled}>
            <td><input type="checkbox" bind:checked={p.enabled} aria-label="Send part {i + 1}" /></td>
            <td><input bind:value={p.name} placeholder="name" aria-label="Part {i + 1} name" /></td>
            <td>
              <select bind:value={p.kind} aria-label="Part {i + 1} kind">
                <option value="text">text</option>
                <option value="file">file</option>
              </select>
            </td>
            <td>
              {#if p.kind === "text"}
                <input bind:value={p.value} placeholder="value" aria-label="Part {i + 1} value" />
              {:else}
                <input bind:value={p.file} placeholder={pathHint} aria-label="Part {i + 1} file" />
              {/if}
            </td>
            <td>
              {#if p.kind === "file"}
                <span class="pair">
                  <input bind:value={p.filename} placeholder="file name" aria-label="Part {i + 1} file name" />
                  <input bind:value={p.contentType} placeholder="content type" list="body-content-types" aria-label="Part {i + 1} content type" />
                </span>
              {/if}
            </td>
            <td><button type="button" class="icon" onclick={() => body.parts.splice(i, 1)} aria-label="Remove part {i + 1}">✕</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
    <button type="button" class="add" onclick={() => body.parts.push(part())}>+ Add part</button>
  {:else if body.type === "file"}
    <label class="field">File <input bind:value={body.file} placeholder={pathHint} /></label>
    <label class="field">Content type <input bind:value={body.contentType} list="body-content-types" placeholder="guessed from the extension" /></label>
  {:else}
    <p class="muted">This request has no body.</p>
  {/if}
  <datalist id="body-content-types">
    {#each CONTENT_TYPES as c (c)}<option value={c}></option>{/each}
  </datalist>
</div>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-height: 0;
    flex: 1;
  }
  .types {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem;
    font-size: 0.85rem;
  }
  .row {
    display: flex;
    gap: 0.8rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .ct input {
    width: 16rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.8rem;
    color: var(--muted);
    max-width: 32rem;
  }
  :global(.grow) {
    flex: 1;
  }
  .warn {
    color: var(--warn);
    font-size: 0.8rem;
    margin: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 0.75rem;
    color: var(--muted);
  }
  td {
    padding: 0.15rem 0.3rem;
  }
  td input:not([type="checkbox"]) {
    width: 100%;
  }
  .pair {
    display: flex;
    gap: 0.3rem;
  }
  .on {
    width: 1.8rem;
  }
  tr.off input:not([type="checkbox"]) {
    opacity: 0.55;
  }
  .add {
    align-self: flex-start;
  }
</style>
