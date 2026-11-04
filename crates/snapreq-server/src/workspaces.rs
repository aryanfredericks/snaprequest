//! Workspaces with roles (PLAN P8.2, ADR-024).
//!
//! A workspace is a directory of collections. `default` is the workspace directory itself (where
//! collections lived before workspaces existed); every other workspace `<id>` is the subdirectory
//! `<workspace dir>/<id>`. The database records who belongs to which workspace and with which
//! role: viewers read and run, editors also change files, owners also manage members and the
//! workspace itself. Without accounts the single user owns every workspace.
//!
//! The API picks the workspace with the `X-SnapReq-Workspace` header (default `default`); WebSocket
//! jobs carry a `workspace` field. A workspace the caller does not belong to does not exist for them.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, Path as UrlPath, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::api::{ApiError, ApiResult, ErrorBody, Shared, valid_segment};
use crate::auth::Identity;
use crate::store::{Owner, Store, db_err, now_ms};
use crate::{AppState, Error, Result};

/// The workspace that holds the collections of the workspace directory itself.
pub const DEFAULT: &str = "default";

/// Header that selects the workspace of a request.
pub const HEADER: &str = "x-snapreq-workspace";

/// What a member may do.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read collections, execute and run them.
    Viewer,
    /// Also create, change and delete collections, requests, flows, environments and secrets.
    Editor,
    /// Also manage members, rename and delete the workspace.
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Owner => "owner",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Self::Viewer),
            "editor" => Some(Self::Editor),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// A workspace as its member sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    /// The caller's role.
    pub role: Role,
}

/// A member of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Member {
    pub username: String,
    pub role: Role,
}

fn role_col(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Role> {
    let text: String = r.get(i)?;
    Role::parse(&text).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            format!("unknown role `{text}`").into(),
        )
    })
}

/// The directory of workspace `id` inside the workspace directory `root`.
pub fn dir_of(root: &std::path::Path, id: &str) -> PathBuf {
    if id == DEFAULT {
        root.to_path_buf()
    } else {
        root.join(id)
    }
}

/// How many owners `ws` would have if `user` had `role` (`None`: was removed).
fn owners_after(
    tx: &rusqlite::Transaction<'_>,
    ws: &str,
    user: i64,
    role: Option<Role>,
) -> Result<i64> {
    let others: i64 = tx
        .query_row(
            "SELECT count(*) FROM members WHERE workspace_id = ?1 AND role = 'owner' AND user_id != ?2",
            params![ws, user],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    Ok(others + i64::from(role == Some(Role::Owner)))
}

impl Store {
    /// The workspaces `owner` belongs to, by name. Without accounts (`None`): all, as owner.
    pub async fn workspaces(&self, owner: Owner) -> Result<Vec<Workspace>> {
        self.with(move |c| {
            let row = |r: &rusqlite::Row<'_>| {
                Ok(Workspace {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    role: role_col(r, 3)?,
                })
            };
            let mut stmt = c
                .prepare(
                    "SELECT w.id, w.name, w.created_at, coalesce(m.role, 'owner') FROM workspaces w
                     LEFT JOIN members m ON m.workspace_id = w.id AND m.user_id = ?1
                     WHERE ?1 IS NULL OR m.user_id IS NOT NULL
                     ORDER BY w.id != 'default', lower(w.name), w.id",
                )
                .map_err(db_err)?;
            let rows = stmt.query_map([owner], row).map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_err)
        })
        .await
    }

    /// Workspace `id` as `owner` sees it; `None` if it does not exist or they are not a member.
    pub async fn workspace(&self, owner: Owner, id: &str) -> Result<Option<Workspace>> {
        let id = id.to_owned();
        Ok(self
            .workspaces(owner)
            .await?
            .into_iter()
            .find(|w| w.id == id))
    }

    /// Records a new workspace with `owner` as its owner (none without accounts).
    pub async fn create_workspace(&self, id: &str, name: &str, owner: Owner) -> Result<Workspace> {
        let (id, name) = (id.to_owned(), name.trim().to_owned());
        self.with(move |c| {
            let tx = c.transaction().map_err(db_err)?;
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = ?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if exists {
                return Err(Error::Account(format!("workspace `{id}` already exists")));
            }
            let created_at = now_ms();
            tx.execute(
                "INSERT INTO workspaces (id, name, created_at) VALUES (?1, ?2, ?3)",
                params![id, name, created_at],
            )
            .map_err(db_err)?;
            if let Some(user) = owner {
                tx.execute(
                    "INSERT INTO members (workspace_id, user_id, role) VALUES (?1, ?2, 'owner')",
                    params![id, user],
                )
                .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
            Ok(Workspace {
                id,
                name,
                created_at,
                role: Role::Owner,
            })
        })
        .await
    }

    pub async fn rename_workspace(&self, id: &str, name: &str) -> Result<()> {
        let (id, name) = (id.to_owned(), name.trim().to_owned());
        self.with(move |c| {
            c.execute(
                "UPDATE workspaces SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map(drop)
            .map_err(db_err)
        })
        .await
    }

    /// Forgets a workspace and its memberships (its directory is removed by the caller).
    pub async fn delete_workspace(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.with(move |c| {
            c.execute("DELETE FROM workspaces WHERE id = ?1", [id])
                .map(drop)
                .map_err(db_err)
        })
        .await
    }

    /// The members of a workspace, owners first.
    pub async fn members(&self, ws: &str) -> Result<Vec<Member>> {
        let ws = ws.to_owned();
        self.with(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT u.username, m.role FROM members m JOIN users u ON u.id = m.user_id
                     WHERE m.workspace_id = ?1
                     ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'editor' THEN 1 ELSE 2 END, u.username",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([ws], |r| {
                    Ok(Member {
                        username: r.get(0)?,
                        role: role_col(r, 1)?,
                    })
                })
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_err)
        })
        .await
    }

    /// Adds `username` to a workspace, changes their role (`Some`) or removes them (`None`).
    /// A workspace keeps at least one owner. Returns the member, or `None` after a removal.
    pub async fn set_member(
        &self,
        ws: &str,
        username: &str,
        role: Option<Role>,
    ) -> Result<Option<Member>> {
        let (ws, username) = (ws.to_owned(), username.to_owned());
        self.with(move |c| {
            let tx = c.transaction().map_err(db_err)?;
            let user: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, username FROM users WHERE username = ?1",
                    [&username],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db_err)?;
            let Some((user, name)) = user else {
                return Err(Error::Account(format!("no user `{username}`")));
            };
            if owners_after(&tx, &ws, user, role)? == 0 {
                return Err(Error::Account(format!(
                    "workspace `{ws}` needs an owner: make someone else owner first"
                )));
            }
            match role {
                Some(role) => tx
                    .execute(
                        "INSERT INTO members (workspace_id, user_id, role) VALUES (?1, ?2, ?3)
                         ON CONFLICT (workspace_id, user_id) DO UPDATE SET role = excluded.role",
                        params![ws, user, role.as_str()],
                    )
                    .map_err(db_err)?,
                None => tx
                    .execute(
                        "DELETE FROM members WHERE workspace_id = ?1 AND user_id = ?2",
                        params![ws, user],
                    )
                    .map_err(db_err)?,
            };
            tx.commit().map_err(db_err)?;
            Ok(role.map(|role| Member {
                username: name,
                role,
            }))
        })
        .await
    }

    /// Workspaces that `user_id` is the only owner of while others belong to them (deleting the
    /// user would leave them without an owner).
    pub async fn sole_ownerships(&self, user_id: i64) -> Result<Vec<String>> {
        self.with(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT m.workspace_id FROM members m WHERE m.user_id = ?1 AND m.role = 'owner'
                     AND NOT EXISTS (SELECT 1 FROM members o WHERE o.workspace_id = m.workspace_id AND o.role = 'owner' AND o.user_id != ?1)
                     AND EXISTS (SELECT 1 FROM members o WHERE o.workspace_id = m.workspace_id AND o.user_id != ?1)
                     ORDER BY m.workspace_id",
                )
                .map_err(db_err)?;
            let rows = stmt.query_map([user_id], |r| r.get(0)).map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<String>, _>>()
                .map_err(db_err)
        })
        .await
    }
}

/// The workspace a request works in, with the caller's role; handlers take it as an extractor.
/// [`EditSpace`] additionally requires the editor role.
#[derive(Debug, Clone)]
pub struct Space {
    pub id: String,
    pub role: Role,
    /// The workspace's directory.
    pub dir: PathBuf,
}

impl Space {
    /// The workspace `id` for `identity`: a 404 if they are not a member.
    pub async fn resolve(state: &AppState, identity: &Identity, id: &str) -> ApiResult<Self> {
        let ws = state
            .store
            .workspace(identity.owner(), id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("no workspace `{id}`")))?;
        Ok(Self {
            dir: dir_of(&state.config.workspace, &ws.id),
            id: ws.id,
            role: ws.role,
        })
    }

    /// An error unless the caller has at least `role`.
    pub fn require(&self, role: Role) -> ApiResult<()> {
        if self.role >= role {
            Ok(())
        } else {
            Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                format!(
                    "you are {} {} of workspace `{}`; this needs the {} role",
                    if self.role == Role::Owner { "an" } else { "a" },
                    self.role.as_str(),
                    self.id,
                    role.as_str()
                ),
            ))
        }
    }
}

/// The workspace named by the request's header (`default` without one).
pub fn requested(headers: &HeaderMap) -> String {
    headers
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT)
        .to_owned()
}

impl FromRequestParts<Arc<AppState>> for Space {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let identity = Identity::from_request_parts(parts, state).await?;
        Self::resolve(state, &identity, &requested(&parts.headers)).await
    }
}

/// A [`Space`] whose caller may change files (editor or owner).
#[derive(Debug, Clone)]
pub struct EditSpace(pub Space);

impl std::ops::Deref for EditSpace {
    type Target = Space;
    fn deref(&self) -> &Space {
        &self.0
    }
}

impl FromRequestParts<Arc<AppState>> for EditSpace {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let space = Space::from_request_parts(parts, state).await?;
        space.require(Role::Editor)?;
        Ok(Self(space))
    }
}

// --- Endpoints ---------------------------------------------------------------

/// The `/api/v1/workspaces` routes.
pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(list_workspaces, create_workspace))
        .routes(routes!(update_workspace, delete_workspace))
        .routes(routes!(list_members))
        .routes(routes!(put_member, delete_member))
}

#[derive(Debug, utoipa::ToSchema)]
pub struct WorkspaceList {
    pub workspaces: Vec<Workspace>,
}

#[utoipa::path(get, path = "/workspaces", tag = "workspaces", responses((status = 200, description = "The caller's workspaces with their role (`default` first)", body = WorkspaceList)))]
async fn list_workspaces(State(state): Shared, identity: Identity) -> ApiResult<Json<Value>> {
    let list = state.store.workspaces(identity.owner()).await?;
    Ok(Json(json!({ "workspaces": list })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewWorkspace {
    /// Directory name: letters, digits, `_`, `-`, `.`.
    pub id: String,
    /// Display name (default: the id).
    pub name: Option<String>,
}

fn check_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() || name.chars().count() > 100 {
        return Err(ApiError::invalid(
            "a workspace name has 1 to 100 characters",
        ));
    }
    Ok(())
}

#[utoipa::path(post, path = "/workspaces", tag = "workspaces", request_body = NewWorkspace, responses((status = 201, description = "Created; the caller is its owner", body = Workspace), (status = 400, description = "Invalid id", body = ErrorBody), (status = 409, description = "The id is taken", body = ErrorBody)))]
/// `POST /api/v1/workspaces`: creates a workspace (a directory) owned by the caller.
async fn create_workspace(
    State(state): Shared,
    identity: Identity,
    Json(body): Json<NewWorkspace>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if !valid_segment(&body.id) || body.id == DEFAULT {
        return Err(ApiError::bad_request(format!(
            "invalid workspace id `{}` (letters, digits, `_`, `-`, `.`; not `default`)",
            body.id
        )));
    }
    let name = body.name.clone().unwrap_or_else(|| body.id.clone());
    check_name(&name)?;
    let conflict = |what: String| ApiError::new(StatusCode::CONFLICT, "conflict", what);
    let dir = dir_of(&state.config.workspace, &body.id);
    // The default workspace's collections live next to the other workspaces.
    if dir.exists() {
        return Err(conflict(format!(
            "`{}` is taken (a workspace or a collection of the default workspace)",
            body.id
        )));
    }
    let ws = state
        .store
        .create_workspace(&body.id, &name, identity.owner())
        .await
        .map_err(|e| match e {
            Error::Account(m) => conflict(m),
            other => other.into(),
        })?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let _ = state.store.delete_workspace(&body.id).await;
        return Err(ApiError::internal(format!(
            "cannot create `{}`: {e}",
            dir.display()
        )));
    }
    tracing::info!(workspace = %ws.id, "workspace created");
    Ok((StatusCode::CREATED, Json(json!(ws))))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceUpdate {
    pub name: String,
}

/// The workspace in the path, for an owner.
async fn owned(state: &AppState, identity: &Identity, id: &str) -> ApiResult<Space> {
    let space = Space::resolve(state, identity, id).await?;
    space.require(Role::Owner)?;
    Ok(space)
}

#[utoipa::path(put, path = "/workspaces/{ws}", tag = "workspaces", params(("ws" = String, Path, description = "Workspace id")), request_body = WorkspaceUpdate, responses((status = 204, description = "Renamed"), (status = 403, description = "Not an owner", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody)))]
async fn update_workspace(
    State(state): Shared,
    identity: Identity,
    UrlPath(ws): UrlPath<String>,
    Json(body): Json<WorkspaceUpdate>,
) -> ApiResult<StatusCode> {
    owned(&state, &identity, &ws).await?;
    check_name(&body.name)?;
    state.store.rename_workspace(&ws, &body.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/workspaces/{ws}", tag = "workspaces", params(("ws" = String, Path, description = "Workspace id")), responses((status = 204, description = "Deleted"), (status = 403, description = "Not an owner", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 409, description = "Still has collections, or is the default workspace", body = ErrorBody)))]
/// `DELETE /api/v1/workspaces/{ws}`: deletes an empty workspace (delete its collections first).
async fn delete_workspace(
    State(state): Shared,
    identity: Identity,
    UrlPath(ws): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let space = owned(&state, &identity, &ws).await?;
    let conflict = |what: String| ApiError::new(StatusCode::CONFLICT, "conflict", what);
    if ws == DEFAULT {
        return Err(conflict("the default workspace cannot be deleted".into()));
    }
    let left = std::fs::read_dir(&space.dir)
        .map(|d| d.filter_map(std::result::Result::ok).count())
        .unwrap_or(0);
    if left > 0 {
        return Err(conflict(format!(
            "workspace `{ws}` is not empty: delete its collections first"
        )));
    }
    let _ = std::fs::remove_dir(&space.dir);
    state.store.delete_workspace(&ws).await?;
    tracing::info!(workspace = %ws, "workspace deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, utoipa::ToSchema)]
pub struct MemberList {
    pub members: Vec<Member>,
}

#[utoipa::path(get, path = "/workspaces/{ws}/members", tag = "workspaces", params(("ws" = String, Path, description = "Workspace id")), responses((status = 200, description = "Members, owners first", body = MemberList), (status = 404, description = "Not found", body = ErrorBody), (status = 409, description = "The server has no accounts", body = ErrorBody)))]
async fn list_members(
    State(state): Shared,
    identity: Identity,
    UrlPath(ws): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    identity.account()?;
    Space::resolve(&state, &identity, &ws).await?;
    Ok(Json(json!({ "members": state.store.members(&ws).await? })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberBody {
    pub role: Role,
}

fn member_error(e: Error) -> ApiError {
    match e {
        Error::Account(m) if m.starts_with("no user") => ApiError::not_found(m),
        Error::Account(m) => ApiError::new(StatusCode::CONFLICT, "conflict", m),
        other => other.into(),
    }
}

#[utoipa::path(put, path = "/workspaces/{ws}/members/{username}", tag = "workspaces", params(("ws" = String, Path, description = "Workspace id"), ("username" = String, Path, description = "An existing user")), request_body = MemberBody, responses((status = 200, description = "The member with their role", body = Member), (status = 403, description = "Not an owner", body = ErrorBody), (status = 404, description = "No such workspace or user", body = ErrorBody), (status = 409, description = "The workspace would have no owner, or the server has no accounts", body = ErrorBody)))]
/// `PUT /api/v1/workspaces/{ws}/members/{username}`: adds a user or changes their role.
async fn put_member(
    State(state): Shared,
    identity: Identity,
    UrlPath((ws, username)): UrlPath<(String, String)>,
    Json(body): Json<MemberBody>,
) -> ApiResult<Json<Value>> {
    identity.account()?;
    owned(&state, &identity, &ws).await?;
    let member = state
        .store
        .set_member(&ws, &username, Some(body.role))
        .await
        .map_err(member_error)?;
    tracing::info!(workspace = %ws, member = %username, role = body.role.as_str(), "member set");
    Ok(Json(json!(member)))
}

#[utoipa::path(delete, path = "/workspaces/{ws}/members/{username}", tag = "workspaces", params(("ws" = String, Path, description = "Workspace id"), ("username" = String, Path, description = "A member")), responses((status = 204, description = "Removed (members may also remove themselves)"), (status = 403, description = "Not an owner", body = ErrorBody), (status = 404, description = "No such workspace or user", body = ErrorBody), (status = 409, description = "The workspace would have no owner", body = ErrorBody)))]
/// `DELETE /api/v1/workspaces/{ws}/members/{username}`: removes a member; anyone may leave.
async fn delete_member(
    State(state): Shared,
    identity: Identity,
    UrlPath((ws, username)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let me = identity.account()?;
    let space = Space::resolve(&state, &identity, &ws).await?;
    if !me.username.eq_ignore_ascii_case(&username) {
        space.require(Role::Owner)?;
    }
    state
        .store
        .set_member(&ws, &username, None)
        .await
        .map_err(member_error)?;
    tracing::info!(workspace = %ws, member = %username, "member removed");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Owner > Role::Editor && Role::Editor > Role::Viewer);
        let viewer = Space {
            id: "team".into(),
            role: Role::Viewer,
            dir: PathBuf::new(),
        };
        assert!(viewer.require(Role::Viewer).is_ok());
        let err = viewer.require(Role::Editor).unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert!(
            err.message.contains("a viewer of workspace `team`"),
            "{}",
            err.message
        );
    }

    #[test]
    fn directories_and_headers() {
        let root = std::path::Path::new("/w");
        assert_eq!(dir_of(root, DEFAULT), PathBuf::from("/w"));
        assert_eq!(dir_of(root, "team"), PathBuf::from("/w/team"));
        let mut h = HeaderMap::new();
        assert_eq!(requested(&h), DEFAULT);
        h.insert(HEADER, "team".parse().unwrap());
        assert_eq!(requested(&h), "team");
    }
}
