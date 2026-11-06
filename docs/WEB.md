# The web UI

`snapreq serve` serves the web UI at `http://127.0.0.1:7878/` (see `docs/SERVER.md` for the server and
`docs/CLI.md`, "Server", for its options). It works on the same collection files as the CLI, in the
server's workspace directory, so collections edited in the browser can be committed and run in CI.

## Requests

- **New request** (`Alt+N`) opens a request outside any collection. Choose **Save** to put it into a
  collection (it gets a key such as `users/create-user`).
- The URL, header values, body and auth fields highlight `{{variables}}`: green when defined (hover for
  the value and the scope it comes from), purple for secrets (never shown), red when undefined. Typing
  `{{` suggests the variables the request can see.
- **Params**, **Headers** (with *Bulk edit* for pasting `Name: value` lines), **Body** (JSON with
  formatting and validation, text, form, multipart, file), **Auth** (basic, bearer, digest, API key,
  OAuth 2.0, AWS Signature v4, or inherited from the folder or collection), **Checks & vars** (the file's
  `[assert]`, `[capture]` and `[vars]` as JSON) and **Settings** (timeouts, redirects, HTTP version,
  TLS verification and certificates, proxy, cookies; outside a collection also host pinning).
- **Send** (`Ctrl+Enter`) sends what is in the editor, saved or not, with the selected environment.
  **Save** (`Ctrl+S`) writes the request file.
- **Scripts** (collection requests) edits the request's pre-request and test JavaScript (`[script]`, see
  docs/FORMAT.md); test results appear with the checks, and `console.log` output under *Script output*.
- **Code** shows the request as a cURL command or a Python, JavaScript, Go or Rust snippet, with
  variables filled in and secret values read from environment variables (`SNAPREQ_SECRET_<NAME>`).

## Responses and the debug view

The response shows the status, time, size and protocol, then:

- **Body**: *Pretty* (JSON and XML formatted), *Raw*, *Preview* (HTML in a sandboxed frame without
  scripts, images) and *Hex*; *Search* (`Ctrl+F`), *Copy* and *Download*. Large bodies stay responsive:
  only what is on screen is drawn.
- **Headers**, **Cookies** (parsed `Set-Cookie`), **Checks** (assertions and captured values of
  collection requests).
- **Debug**: a **timing waterfall** of every phase (DNS, connect, TLS, send, wait, download) for each
  redirect hop, live while the request runs; the **event list** with the headers as sent and received;
  **redirects and connection** (addresses, reuse); **TLS** (version, cipher, ALPN, the certificate chain
  with expiry); the **wire log** (the bytes sent and received, text or hex; HTTP/1.1). Credentials are
  redacted everywhere.

## Collections and environments

The **Collections** sidebar shows each collection's folders and requests, its flows and environments.
Drag requests and folders to move or reorder them (or use *Move up*, *Move down*, *Move to folder…* in
an item's `⋯` menu); the menus also create, rename, duplicate and delete items, open collection and
folder settings (variables, headers, auth, settings that requests inherit) and start runs.

**Import** (`⇩` next to *New collection*) turns a file or pasted text into a new collection: a cURL
command, a Postman collection (with environment files), an OpenAPI document, an Insomnia export, a HAR
recording or a Hurl file (which becomes a flow); Postman scripts are translated to SnapRequest scripts.
Credentials found in it are stored as secret values, and
anything that could not be imported exactly is listed before the dialog closes.

The environment picker at the top selects the environment for the current collection. An
environment's editor lists its variables; variables marked secret keep their values out of the file:
**Set value…** stores one in the collection's git-ignored `.secrets/` directory, and it is never shown
again (`SNAPREQ_SECRET_<NAME>` on the server takes precedence).

## History

The **History** sidebar lists every request sent from the UI or the API, newest first, with search by URL
and filters by method, status class, failures and collection. An entry shows the stored request and
response (with its debug view). **Re-run** sends a collection request again as its file is now; other
requests are rebuilt in a new tab without their credentials, which are never stored. Select two entries
and choose **Compare** to see the differences in body (JSON pretty-printed), headers and timing.
**Download HAR** saves an entry as an HTTP Archive for browser dev tools and HAR viewers.

## Runs and flows

- **Run collection / folder / flow** (from the `⋯` menus) opens the runner: environment, iterations or
  data rows (CSV with a header row or a JSON array, pasted or loaded from a file), parallel iterations,
  delay, per-request timeout, TLS verification, stop at the first failure and run variables. Progress
  appears live; the report lists every request or step with its checks, and selecting a step shows its
  response, waterfall and wire log. Reports download as HTML, JUnit XML or JSON (the same files as
  `snapreq run --report`). The **Runs** sidebar lists past runs.
- The **flow editor** edits a `*.flow.toml`: steps are collection requests or inline requests (with the
  same editors as requests), with checks, captures, step variables, retries and delays. Reorder steps by
  dragging or with ↑ ↓. **Run all** runs the flow; **Step** runs the next step; **▶** on a step runs from
  there using the values the steps before it captured in the last run (the server keeps these, secrets
  included, in memory for an hour and never sends them to the browser). Each step's result shows the
  variables captured so far, its checks, response, waterfall and wire log.

## Login and API tokens

On a server with accounts (docs/SERVER.md, "Accounts") the UI opens with a login page. The account
button in the header (your user name) opens a dialog to create and revoke API tokens (the secret is
shown once, with a Copy button), change your password and log out. When a session ends while the UI
is open (it expired, or the password was changed elsewhere), the login comes back and the open tabs are
kept; logging in as another user starts with a fresh page. History and runs are per user.

## Workspaces

The workspace picker in the header switches between your workspaces (the page reloads, so tabs belong
to one workspace). The ⚙ button next to it shows your role, lets owners rename the workspace, manage
members (add a user by name as editor, viewer or owner; change roles; remove) and delete an empty
workspace, lets members leave, and creates new workspaces. Viewers see a "read-only" badge; changes
they try are refused by the server. A user without any workspace gets a page to create one.

## Themes, keyboard and accessibility

The theme follows the system or is chosen at the top (light, dark). Press `?` for the keyboard
shortcuts:

| Keys | Action |
|---|---|
| `Ctrl+Enter` | Send the request, start the run, or run the flow |
| `Ctrl+S` | Save |
| `Alt+N` / `Alt+W` | New request / close the tab |
| `Alt+[` / `Alt+]` | Previous / next tab |
| `Alt+L` | Go to the URL |
| `Alt+1` / `Alt+2` / `Alt+3` | Collections / history / runs |
| `↑ ↓ ← →` | Move in the collections list (← → close and open folders) and between tabs of a panel |

(`⌘` and `⌥` on macOS.) The main screens are checked against WCAG 2.1 AA with axe in both themes
(`web/e2e/a11y.spec.ts`), and everything can be done with the keyboard: menus, dialogs, drag and drop
alternatives and resizing the request/response split (arrow keys on the divider).

## What the UI does not do (yet)

- No user administration: accounts are created on the server with `snapreq user add`.
- The OAuth 2.0 *authorization code* grant needs a browser on the machine that runs the server; use it
  from the CLI, or use another grant.
- `--secret VALUE` (hiding arbitrary values) has no UI; declare such values as secret variables instead.
