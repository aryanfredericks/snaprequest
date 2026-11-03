<script lang="ts">
  import type { SettingsSpec } from "../api/types";

  interface Props {
    settings: SettingsSpec;
    inCollection: boolean;
    /** Requests outside a collection: `host:port:addr` pins (curl's `--resolve`). */
    resolve?: string;
  }
  let { settings, inCollection, resolve = $bindable() }: Props = $props();

  type Text = "proxy" | "ca_cert" | "client_cert" | "client_key";
  function setText(k: Text, v: string) {
    if (v.trim()) settings[k] = v.trim();
    else delete settings[k];
  }
  function setTls(k: "tls_min_version" | "tls_max_version", v: string) {
    if (v === "1.2" || v === "1.3") settings[k] = v;
    else delete settings[k];
  }

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
  <label for="set-proxy">Proxy</label>
  <input id="set-proxy" value={settings.proxy ?? ""} placeholder="{inherit} (env: HTTP_PROXY…); direct; http:// or socks5h://" oninput={(e) => setText("proxy", e.currentTarget.value)} />
  <label for="set-ca">Extra CA certificates (PEM)</label>
  <input id="set-ca" value={settings.ca_cert ?? ""} placeholder={inCollection ? "path relative to the file" : "path on the server"} oninput={(e) => setText("ca_cert", e.currentTarget.value)} />
  <label for="set-cert">Client certificate (PEM)</label>
  <input id="set-cert" value={settings.client_cert ?? ""} placeholder="for mutual TLS" oninput={(e) => setText("client_cert", e.currentTarget.value)} />
  <label for="set-key">Client key (PEM)</label>
  <input id="set-key" value={settings.client_key ?? ""} placeholder="for mutual TLS" oninput={(e) => setText("client_key", e.currentTarget.value)} />
  {#each [{ k: "tls_min_version", l: "Lowest TLS version" }, { k: "tls_max_version", l: "Highest TLS version" }] as const as t (t.k)}
    <label for="set-{t.k}">{t.l}</label>
    <select id="set-{t.k}" value={settings[t.k] ?? ""} onchange={(e) => setTls(t.k, e.currentTarget.value)}>
      <option value="">{inherit}</option>
      <option value="1.2">TLS 1.2</option>
      <option value="1.3">TLS 1.3</option>
    </select>
  {/each}
  {#if !inCollection && resolve !== undefined}
    <label for="set-resolve">Pin hosts to addresses</label>
    <textarea id="set-resolve" bind:value={resolve} rows="2" placeholder="api.example.com:443:127.0.0.1 (one per line; port * = any)"></textarea>
  {/if}
</div>
{#if inCollection}
  <p class="muted small">Settings not set here come from the folders and the collection. Proxy and client certificates are set in the request file.</p>
{/if}

<style>
  textarea {
    font-family: var(--mono);
  }
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
