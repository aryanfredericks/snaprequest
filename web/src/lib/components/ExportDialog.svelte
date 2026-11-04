<script lang="ts">
  import { app, type RequestTab } from "../state/app.svelte";
  import { EXPORT_FORMATS, lastFormat, rememberFormat, type ExportFormat } from "../model/exporting";
  import CodeEditor from "./CodeEditor.svelte";

  let dialog: HTMLDialogElement;
  let tab = $state<RequestTab | null>(null);
  let format = $state<ExportFormat>("curl");
  let code = $state("");
  let error = $state<string | null>(null);
  let status = $state("");
  let loading = $state(false);
  const lang = $derived(EXPORT_FORMATS.find((f) => f.value === format)?.lang ?? "text");

  export function open(t: RequestTab) {
    tab = t;
    format = lastFormat();
    dialog.showModal();
    void load();
  }

  async function load() {
    if (!tab) return;
    loading = true;
    error = null;
    status = "";
    try {
      code = await app.exportRequest(tab, format);
    } catch (e) {
      code = "";
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function choose(f: ExportFormat) {
    format = f;
    rememberFormat(f);
    void load();
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      status = "Copied to the clipboard.";
    } catch {
      status = "Copying is not allowed here: select the code and copy it.";
    }
  }
</script>

<dialog bind:this={dialog} aria-labelledby="export-title" class="export">
  <header>
    <h2 id="export-title">Request as code</h2>
    <form method="dialog"><button type="submit" aria-label="Close">✕</button></form>
  </header>
  <div class="bar">
    <label for="export-format">Format</label>
    <select id="export-format" value={format} onchange={(e) => choose(e.currentTarget.value as ExportFormat)}>
      {#each EXPORT_FORMATS as f (f.value)}<option value={f.value}>{f.label}</option>{/each}
    </select>
    <button type="button" class="primary" onclick={copy} disabled={loading || !code}>Copy</button>
    <span class="muted small" role="status">{loading ? "Building…" : status}</span>
  </div>
  {#if error}
    <p class="err" role="alert">{error}</p>
  {:else}
    {#key format + code}
      <CodeEditor value={code} readonly {lang} label="Code" />
    {/key}
  {/if}
  <p class="muted small">
    Variables are filled in. Secret values are not: the code reads them from environment variables such as
    <code>SNAPREQ_SECRET_TOKEN</code>.
  </p>
</dialog>

<style>
  .export {
    width: min(52rem, 96vw);
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
  .bar {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin: 0.6rem 0;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: 0.85rem;
  }
  .err {
    color: var(--err, #c62828);
  }
</style>
