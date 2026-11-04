// Who is logged in (docs/SERVER.md, "Accounts"). Without accounts (`mode: "none"`) the UI never
// shows a login; with accounts it shows one until `/auth/login` succeeds, and again when any
// API call answers 401 (the session expired or was ended elsewhere).
import { api, onUnauthorized, unwrap, type Schemas } from "../api/client";

export type User = Schemas["User"];
export type ApiToken = Schemas["ApiToken"];

class AuthState {
  /** `null` until the server answered. */
  mode = $state<"none" | "accounts" | null>(null);
  user = $state<User | null>(null);
  /** The session ended while the app was open. */
  expired = $state(false);
  /** The server could not be asked. */
  error = $state<string | null>(null);

  /** Whether the app can be shown. */
  get ready(): boolean {
    return this.mode === "none" || (this.mode === "accounts" && this.user !== null);
  }

  async check() {
    try {
      const s = unwrap(await api.GET("/api/v1/auth/status"));
      this.mode = s.mode === "accounts" ? "accounts" : "none";
      this.user = s.user ?? null;
      this.error = null;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async login(username: string, password: string) {
    const previous = sessionStorage.getItem("snapreq.user");
    const s = unwrap(await api.POST("/api/v1/auth/login", { body: { username, password } }));
    const user = s.user ?? null;
    if (user && previous !== null && previous !== String(user.id)) {
      // Another user: drop everything the previous one had open.
      sessionStorage.setItem("snapreq.user", String(user.id));
      location.reload();
      return;
    }
    if (user) sessionStorage.setItem("snapreq.user", String(user.id));
    this.user = user;
    this.expired = false;
  }

  async logout() {
    await api.POST("/api/v1/auth/logout");
    sessionStorage.removeItem("snapreq.user");
    // A fresh page: no open tabs, responses or sockets of this user stay in memory.
    location.reload();
  }

  async changePassword(current: string, next: string) {
    const r = await api.POST("/api/v1/auth/password", { body: { current, new: next } });
    if (r.error !== undefined) unwrap(r);
  }

  async tokens(): Promise<ApiToken[]> {
    return unwrap(await api.GET("/api/v1/auth/tokens")).tokens;
  }

  async createToken(name: string, expiresInDays: number | null): Promise<{ token: ApiToken; secret: string }> {
    return unwrap(await api.POST("/api/v1/auth/tokens", { body: { name, expires_in_days: expiresInDays } }));
  }

  async revokeToken(id: number) {
    const r = await api.DELETE("/api/v1/auth/tokens/{id}", { params: { path: { id } } });
    if (r.error !== undefined) unwrap(r);
  }

  /** Asks the server whether the session still exists (after a refused WebSocket). */
  async verify() {
    if (this.mode !== "accounts" || !this.user) return;
    try {
      const s = unwrap(await api.GET("/api/v1/auth/status"));
      if (!s.user) this.ended();
    } catch {
      // unreachable server: the caller reports its own error
    }
  }

  /** A 401 from any call: show the login again, keeping the open tabs. */
  ended() {
    if (this.mode !== "accounts" || !this.user) return;
    this.user = null;
    this.expired = true;
  }
}

export const auth = new AuthState();
onUnauthorized(
  () => auth.ended(),
  () => void auth.verify(),
);
