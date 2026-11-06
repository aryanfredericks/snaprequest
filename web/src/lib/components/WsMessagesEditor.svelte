<script lang="ts">
  // The `[websocket]` table of a request file: subprotocols and saved messages.
  import type { WebSocketSpec, WsMessageSpec } from "../api/types";

  interface Props {
    spec: WebSocketSpec | undefined;
    onchange: (spec: WebSocketSpec | undefined) => void;
  }
  let { spec, onchange }: Props = $props();

  const messages = $derived(spec?.messages ?? []);
  const subprotocols = $derived((spec?.subprotocols ?? []).join(", "));

  function update(next: WebSocketSpec) {
    const out: WebSocketSpec = {};
    if (next.subprotocols?.length) out.subprotocols = next.subprotocols;
    if (next.messages?.length) out.messages = next.messages;
    // An empty table still marks the request as a WebSocket.
    onchange(out);
  }

  function setMessage(i: number, patch: Partial<WsMessageSpec>) {
    const list = messages.map((m, j) => (j === i ? { ...m, ...patch } : m));
    for (const m of list) if (m.kind === "text") delete m.kind;
    update({ ...spec, messages: list });
  }

  function add() {
    const taken = new Set(messages.map((m) => m.name));
    let n = messages.length + 1;
    while (taken.has(`message-${n}`)) n++;
    update({ ...spec, messages: [...messages, { name: `message-${n}`, content: "" }] });
  }

  function remove(i: number) {
    update({ ...spec, messages: messages.filter((_, j) => j !== i) });
  }
</script>

<div class="ws-spec">
  <label class="sub">
    Subprotocols
    <input
      value={subprotocols}
      placeholder="chat, v2.chat"
      aria-label="Subprotocols"
      onchange={(e) =>
        update({
          ...spec,
          subprotocols: e.currentTarget.value
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean),
        })}
    />
  </label>
  <h4>Saved messages <span class="muted">Sent from the message log; placeholders are resolved by the server.</span></h4>
  {#each messages as m, i (i)}
    <div class="msg">
      <input value={m.name} aria-label="Message name" onchange={(e) => setMessage(i, { name: e.currentTarget.value.trim() })} />
      <select value={m.kind ?? "text"} aria-label="Message kind" onchange={(e) => setMessage(i, { kind: e.currentTarget.value as WsMessageSpec["kind"] })}>
        <option value="text">text</option>
        <option value="json">json</option>
        <option value="binary">binary (base64)</option>
      </select>
      <textarea rows="2" value={m.content} aria-label="Message content {m.name}" oninput={(e) => setMessage(i, { content: e.currentTarget.value })}></textarea>
      <button type="button" class="icon" title="Remove message" aria-label="Remove message {m.name}" onclick={() => remove(i)}>✕</button>
    </div>
  {/each}
  <div><button type="button" onclick={add}>+ Message</button></div>
</div>

<style>
  .ws-spec {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .sub {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .sub input {
    flex: 1;
  }
  h4 {
    margin: 0.4rem 0 0;
    font-size: 0.85rem;
  }
  .msg {
    display: grid;
    grid-template-columns: 10rem 8rem 1fr auto;
    gap: 0.4rem;
    align-items: start;
  }
  textarea {
    font-family: var(--mono);
    font-size: 0.82rem;
    resize: vertical;
  }
</style>
