//! The collection tree: folders (`folder.toml`), moving and reordering items, and secret
//! values (`.secrets/`). Used by the web UI's sidebar and editors.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::load::{COLLECTION_FILE, FOLDER_FILE};
use snapreq_collection::save::{save_collection, save_folder};
use snapreq_collection::{Collection, FolderFile, Item};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::api::{ApiError, ApiResult, collection_dir, key_path, load_collection, valid_segment};
use crate::workspaces::{EditSpace, Space};

pub(crate) fn router() -> OpenApiRouter<std::sync::Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(get_folder, put_folder, delete_folder))
        .routes(routes!(move_item))
        .routes(routes!(set_secret))
}

/// The items of the folder at `key` (`""`: the collection root), in display order.
fn children<'a>(c: &'a Collection, key: &str) -> Option<&'a [Item]> {
    let mut items = c.items.as_slice();
    for part in key.split('/').filter(|p| !p.is_empty()) {
        items = items.iter().find_map(|i| match i {
            Item::Folder(f) if f.key == part => Some(f.items.as_slice()),
            _ => None,
        })?;
    }
    Some(items)
}

fn folder_file<'a>(c: &'a Collection, key: &str) -> Option<&'a FolderFile> {
    let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
    children(c, parent)?.iter().find_map(|i| match i {
        Item::Folder(f) if f.key == name => Some(&f.file),
        _ => None,
    })
}

/// Writes the `order` list of the folder at `key` (`""`: the collection).
fn write_order(c: &Collection, key: &str, order: Vec<String>) -> ApiResult<()> {
    let internal = |e: snapreq_collection::Error| ApiError::internal(e.to_string());
    if key.is_empty() {
        let mut file = c.file.clone();
        file.order = order;
        save_collection(&c.root.join(COLLECTION_FILE), &file).map_err(internal)
    } else {
        let mut file = folder_file(c, key).cloned().unwrap_or_default();
        file.order = order;
        save_folder(&c.root.join(key).join(FOLDER_FILE), &file).map_err(internal)
    }
}

fn order_of(c: &Collection, key: &str) -> Vec<String> {
    if key.is_empty() {
        c.file.order.clone()
    } else {
        folder_file(c, key)
            .map(|f| f.order.clone())
            .unwrap_or_default()
    }
}

/// Removes `name` from the `order` of folder `parent` (after it was deleted or moved away).
pub(crate) fn forget_in_order(root: &Path, parent: &str, name: &str) -> ApiResult<()> {
    let Ok((c, _)) = Collection::load(root) else {
        return Ok(());
    };
    let mut order = order_of(&c, parent);
    let before = order.len();
    order.retain(|n| n != name);
    if order.len() != before {
        write_order(&c, parent, order)?;
    }
    Ok(())
}

fn split_key(key: &str) -> (&str, &str) {
    key.rsplit_once('/').unwrap_or(("", key))
}

// --- Folders ---------------------------------------------------------------------------

#[utoipa::path(get, path = "/collections/{id}/folders/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Folder key, e.g. `users/admin`")), responses((status = 200, description = "`{ key, file }`: the folder's `folder.toml` as JSON (defaults when absent)", body = crate::api::JsonDocument), (status = 400, description = "Bad request", body = crate::api::ErrorBody), (status = 404, description = "Not found", body = crate::api::ErrorBody), (status = 422, description = "Does not validate", body = crate::api::ErrorBody)))]
async fn get_folder(
    space: Space,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    let key = key.trim_matches('/');
    let file =
        folder_file(&c, key).ok_or_else(|| ApiError::not_found(format!("no folder `{key}`")))?;
    Ok(Json(json!({ "key": key, "file": file })))
}

#[utoipa::path(put, path = "/collections/{id}/folders/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Folder key")), request_body(content = Object, description = "`folder.toml` as JSON (`{}` for a plain folder)"), responses((status = 200, description = "Saved; the folder is created if needed", body = crate::api::JsonDocument), (status = 400, description = "Bad request", body = crate::api::ErrorBody), (status = 404, description = "Not found", body = crate::api::ErrorBody), (status = 422, description = "Does not validate", body = crate::api::ErrorBody)))]
async fn put_folder(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
    Json(file): Json<FolderFile>,
) -> ApiResult<Json<Value>> {
    load_collection(&space, &id)?;
    let key = key.trim_matches('/');
    let dir = collection_dir(&space, &id)?.join(key_path(key, "")?);
    if dir.is_file() || dir.with_extension("toml").is_file() && !dir.is_dir() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("a request `{key}` exists"),
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let path = dir.join(FOLDER_FILE);
    if file == FolderFile::default() {
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| ApiError::internal(e.to_string()))?;
        }
    } else {
        save_folder(&path, &file).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(Json(json!({ "key": key, "file": file })))
}

#[utoipa::path(delete, path = "/collections/{id}/folders/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Folder key")), responses((status = 204, description = "Deleted with everything in it"), (status = 400, description = "Bad request", body = crate::api::ErrorBody), (status = 404, description = "Not found", body = crate::api::ErrorBody), (status = 422, description = "Does not validate", body = crate::api::ErrorBody)))]
async fn delete_folder(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    load_collection(&space, &id)?;
    let key = key.trim_matches('/');
    let root = collection_dir(&space, &id)?;
    let dir = root.join(key_path(key, "")?);
    if !dir.is_dir() {
        return Err(ApiError::not_found(format!("no folder `{key}`")));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let (parent, name) = split_key(key);
    forget_in_order(&root, parent, name)?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Moving ----------------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Request,
    Folder,
    Flow,
}

/// Where a moved item goes in its folder's order.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// The sibling (by name) to put the item before; absent puts it last.
    pub before: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveBody {
    pub kind: ItemKind,
    /// Key of the item to move.
    pub from: String,
    /// Key of the destination folder; `""` is the collection root.
    #[serde(default)]
    pub to: String,
    /// New name (file or directory name without extension); absent keeps the name.
    pub name: Option<String>,
    /// Position among the destination's items (written to its `order`); absent keeps
    /// the order rules as they are. Not used for flows.
    pub place: Option<Place>,
}

#[utoipa::path(post, path = "/collections/{id}/move", tag = "collections", params(("id" = String, Path, description = "Collection id")), request_body = MoveBody, responses((status = 200, description = "`{ key }`: the item's new key", body = crate::api::JsonDocument), (status = 400, description = "Bad request", body = crate::api::ErrorBody), (status = 404, description = "Not found", body = crate::api::ErrorBody), (status = 409, description = "The destination exists", body = crate::api::ErrorBody), (status = 422, description = "Does not validate", body = crate::api::ErrorBody)))]
/// Moves, renames or reorders a request, folder or flow.
async fn move_item(
    space: EditSpace,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<MoveBody>,
) -> ApiResult<Json<Value>> {
    load_collection(&space, &id)?;
    let root = collection_dir(&space, &id)?;
    let from = body.from.trim_matches('/').to_owned();
    let to = body.to.trim_matches('/').to_owned();
    let (old_parent, old_name) = split_key(&from);
    let name = body.name.as_deref().unwrap_or(old_name).to_owned();
    if !valid_segment(&name) {
        return Err(ApiError::bad_request(format!("invalid name `{name}`")));
    }
    let new_key = if to.is_empty() {
        name.clone()
    } else {
        format!("{to}/{name}")
    };
    let suffix = match body.kind {
        ItemKind::Request => ".toml",
        ItemKind::Folder => "",
        ItemKind::Flow => snapreq_collection::flow::FLOW_SUFFIX,
    };
    let source = root.join(key_path(&from, suffix)?);
    let target = root.join(key_path(&new_key, suffix)?);
    let exists = match body.kind {
        ItemKind::Folder => source.is_dir(),
        _ => source.is_file(),
    };
    if !exists {
        return Err(ApiError::not_found(format!(
            "no {} `{from}`",
            kind_name(&body.kind)
        )));
    }
    if !to.is_empty() && !root.join(key_path(&to, "")?).is_dir() {
        return Err(ApiError::not_found(format!("no folder `{to}`")));
    }
    if matches!(body.kind, ItemKind::Folder) && (to == from || to.starts_with(&format!("{from}/")))
    {
        return Err(ApiError::bad_request(
            "a folder cannot be moved into itself",
        ));
    }
    if new_key != from {
        if target.exists() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "conflict",
                format!("`{new_key}` already exists"),
            ));
        }
        std::fs::rename(&source, &target).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if matches!(body.kind, ItemKind::Flow) {
        return Ok(Json(json!({ "key": new_key })));
    }

    // Keep the `order` lists in step: the old entry goes, the new one goes where it was asked
    // to (or where the old one was, for a rename in place).
    let (c, _) = Collection::load(&root).map_err(|e| ApiError::invalid(e.to_string()))?;
    let mut old_order = order_of(&c, old_parent);
    let old_position = old_order.iter().position(|n| n == old_name);
    if old_parent != to || old_name != name {
        if let (Some(i), true) = (old_position, old_parent == to && body.place.is_none()) {
            old_order[i] = name.clone();
            write_order(&c, old_parent, old_order)?;
        } else if old_position.is_some() {
            old_order.retain(|n| n != old_name);
            write_order(&c, old_parent, old_order)?;
        }
    }
    if let Some(place) = &body.place {
        let (c, _) = Collection::load(&root).map_err(|e| ApiError::invalid(e.to_string()))?;
        let mut order: Vec<String> = children(&c, &to)
            .unwrap_or_default()
            .iter()
            .map(|i| i.key().to_owned())
            .filter(|k| *k != name)
            .collect();
        let at = place
            .before
            .as_ref()
            .and_then(|b| order.iter().position(|k| k == b))
            .unwrap_or(order.len());
        order.insert(at, name.clone());
        write_order(&c, &to, order)?;
    }
    Ok(Json(json!({ "key": new_key })))
}

fn kind_name(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Request => "request",
        ItemKind::Folder => "folder",
        ItemKind::Flow => "flow",
    }
}

// --- Secrets ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SecretBody {
    pub value: String,
}

#[utoipa::path(put, path = "/collections/{id}/secrets/{scope}/{name}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("scope" = String, Path, description = "An environment name, or `shared` for every environment"), ("name" = String, Path, description = "Variable name")), request_body = SecretBody, responses((status = 204, description = "Stored in `.secrets/<scope>.toml` (never returned)"), (status = 400, description = "Bad request", body = crate::api::ErrorBody), (status = 404, description = "Not found", body = crate::api::ErrorBody), (status = 422, description = "Does not validate", body = crate::api::ErrorBody)))]
/// Like `snapreq secret set`: stores a secret value in the collection's `.secrets/` directory.
async fn set_secret(
    space: EditSpace,
    UrlPath((id, scope, name)): UrlPath<(String, String, String)>,
    Json(body): Json<SecretBody>,
) -> ApiResult<StatusCode> {
    let c = load_collection(&space, &id)?;
    if body.value.is_empty() {
        return Err(ApiError::bad_request("the value is empty"));
    }
    let env = match scope.as_str() {
        "shared" => None,
        env if valid_segment(env) => Some(env),
        other => return Err(ApiError::bad_request(format!("invalid scope `{other}`"))),
    };
    if let Some(env) = env {
        snapreq_collection::Environment::load(&c, env)
            .map_err(|e| ApiError::not_found(e.to_string()))?;
    }
    let path: PathBuf = snapreq_collection::secrets::store(&c.root, env, &name, &body.value)
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    tracing::info!(collection = %id, scope = %scope, variable = %name, file = %path.display(), "secret stored");
    Ok(StatusCode::NO_CONTENT)
}
