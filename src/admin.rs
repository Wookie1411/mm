//! Admin settings: /api/admin/extensions/dev.emrekmek.modmanager/{settings,eggs}

use crate::settings::ExtensionSettingsData;
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use shared::{
    GetState, State,
    models::{admin_activity::GetAdminActivityLogger, user::GetPermissionManager},
    response::{ApiResponse, ApiResponseResult},
};
use utoipa_axum::router::OpenApiRouter;

/// The settings as the admin form sees them: the CurseForge key only as "set / not set".
fn public(s: &ExtensionSettingsData) -> Value {
    json!({
        "access": s.access,
        "users": s.users,
        "eggs": s.eggs,
        "modrinth_enabled": s.modrinth_enabled,
        "curseforge_enabled": s.curseforge_enabled,
        "curseforge_key_set": !s.curseforge_api_key.trim().is_empty(),
        "switch_eggs": s.switch_eggs,
    })
}

async fn read(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let settings = crate::settings::load(&state.0).await?;

    ApiResponse::new_serialized(json!({ "settings": public(&settings) })).ok()
}

/// Every egg, for the egg picker.
async fn eggs(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT nest_eggs.uuid, nest_eggs.name, nests.name
        FROM nest_eggs JOIN nests ON nests.uuid = nest_eggs.nest_uuid
        ORDER BY nests.name, nest_eggs.name",
    )
    .fetch_all(state.database.read())
    .await?;

    ApiResponse::new_serialized(json!({
        "eggs": rows.into_iter().map(|(uuid, name, nest)| json!({ "uuid": uuid, "name": name, "nest": nest })).collect::<Vec<_>>(),
    }))
    .ok()
}

#[derive(Deserialize)]
struct Payload {
    access: Option<compact_str::CompactString>,
    users: Option<Vec<uuid::Uuid>>,
    eggs: Option<Vec<uuid::Uuid>>,
    modrinth_enabled: Option<bool>,
    curseforge_enabled: Option<bool>,
    switch_eggs: Option<bool>,
    /// Absent = keep, "" = remove.
    curseforge_api_key: Option<compact_str::CompactString>,
}

async fn update(
    state: GetState,
    permissions: GetPermissionManager,
    logger: GetAdminActivityLogger,
    shared::Payload(data): shared::Payload<Payload>,
) -> ApiResponseResult {
    if data.eggs.as_ref().is_some_and(|e| e.len() > 500) {
        return Err(ApiResponse::error("Too many eggs."));
    }
    if data
        .curseforge_api_key
        .as_ref()
        .is_some_and(|k| k.len() > 256 || k.chars().any(char::is_control))
    {
        return Err(ApiResponse::error("That does not look like a CurseForge API key."));
    }
    permissions.has_admin_permission("extensions.manage")?;

    let mut settings = state.settings.get_mut().await?;
    let ext: &mut ExtensionSettingsData = settings.find_mut_extension_settings()?;
    if let Some(v) = data.access {
        if !matches!(v.as_str(), "admins" | "everyone" | "users") {
            return Err(ApiResponse::error("Unknown access mode."));
        }
        ext.access = v;
    }
    if let Some(v) = data.users {
        if v.len() > 1000 {
            return Err(ApiResponse::error("Too many users."));
        }
        ext.users = v;
    }
    if let Some(v) = data.eggs {
        ext.eggs = v;
    }
    if let Some(v) = data.modrinth_enabled {
        ext.modrinth_enabled = v;
    }
    if let Some(v) = data.curseforge_enabled {
        ext.curseforge_enabled = v;
    }
    if let Some(v) = data.switch_eggs {
        ext.switch_eggs = v;
    }
    let key_changed = data.curseforge_api_key.is_some();
    if let Some(v) = data.curseforge_api_key {
        ext.curseforge_api_key = crate::providers::curseforge::clean_key(&v).into();
    }
    let saved = public(ext);
    settings.save().await?;

    logger
        .log(
            "settings:extensions:update",
            json!({ "extension": "dev.emrekmek.modmanager", "settings": saved, "curseforge_api_key_changed": key_changed }),
        )
        .await;

    ApiResponse::new_serialized(json!({ "settings": saved })).ok()
}

#[derive(Deserialize)]
struct TestPayload {
    /// A key typed into the form but not saved yet; empty = the stored key.
    key: Option<String>,
}

/// Try a CurseForge key (typed or stored) and say what CurseForge answered.
async fn test_curseforge(
    state: GetState,
    permissions: GetPermissionManager,
    shared::Payload(data): shared::Payload<TestPayload>,
) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.manage")?;
    let typed = data.key.as_deref().map(crate::providers::curseforge::clean_key).filter(|k| !k.is_empty());
    let (key, source) = match typed {
        Some(k) => (k, "typed"),
        None => (crate::settings::load(&state.0).await?.curseforge_api_key.to_string(), "stored"),
    };
    if key.is_empty() {
        return ApiResponse::new_serialized(json!({
            "ok": false, "status": null, "source": source, "message": "No key is stored. Paste one first.",
            "shape": crate::providers::curseforge::key_shape(""),
        }))
        .ok();
    }
    let (ok, status, message) = crate::providers::curseforge::test_key(&state.client, &key).await;

    ApiResponse::new_serialized(json!({
        "ok": ok, "status": status, "source": source, "message": message,
        "shape": crate::providers::curseforge::key_shape(&key),
    }))
    .ok()
}

/// Every user, for the access picker.
async fn users(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let rows: Vec<(uuid::Uuid, String, String, bool)> =
        sqlx::query_as("SELECT uuid, username, email, admin FROM users ORDER BY username LIMIT 5000")
            .fetch_all(state.database.read())
            .await?;

    ApiResponse::new_serialized(serde_json::json!({
        "users": rows
            .into_iter()
            .map(|(uuid, username, email, admin)| serde_json::json!({ "uuid": uuid, "username": username, "email": email, "admin": admin }))
            .collect::<Vec<_>>(),
    }))
    .ok()
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .route("/settings", get(read).put(update))
        .route("/update", get(crate::update::check).post(crate::update::install))
        .route("/egg", get(crate::eggs::status).post(crate::eggs::add))
        .route("/egg/download", get(crate::eggs::download))
        .route("/users", get(users))
        .route("/eggs", get(eggs))
        .route("/test-curseforge", post(test_curseforge))
        .with_state(state.clone())
}
