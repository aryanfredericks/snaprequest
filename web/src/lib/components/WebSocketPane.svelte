<script lang="ts">
  // The message log of a WebSocket request: status, a composer, saved messages and the log.
  import type { RequestTab } from "../state/app.svelte";
  import { messageSize, messageText, type WsLogEntry } from "../state/websocket.svelte";
  import Tabs from "./Tabs.svelte";
  import TraceTimeline from "./TraceTimeline.svelte";
  import HeadersTable from "./HeadersTable.svelte";

  let { tab }: { tab: RequestTab } = $props();
  const ws = $derived(tab.ws);

  let view = $state<"messages" | "handshake">("messages");
  let draft = $state("");
  let kind = $state<"text" | "binary">("text");
  let filter = $state("");
  let showControl = $state(false);
  let expanded = $state<Record<number, boolean>>({});
  let problem = $state<string | null>(null);

  const shown = $derived(
    ws.log.filter((e) => {
      if (!showControl && (e.message.kind === "ping" || e.message.kind === "pong")) return false;
      return !filter.trim() || messageText(e.message).toLowerCase().includes(filter.trim().toLowerCase());
    }),
  );

  function send() {
    problem = null;
    if (kind === "binary") {
      try {
        atob(draft.trim());
      } catch {
        problem = "Binary messages are written as base64.";
        return;
      }
      ws.send({ kind: "binary", data: draft.trim() });
    } else {
      ws.send({ kind: "text", data: draft });
    }
  }

  function pretty(e: WsLogEntry): string | null {
    if (e.message.kind !== "text") return null;
    const t = e.message.data.trim();
    if (!(t.startsWith("{") || t.startsWith("["))) return null;
    try {
      return JSON.stringify(JSON.parse(t), null, 2);
    } catch {
      return null;
    }
  }

  const statusText = $derived(
    ws.status === "idle"
      ? "Not connected"
      : ws.status === "connecting"
        ? "Connecting…"
        : ws.status === "open"
          ? `Connected${ws.protocol ? ` (subprotocol ${ws.protocol})` : ""}`
          : (ws.closeInfo ?? "Closed"),
  );
</script>

<section class="ws" aria-label="WebSocket">
  <div class="status">
    <span class="dot s-{ws.status}" aria-hidden="true"></span>
    <strong data-testid="ws-status">{statusText}</strong>
    {#if ws.handshake}
      <span class="muted">{ws.handshake.url} · {ws.handshake.timings.total_ms.toFixed(0)} ms handshake</span>
    {/if}
    <span class="grow"></span>
    <span class="muted">{ws.log.length} message(s)</span>
    <button type="button" onclick={() => ws.clear()} disabled={!ws.log.length}>Clear</button>
  </div>
  {#if ws.error}
    <p class="error" role="alert">{ws.error}</p>
  {/if}
  {#if tab.error}
    <p class="error" role="alert">{tab.error}</p>
  {/if}

  <Tabs
    tabs={[
      { id: "messages" as const, label: "Messages", badge: ws.log.length || "" },
      { id: "handshake" as const, label: "Handshake", badge: ws.trace.length || "" },
    ]}
    selected={view}
    label="WebSocket views"
    idPrefix="ws-{tab.id}"
    onselect={(v) => (view = v)}
  />
  <div class="panel" role="tabpanel" id="ws-{tab.id}-panel" aria-labelledby="ws-{tab.id}-tab-{view}">
    {#if view === "messages"}
      <form class="compose" onsubmit={(e) => (e.preventDefault(), send())}>
        <textarea
          rows="2"
          bind:value={draft}
          aria-label="Message to send"
          placeholder={kind === "binary" ? "base64, e.g. AAEC" : "Message text"}
          onkeydown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              send();
            }
          }}
        ></textarea>
        <div class="actions">
          <select bind:value={kind} aria-label="Message kind">
            <option value="text">Text</option>
            <option value="binary">Binary</option>
          </select>
          <button type="submit" class="primary" disabled={ws.status !== "open"} title="Send (Ctrl+Enter)">Send</button>
        </div>
      </form>
      {#if problem}<p class="error">{problem}</p>{/if}
      {#if ws.saved.length}
        <div class="saved" role="group" aria-label="Saved messages">
          <span class="muted">Saved:</span>
          {#each ws.saved as name (name)}
            <button type="button" disabled={ws.status !== "open"} onclick={() => ws.sendSaved(name)}>{name}</button>
          {/each}
        </div>
      {/if}
      <div class="filters">
        <input type="search" bind:value={filter} placeholder="Filter messages" aria-label="Filter messages" />
        <label><input type="checkbox" bind:checked={showControl} /> ping/pong</label>
      </div>
      <ol class="log" aria-label="Message log">
        {#each shown as e (e.id)}
          {@const json = pretty(e)}
          <li class="d-{e.direction}">
            <span class="arrow" aria-label={e.direction}>{e.direction === "sent" ? "↑" : "↓"}</span>
            <span class="at">{(e.atMs / 1000).toFixed(3)} s</span>
            <span class="kind">{e.message.kind}</span>
            <span class="size">{messageSize(e.message)} B</span>
            <span class="data">
              {#if json && expanded[e.id]}
                <pre>{json}</pre>
              {:else}
                <code>{messageText(e.message)}</code>
              {/if}
            </span>
            {#if json}
              <button type="button" class="link" aria-expanded={!!expanded[e.id]} onclick={() => (expanded[e.id] = !expanded[e.id])}>
                {expanded[e.id] ? "raw" : "pretty"}
              </button>
            {/if}
          </li>
        {:else}
          <li class="empty muted">
            {ws.status === "open" ? "No messages yet." : "Connect to send and receive messages."}
          </li>
        {/each}
      </ol>
    {:else}
      {#if ws.handshake}
        <HeadersTable headers={ws.handshake.headers} label="Handshake response headers" />
      {/if}
      <TraceTimeline trace={ws.trace} />
    {/if}
  </div>
</section>

<style>
  .ws {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    height: 100%;
    min-height: 0;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }
  .grow {
    flex: 1;
  }
  .dot {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    background: var(--muted);
  }
  .s-open {
    background: var(--ok);
  }
  .s-connecting {
    background: var(--warn);
  }
  .error {
    color: var(--danger);
    margin: 0;
    font-size: 0.85rem;
  }
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .compose {
    display: flex;
    gap: 0.4rem;
  }
  .compose textarea {
    flex: 1;
    font-family: var(--mono);
    font-size: 0.82rem;
    resize: vertical;
  }
  .actions {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .saved,
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .filters input[type="search"] {
    flex: 1;
  }
  .log {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 0.82rem;
  }
  .log li {
    display: grid;
    grid-template-columns: 1.2rem 5rem 3.6rem 4.5rem 1fr auto;
    gap: 0.4rem;
    padding: 0.2rem 0.3rem;
    border-bottom: 1px solid var(--border);
    align-items: start;
  }
  .log li.empty {
    display: block;
  }
  .d-sent .arrow {
    color: var(--accent);
  }
  .d-received .arrow {
    color: var(--ok);
  }
  .at,
  .size,
  .kind {
    color: var(--muted);
    font-family: var(--mono);
  }
  .data {
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
  .data code,
  .data pre {
    font-family: var(--mono);
    margin: 0;
  }
</style>
