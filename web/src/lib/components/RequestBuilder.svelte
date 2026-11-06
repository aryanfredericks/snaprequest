<script lang="ts">
  import { app, type RequestTab } from "../state/app.svelte";
  import { HEADER_NAMES } from "../model/headers";
  import AuthEditor from "./AuthEditor.svelte";
  import BodyEditor from "./BodyEditor.svelte";
  import CodeEditor from "./CodeEditor.svelte";
  import ExportDialog from "./ExportDialog.svelte";
  import JsonField from "./JsonField.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";
  import MethodSelect from "./MethodSelect.svelte";
  import SaveDialog from "./SaveDialog.svelte";
  import SettingsEditor from "./SettingsEditor.svelte";
  import Tabs from "./Tabs.svelte";
  import WsMessagesEditor from "./WsMessagesEditor.svelte";
  import { isGrpc, isWebSocket } from "../model/draft";
  import GrpcCallEditor from "./GrpcCallEditor.svelte";

  interface Props {
    tab: RequestTab;
  }
  let { tab }: Props = $props();

  type Section = "call" | "params" | "headers" | "body" | "messages" | "auth" | "checks" | "scripts" | "settings";
  let section = $state<Section>("params");
  let saving = $state(false);
  let exporter: ExportDialog | undefined = $state();
  const d = $derived(tab.draft);
  const inCollection = $derived(tab.collection !== null);
  const count = (rows: { name: string; enabled: boolean }[]) => rows.filter((r) => r.enabled && r.name.trim()).length;
  const checks = $derived(
    Object.keys(d.rest.assert ?? {}).length + Object.keys(d.rest.capture ?? {}).length,
  );
  const scripts = $derived([d.rest.script?.pre_request || d.rest.script?.pre_request_file, d.rest.script?.test || d.rest.script?.test_file].filter(Boolean).length || "");
  const websocket = $derived(isWebSocket(d));
  const grpcMode = $derived(isGrpc(d));
  const wsState = $derived(tab.ws.status);
  const sections = $derived([
    ...(grpcMode ? [{ id: "call" as const, label: "Call", badge: d.rest.grpc?.method?.split("/").pop() ?? "" }] : [{ id: "params" as const, label: "Params", badge: count(d.query) }]),
    { id: "headers" as const, label: grpcMode ? "Metadata" : "Headers", badge: count(d.headers) },
    ...(grpcMode
      ? []
      : websocket
      ? [{ id: "messages" as const, label: "Saved messages", badge: d.rest.websocket?.messages?.length ?? "" }]
      : [{ id: "body" as const, label: "Body", badge: d.body.type === "none" ? "" : d.body.type }]),
    { id: "auth" as const, label: "Auth", badge: d.auth.type === "inherit" || d.auth.type === "none" ? "" : d.auth.type },
    ...(inCollection && !websocket && !grpcMode ? [{ id: "checks" as const, label: "Checks & vars", badge: checks }] : []),
    ...(inCollection ? [{ id: "scripts" as const, label: "Scripts", badge: scripts }] : []),
    { id: "settings" as const, label: "Settings", badge: Object.keys(d.settings).length },
  ]);
  function setScript(key: "pre_request" | "test", code: string) {
    d.rest.script = { ...(d.rest.script ?? {}), [key]: code };
  }
  // GraphQL is sent with POST: switching to it moves a GET request to POST.
  let lastBodyType: string | null = null;
  $effect(() => {
    const t = d.body.type;
    if (lastBodyType !== null && t !== lastBodyType && t === "graphql" && d.method === "GET") d.method = "POST";
    lastBodyType = t;
  });
  const current = $derived(sections.some((s) => s.id === section) ? section : (sections[0]?.id ?? "params"));
  const inheritedAuth = $derived.by(() => {
    const doc = tab.collection ? app.docs[tab.collection] : null;
    return doc?.file.auth?.type ?? null;
  });

  export function save() {
    if (tab.collection && tab.key) void app.saveRequest(tab);
    else saving = true;
  }
</script>

<section class="builder" aria-label="Request">
  <div class="title">
    <input class="name" bind:value={d.name} placeholder={tab.key?.split("/").pop() ?? "Untitled request"} aria-label="Request name" />
    {#if tab.collection}
      <span class="where">{tab.collection} / {tab.key}</span>
    {:else}
      <span class="where">not saved in a collection</span>
    {/if}
  </div>
  <form class="line" onsubmit={(e) => (e.preventDefault(), app.send(tab))}>
    {#if grpcMode}
      <span class="proto" title="A gRPC request (grpc:// or grpcs:// URL)">gRPC</span>
    {:else if websocket}
      <span class="proto" title="A WebSocket request (ws:// or wss:// URL)">WS</span>
    {:else}
      <MethodSelect bind:value={d.method} label="Method" />
    {/if}
    <CodeEditor
      value={d.url}
      singleLine
      highlightVars
      vars={tab.vars}
      label="URL"
      placeholder={inCollection ? "{{base_url}}/users" : "https://api.example.com/users"}
      onchange={(v) => (d.url = v.trim() === v ? v : v.replace(/^\s+/, ""))}
      onenter={() => app.send(tab)}
      class="url"
    />
    {#if grpcMode}
      <button type="submit" class="primary" title="Call the method (Ctrl+Enter)">{tab.grpc.running ? "Cancel" : "Invoke"}</button>
    {:else if websocket}
      <button type="submit" class="primary" title="Connect or disconnect (Ctrl+Enter)">
        {wsState === "open" ? "Disconnect" : wsState === "connecting" ? "Cancel" : "Connect"}
      </button>
    {:else}
      {#if tab.running}
        <button type="button" class="primary" onclick={() => tab.stop?.()} title="Stop reading the response (streams end with what arrived)">Stop</button>
      {:else}
        <button type="submit" class="primary" title="Send (Ctrl+Enter)">Send</button>
      {/if}
    {/if}
    <button type="button" onclick={save} title="Save (Ctrl+S)">{tab.dirty ? "Save*" : "Save"}</button>
    {#if !websocket && !grpcMode}
      <button type="button" onclick={() => exporter?.open(tab)} title="Copy as cURL, Python, JavaScript, Go or Rust">Code</button>
    {/if}
  </form>
  <ExportDialog bind:this={exporter} />

  <Tabs tabs={sections} selected={current} label="Request parts" idPrefix="req-{tab.id}" onselect={(s) => (section = s)} />
  <div class="panel" role="tabpanel" id="req-{tab.id}-panel" aria-labelledby="req-{tab.id}-tab-{current}">
    {#key tab.id}
      {#if current === "params"}
        <KeyValueEditor bind:rows={d.query} label="Query parameters" vars={tab.vars} />
        <p class="muted small">Added to the query string of the URL (after any parameters written in the URL itself).</p>
      {:else if current === "headers"}
        <KeyValueEditor bind:rows={d.headers} label="Headers" vars={tab.vars} names={HEADER_NAMES} />
      {:else if current === "call"}
        <GrpcCallEditor spec={d.rest.grpc} onchange={(v) => (d.rest.grpc = v)} services={tab.grpcServices} describeState={tab.describeState} ondescribe={() => app.describeGrpc(tab)} {inCollection} />
      {:else if current === "messages"}
        <WsMessagesEditor spec={d.rest.websocket} onchange={(v) => (d.rest.websocket = v)} />
      {:else if current === "body"}
        <BodyEditor body={d.body} vars={tab.vars} {inCollection} graphql={tab.graphql} schemaState={tab.schemaState} onschema={() => app.fetchSchema(tab)} />
      {:else if current === "auth"}
        <AuthEditor auth={d.auth} vars={tab.vars} {inCollection} inherited={inheritedAuth} />
      {:else if current === "checks"}
        <div class="checks">
          <h4>Checks <span class="muted">(<code>[assert]</code>, see docs/FORMAT.md)</span></h4>
          <JsonField value={d.rest.assert} label="Checks" placeholder={'{ "status": 200, "json": { "$.id": { "type": "integer" } } }'} onchange={(v) => (d.rest.assert = v as Record<string, unknown> | undefined)} />
          <h4>Captures <span class="muted">(<code>[capture]</code>)</span></h4>
          <JsonField value={d.rest.capture} label="Captures" placeholder={'{ "token": { "json": "$.token", "secret": true } }'} onchange={(v) => (d.rest.capture = v as Record<string, unknown> | undefined)} />
          <h4>Request variables <span class="muted">(<code>[vars]</code>)</span></h4>
          <JsonField value={d.rest.vars} label="Request variables" placeholder={'{ "role": "admin" }'} onchange={(v) => (d.rest.vars = v as typeof d.rest.vars)} />
        </div>
      {:else if current === "scripts"}
        <div class="checks">
          {#each [{ key: "pre_request" as const, file: d.rest.script?.pre_request_file, title: "Pre-request script", hint: "Runs before the request is built: snapreq.variables, snapreq.request." }, { key: "test" as const, file: d.rest.script?.test_file, title: "Test script", hint: "Runs after the response: snapreq.response, snapreq.test(name, fn), snapreq.expect(value)." }] as s (s.key)}
            <h4>{s.title} <span class="muted">{s.hint}</span></h4>
            {#if s.file}
              <p class="muted small">Read from <code>{s.file}</code>; edit that file.</p>
            {:else}
              <CodeEditor value={d.rest.script?.[s.key] ?? ""} lang="javascript" label={s.title} placeholder={s.key === "test" ? 'snapreq.test("status is 200", () => snapreq.expect(snapreq.response.status).toBe(200));' : 'snapreq.variables.set("ts", Date.now());'} onchange={(v) => setScript(s.key, v)} />
            {/if}
          {/each}
          <p class="muted small">Scripts run in a sandbox without file or network access, with time and memory limits. Collection and folder scripts run first (see docs/FORMAT.md).</p>
        </div>
      {:else if current === "settings"}
        <SettingsEditor settings={d.settings} {inCollection} bind:resolve={d.resolve} />
      {/if}
    {/key}
  </div>
</section>

{#if saving}
  <SaveDialog {tab} onclose={() => (saving = false)} />
{/if}

<style>
  .builder {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-height: 0;
    height: 100%;
  }
  .title {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
  }
  .name {
    font-weight: 600;
    border-color: transparent;
    background: transparent;
    min-width: 12rem;
  }
  .name:hover,
  .name:focus {
    border-color: var(--border);
  }
  .where {
    color: var(--muted);
    font-size: 0.8rem;
  }
  .line {
    display: flex;
    gap: 0.4rem;
    align-items: stretch;
  }
  .proto {
    display: inline-flex;
    align-items: center;
    padding: 0 0.6rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    font-weight: 600;
    font-size: 0.8rem;
    color: var(--accent);
  }
  .line :global(.url) {
    flex: 1;
  }
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.2rem 0;
  }
  .checks {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  h4 {
    margin: 0.4rem 0 0;
    font-size: 0.85rem;
  }
  .small {
    font-size: 0.78rem;
  }
</style>
