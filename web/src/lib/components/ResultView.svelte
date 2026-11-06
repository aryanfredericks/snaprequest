<script lang="ts" module>
  import type { CoreResponse, RawBytes } from "../api/types";

  /** A response as sent or as stored in history (the stored body may be shortened). */
  export type ShownResponse = Omit<CoreResponse, "body" | "remote_addr"> & {
    body: (RawBytes & { truncated?: boolean }) | null;
    remote_addr?: string;
  };
</script>

<script lang="ts">
  import type { AssertionOutcome, CaptureOutcome, ExecutionError, SseEvent, TraceEntry } from "../api/types";
  import EventsView from "./EventsView.svelte";
  import { formatBytes, formatMs, header, PHASES, responseCookies, statusClass, statusText } from "../model/http";
  import BodyViewer from "./BodyViewer.svelte";
  import ChecksView from "./ChecksView.svelte";
  import CookiesTable from "./CookiesTable.svelte";
  import DebugView from "./DebugView.svelte";
  import HeadersTable from "./HeadersTable.svelte";
  import Tabs from "./Tabs.svelte";

  interface Props {
    response: ShownResponse | null;
    /** Why sending failed (with the trace up to the failure). */
    failure?: ExecutionError | null;
    /** A problem outside the request (connection to this server, bad input). */
    problem?: string | null;
    assertions?: AssertionOutcome[];
    captures?: CaptureOutcome[];
    logs?: string[];
    running?: boolean;
    live?: TraceEntry[];
    /** Server-Sent Events (live while running); null when the response is not a stream. */
    events?: SseEvent[] | null;
    /** Base file name for downloads. */
    name?: string;
    idPrefix: string;
    label?: string;
    empty?: string;
  }
  let {
    response: r,
    failure = null,
    problem = null,
    assertions = [],
    captures = [],
    logs = [],
    running = false,
    live = [],
    events = null,
    name = "response",
    idPrefix,
    label = "Response",
    empty = "Send the request to see the response.",
  }: Props = $props();

  type Section = "events" | "body" | "headers" | "cookies" | "checks" | "debug";
  /** What the user picked; shown whenever it is available (only Debug is, while sending). */
  let chosen = $state<Section>("body");

  const cookies = $derived(responseCookies(r?.headers));
  const failed = $derived(assertions.filter((a) => !a.passed).length);
  const trace = $derived(running ? live : (r?.trace ?? failure?.trace ?? []));
  const done = $derived(!running && !problem);
  const streaming = $derived(events !== null && (events.length > 0 || !running));
  const sections = $derived([
    ...((running && events?.length) || (done && events !== null) ? [{ id: "events" as const, label: running ? "Events (live)" : "Events", badge: events?.length ?? 0 }] : []),
    ...(r && done
      ? [
          { id: "body" as const, label: "Body" },
          { id: "headers" as const, label: "Headers", badge: r.headers.length },
          { id: "cookies" as const, label: "Cookies", badge: cookies.length },
        ]
      : []),
    ...(done && (assertions.length || captures.length || logs.length)
      ? [{ id: "checks" as const, label: "Checks", badge: failed ? `${failed} failed` : assertions.length ? `${assertions.length} ✓` : "" }]
      : []),
    ...(trace.length || running ? [{ id: "debug" as const, label: running ? "Debug (live)" : "Debug" }] : []),
  ]);
  // A stream opens on its events, unless something else was picked.
  let picked = $state(false);
  const section = $derived(
    sections.some((s) => s.id === chosen) && (picked || !streaming) ? chosen : (sections[0]?.id ?? "body"),
  );
  const fileName = $derived.by(() => {
    const ct = header(r?.headers, "content-type") ?? "";
    const ext = ct.includes("json") ? "json" : ct.includes("html") ? "html" : ct.includes("xml") ? "xml" : ct.startsWith("text/") ? "txt" : "bin";
    return `${name.replace(/[^\w.-]+/g, "-")}.${ext}`;
  });
</script>

<section class="response" aria-label={label} aria-busy={running}>
  {#if running}
    <p class="muted" role="status">Sending… {live.length} trace events so far{events?.length ? `, ${events.length} event(s)` : ""}</p>
  {:else if problem}
    <p class="s-err" role="alert">{problem}</p>
  {:else if failure}
    <div class="failure" role="alert">
      <strong class="s-err">
        {failure.kind === "execution" ? `Failed during ${failure.phase ? PHASES[failure.phase] : "the request"}` : "Invalid request"}{failure.timed_out ? " (timed out)" : ""}
      </strong>
      <p>{failure.message}</p>
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
    <p class="muted">{empty}</p>
  {/if}

  {#if sections.length}
    <Tabs tabs={sections} selected={section} label="Response parts" {idPrefix} onselect={(s) => ((chosen = s), (picked = true))} />
    <div class="panel" role="tabpanel" id="{idPrefix}-panel" aria-labelledby="{idPrefix}-tab-{section}">
      {#if section === "events"}
        <EventsView events={events ?? []} live={running} />
      {:else if section === "debug"}
        <DebugView {trace} live={running} idPrefix="{idPrefix}-dbg" />
      {:else if !r}
        <!-- only Debug without a response -->
      {:else if section === "body"}
        <BodyViewer body={r.body} contentType={header(r.headers, "content-type")} name={fileName} />
      {:else if section === "headers"}
        <HeadersTable headers={r.headers} label="Response headers" />
      {:else if section === "cookies"}
        <CookiesTable {cookies} />
      {:else if section === "checks"}
        <ChecksView {assertions} {captures} {logs} />
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
