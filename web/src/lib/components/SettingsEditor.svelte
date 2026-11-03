<script lang="ts">
  import type { SettingsSpec } from "../api/types";

  interface Props {
    settings: SettingsSpec;
    inCollection: boolean;
  }
  let { settings, inCollection }: Props = $props();

  type Flag = "follow_redirects" | "verify_tls" | "decompress" | "cookies";
  const flags: { key: Flag; label: string; def: string }[] = [
    { key: "follow_redirects", label: "Follow redirects", def: "yes" },
    { key: "verify_tls", label: "Verify TLS certificates", def: "yes" },
    { key: "decompress", label: "Decompress responses", def: "yes" },
    { key: "cookies", label: "Send and store cookies", def: "yes" },
  ];
  const inherit = $derived(inCollection ? "inherit" : "default");

  function flagValue(k: Flag): string {
    const v = settings[k];
    return v === undefined ? "" : v ? "yes" : "no";
  }
  function setFlag(k: Flag, v: string) {
    if (v === "") delete settings[k];
    else settings[k] = v === "yes";
  }
  function setNumber(k: "timeout_ms" | "connect_timeout_ms" | "max_redirects", v: string) {
    const n = Number(v);
    if (v.trim() === "" || !Number.isFinite(n) || n < 0) delete settings[k];
    else settings[k] = Math.round(n);
  }
</script>

<div class="grid">
  <label for="set-timeout">Timeout (ms)</label>
  <input id="set-timeout" type="number" min="0" value={settings.timeout_ms ?? ""} placeholder="{inherit} (30000; 0 = none)" oninput={(e) => setNumber("timeout_ms", e.currentTarget.value)} />
  <label for="set-ctimeout">Connect timeout (ms)</label>
  <input id="set-ctimeout" type="number" min="0" value={settings.connect_timeout_ms ?? ""} placeholder="{inherit} (10000)" oninput={(e) => setNumber("connect_timeout_ms", e.currentTarget.value)} />
  <label for="set-maxred">Max redirects</label>
  <input id="set-maxred" type="number" min="0" value={settings.max_redirects ?? ""} placeholder="{inherit} (10)" oninput={(e) => setNumber("max_redirects", e.currentTarget.value)} />
  <label for="set-http">HTTP version</label>
  <select id="set-http" value={settings.http_version ?? ""} onchange={(e) => {
    const v = e.currentTarget.value;
    if (v) settings.http_version = v as SettingsSpec["http_version"]; else delete settings.http_version;
  }}>
    <option value="">{inherit} (auto)</option>
    <option value="auto">auto (HTTP/2 when the server offers it)</option>
    <option value="http1">HTTP/1.1 only</option>
    <option value="http2">HTTP/2 only</option>
  </select>
  {#each flags as f (f.key)}
    <label for="set-{f.key}">{f.label}</label>
    <select id="set-{f.key}" value={flagValue(f.key)} onchange={(e) => setFlag(f.key, e.currentTarget.value)}>
      <option value="">{inherit} ({f.def})</option>
      <option value="yes">yes</option>
      <option value="no">no</option>
    </select>
  {/each}
</div>
{#if inCollection}
  <p class="muted small">Settings not set here come from the folders and the collection. Proxy and client certificates are set in the request file.</p>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: 13rem 18rem;
    gap: 0.35rem 0.6rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .small {
    font-size: 0.78rem;
  }
</style>
