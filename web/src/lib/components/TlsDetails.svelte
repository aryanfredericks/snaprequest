<script lang="ts">
  import type { TlsInfo } from "../api/types";
  import { daysLeft } from "../model/trace";

  let { info, host }: { info: TlsInfo; host?: string } = $props();
  const certs = $derived(info.peer_certificates ?? []);
</script>

<div class="tls">
  <dl class="summary">
    <dt>Protocol</dt><dd>{info.version}</dd>
    <dt>Cipher suite</dt><dd><code>{info.cipher}</code></dd>
    <dt>ALPN</dt><dd>{info.alpn ?? "none (HTTP/1.1)"}</dd>
    {#if host}<dt>Server name</dt><dd>{host}</dd>{/if}
  </dl>
  {#if certs.length}
    <h4>Certificate chain ({certs.length})</h4>
    <ol class="chain">
      {#each certs as c, i (i)}
        {@const days = daysLeft(c.not_after)}
        <li>
          <details open={i === 0}>
            <summary>
              <strong>{i === 0 ? "Server" : i === certs.length - 1 ? "Top of the chain" : "Intermediate"}:</strong> {c.subject}
              {#if days !== null && days < 0}<span class="s-err"> expired</span>{:else if days !== null && days < 30}<span class="warn"> expires in {days} days</span>{/if}
            </summary>
            <dl>
              <dt>Subject</dt><dd>{c.subject}</dd>
              <dt>Issuer</dt><dd>{c.issuer}</dd>
              <dt>Valid from</dt><dd>{c.not_before}</dd>
              <dt>Valid until</dt><dd>{c.not_after}{days !== null ? ` (${days < 0 ? `${-days} days ago` : `${days} days left`})` : ""}</dd>
              {#if c.subject_alt_names?.length}<dt>Names</dt><dd>{c.subject_alt_names.join(", ")}</dd>{/if}
              <dt>Serial</dt><dd><code>{c.serial}</code></dd>
              <dt>SHA-256</dt><dd><code class="fp">{c.sha256_fingerprint}</code></dd>
            </dl>
          </details>
        </li>
      {/each}
    </ol>
  {:else}
    <p class="muted">The server sent no certificates (or verification was off and none were recorded).</p>
  {/if}
</div>

<style>
  .tls {
    font-size: 0.82rem;
  }
  dl {
    display: grid;
    grid-template-columns: 8rem 1fr;
    gap: 0.2rem 0.6rem;
    margin: 0.3rem 0;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    word-break: break-word;
  }
  h4 {
    margin: 0.8rem 0 0.3rem;
    font-size: 0.85rem;
  }
  .chain {
    margin: 0;
    padding-left: 1.2rem;
  }
  summary {
    cursor: pointer;
    padding: 0.2rem 0;
  }
  .warn {
    color: var(--warn);
  }
  .fp {
    word-break: break-all;
  }
</style>
