//! Launcher modpack exports (Prism Launcher and others) as mod lists: Modrinth `.mrpack`
//! (modrinth.index.json) and CurseForge `.zip` (manifest.json). The browser unzips the pack
//! and sends only the index plus the SHA-1 of jars in its overrides; this turns that into
//! a [`ModList`] which the page previews like an exported list.

use crate::{
    ctx::{Ctx, display},
    modlist::{Entry, FORMAT, ModList},
    providers::{Provider, Version},
};
use axum::extract::Path;
use serde::Deserialize;
use serde_json::Value;
use shared::{
    GetState,
    models::{
        server::GetServer,
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::collections::HashMap;

const MAX_PACK_FILES: usize = 1000;

#[derive(Deserialize)]
pub struct OverrideJar {
    /// Path inside the pack, e.g. overrides/mods/foo.jar.
    path: String,
    sha1: String,
}

#[derive(Deserialize)]
pub struct PackPayload {
    /// mrpack (modrinth.index.json) / curseforge (manifest.json)
    kind: String,
    index: Value,
    #[serde(default)]
    overrides: Vec<OverrideJar>,
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn is_sha1(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Modrinth pack dependency keys ("fabric-loader") / CurseForge loader ids ("fabric-0.15.7").
fn pack_loader(raw: &str) -> Option<&'static str> {
    match raw.split('-').next().unwrap_or(raw) {
        "fabric" => Some("fabric"),
        "quilt" => Some("quilt"),
        "forge" => Some("forge"),
        "neoforge" => Some("neoforge"),
        _ => None,
    }
}

/// What a pack index lists, before anything is looked up.
#[derive(Debug, Default)]
struct Parsed {
    name: Option<String>,
    loader: Option<String>,
    game_version: Option<String>,
    /// CurseForge entries (project + file id are in the manifest).
    entries: Vec<Entry>,
    /// (file name, sha1) to identify on Modrinth.
    hashed: Vec<(String, String)>,
    unrecognised: Vec<String>,
    skipped: Vec<String>,
}

fn parse(kind: &str, index: &Value) -> Result<Parsed, anyhow::Error> {
    let mut p = Parsed {
        name: index["name"].as_str().map(|n| n.chars().take(100).collect()),
        ..Default::default()
    };
    match kind {
        "mrpack" => {
            let deps = index["dependencies"].as_object().cloned().unwrap_or_default();
            p.game_version = deps.get("minecraft").and_then(Value::as_str).map(String::from);
            p.loader = deps.keys().find_map(|k| pack_loader(k)).map(String::from);
            for f in index["files"].as_array().cloned().unwrap_or_default() {
                let path = f["path"].as_str().unwrap_or_default();
                if !(path.starts_with("mods/") || path.starts_with("plugins/")) {
                    continue; // resource packs, shaders, configs
                }
                let name = file_name(path);
                if f["env"]["server"].as_str() == Some("unsupported") {
                    p.skipped.push(name);
                    continue;
                }
                match f["hashes"]["sha1"].as_str().filter(|h| is_sha1(h)) {
                    Some(h) => p.hashed.push((name, h.to_lowercase())),
                    None => p.unrecognised.push(name),
                }
            }
        }
        "curseforge" => {
            p.game_version = index["minecraft"]["version"].as_str().map(String::from);
            let loaders = index["minecraft"]["modLoaders"].as_array().cloned().unwrap_or_default();
            p.loader = loaders
                .iter()
                .find(|l| l["primary"].as_bool() == Some(true))
                .or(loaders.first())
                .and_then(|l| l["id"].as_str())
                .and_then(pack_loader)
                .map(String::from);
            for f in index["files"].as_array().cloned().unwrap_or_default() {
                let (Some(project), Some(file)) = (f["projectID"].as_u64(), f["fileID"].as_u64()) else {
                    continue;
                };
                p.entries.push(Entry {
                    provider: Provider::Curseforge,
                    project_id: project.to_string(),
                    slug: String::new(),
                    title: format!("CurseForge project {project}"),
                    version_id: Some(file.to_string()),
                    version_number: None,
                    file: None,
                    // "required": false = the pack ships it switched off
                    enabled: f["required"].as_bool().unwrap_or(true),
                });
            }
        }
        other => return Err(display(format!("Unknown pack type \"{other}\"."))),
    }

    Ok(p)
}

pub async fn from_pack(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    shared::Payload(data): shared::Payload<PackPayload>,
) -> ApiResponseResult {
    let mut p = parse(&data.kind, &data.index)?;
    for o in &data.overrides {
        let name = file_name(&o.path);
        if is_sha1(&o.sha1) {
            p.hashed.push((name, o.sha1.to_lowercase()));
        } else {
            p.unrecognised.push(name);
        }
    }
    if p.hashed.len() + p.entries.len() > MAX_PACK_FILES {
        return Err(display(format!("The pack has more than {MAX_PACK_FILES} mods.")).into());
    }
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;

    // CurseForge names and file versions, when CurseForge is set up (otherwise the preview says so)
    if !p.entries.is_empty()
        && let Ok(cf) = ctx.curseforge()
    {
        let ids: Vec<String> = p.entries.iter().map(|e| e.project_id.clone()).collect();
        let files: Vec<String> = p.entries.iter().filter_map(|e| e.version_id.clone()).collect();
        let (projects, versions) = tokio::join!(cf.projects(&ids), cf.versions_by_id(&files));
        let projects = projects.unwrap_or_default();
        let versions = versions.unwrap_or_default();
        for e in &mut p.entries {
            if let Some(pr) = projects.iter().find(|pr| pr.id == e.project_id) {
                e.title = pr.title.clone();
                e.slug = pr.slug.clone();
            }
            if let Some(v) = versions.iter().find(|v| Some(&v.id) == e.version_id.as_ref()) {
                e.version_number = Some(v.version_number.clone());
                e.file = v.file.as_ref().map(|f| f.name.clone());
            }
        }
    }

    // every hashed jar (mrpack files, override jars) on Modrinth
    if !p.hashed.is_empty() {
        match ctx.modrinth() {
            Ok(m) => {
                let hashes: Vec<String> = p.hashed.iter().map(|(_, h)| h.clone()).collect();
                let mut found = HashMap::new();
                for chunk in hashes.chunks(200) {
                    found.extend(m.identify(chunk).await?);
                }
                let mut matched: Vec<(String, Version)> = Vec::new();
                for (name, hash) in &p.hashed {
                    match found.get(hash) {
                        Some((_, v)) => {
                            if !matched.iter().any(|(_, m)| m.project_id == v.project_id) {
                                matched.push((name.clone(), v.clone()));
                            }
                        }
                        None => p.unrecognised.push(name.clone()),
                    }
                }
                let ids: Vec<String> = matched.iter().map(|(_, v)| v.project_id.clone()).collect();
                let projects = m.projects_with_side(&ids).await.unwrap_or_default();
                for (name, v) in matched {
                    let info = projects.iter().find(|(pr, _)| pr.id == v.project_id);
                    if info.is_some_and(|(_, side)| side == "unsupported") {
                        p.skipped.push(info.map(|(pr, _)| pr.title.clone()).unwrap_or(name));
                        continue;
                    }
                    p.entries.push(Entry {
                        provider: Provider::Modrinth,
                        project_id: v.project_id.clone(),
                        slug: info.map(|(pr, _)| pr.slug.clone()).unwrap_or_default(),
                        title: info.map(|(pr, _)| pr.title.clone()).unwrap_or_else(|| name.clone()),
                        version_id: Some(v.id.clone()),
                        version_number: Some(v.version_number.clone()),
                        file: Some(name),
                        enabled: true,
                    });
                }
            }
            Err(_) => p.unrecognised.extend(p.hashed.iter().map(|(name, _)| name.clone())),
        }
    }

    if p.entries.is_empty() {
        let skipped = if p.skipped.is_empty() {
            String::new()
        } else {
            format!(" ({} client-only mods were skipped)", p.skipped.len())
        };
        return Err(display(format!("No server mods found in the pack{skipped}.")).into());
    }
    p.entries.sort_by_key(|e| e.title.to_lowercase());

    ApiResponse::new_serialized(ModList {
        format: FORMAT.into(),
        format_version: 1,
        exported_at: None,
        server: p.name,
        loader: p.loader,
        game_version: p.game_version,
        mods: p.entries,
        unrecognised: p.unrecognised,
        skipped: p.skipped,
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loaders() {
        assert_eq!(pack_loader("fabric-loader"), Some("fabric"));
        assert_eq!(pack_loader("fabric-0.15.7"), Some("fabric"));
        assert_eq!(pack_loader("neoforge-21.1.77"), Some("neoforge"));
        assert_eq!(pack_loader("quilt-loader"), Some("quilt"));
        assert_eq!(pack_loader("minecraft"), None);
    }

    #[test]
    fn mrpack() {
        let index = json!({
            "formatVersion": 1, "game": "minecraft", "name": "My Pack",
            "dependencies": { "minecraft": "26.2", "fabric-loader": "0.19.3" },
            "files": [
                { "path": "mods/fabric-api.jar", "hashes": { "sha1": "332DA34EBB72171E603A0C538DE17F4C54EA9E29" }, "env": { "client": "required", "server": "required" } },
                { "path": "mods/sodium.jar", "hashes": { "sha1": "0000000000000000000000000000000000000000" }, "env": { "client": "required", "server": "unsupported" } },
                { "path": "resourcepacks/x.zip", "hashes": { "sha1": "1111111111111111111111111111111111111111" } },
                { "path": "mods/broken.jar", "hashes": {} }
            ]
        });
        let p = parse("mrpack", &index).unwrap();
        assert_eq!(p.name.as_deref(), Some("My Pack"));
        assert_eq!(p.loader.as_deref(), Some("fabric"));
        assert_eq!(p.game_version.as_deref(), Some("26.2"));
        assert_eq!(p.hashed, vec![("fabric-api.jar".to_string(), "332da34ebb72171e603a0c538de17f4c54ea9e29".to_string())]);
        assert_eq!(p.skipped, vec!["sodium.jar"]);
        assert_eq!(p.unrecognised, vec!["broken.jar"]);
    }

    #[test]
    fn curseforge_manifest() {
        let index = json!({
            "manifestType": "minecraftModpack", "name": "CF Pack", "overrides": "overrides",
            "minecraft": { "version": "1.20.1", "modLoaders": [{ "id": "forge-47.2.0", "primary": true }] },
            "files": [
                { "projectID": 238222, "fileID": 5000001, "required": true },
                { "projectID": 1, "fileID": 2, "required": false },
                { "projectID": "bad" }
            ]
        });
        let p = parse("curseforge", &index).unwrap();
        assert_eq!(p.loader.as_deref(), Some("forge"));
        assert_eq!(p.game_version.as_deref(), Some("1.20.1"));
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].version_id.as_deref(), Some("5000001"));
        assert!(!p.entries[1].enabled);
        assert!(parse("zip", &index).is_err());
    }
}
