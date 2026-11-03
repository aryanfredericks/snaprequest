<script lang="ts">
  import type { VariableInfo } from "../api/types";
  import { AUTH_FIELDS, AUTH_TYPES, type AuthDraft, type AuthType } from "../model/draft";
  import CodeEditor from "./CodeEditor.svelte";
  import KeyValueEditor from "./KeyValueEditor.svelte";

  interface Props {
    auth: AuthDraft;
    vars?: VariableInfo[] | null;
    inCollection: boolean;
    /** What `inherit` resolves to, if known (`bearer`, `none`, ...). */
    inherited?: string | null;
  }
  let { auth, vars = null, inCollection, inherited = null }: Props = $props();

  const labels: Record<AuthType, string> = {
    inherit: "Inherit from folder / collection",
    none: "No auth",
    basic: "Basic",
    bearer: "Bearer token",
    digest: "Digest",
    api_key: "API key",
    oauth2: "OAuth 2.0",
    aws_sigv4: "AWS Signature v4",
  };
  const types = $derived(AUTH_TYPES.filter((t) => inCollection || t !== "inherit"));
  const grant = $derived(auth.fields.grant || "client_credentials");
  const fields = $derived(AUTH_FIELDS[auth.type].filter((f) => !f.grants || f.grants.includes(grant)));
  let reveal = $state(false);
</script>

<div class="auth">
  <label class="type">
    Type
    <select bind:value={auth.type}>
      {#each types as t (t)}<option value={t}>{labels[t]}</option>{/each}
    </select>
  </label>

  {#if auth.type === "inherit"}
    <p class="muted">Uses the auth of the enclosing folder or the collection{inherited ? ` (${inherited})` : ""}.</p>
  {:else if auth.type === "none"}
    <p class="muted">No credentials are added. Headers set by hand are still sent.</p>
  {:else}
    <div class="grid">
      {#each fields as f (f.key)}
        <label for="auth-{f.key}">{f.label}{f.optional ? "" : " *"}</label>
        {#if f.options}
          <select id="auth-{f.key}" bind:value={auth.fields[f.key]}>
            {#each f.options as o (o)}<option value={o}>{o || "default"}</option>{/each}
          </select>
        {:else if f.secret && !reveal && !(auth.fields[f.key] ?? "").includes("{{")}
          <input id="auth-{f.key}" type="password" autocomplete="off" bind:value={auth.fields[f.key]} />
        {:else}
          <CodeEditor
            value={auth.fields[f.key] ?? ""}
            singleLine
            highlightVars
            {vars}
            label={f.label}
            onchange={(v) => (auth.fields[f.key] = v)}
          />
        {/if}
      {/each}
    </div>
    {#if fields.some((f) => f.secret)}
      <label class="check"><input type="checkbox" bind:checked={reveal} /> Show secret values</label>
      <p class="muted small">
        Tip: put credentials in a secret variable (<code>{"{{token}}"}</code>) instead of the file; secret values are never
        written to collection files or shown in history.
      </p>
    {/if}
    {#if auth.type === "oauth2"}
      <h4>Extra parameters</h4>
      <KeyValueEditor bind:rows={auth.params} label="OAuth 2.0 parameters" {vars} namePlaceholder="audience" />
    {/if}
  {/if}
</div>

<style>
  .auth {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    max-width: 44rem;
  }
  .type {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .grid {
    display: grid;
    grid-template-columns: 11rem 1fr;
    gap: 0.35rem 0.6rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .small {
    font-size: 0.78rem;
  }
  h4 {
    margin: 0.4rem 0 0;
    font-size: 0.85rem;
  }
</style>
