<script lang="ts">
  import { auth, type ApiToken } from "../state/auth.svelte";

  let dialog: HTMLDialogElement;
  let tokens = $state<ApiToken[]>([]);
  let error = $state<string | null>(null);
  let created = $state<{ name: string; secret: string } | null>(null);
  let tokenName = $state("");
  let tokenDays = $state("90");
  let current = $state("");
  let next = $state("");
  let repeat = $state("");
  let passwordNote = $state<string | null>(null);
  let events = $state<Awaited<ReturnType<typeof auth.activity>>>([]);

  export function open() {
    created = null;
    error = null;
    passwordNote = null;
    dialog.showModal();
    void load();
  }

  async function load() {
    try {
      events = await auth.activity();
      tokens = await auth.tokens();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function create(e: SubmitEvent) {
    e.preventDefault();
    error = null;
    try {
      const days = tokenDays === "" ? null : Number(tokenDays);
      const t = await auth.createToken(tokenName.trim(), days);
      created = { name: t.token.name, secret: t.secret };
      tokenName = "";
      await load();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function revoke(t: ApiToken) {
    error = null;
    try {
      await auth.revokeToken(t.id);
      await load();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function change(e: SubmitEvent) {
    e.preventDefault();
    passwordNote = null;
    if (next !== repeat) {
      passwordNote = "The new passwords do not match.";
      return;
    }
    try {
      await auth.changePassword(current, next);
      current = next = repeat = "";
      passwordNote = "Password changed. Your other sessions are logged out.";
    } catch (err) {
      passwordNote = err instanceof Error ? err.message : String(err);
    }
  }

  const date = (ms: number | null | undefined) => (ms ? new Date(ms).toLocaleDateString() : "never");
</script>

<dialog bind:this={dialog} aria-labelledby="account-title" class="account">
  <header>
    <h2 id="account-title">Account: {auth.user?.username}</h2>
    <form method="dialog"><button type="submit" aria-label="Close">✕</button></form>
  </header>

  <section aria-labelledby="tokens-title">
    <h3 id="tokens-title">API tokens</h3>
    <p class="muted">
      Scripts and CI use a token as <code>Authorization: Bearer …</code>. It acts as you and sees your history and
      runs.
    </p>
    {#if created}
      <div class="created" role="status">
        <p>New token <strong>{created.name}</strong>. Copy it now: it is not shown again.</p>
        <div class="secret">
          <input readonly aria-label="New token" value={created.secret} onfocus={(e) => e.currentTarget.select()} />
          <button type="button" onclick={() => void navigator.clipboard?.writeText(created?.secret ?? "")}>Copy</button>
        </div>
      </div>
    {/if}
    {#if tokens.length}
      <table>
        <thead><tr><th>Name</th><th>Ends in</th><th>Created</th><th>Expires</th><th>Last used</th><th></th></tr></thead>
        <tbody>
          {#each tokens as t (t.id)}
            <tr>
              <td>{t.name}</td>
              <td><code>…{t.hint}</code></td>
              <td>{date(t.created_at)}</td>
              <td>{date(t.expires_at)}</td>
              <td>{t.last_used_at ? date(t.last_used_at) : "not yet"}</td>
              <td><button type="button" class="danger" aria-label="Revoke {t.name}" onclick={() => revoke(t)}>Revoke</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="muted">No tokens.</p>
    {/if}
    <form class="row" onsubmit={create}>
      <label>Name <input bind:value={tokenName} required maxlength="100" placeholder="ci" /></label>
      <label>
        Expires
        <select bind:value={tokenDays}>
          <option value="30">in 30 days</option>
          <option value="90">in 90 days</option>
          <option value="365">in a year</option>
          <option value="">never</option>
        </select>
      </label>
      <button type="submit" class="primary">Create token</button>
    </form>
    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </section>

  <section aria-labelledby="password-title">
    <h3 id="password-title">Password</h3>
    <form class="password" onsubmit={change}>
      <input type="text" autocomplete="username" value={auth.user?.username ?? ""} hidden readonly />
      <label>Current password <input type="password" bind:value={current} autocomplete="current-password" required /></label>
      <label>New password <input type="password" bind:value={next} autocomplete="new-password" minlength="8" required /></label>
      <label>Repeat it <input type="password" bind:value={repeat} autocomplete="new-password" minlength="8" required /></label>
      <button type="submit">Change password</button>
    </form>
    {#if passwordNote}<p role="status">{passwordNote}</p>{/if}
  </section>

  <section aria-labelledby="activity-title">
    <h3 id="activity-title">Recent activity</h3>
    <p class="muted">Your logins and changes, and changes in workspaces you own.</p>
    {#if events.length}
      <table>
        <thead><tr><th>When</th><th>What</th><th>Who</th><th>Details</th></tr></thead>
        <tbody>
          {#each events as e (e.id)}
            <tr>
              <td>{new Date(e.at).toLocaleString()}</td>
              <td><code>{e.action}</code></td>
              <td>{e.username ?? "–"}</td>
              <td>{[e.workspace ? `in ${e.workspace}` : "", e.target ?? "", e.ip ? `from ${e.ip}` : ""].filter(Boolean).join(" · ")}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="muted">Nothing yet.</p>
    {/if}
  </section>

  <footer>
    <button type="button" onclick={() => void auth.logout()}>Log out</button>
  </footer>
</dialog>

<style>
  .account {
    width: min(44rem, 94vw);
  }
  header,
  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  footer {
    justify-content: flex-end;
    border-top: 1px solid var(--border);
    padding-top: 0.8rem;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  h3 {
    font-size: 0.92rem;
    margin: 1rem 0 0.3rem;
  }
  .muted {
    color: var(--muted);
    font-size: 0.85rem;
    margin: 0.2rem 0 0.6rem;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.85rem;
    margin-bottom: 0.6rem;
  }
  th,
  td {
    text-align: left;
    padding: 0.3rem 0.5rem 0.3rem 0;
    border-bottom: 1px solid var(--border);
  }
  .row {
    display: flex;
    gap: 0.6rem;
    align-items: flex-end;
    flex-wrap: wrap;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.82rem;
    color: var(--fg-soft);
  }
  .password {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0.6rem;
    align-items: end;
  }
  .password button {
    justify-self: start;
  }
  .created {
    padding: 0.6rem;
    border-radius: 6px;
    background: var(--ok-bg);
    margin-bottom: 0.6rem;
    font-size: 0.88rem;
  }
  .created p {
    margin: 0 0 0.4rem;
  }
  .secret {
    display: flex;
    gap: 0.4rem;
  }
  .secret input {
    flex: 1;
    font-family: var(--mono);
  }
  .err {
    color: var(--danger);
  }
</style>
