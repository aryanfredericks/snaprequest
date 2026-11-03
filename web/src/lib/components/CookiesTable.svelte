<script lang="ts">
  import type { Cookie } from "../model/http";

  let { cookies }: { cookies: Cookie[] } = $props();
</script>

{#if cookies.length === 0}
  <p class="muted">The response sets no cookies. (Cookies are kept per server in a shared jar and sent with later requests unless a request turns cookies off.)</p>
{:else}
  <table aria-label="Cookies set by the response">
    <thead>
      <tr><th>Name</th><th>Value</th><th>Domain</th><th>Path</th><th>Expires</th><th>Flags</th></tr>
    </thead>
    <tbody>
      {#each cookies as c, i (i)}
        <tr>
          <td class="n">{c.name}</td>
          <td class="v">{c.value}</td>
          <td>{c.domain ?? "(host)"}</td>
          <td>{c.path ?? "/"}</td>
          <td>{c.maxAge ? `in ${c.maxAge} s` : (c.expires ?? "session")}</td>
          <td>{[c.secure && "Secure", c.httpOnly && "HttpOnly", c.sameSite && `SameSite=${c.sameSite}`, c.partitioned && "Partitioned"].filter(Boolean).join(", ")}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.82rem;
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--muted);
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
  }
  td {
    padding: 0.25rem 0.5rem;
    border-top: 1px solid var(--border);
  }
  .n {
    font-weight: 600;
  }
  .v {
    font-family: var(--mono);
    word-break: break-all;
  }
</style>
