//! Workspaces with roles end to end (PLAN P8.2): members see a workspace's collections according to
//! their role; others cannot see it at all.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::PathBuf;

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::{Retention, Store};
use snapreq_server::{Config, Server};

struct Srv {
    addr: std::net::SocketAddr,
    store: Store,
    root: PathBuf,
}

async fn start(tag: &str, mode: AuthMode) -> Srv {
    let root = std::env::temp_dir().join(format!("snapreq-ws-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode,
            ..AuthConfig::default()
        },
        oidc: None,
    };
    let server = Server::bind(config).await.unwrap();
    let addr = server.local_addr().unwrap();
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    Srv {
        addr,
        store,
        root: root.join("workspace"),
    }
}

/// A user with an API token, working in one workspace at a time.
struct User {
    base: String,
    token: Option<String>,
    ws: Option<String>,
}

impl User {
    async fn new(srv: &Srv, name: &str) -> Self {
        let user = srv
            .store
            .create_user(name, &format!("{name}-password"))
            .await
            .unwrap();
        let (_, token) = srv.store.create_token(user.id, "t", None).await.unwrap();
        Self {
            base: format!("http://{}/api/v1", srv.addr),
            token: Some(token),
            ws: None,
        }
    }

    fn anonymous(srv: &Srv) -> Self {
        Self {
            base: format!("http://{}/api/v1", srv.addr),
            token: None,
            ws: None,
        }
    }

    fn in_ws(&self, ws: &str) -> Self {
        Self {
            base: self.base.clone(),
            token: self.token.clone(),
            ws: Some(ws.into()),
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::new(
            http::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", self.base),
        );
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(ws) = &self.ws {
            req = req.header("X-SnapReq-Workspace", ws);
        }
        if let Some(b) = body {
            req.body = Body::Json {
                content: b.to_string(),
            };
        }
        let resp = Client::default().execute(&req).await.unwrap();
        let doc = if resp.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!(resp.text()))
        };
        (resp.status, doc)
    }

    async fn status(&self, method: &str, path: &str, body: Option<Value>) -> u16 {
        self.call(method, path, body).await.0
    }
}

fn ids(doc: &Value, list: &str, field: &str) -> Vec<String> {
    doc[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w[field].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_second_user_cannot_read_another_users_workspace() {
    let srv = start("two", AuthMode::Accounts).await;
    let ana = User::new(&srv, "ana").await;
    let bo = User::new(&srv, "bo").await;

    // The first account owns `default`; the second belongs to nothing yet.
    let (_, doc) = ana.call("GET", "/workspaces", None).await;
    assert_eq!(doc["workspaces"][0]["id"], "default");
    assert_eq!(doc["workspaces"][0]["role"], "owner");
    assert_eq!(
        bo.call("GET", "/workspaces", None).await.1,
        json!({ "workspaces": [] })
    );
    assert_eq!(bo.status("GET", "/collections", None).await, 404);

    // Ana's private workspace is a subdirectory with her collection.
    let (status, doc) = ana
        .call(
            "POST",
            "/workspaces",
            Some(json!({ "id": "ana-lab", "name": "Ana's lab" })),
        )
        .await;
    assert_eq!(status, 201, "{doc}");
    assert_eq!(doc["role"], "owner");
    let lab = ana.in_ws("ana-lab");
    assert_eq!(
        lab.status("POST", "/collections", Some(json!({ "id": "secret-api" })))
            .await,
        201
    );
    assert!(srv.root.join("ana-lab/secret-api/snapreq.toml").is_file());
    std::fs::write(
        srv.root.join("ana-lab/secret-api/ping.toml"),
        format!("method = \"GET\"\nurl = \"http://{}/healthz\"\n", srv.addr),
    )
    .unwrap();
    assert_eq!(
        ids(
            &lab.call("GET", "/collections", None).await.1,
            "collections",
            "id"
        ),
        ["secret-api"]
    );
    assert_eq!(
        ana.call("GET", "/collections", None).await.1,
        json!({ "collections": [] })
    );

    // Bo cannot see, read, change, run or execute anything in it.
    let bo_lab = bo.in_ws("ana-lab");
    let (status, doc) = bo_lab.call("GET", "/collections", None).await;
    assert_eq!(
        (status, doc["error"]["message"].as_str()),
        (404, Some("no workspace `ana-lab`"))
    );
    assert_eq!(
        bo_lab.status("GET", "/collections/secret-api", None).await,
        404
    );
    assert_eq!(
        bo_lab
            .status("GET", "/collections/secret-api/requests/ping", None)
            .await,
        404
    );
    assert_eq!(
        bo_lab
            .status("GET", "/collections/secret-api/variables", None)
            .await,
        404
    );
    assert_eq!(
        bo_lab
            .status(
                "PUT",
                "/collections/secret-api/requests/x",
                Some(json!({ "method": "GET", "url": "http://x" }))
            )
            .await,
        404
    );
    assert_eq!(
        bo_lab
            .status("DELETE", "/collections/secret-api", None)
            .await,
        404
    );
    assert_eq!(
        bo_lab
            .status("POST", "/runs", Some(json!({ "collection": "secret-api" })))
            .await,
        404
    );
    assert_eq!(
        bo_lab
            .status(
                "POST",
                "/requests/execute",
                Some(json!({ "collection": "secret-api", "key": "ping" }))
            )
            .await,
        404
    );
    assert_eq!(
        bo.status("GET", "/workspaces/ana-lab/members", None).await,
        404
    );
    assert_eq!(
        bo.status(
            "PUT",
            "/workspaces/ana-lab/members/bo",
            Some(json!({ "role": "owner" }))
        )
        .await,
        404
    );
    assert_eq!(bo.status("DELETE", "/workspaces/ana-lab", None).await, 404);
    assert!(srv.root.join("ana-lab/secret-api/snapreq.toml").is_file());

    // Nor without logging in.
    let nobody = User::anonymous(&srv).in_ws("ana-lab");
    assert_eq!(nobody.status("GET", "/collections", None).await, 401);
}

#[tokio::test]
async fn roles_decide_what_members_may_do() {
    let srv = start("roles", AuthMode::Accounts).await;
    let ana = User::new(&srv, "ana").await;
    let bo = User::new(&srv, "bo").await;
    let cy = User::new(&srv, "cy").await;
    ana.call("POST", "/workspaces", Some(json!({ "id": "team" })))
        .await;
    let (a, b, c) = (ana.in_ws("team"), bo.in_ws("team"), cy.in_ws("team"));
    a.call("POST", "/collections", Some(json!({ "id": "shop" })))
        .await;
    std::fs::write(
        srv.root.join("team/shop/ping.toml"),
        format!("method = \"GET\"\nurl = \"http://{}/healthz\"\n", srv.addr),
    )
    .unwrap();

    // Only owners add members, and only existing users.
    assert_eq!(
        b.status(
            "PUT",
            "/workspaces/team/members/bo",
            Some(json!({ "role": "owner" }))
        )
        .await,
        404
    );
    let (status, doc) = ana
        .call(
            "PUT",
            "/workspaces/team/members/bo",
            Some(json!({ "role": "editor" })),
        )
        .await;
    assert_eq!(
        (status, doc),
        (200, json!({ "username": "bo", "role": "editor" }))
    );
    assert_eq!(
        ana.status(
            "PUT",
            "/workspaces/team/members/cy",
            Some(json!({ "role": "viewer" }))
        )
        .await,
        200
    );
    assert_eq!(
        ana.status(
            "PUT",
            "/workspaces/team/members/zed",
            Some(json!({ "role": "viewer" }))
        )
        .await,
        404
    );
    assert_eq!(
        ana.status(
            "PUT",
            "/workspaces/team/members/cy",
            Some(json!({ "role": "admin" }))
        )
        .await,
        422
    );
    let (_, doc) = c.call("GET", "/workspaces/team/members", None).await;
    assert_eq!(ids(&doc, "members", "username"), ["ana", "bo", "cy"]);

    // Viewers read, execute and run; they change nothing.
    assert_eq!(c.status("GET", "/collections/shop", None).await, 200);
    assert_eq!(
        c.status(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "shop", "key": "ping" }))
        )
        .await,
        200
    );
    assert_eq!(
        c.status("POST", "/runs", Some(json!({ "collection": "shop" })))
            .await,
        200
    );
    let (status, doc) = c
        .call(
            "PUT",
            "/collections/shop/requests/new",
            Some(json!({ "method": "GET", "url": "http://x" })),
        )
        .await;
    assert_eq!(
        (status, doc["error"]["kind"].as_str()),
        (403, Some("forbidden"))
    );
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("a viewer of workspace `team`")
    );
    assert_eq!(
        c.status("DELETE", "/collections/shop/requests/ping", None)
            .await,
        403
    );
    assert_eq!(
        c.status("POST", "/collections", Some(json!({ "id": "other" })))
            .await,
        403
    );
    assert_eq!(
        c.status(
            "PUT",
            "/collections/shop/secrets/shared/token",
            Some(json!({ "value": "x" }))
        )
        .await,
        403
    );
    assert_eq!(
        c.status("PUT", "/collections/shop/folders/f", Some(json!({})))
            .await,
        403
    );
    assert_eq!(
        c.status("PUT", "/collections/shop/environments/dev", Some(json!({})))
            .await,
        403
    );

    // Editors change files, but not members or the workspace.
    assert_eq!(
        b.status(
            "PUT",
            "/collections/shop/requests/new",
            Some(json!({ "method": "GET", "url": "http://x" }))
        )
        .await,
        200
    );
    assert!(srv.root.join("team/shop/new.toml").is_file());
    assert_eq!(
        bo.status(
            "PUT",
            "/workspaces/team/members/cy",
            Some(json!({ "role": "editor" }))
        )
        .await,
        403
    );
    assert_eq!(
        bo.status("PUT", "/workspaces/team", Some(json!({ "name": "Mine" })))
            .await,
        403
    );

    // A workspace always has an owner; members may leave.
    let (status, doc) = ana
        .call(
            "PUT",
            "/workspaces/team/members/ana",
            Some(json!({ "role": "editor" })),
        )
        .await;
    assert_eq!(status, 409, "{doc}");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("needs an owner")
    );
    assert!(
        srv.store
            .delete_user("ana")
            .await
            .unwrap_err()
            .to_string()
            .contains("only owner of workspace `team`")
    );
    assert_eq!(
        cy.status("DELETE", "/workspaces/team/members/cy", None)
            .await,
        204
    );
    assert_eq!(c.status("GET", "/collections/shop", None).await, 404);
    assert_eq!(
        ana.status(
            "PUT",
            "/workspaces/team/members/bo",
            Some(json!({ "role": "owner" }))
        )
        .await,
        200
    );
    assert_eq!(
        ana.status("DELETE", "/workspaces/team/members/ana", None)
            .await,
        204
    );
    assert_eq!(a.status("GET", "/collections", None).await, 404);

    // Owners rename it, and delete it once it is empty.
    assert_eq!(
        bo.status(
            "PUT",
            "/workspaces/team",
            Some(json!({ "name": "Team shop" }))
        )
        .await,
        204
    );
    assert_eq!(
        bo.call("GET", "/workspaces", None).await.1["workspaces"][0]["name"],
        "Team shop"
    );
    assert_eq!(bo.status("DELETE", "/workspaces/team", None).await, 409);
    assert_eq!(b.status("DELETE", "/collections/shop", None).await, 204);
    assert_eq!(bo.status("DELETE", "/workspaces/team", None).await, 204);
    assert!(!srv.root.join("team").exists());
    assert_eq!(
        bo.call("GET", "/workspaces", None).await.1,
        json!({ "workspaces": [] })
    );
}

#[tokio::test]
async fn workspace_ids_cannot_collide_or_escape() {
    let srv = start("ids", AuthMode::Accounts).await;
    let ana = User::new(&srv, "ana").await;
    ana.call("POST", "/collections", Some(json!({ "id": "shop" })))
        .await;
    for (id, status) in [
        ("shop", 409),
        ("default", 400),
        ("../x", 400),
        (".hidden", 400),
    ] {
        assert_eq!(
            ana.status("POST", "/workspaces", Some(json!({ "id": id })))
                .await,
            status,
            "{id}"
        );
    }
    assert_eq!(
        ana.status("POST", "/workspaces", Some(json!({ "id": "team" })))
            .await,
        201
    );
    assert_eq!(
        ana.status("POST", "/workspaces", Some(json!({ "id": "team" })))
            .await,
        409
    );
    // The default workspace cannot use a workspace's directory for a collection.
    assert_eq!(
        ana.status("POST", "/collections", Some(json!({ "id": "team" })))
            .await,
        409
    );
    assert_eq!(ana.status("DELETE", "/workspaces/default", None).await, 409);
    // Workspace directories are not collections of the default workspace.
    assert_eq!(
        ids(
            &ana.call("GET", "/collections", None).await.1,
            "collections",
            "id"
        ),
        ["shop"]
    );
}

#[tokio::test]
async fn history_and_runs_record_their_workspace() {
    let srv = start("history", AuthMode::Accounts).await;
    let ana = User::new(&srv, "ana").await;
    ana.call("POST", "/workspaces", Some(json!({ "id": "team" })))
        .await;
    for ws in ["default", "team"] {
        let u = ana.in_ws(ws);
        u.call("POST", "/collections", Some(json!({ "id": "shop" })))
            .await;
        std::fs::write(
            srv.root.join(if ws == "default" {
                "shop/ping.toml".into()
            } else {
                format!("{ws}/shop/ping.toml")
            }),
            format!(
                "method = \"GET\"\nurl = \"http://{}/healthz?ws={ws}\"\n",
                srv.addr
            ),
        )
        .unwrap();
        assert_eq!(
            u.status(
                "POST",
                "/requests/execute",
                Some(json!({ "collection": "shop", "key": "ping" }))
            )
            .await,
            200
        );
        let (_, run) = u
            .call("POST", "/runs", Some(json!({ "collection": "shop" })))
            .await;
        assert_eq!(run["workspace"], ws);
    }
    // The collection filter applies to the selected workspace.
    let (_, doc) = ana
        .in_ws("team")
        .call("GET", "/history?collection=shop", None)
        .await;
    assert_eq!(doc["entries"].as_array().unwrap().len(), 1);
    assert_eq!(doc["entries"][0]["workspace"], "team");
    assert!(
        doc["entries"][0]["url"]
            .as_str()
            .unwrap()
            .contains("ws=team")
    );
    let (_, doc) = ana.call("GET", "/history", None).await;
    assert_eq!(
        doc["entries"].as_array().unwrap().len(),
        2,
        "history lists every workspace"
    );
    let (_, doc) = ana.call("GET", "/runs", None).await;
    assert_eq!(ids(&doc, "runs", "workspace"), ["team", "default"]);
}

#[tokio::test]
async fn without_accounts_the_single_user_owns_every_workspace() {
    let srv = start("none", AuthMode::None).await;
    let me = User::anonymous(&srv);
    assert_eq!(
        me.status("POST", "/workspaces", Some(json!({ "id": "side" })))
            .await,
        201
    );
    let (_, doc) = me.call("GET", "/workspaces", None).await;
    assert_eq!(ids(&doc, "workspaces", "id"), ["default", "side"]);
    assert_eq!(ids(&doc, "workspaces", "role"), ["owner", "owner"]);
    assert_eq!(
        me.in_ws("side")
            .status("POST", "/collections", Some(json!({ "id": "x" })))
            .await,
        201
    );
    assert_eq!(
        me.status("GET", "/workspaces/side/members", None).await,
        409
    );
}
