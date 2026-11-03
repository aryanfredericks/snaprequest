<script lang="ts">
  import type { RequestTab } from "../state/app.svelte";
  import { formatBytes, formatMs, header, PHASES, responseCookies, statusClass, statusText } from "../model/http";
  import BodyViewer from "./BodyViewer.svelte";
  import ChecksView from "./ChecksView.svelte";
  import CookiesTable from "./CookiesTable.svelte";
  import DebugView from "./DebugView.svelte";
  import HeadersTable from "./HeadersTable.svelte";
  import Tabs from "./Tabs.svelte";

  interface Props {
    tab: RequestTab;
  }
  let { tab }: Props = $props();

  type Section = "body" | "headers" | "cookies" | "checks" | "debug";
  /** What the user picked; shown whenever it is available (only Debug is, while sending). */
  let chosen = $state<Section>("body");

  const result = $derived(tab.result);
  const r = $derived(result?.response ?? null);
  const cookies = $derived(responseCookies(r?.headers));
  const assertions = $derived(result?.assertions ?? []);
  const captures = $derived(result?.captures ?? []);
  const failed = $derived(assertions.filter((a) => !a.passed).length);
  const trace = $derived(tab.running ? tab.live : (r?.trace ?? result?.error?.trace ?? []));
  const done = $derived(!tab.running && !tab.error);
  const sections = $derived([
    ...(r && done
      ? [
          { id: "body" as const, label: "Body" },
          { id: "headers" as const, label: "Headers", badge: r.headers.length },
          { id: "cookies" as const, label: "Cookies", badge: cookies.length },
        ]
      : []),
    ...(done && (assertions.length || captures.length)
      ? [{ id: "checks" as const, label: "Checks", badge: failed ? `${failed} failed` : `${assertions.length} ✓` }]
      : []),
    ...(trace.length || tab.running ? [{ id: "debug" as const, label: tab.running ? "Debug (live)" : "Debug" }] : []),
  ]);
  const section = $derived(sections.some((s) => s.id === chosen) ? chosen : (sections[0]?.id ?? "body"));
  const fileName = $derived.by(() => {
    const ct = header(r?.headers, "content-type") ?? "";
    const ext = ct.includes("json") ? "json" : ct.includes("html") ? "html" : ct.includes("xml") ? "xml" : ct.startsWith("text/") ? "txt" : "bin";
    return `${(tab.draft.name || tab.key?.split("/").pop() || "response").replace(/[^\w.-]+/g, "-")}.${ext}`;
  });
</script>

<section class="response" aria-label="Response" aria-busy={tab.running}>
  {#if tab.running}
    <p class="muted" role="status">Sending… {tab.live.length} trace events so far</p>
  {:else if tab.error}
    <p class="s-err" role="alert">{tab.error}</p>
  {:else if result?.error}
    <div class="failure" role="alert">
      <strong class="s-err">
        {result.error.kind === "execution" ? `Failed during ${result.error.phase ? PHASES[result.error.phase] : "the request"}` : "Invalid request"}{result.error.timed_out ? " (timed out)" : ""}
      </strong>
      <p>{result.error.message}</p>
    </div>
  {:else if r}
    <div class="summary" role="status">
      <strong class="status {statusClass(r.status)}">{r.status} {statusText(r.status)}</strong>
      <span title="Total time">{formatMs(r.timings.total_ms)}</span>
      <span title="Body size (transferred / decoded)">
        {formatBytes(r.sizes.response_body)}{r.sizes.response_body_decoded !== r.sizes.response_body ? ` → ${formatBytes(r.sizes.response_body_decoded)}` : ""}
      </span>
      <span class="muted">{r.version}</span>
      {#if r.remote_addr}<span class="muted" title="Remote address">{r.remote_addr}</span>{/if}
      {#if assertions.length}
        <span class={failed ? "s-err" : "s-2"}>{failed ? `${failed} of ${assertions.length} checks failed` : `${assertions.length} checks passed`}</span>
      {/if}
    </div>
  {:else}
    <p class="muted">Send the request to see the response.</p>
  {/if}

  {#if sections.length}
    <Tabs tabs={sections} selected={section} label="Response parts" idPrefix="res-{tab.id}" onselect={(s) => (chosen = s)} />
    <div class="panel" role="tabpanel" id="res-{tab.id}-panel" aria-labelledby="res-{tab.id}-tab-{section}">
      {#if section === "debug"}
        <DebugView {trace} live={tab.running} idPrefix="dbg-{tab.id}" />
      {:else if !r}
        <!-- only Debug without a response -->
      {:else if section === "body"}
        <BodyViewer body={r.body} contentType={header(r.headers, "content-type")} name={fileName} />
      {:else if section === "headers"}
        <HeadersTable headers={r.headers} label="Response headers" />
      {:else if section === "cookies"}
        <CookiesTable {cookies} />
      {:else if section === "checks"}
        <ChecksView {assertions} {captures} />
      {/if}
    </div>
  {/if}
</section>

<style>
  .response {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    height: 100%;
    min-height: 0;
  }
  .summary {
    display: flex;
    gap: 1rem;
    align-items: baseline;
    flex-wrap: wrap;
    font-size: 0.85rem;
  }
  .status {
    font-size: 0.95rem;
  }
  .failure p {
    margin: 0.3rem 0 0;
    font-family: var(--mono);
    font-size: 0.82rem;
    white-space: pre-wrap;
  }
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
</style>
