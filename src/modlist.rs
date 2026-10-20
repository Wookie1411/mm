//! Mod lists: export what is installed as JSON, and preview importing such a list on
//! another server (e.g. the same modpack on a newer Minecraft version). The preview picks
//! a version for the *importing* server's loader / Minecraft version; the page then
//! installs the chosen entries one by one through /install.

use crate::{
    ctx::{Ctx, display},
    providers::{self, Project, Provider, Version},
    routes::{Target, already_installed, fetch_versions, installed_list, load_version},
};
use axum::extract::{Path, Query};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::{
    GetState,
    models::{
        server::GetServer,
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};

pub const FORMAT: &str = "calagopus-modmanager";
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub provider: Provider,
    pub project_id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub version_number: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModList {
    pub format: String,
    #[serde(default)]
    pub format_version: u32,
    #[serde(default)]
    pub exported_at: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub game_version: Option<String>,
    pub mods: Vec<Entry>,
    /// Jars no provider knows; listed so the user can copy them by hand.
    #[serde(default)]
    pub unrecognised: Vec<String>,
    /// Client-only mods left out of an imported launcher pack.
    #[serde(default)]
    pub skipped: Vec<String>,
}

pub async fn export(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    Query(target): Query<Target>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let t = target.resolve(&ctx).await?;
    let list = installed_list(&ctx, &t, false).await;

    let mut mods = Vec::new();
    let mut unrecognised = Vec::new();
    for m in list.mods {
        match (m.provider, &m.project) {
            (Some(provider), Some(project)) => mods.push(Entry {
                provider,
                project_id: project.id.clone(),
                slug: project.slug.clone(),
                title: project.title.clone(),
                version_id: m.version.as_ref().map(|v| v.id.clone()),
                version_number: m.version.as_ref().map(|v| v.version_number.clone()),
                file: Some(m.file.clone()),
                enabled: m.enabled,
            }),
            _ => unrecognised.push(m.file.clone()),
        }
    }
    mods.sort_by_key(|e| e.title.to_lowercase());

    ApiResponse::new_serialized(ModList {
        format: FORMAT.into(),
        format_version: 1,
        exported_at: Some(chrono::Utc::now().to_rfc3339()),
        server: Some(ctx.server.name.to_string()),
        loader: t.loader.map(String::from),
        game_version: t.game_version.clone(),
        mods,
        unrecognised,
        skipped: vec![],
    })
    .ok()
}

#[derive(Deserialize)]
pub struct PreviewPayload {
    list: ModList,
    /// Use the exported version ids instead of the newest compatible version.
    #[serde(default)]
    exact: bool,
    #[serde(flatten)]
    target: Target,
}

#[derive(Debug, Serialize)]
struct PreviewRow {
    entry: Entry,
    /// ready / installed / incompatible / blocked / error
    status: &'static str,
    message: Option<String>,
    version: Option<Version>,
    /// The jar already in the folder for this project.
    installed_file: Option<String>,
}

fn project_of(e: &Entry) -> Project {
    Project {
        provider: e.provider,
        id: e.project_id.clone(),
        slug: e.slug.clone(),
        title: e.title.clone(),
        description: String::new(),
        author: None,
        downloads: 0,
        follows: 0,
        icon_url: None,
        url: String::new(),
        categories: vec![],
        updated: None,
        server_side: None,
    }
}

pub async fn preview(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    shared::Payload(data): shared::Payload<PreviewPayload>,
) -> ApiResponseResult {
    if data.list.format != FORMAT {
        return Err(display("This is not a Mod Manager mod list (exported from the Manage tab).").into());
    }
    if data.list.mods.is_empty() {
        return Err(display("The mod list is empty.").into());
    }
    if data.list.mods.len() > MAX_ENTRIES {
        return Err(display(format!("A mod list can hold at most {MAX_ENTRIES} mods.")).into());
    }
    for e in &data.list.mods {
        providers::id(&e.project_id)?;
        if let Some(v) = &e.version_id {
            providers::id(v)?;
        }
    }
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let t = data.target.resolve(&ctx).await?;
    if t.directory.is_none() {
        return Err(display("Pick a Game Loader first, so the mods can go into the right folder.").into());
    }
    let installed = installed_list(&ctx, &t, true).await.mods;
    let exact = data.exact;

    let ctx_ref = &ctx;
    let t_ref = &t;
    let installed_ref = &installed;
    let mut rows: Vec<(usize, PreviewRow)> = stream::iter(data.list.mods.into_iter().enumerate())
        .map(|(i, entry)| async move {
            let row = |status, message: Option<String>, version, installed_file| PreviewRow {
                entry: entry.clone(),
                status,
                message,
                version,
                installed_file,
            };
            if let Err(e) = ctx_ref.check_provider(entry.provider) {
                return (i, row("error", Some(e.to_string()), None, None));
            }
            if let Some(m) = already_installed(installed_ref, entry.provider, &project_of(&entry)) {
                let msg = m.version.as_ref().map(|v| format!("Already installed ({})", v.version_number));
                return (i, row("installed", msg, None, Some(m.file.clone())));
            }
            let version = match (&entry.version_id, exact) {
                (Some(v), true) => load_version(ctx_ref, entry.provider, &entry.project_id, v, t_ref.loader).await.map(Some),
                _ => fetch_versions(
                    ctx_ref,
                    entry.provider,
                    &entry.project_id,
                    t_ref.loader,
                    t_ref.game_version.as_deref(),
                )
                .await
                .map(providers::pick_best),
            };
            match version {
                Ok(Some(v)) if v.file.as_ref().is_some_and(|f| f.url.is_some()) => (i, row("ready", None, Some(v), None)),
                Ok(Some(v)) => (
                    i,
                    row(
                        "blocked",
                        Some("The author blocks third-party downloads; download it from CurseForge by hand.".into()),
                        Some(v),
                        None,
                    ),
                ),
                Ok(None) => (
                    i,
                    row(
                        "incompatible",
                        Some(format!(
                            "No version for {}{} yet.",
                            t_ref.loader.unwrap_or("this loader"),
                            t_ref.game_version.as_deref().map(|v| format!(" {v}")).unwrap_or_default()
                        )),
                        None,
                        None,
                    ),
                ),
                Err(e) => (i, row("error", Some(e.to_string()), None, None)),
            }
        })
        .buffer_unordered(6)
        .collect()
        .await;
    rows.sort_by_key(|(i, _)| *i);
    let rows: Vec<PreviewRow> = rows.into_iter().map(|(_, r)| r).collect();

    ApiResponse::new_serialized(json!({
        "rows": rows,
        "unrecognised": data.list.unrecognised,
        "skipped": data.list.skipped,
        "loader": t.loader,
        "game_version": t.game_version,
        "directory": t.directory,
        "source": { "server": data.list.server, "loader": data.list.loader, "game_version": data.list.game_version },
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_list() {
        let list: ModList = serde_json::from_value(json!({
            "format": FORMAT,
            "mods": [{ "provider": "modrinth", "project_id": "P7dR8mSH" }],
        }))
        .unwrap();
        assert!(list.mods[0].enabled);
        assert!(list.unrecognised.is_empty());
    }
}
