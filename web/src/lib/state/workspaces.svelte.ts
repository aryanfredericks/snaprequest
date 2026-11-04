// The workspaces the user belongs to and the one the UI works in (docs/SERVER.md, "Workspaces").
// Switching reloads the page, so no tab of one workspace stays open in another.
import { api, currentWorkspace, rememberWorkspace, unwrap, type Schemas } from "../api/client";

export type Workspace = Schemas["Workspace"];
export type Member = Schemas["Member"];
export type Role = Schemas["Role"];

class WorkspaceState {
  list = $state<Workspace[]>([]);
  loaded = $state(false);
  /** The selected workspace's id. */
  readonly id = currentWorkspace();

  get current(): Workspace | null {
    return this.list.find((w) => w.id === this.id) ?? null;
  }

  get canEdit(): boolean {
    return this.current?.role === "owner" || this.current?.role === "editor";
  }

  /** Loads the list; picks another workspace (reloading) if the remembered one is gone. */
  async load() {
    this.list = unwrap(await api.GET("/api/v1/workspaces")).workspaces;
    this.loaded = true;
    const first = this.list[0];
    if (!this.current && first) this.switch(first.id);
  }

  switch(id: string) {
    rememberWorkspace(id);
    location.reload();
  }

  async create(id: string, name: string) {
    unwrap(await api.POST("/api/v1/workspaces", { body: { id, name: name || null } }));
    this.switch(id);
  }

  async rename(id: string, name: string) {
    const r = await api.PUT("/api/v1/workspaces/{ws}", { params: { path: { ws: id } }, body: { name } });
    if (r.error !== undefined) unwrap(r);
    await this.load();
  }

  async remove(id: string) {
    const r = await api.DELETE("/api/v1/workspaces/{ws}", { params: { path: { ws: id } } });
    if (r.error !== undefined) unwrap(r);
    this.switch("default");
  }

  async members(id: string): Promise<Member[]> {
    return unwrap(await api.GET("/api/v1/workspaces/{ws}/members", { params: { path: { ws: id } } })).members;
  }

  async setMember(id: string, username: string, role: Role) {
    unwrap(await api.PUT("/api/v1/workspaces/{ws}/members/{username}", { params: { path: { ws: id, username } }, body: { role } }));
  }

  async removeMember(id: string, username: string) {
    const r = await api.DELETE("/api/v1/workspaces/{ws}/members/{username}", { params: { path: { ws: id, username } } });
    if (r.error !== undefined) unwrap(r);
  }
}

export const workspaces = new WorkspaceState();
