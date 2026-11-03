<script lang="ts">
  import { app, type ScopeTab } from "../state/app.svelte";
  import { HEADER_NAMES } from "../model/headers";
  import AuthEditor from "./AuthEditor.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";
  import SettingsEditor from "./SettingsEditor.svelte";
  import Tabs from "./Tabs.svelte";
  import VarsEditor from "./VarsEditor.svelte";

  let { tab }: { tab: ScopeTab } = $props();
  type Part = "vars" | "headers" | "auth" | "settings";
  let part = $state<Part>("vars");
  const d = $derived(tab.draft);
  const parts = $derived([
    { id: "vars" as const, label: "Variables", badge: d.vars.length },
    { id: "headers" as const, label: "Headers", badge: d.headers.length },
    { id: "auth" as const, label: "Auth", badge: d.auth.type === "inherit" || d.auth.type === "none" ? "" : d.auth.type },
    { id: "settings" as const, label: "Settings", badge: Object.keys(d.settings).length },
  ]);
</script>

<section class="scope" aria-label={tab.isCollection ? "Collection settings" : "Folder settings"}>
  <div class="head">
    <h2>{tab.isCollection ? "Collection" : "Folder"} settings</h2>
    <span class="muted">{tab.collection}{tab.key ? ` / ${tab.key}` : ""} · {tab.isCollection ? "snapreq.toml" : `${tab.key}/folder.toml`}</span>
    <button type="button" class="primary save" onclick={() => app.save(tab)} disabled={!tab.dirty}>{tab.dirty ? "Save" : "Saved"}</button>
  </div>
  <div class="fields">
    <label>Name <input bind:value={d.name} placeholder={tab.isCollection ? "My API" : tab.key.split("/").pop()} /></label>
    <label>Description <input bind:value={d.description} placeholder="optional" /></label>
  </div>
  <p class="muted small">
    Everything here is inherited by the requests {tab.isCollection ? "of the collection" : "in this folder"}; requests and inner folders can override it.
  </p>
  <Tabs tabs={parts} selected={part} label="Settings parts" idPrefix="scope-{tab.id}" onselect={(p) => (part = p)} />
  <div class="panel" role="tabpanel" id="scope-{tab.id}-panel" aria-labelledby="scope-{tab.id}-tab-{part}">
    {#if part === "vars"}
      <VarsEditor
        bind:rows={d.vars}
        label="Variables"
        onsetsecret={(name) => app.setSecret(tab.collection, "shared", name)}
        secretHint="Secret values never go into the file. “Set value…” stores one for every environment in .secrets/shared.toml (git-ignored); SNAPREQ_SECRET_<NAME> and environment-specific values take precedence."
      />
    {:else if part === "headers"}
      <KeyValueEditor bind:rows={d.headers} label="Headers" names={HEADER_NAMES} />
    {:else if part === "auth"}
      <AuthEditor auth={d.auth} inCollection={!tab.isCollection} />
    {:else}
      <SettingsEditor settings={d.settings} inCollection={!tab.isCollection} />
    {/if}
  </div>
</section>

<style>
  .scope {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.8rem 1rem;
    overflow: auto;
    height: 100%;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 0.8rem;
  }
  h2 {
    margin: 0;
    font-size: 1.05rem;
  }
  .save {
    margin-left: auto;
  }
  .fields {
    display: grid;
    grid-template-columns: 1fr 2fr;
    gap: 0.8rem;
    max-width: 50rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.8rem;
    color: var(--muted);
  }
  .small {
    font-size: 0.8rem;
    margin: 0;
  }
  .panel {
    max-width: 60rem;
  }
</style>
