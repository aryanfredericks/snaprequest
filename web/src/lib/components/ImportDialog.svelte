<script lang="ts">
  import { app } from "../state/app.svelte";
  import { describe, IMPORT_FORMATS, importBody, MAX_IMPORT_BYTES, type ImportSummary } from "../model/importing";

  let dialog: HTMLDialogElement;
  let text = $state("");
  let file = $state<File | null>(null);
  let envFiles = $state<File[]>([]);
  let format = $state("");
  let id = $state("");
  let name = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let result = $state<ImportSummary | null>(null);
  let fileInput = $state<HTMLInputElement>();
  let envInput = $state<HTMLInputElement>();

  export function open() {
    text = "";
    file = null;
    envFiles = [];
    format = "";
    id = "";
    name = "";
    error = null;
    result = null;
    if (fileInput) fileInput.value = "";
    if (envInput) envInput.value = "";
    dialog.showModal();
  }

  async function read(f: File): Promise<string> {
    if (f.size > MAX_IMPORT_BYTES) throw new Error(`${f.name} is larger than 32 MiB`);
    return f.text();
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    error = null;
    busy = true;
    try {
      const content = file ? await read(file) : text;
      if (!content.trim()) throw new Error("Choose a file or paste something to import.");
      const environments = await Promise.all(envFiles.map(read));
      result = await app.importCollection(importBody(content, format, id, name, environments));
      if (result.warnings.length === 0) dialog.close();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<dialog bind:this={dialog} aria-labelledby="import-title" class="import">
  <header>
    <h2 id="import-title">Import</h2>
    <form method="dialog"><button type="submit" aria-label="Close">✕</button></form>
  </header>

  {#if result}
    <p role="status">Imported <strong>{result.name}</strong> into <code>{result.id}</code>: {describe(result)}.</p>
    {#if result.secrets}
      <p class="muted">{result.secrets} credential{result.secrets === 1 ? " was" : "s were"} stored as secret values, not in collection files.</p>
    {/if}
    <h3>Things to check</h3>
    <ul class="warnings">
      {#each result.warnings as w (w)}<li>{w}</li>{/each}
    </ul>
    <p><button type="button" class="primary" onclick={() => dialog.close()}>Done</button></p>
  {:else}
    <p class="muted">
      A cURL command, a Postman collection, an OpenAPI document, an Insomnia export, a HAR recording or a Hurl file
      becomes a new collection. Passwords, tokens and keys in it become secret variables.
    </p>
    <form onsubmit={submit}>
      <label for="import-file">File</label>
      <input
          id="import-file"
          bind:this={fileInput}
          type="file"
          accept=".json,.yaml,.yml,.har,.hurl,.curl,.sh,.txt"
          onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)}
        />
      <label for="import-text">…or paste</label>
      <textarea id="import-text" bind:value={text} rows="5" disabled={!!file} placeholder="curl https://api.example.com/users -H 'Accept: application/json'" spellcheck="false"></textarea>
      <div class="row">
        <div class="field">
          <label for="import-format">Format</label>
          <select id="import-format" bind:value={format}>
            <option value="">Detect automatically</option>
            {#each IMPORT_FORMATS as f (f.value)}<option value={f.value}>{f.label}</option>{/each}
          </select>
        </div>
        <div class="field">
          <label for="import-id">Id (directory)</label>
          <input id="import-id" bind:value={id} pattern="[A-Za-z0-9_\-][A-Za-z0-9_.\-]*" placeholder="from the name" />
        </div>
        <div class="field">
          <label for="import-name">Name</label>
          <input id="import-name" bind:value={name} maxlength="100" placeholder="from the file" />
        </div>
      </div>
      <label for="import-envs">Postman environments (optional)</label>
      <input
          id="import-envs"
          bind:this={envInput}
          type="file"
          accept=".json"
          multiple
          onchange={(e) => (envFiles = [...(e.currentTarget.files ?? [])])}
        />
      <p><button type="submit" class="primary" disabled={busy}>{busy ? "Importing…" : "Import"}</button></p>
    </form>
  {/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</dialog>

<style>
  .import {
    width: min(40rem, 94vw);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  h3 {
    font-size: 0.92rem;
    margin: 1rem 0 0.3rem;
  }
  form > label {
    display: block;
    margin: 0.6rem 0 0.2rem;
  }
  form > input,
  textarea {
    width: 100%;
    box-sizing: border-box;
  }
  textarea {
    font-family: var(--mono, monospace);
    font-size: 0.82rem;
  }
  .row {
    display: flex;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .row .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    flex: 1 1 10rem;
  }
  .muted {
    color: var(--muted);
    font-size: 0.85rem;
  }
  .warnings {
    font-size: 0.85rem;
    max-height: 14rem;
    overflow: auto;
  }
  .err {
    color: var(--err, #c62828);
  }
</style>
