<script lang="ts">
  import { auth } from "../state/auth.svelte";
  import { prompt } from "../state/prompt.svelte";
  import { workspaces, type Member, type Role } from "../state/workspaces.svelte";

  let dialog: HTMLDialogElement;
  let members = $state<Member[]>([]);
  let error = $state<string | null>(null);
  let name = $state("");
  let newUser = $state("");
  let newRole = $state<Role>("editor");
  let newId = $state("");
  let newName = $state("");

  const ws = $derived(workspaces.current);
  const owner = $derived(ws?.role === "owner");
  const accounts = $derived(auth.mode === "accounts");

  export function open() {
    error = null;
    name = ws?.name ?? "";
    dialog.showModal();
    void loadMembers();
  }

  async function run(what: () => Promise<void>) {
    error = null;
    try {
      await what();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function loadMembers() {
    if (accounts && ws) await run(async () => void (members = await workspaces.members(ws.id)));
  }

  const rename = (e: SubmitEvent) => {
    e.preventDefault();
    if (ws) void run(() => workspaces.rename(ws.id, name.trim()));
  };

  const add = (e: SubmitEvent) => {
    e.preventDefault();
    if (!ws) return;
    void run(async () => {
      await workspaces.setMember(ws.id, newUser.trim(), newRole);
      newUser = "";
      await loadMembers();
    });
  };

  const setRole = (m: Member, role: Role) =>
    ws &&
    run(async () => {
      await workspaces.setMember(ws.id, m.username, role);
      await loadMembers();
    });

  async function removeMember(m: Member) {
    if (!ws) return;
    const me = m.username.toLowerCase() === auth.user?.username.toLowerCase();
    const ok = await prompt.confirm(
      me ? `Leave ${ws.name}?` : `Remove ${m.username}?`,
      me ? "You will no longer see this workspace's collections." : `${m.username} will no longer see this workspace's collections.`,
      me ? "Leave" : "Remove",
    );
    if (!ok) return;
    await run(async () => {
      await workspaces.removeMember(ws.id, m.username);
      if (me) workspaces.switch("default");
      else await loadMembers();
    });
  }

  async function removeWorkspace() {
    if (!ws) return;
    if (!(await prompt.confirm(`Delete ${ws.name}?`, "Only an empty workspace can be deleted: delete its collections first."))) return;
    await run(() => workspaces.remove(ws.id));
  }

  const create = (e: SubmitEvent) => {
    e.preventDefault();
    void run(() => workspaces.create(newId.trim(), newName.trim()));
  };
</script>

<dialog bind:this={dialog} aria-labelledby="ws-title" class="ws">
  <header>
    <h2 id="ws-title">Workspace: {ws?.name ?? "none"}</h2>
    <form method="dialog"><button type="submit" aria-label="Close">✕</button></form>
  </header>

  {#if ws}
    <p class="muted">
      Directory <code>{ws.id === "default" ? "(the workspace directory)" : `${ws.id}/`}</code>; you are
      {ws.role === "owner" ? "an" : "a"} <strong>{ws.role}</strong>.
      {#if ws.role === "viewer"}Viewers read and run collections but cannot change them.{/if}
    </p>
    {#if owner}
      <form class="row" onsubmit={rename}>
        <label>Name <input bind:value={name} required maxlength="100" /></label>
        <button type="submit">Rename</button>
      </form>
    {/if}

    {#if accounts}
      <section aria-labelledby="members-title">
        <h3 id="members-title">Members</h3>
        <table>
          <thead><tr><th>User</th><th>Role</th><th><span class="sr-only">Actions</span></th></tr></thead>
          <tbody>
            {#each members as m (m.username)}
              {@const me = m.username.toLowerCase() === auth.user?.username.toLowerCase()}
              <tr>
                <td>{m.username}{me ? " (you)" : ""}</td>
                <td>
                  {#if owner}
                    <select aria-label="Role of {m.username}" value={m.role} onchange={(e) => setRole(m, e.currentTarget.value as Role)}>
                      <option value="owner">owner</option>
                      <option value="editor">editor</option>
                      <option value="viewer">viewer</option>
                    </select>
                  {:else}
                    {m.role}
                  {/if}
                </td>
                <td>
                  {#if owner || me}
                    <button type="button" class="danger" onclick={() => removeMember(m)}>{me ? "Leave" : `Remove ${m.username}`}</button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if owner}
          <form class="row" onsubmit={add}>
            <label>User name <input bind:value={newUser} required autocapitalize="none" /></label>
            <label>
              Role
              <select bind:value={newRole} aria-label="Role of the new member">
                <option value="editor">editor</option>
                <option value="viewer">viewer</option>
                <option value="owner">owner</option>
              </select>
            </label>
            <button type="submit" class="primary">Add member</button>
          </form>
        {/if}
      </section>
    {/if}
    {#if owner && ws.id !== "default"}
      <p><button type="button" class="danger" onclick={removeWorkspace}>Delete workspace</button></p>
    {/if}
  {/if}

  <section aria-labelledby="new-ws-title">
    <h3 id="new-ws-title">New workspace</h3>
    <form class="row" onsubmit={create}>
      <label>Id (directory) <input bind:value={newId} required pattern="[A-Za-z0-9_\-][A-Za-z0-9_.\-]*" placeholder="team-api" /></label>
      <label>Name <input bind:value={newName} maxlength="100" placeholder="Team API" /></label>
      <button type="submit">Create and open</button>
    </form>
  </section>
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</dialog>

<style>
  .ws {
    width: min(40rem, 94vw);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
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
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.86rem;
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
  .err {
    color: var(--danger);
  }
</style>
