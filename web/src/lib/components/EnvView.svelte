<script lang="ts">
  import { app, type EnvTab } from "../state/app.svelte";
  import VarsEditor from "./VarsEditor.svelte";

  let { tab }: { tab: EnvTab } = $props();
  const inUse = $derived(app.envs[tab.collection] === tab.name);
</script>

<section class="env" aria-label="Environment {tab.name}">
  <div class="head">
    <h2>Environment <code>{tab.name}</code></h2>
    <span class="muted">{tab.collection} · environments/{tab.name}.toml</span>
    <label class="check">
      <input type="checkbox" checked={inUse} onchange={(e) => app.selectEnv(tab.collection, e.currentTarget.checked ? tab.name : null)} />
      Use for {tab.collection}
    </label>
    <button type="button" class="primary save" onclick={() => app.save(tab)} disabled={!tab.dirty}>{tab.dirty ? "Save" : "Saved"}</button>
  </div>
  <p class="muted small">Values here override the collection's variables when this environment is selected.</p>
  <div class="vars">
    <VarsEditor
      bind:rows={tab.rows}
      label="Environment variables"
      secretsSet={tab.secretsSet}
      onsetsecret={(name) => app.setSecret(tab.collection, tab.name, name)}
      secretHint="Secret values never go into the file. “Set value…” stores one in .secrets/{tab.name}.toml (git-ignored); SNAPREQ_SECRET_<NAME> on the server takes precedence. Save the environment after marking a variable secret."
    />
  </div>
  <div class="danger-zone">
    <button type="button" class="link danger" onclick={() => app.deleteEnvironment(tab.collection, tab.name)}>Delete this environment…</button>
  </div>
</section>

<style>
  .env {
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
    gap: 0.9rem;
    flex-wrap: wrap;
  }
  h2 {
    margin: 0;
    font-size: 1.05rem;
  }
  .save {
    margin-left: auto;
  }
  .small {
    font-size: 0.8rem;
    margin: 0;
  }
  .vars {
    max-width: 60rem;
  }
  .danger {
    color: var(--danger);
  }
</style>
