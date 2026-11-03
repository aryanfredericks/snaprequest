<script lang="ts">
  import type { EditorView } from "@codemirror/view";
  import { openSearchPanel } from "@codemirror/search";
  import { rawBytes, rawText, type RawBytes } from "../api/types";
  import { languageFor } from "../editor/cm";
  import { formatJson, formatXml } from "../model/format";
  import { formatBytes, isTextual } from "../model/http";
  import CodeEditor from "./CodeEditor.svelte";
  import HexView from "./HexView.svelte";

  interface Props {
    body: (RawBytes & { truncated?: boolean }) | null | undefined;
    contentType?: string;
    /** File name offered when downloading. */
    name?: string;
  }
  let { body, contentType, name = "response" }: Props = $props();

  /** Above this, bodies are not pretty-printed (it would block the page). */
  const PRETTY_LIMIT = 8 * 1024 * 1024;

  type Mode = "pretty" | "raw" | "preview" | "hex";
  let mode = $state<Mode>("pretty");
  let view: EditorView | undefined;

  const bytes = $derived(rawBytes(body));
  const textual = $derived(isTextual(contentType, bytes, body?.encoding === "utf8"));
  const text = $derived(textual ? rawText(body) : "");
  const lang = $derived(languageFor(contentType));
  const ct = $derived((contentType ?? "").toLowerCase());
  const image = $derived(/^image\/(png|jpeg|gif|webp|svg\+xml|avif|bmp|x-icon|vnd\.microsoft\.icon)/.test(ct));
  const html = $derived(ct.includes("html"));
  const canPreview = $derived(image || html);
  const tooBig = $derived(bytes.length > PRETTY_LIMIT);
  const modes = $derived(
    [
      textual && { id: "pretty" as const, label: "Pretty" },
      textual && { id: "raw" as const, label: "Raw" },
      canPreview && { id: "preview" as const, label: "Preview" },
      { id: "hex" as const, label: "Hex" },
    ].filter((m): m is { id: Mode; label: string } => !!m),
  );
  $effect.pre(() => {
    if (!modes.some((m) => m.id === mode)) mode = modes[0]!.id;
  });

  const shown = $derived.by(() => {
    if (mode !== "pretty" || tooBig) return text;
    if (lang === "json") return formatJson(text);
    if (lang === "xml" || (lang === "html" && !text.includes("<script"))) return text.includes("\n") ? text : formatXml(text);
    return text;
  });

  const imageUrl = $derived.by(() => {
    if (!image || mode !== "preview") return null;
    return URL.createObjectURL(new Blob([bytes as BlobPart], { type: ct.split(";")[0] }));
  });
  $effect(() => {
    const url = imageUrl;
    return () => url && URL.revokeObjectURL(url);
  });

  function download() {
    const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: contentType || "application/octet-stream" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  async function copy() {
    await navigator.clipboard.writeText(text);
  }

  function search() {
    if (view) {
      view.focus();
      openSearchPanel(view);
    }
  }
</script>

<div class="viewer">
  <div class="bar">
    <div class="modes" role="radiogroup" aria-label="Body view">
      {#each modes as m (m.id)}
        <button type="button" role="radio" aria-checked={mode === m.id} class:on={mode === m.id} onclick={() => (mode = m.id)}>{m.label}</button>
      {/each}
    </div>
    <span class="muted small">
      {formatBytes(bytes.length)}{body?.truncated ? " (stored body shortened)" : ""}{tooBig && mode === "pretty" ? " · too large to pretty-print" : ""}
    </span>
    <span class="tools">
      {#if mode === "pretty" || mode === "raw"}
        <button type="button" class="link" onclick={search} title="Search (Ctrl+F in the body)">Search</button>
        <button type="button" class="link" onclick={copy}>Copy</button>
      {/if}
      <button type="button" class="link" onclick={download}>Download</button>
    </span>
  </div>
  {#if bytes.length === 0}
    <p class="muted">The response has no body.</p>
  {:else if mode === "pretty" || mode === "raw"}
    <CodeEditor value={shown} lang={mode === "pretty" ? lang : "text"} readonly label="Response body" onview={(v) => (view = v)} class="body-editor" />
  {:else if mode === "preview" && image && imageUrl}
    <div class="img"><img src={imageUrl} alt="Response body" /></div>
  {:else if mode === "preview" && html}
    <!-- No scripts, forms or same-origin access: the page cannot reach the API or this app. -->
    <iframe title="HTML preview" sandbox="" srcdoc={text}></iframe>
  {:else}
    <HexView {bytes} />
  {/if}
</div>

<style>
  .viewer {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    height: 100%;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }
  .modes {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .modes button {
    border: none;
    border-radius: 0;
    padding: 0.15rem 0.6rem;
    font-size: 0.78rem;
    background: transparent;
  }
  .modes button.on {
    background: var(--accent-bg);
    color: var(--fg);
    font-weight: 600;
  }
  .tools {
    margin-left: auto;
  }
  .small {
    font-size: 0.78rem;
  }
  .viewer :global(.body-editor) {
    flex: 1;
    min-height: 0;
  }
  iframe {
    flex: 1;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: #fff;
    min-height: 12rem;
  }
  .img {
    flex: 1;
    overflow: auto;
    background: repeating-conic-gradient(var(--bg-hover) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
    border-radius: var(--radius);
    padding: 0.5rem;
  }
  img {
    max-width: 100%;
  }
</style>
