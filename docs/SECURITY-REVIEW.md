# Security review: Phase 8 (multi-user and hardening)

Reviewed 2026-11-04 at the end of Phase 8 (P8.1–P8.6), against the server as built. Each item names
where it is implemented and the test that proves it. **Accepted risks** at the end are deliberate.

Threat model: a self-hosted server that executes HTTP requests for its users, reachable by a team
(accounts) or only by its owner on localhost (no login). Attackers: other users of the same server,
anyone who can reach it over the network, web pages open in a user's browser, and someone holding a
copy of the data directory or a backup.

## Authentication and sessions

| # | Check | Status | Where / evidence |
|---|---|---|---|
| A1 | A server reachable from the network requires a login unless the operator says the network is trusted | ✅ | `config.rs` (accounts default on non-loopback binds; `none` needs `--allow-remote`); `config::tests::remote_binds_need_accounts_or_permission` |
| A2 | Passwords hashed with a memory-hard function; minimum length | ✅ | Argon2id (`accounts.rs`); 8+ characters; `accounts::tests` |
| A3 | Login does not reveal whether a user exists | ✅ | Same message; unknown users verified against a dummy hash (`check_login`) |
| A4 | Session and token secrets are unguessable and not stored in plain text | ✅ | 256 random bits, SHA-256 digests only; `accounts::tests::secrets_are_random_and_hashed` |
| A5 | Session cookie cannot be read by scripts or sent cross-site | ✅ | HttpOnly, `SameSite=Strict`, `Secure` option; `auth::sessions_are_cookies_the_page_cannot_read` |
| A6 | Logout, password change and user deletion end sessions (also open WebSockets) | ✅ | `auth::changing_the_password_ends_other_sessions`, `the_websocket_needs_a_login_and_this_origin` |
| A7 | API tokens: shown once, revocable, optional expiry, last use recorded | ✅ | `auth::api_tokens` |
| A8 | Brute force is slowed | ✅ | Failure limits per name and address (`security.rs`); `security::failed_logins_are_limited`, `unknown_tokens_are_limited` |
| A9 | OIDC: code flow with PKCE, state bound to the browser, nonce, issuer/audience/expiry checked | ✅ | `oidc.rs`; `oidc::a_callback_from_another_browser_is_refused`, `oidc::tests::id_tokens_are_checked` |
| A10 | An OIDC identity cannot take over a local account | ✅ | Linked by `iss`+`sub`, names suffixed; `oidc::an_identity_never_takes_over_a_local_account` |
| A11 | OIDC domain allowlist needs verified addresses | ✅ | `oidc::allowed_domains_need_a_verified_address` |

## Authorization and isolation

| # | Check | Status | Where / evidence |
|---|---|---|---|
| B1 | Every API route needs a login except the listed public ones | ✅ | `auth::guard`, `is_public`; `auth::two_users_cannot_see_each_others_history_and_runs` (401s) |
| B2 | **Two users cannot read each other's workspace** (phase acceptance) | ✅ | `workspaces::a_second_user_cannot_read_another_users_workspace`; `web/e2e/accounts.spec.ts` |
| B3 | History, runs, reports and resumable flow states belong to their user | ✅ | Owner filter on every Store query; `auth::two_users_…`, `flow_states::tests` |
| B4 | Roles enforced server-side; collection files reachable only through a checked workspace | ✅ | `Space`/`EditSpace` extractors, `collection_dir(&Space, …)`; `workspaces::roles_decide_what_members_may_do` |
| B5 | Viewers cannot exfiltrate secrets (no unsaved edits, variables or data rows) | ✅ | `api::viewer_overrides`; `workspaces::roles_decide_what_members_may_do` |
| B6 | Non-members cannot learn that a workspace exists | ✅ | 404 for non-members (same as missing) |
| B7 | A workspace always keeps an owner | ✅ | `set_member`, `delete_user`; `workspaces::roles_decide_what_members_may_do` |
| B8 | Paths from requests cannot leave the workspace | ✅ | `valid_segment`, `key_path`, workspace ids from the database; `api::tests::keys_cannot_escape_or_collide`, `workspaces::workspace_ids_cannot_collide_or_escape` |

## Browser attacks

| # | Check | Status | Where / evidence |
|---|---|---|---|
| C1 | CSRF on cookie-authenticated changes | ✅ | `X-SnapReq-CSRF` header required (forces a CORS preflight, never granted); `auth::cookie_requests_that_change_things_need_the_csrf_header` |
| C2 | Cross-site WebSocket hijacking | ✅ | Origin must match `Host`/`X-Forwarded-Host`; `auth::tests::websocket_origins` |
| C3 | DNS rebinding against a no-login localhost server | ✅ | Host allowlist; `security::a_server_without_login_refuses_rebound_host_names` |
| C4 | Clickjacking, MIME sniffing, referrer leaks | ✅ | Security headers; `security::every_response_carries_security_headers` |
| C5 | XSS: UI under a strict CSP, no inline scripts; stored reports never rendered in the app's origin | ✅ | `ui.rs` CSP; report downloads are attachments with `CSP: sandbox` |
| C6 | No CORS: other origins cannot read API responses | ✅ | No CORS headers are ever sent |

## Secrets and data at rest

| # | Check | Status | Where / evidence |
|---|---|---|---|
| D1 | Secret values sealed at rest, bound to their name | ✅ | AES-256-GCM (`sealed.rs`); `sealed::tests::seal_and_open`, `sealed::plain_values_are_sealed_at_start_and_still_used` |
| D2 | Key owner-only, from env or data dir; never logged | ✅ | `load_or_create` (0600); `SecretKey` Debug redacted |
| D3 | Secret values never returned by the API, never in history, traces, run reports or the audit log | ✅ | `api::…` secret tests, `security::the_audit_log_…` (no values), redacted `response.url` |
| D4 | History and runs stored redacted | ✅ | `store::history_is_stored_without_secrets` (since P6.2) |
| D5 | OIDC client secret and session secrets redacted in Debug/logs | ✅ | `OidcConfig` / `Via` Debug impls; `config::tests::oidc_settings` |
| D6 | Backups exclude the key by default; restore refuses traversal and links | ✅ | `backup::without_the_key_sealed_secrets_stay_closed`, `backup::broken_or_malicious_archives_change_nothing` |

## Server-side request forgery and abuse

| # | Check | Status | Where / evidence |
|---|---|---|---|
| E1 | Outbound policy checked after DNS, for redirects and proxies | ✅ | ADR-020; `api::the_outbound_policy_blocks_executed_requests` |
| E2 | Cloud metadata blocked by default when others can reach the server | ✅ | `block_metadata` default on non-loopback / allow_remote; `config::tests::remote_binds_need_accounts_or_permission` |
| E3 | Request bodies and stored responses are bounded | ✅ | axum JSON limit (2 MiB), history bodies cut at 1 MiB, retention pruning |
| E4 | Audit trail of security-relevant actions | ✅ | `security.rs`; `security::the_audit_log_shows_users_their_own_events_and_owners_their_workspaces` |

## Dependencies and build

| # | Check | Status | Where / evidence |
|---|---|---|---|
| F1 | `cargo deny` (advisories, licenses, bans, sources) passes | ✅ | `scripts/check.sh`, CI |
| F2 | No `unsafe` code | ✅ | `unsafe_code = "forbid"` workspace lint |
| F3 | New crypto dependencies are mainstream and maintained | ✅ | `argon2` (RustCrypto), `ring` (already used for TLS), `tar` |

## Accepted risks

- **Editors and owners can use a workspace's secrets** (they can edit requests to send them anywhere). Roles
  are trust levels; give secrets only to workspaces whose editors may see them.
- **Plain-HTTP OIDC issuers** are accepted for local testing; the ID token is trusted because it comes
  from the token endpoint, so production issuers must use HTTPS (documented, ADR-025).
- **Any user can create workspaces** (disk use). Self-hosted, users are known; the audit log shows it.
- **Owners can tell whether a user name exists** when adding members.
- **Behind a reverse proxy** all clients share the proxy's address for rate limits and the audit log
  (`X-Forwarded-For` is not trusted).
- **No admin role**: server-wide administration (accounts, full audit log, backups) happens on the server
  machine with the CLI.
- **Ownerless workspaces** (every member deleted) keep their files but cannot be opened from the UI.
- **Key rotation** for sealed secrets is not implemented yet.
