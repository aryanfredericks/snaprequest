<script lang="ts">
  import { app, type RequestTab } from "../state/app.svelte";
  import { METHODS } from "../model/draft";
  import { HEADER_NAMES } from "../model/headers";
  import AuthEditor from "./AuthEditor.svelte";
  import BodyEditor from "./BodyEditor.svelte";
  import CodeEditor from "./CodeEditor.svelte";
  import JsonField from "./JsonField.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";
  import SaveDialog from "./SaveDialog.svelte";
  import SettingsEditor from "./SettingsEditor.svelte";
  import Tabs from "./Tabs.svelte";

  interface Props {
    tab: RequestTab;
  }
  let { tab }: Props = $props();

  type Section = "params" | "headers" | "body" | "auth" | "checks" | "settings";
  let section = $state<Section>("params");
  let saving = $state(false);
  const d = $derived(tab.draft);
  const inCollection = $derived(tab.collection !== null);
  const count = (rows: { name: string; enabled: boolean }[]) => rows.filter((r) => r.enabled && r.name.trim()).length;
  const checks = $derived(
    Object.keys(d.rest.assert ?? {}).length + Object.keys(d.rest.capture ?? {}).length,
  );
  const sections = $derived([
    { id: "params" as const, label: "Params", badge: count(d.query) },
    { id: "headers" as const, label: "Headers", badge: count(d.headers) },
    { id: "body" as const, label: "Body", badge: d.body.type === "none" ? "" : d.body.type },
    { id: "auth" as const, label: "Auth", badge: d.auth.type === "inherit" || d.auth.type === "none" ? "" : d.auth.type },
    ...(inCollection ? [{ id: "checks" as const, label: "Checks & vars", badge: checks }] : []),
    { id: "settings" as const, label: "Settings", badge: Object.keys(d.settings).length },
  ]);
  const inheritedAuth = $derived.by(() => {
    const doc = tab.collection ? app.docs[tab.collection] : null;
    return doc?.file.auth?.type ?? null;
  });

  export function save() {
    if (tab.collection && tab.key) void app.save(tab);
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
    <input
      class="method m-{d.method.toLowerCase()}"
      bind:value={d.method}
      list="methods"
      aria-label="Method"
      oninput={(e) => (d.method = e.currentTarget.value.toUpperCase())}
      size="7"
    />
    <datalist id="methods">{#each METHODS as m (m)}<option value={m}></option>{/each}</datalist>
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
    <button type="submit" class="primary" disabled={tab.running} title="Send (Ctrl+Enter)">
      {tab.running ? "Sending…" : "Send"}
    </button>
    <button type="button" onclick={save} title="Save (Ctrl+S)">{tab.dirty ? "Save*" : "Save"}</button>
  </form>

  <Tabs tabs={sections} selected={section} label="Request parts" idPrefix="req-{tab.id}" onselect={(s) => (section = s)} />
  <div class="panel" role="tabpanel" id="req-{tab.id}-panel" aria-labelledby="req-{tab.id}-tab-{section}">
    {#key tab.id}
      {#if section === "params"}
        <KeyValueEditor bind:rows={d.query} label="Query parameters" vars={tab.vars} />
        <p class="muted small">Added to the query string of the URL (after any parameters written in the URL itself).</p>
      {:else if section === "headers"}
        <KeyValueEditor bind:rows={d.headers} label="Headers" vars={tab.vars} names={HEADER_NAMES} />
      {:else if section === "body"}
        <BodyEditor body={d.body} vars={tab.vars} {inCollection} />
      {:else if section === "auth"}
        <AuthEditor auth={d.auth} vars={tab.vars} {inCollection} inherited={inheritedAuth} />
      {:else if section === "checks"}
        <div class="checks">
          <h4>Checks <span class="muted">(<code>[assert]</code>, see docs/FORMAT.md)</span></h4>
          <JsonField value={d.rest.assert} label="Checks" placeholder={'{ "status": 200, "json": { "$.id": { "type": "integer" } } }'} onchange={(v) => (d.rest.assert = v as Record<string, unknown> | undefined)} />
          <h4>Captures <span class="muted">(<code>[capture]</code>)</span></h4>
          <JsonField value={d.rest.capture} label="Captures" placeholder={'{ "token": { "json": "$.token", "secret": true } }'} onchange={(v) => (d.rest.capture = v as Record<string, unknown> | undefined)} />
          <h4>Request variables <span class="muted">(<code>[vars]</code>)</span></h4>
          <JsonField value={d.rest.vars} label="Request variables" placeholder={'{ "role": "admin" }'} onchange={(v) => (d.rest.vars = v as typeof d.rest.vars)} />
        </div>
      {:else if section === "settings"}
        <SettingsEditor settings={d.settings} {inCollection} />
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
  .method {
    font-family: var(--mono);
    font-weight: 600;
    text-align: center;
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
  .error {
    color: var(--danger);
    margin: 0;
    font-size: 0.85rem;
  }
</style>
