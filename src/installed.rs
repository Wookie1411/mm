//! What is installed: every jar in the mods / plugins folder, identified by the
//! install record, Modrinth (SHA-1) or CurseForge (fingerprint), with update info.

use crate::{
    ctx::Ctx,
    files::Entry,
    providers::{Identified, Project, Provider, Version, curseforge},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use wings_api::Algorithm;

pub const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installed {
    /// File name in the directory, including `.disabled`.
    pub file: String,
    pub size: u64,
    pub modified: i64,
    pub enabled: bool,
    pub sha1: Option<String>,
    pub provider: Option<Provider>,
    pub project: Option<Project>,
    pub version: Option<Version>,
    /// Newest compatible version when it differs from the installed one.
    pub update: Option<Version>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledList {
    pub directory: Option<String>,
    pub loader: Option<String>,
    pub game_version: Option<String>,
    pub mods: Vec<Installed>,
    /// Lookups that failed (a provider down, no CurseForge key …), shown as a notice.
    pub warnings: Vec<String>,
    pub generated_at: i64,
}

/// A .jar (mods / plugins) or .zip (data packs), enabled or disabled.
pub fn is_content_file(e: &Entry, loader: Option<&str>) -> bool {
    let name = e.name.strip_suffix(DISABLED_SUFFIX).unwrap_or(&e.name).to_lowercase();
    e.file && name.ends_with(crate::providers::extension_for(loader))
}

/// Identifications from the install records, keyed by SHA-1.
async fn records(ctx: &Ctx) -> HashMap<String, Identified> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT sha1, provider, project_id, version_id FROM dev_emrekmek_modmanager_installs WHERE server_uuid = $1",
    )
    .bind(ctx.server.uuid)
    .fetch_all(ctx.state.database.read())
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|(sha1, provider, project_id, version_id)| {
            Some((
                sha1.trim().to_lowercase(),
                Identified {
                    provider: Provider::parse(&provider).ok()?,
                    project_id,
                    version_id,
                },
            ))
        })
        .collect()
}

pub async fn record(ctx: &Ctx, sha1: &str, version: &Version) {
    record_ids(ctx, sha1, version.provider, &version.project_id, &version.id).await
}

pub async fn record_ids(ctx: &Ctx, sha1: &str, provider: Provider, project_id: &str, version_id: &str) {
    if let Err(e) = sqlx::query(
        "INSERT INTO dev_emrekmek_modmanager_installs (server_uuid, sha1, provider, project_id, version_id, installed)
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (server_uuid, sha1) DO UPDATE SET provider = EXCLUDED.provider,
            project_id = EXCLUDED.project_id, version_id = EXCLUDED.version_id, installed = EXCLUDED.installed",
    )
    .bind(ctx.server.uuid)
    .bind(sha1.to_lowercase())
    .bind(provider.as_str())
    .bind(project_id)
    .bind(version_id)
    .execute(ctx.state.database.write())
    .await
    {
        tracing::warn!("modmanager: could not record install: {e:?}");
    }
}

pub async fn scan(ctx: &Ctx, directory: Option<String>, loader: Option<&str>, game_version: Option<&str>) -> InstalledList {
    let mut warnings = Vec::new();
    let now = chrono::Utc::now().timestamp();
    let empty = |warnings| InstalledList {
        directory: directory.clone(),
        loader: loader.map(String::from),
        game_version: game_version.map(String::from),
        mods: vec![],
        warnings,
        generated_at: now,
    };
    let Some(dir) = directory.clone() else {
        return empty(vec![]);
    };
    let entries = match ctx.files.list(&dir).await {
        Ok(Some(entries)) => entries,
        Ok(None) => return empty(vec![]),
        Err(e) => return empty(vec![e.to_string()]),
    };
    let mut entries: Vec<Entry> = entries.into_iter().filter(|e| is_content_file(e, loader)).collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    let paths: Vec<String> = entries.iter().map(|e| format!("{dir}/{}", e.name)).collect();

    let curseforge = ctx.curseforge().ok();
    let (sha1s, fingerprints) = tokio::join!(ctx.files.fingerprints(&paths, Algorithm::Sha1), async {
        match &curseforge {
            Some(_) => ctx.files.fingerprints(&paths, Algorithm::Curseforge).await,
            None => Default::default(),
        }
    });
    let sha1_of = |path: &str| sha1s.get(path).map(|h| h.to_lowercase());

    // --- identify: records, then Modrinth, then CurseForge
    let records = records(ctx).await;
    let mut ident: HashMap<String, Identified> = HashMap::new(); // path -> identified
    let mut known_versions: HashMap<(Provider, String), Version> = HashMap::new();
    for path in &paths {
        if let Some(sha1) = sha1_of(path)
            && let Some(id) = records.get(&sha1)
        {
            ident.insert(path.clone(), id.clone());
        }
    }

    let unknown_sha1: Vec<String> = paths
        .iter()
        .filter(|p| !ident.contains_key(*p))
        .filter_map(|p| sha1_of(p))
        .collect();
    if ctx.settings.modrinth_enabled && !unknown_sha1.is_empty() {
        match ctx.modrinth() {
            Ok(m) => match m.identify(&unknown_sha1).await {
                Ok(found) => {
                    for path in &paths {
                        if let Some(sha1) = sha1_of(path)
                            && let Some((id, v)) = found.get(&sha1)
                        {
                            ident.entry(path.clone()).or_insert_with(|| id.clone());
                            known_versions.insert((Provider::Modrinth, v.id.clone()), v.clone());
                        }
                    }
                }
                Err(e) => warnings.push(e.to_string()),
            },
            Err(e) => warnings.push(e.to_string()),
        }
    }

    if let Some(cf) = &curseforge {
        let unknown: Vec<(String, u32)> = paths
            .iter()
            .filter(|p| !ident.contains_key(*p))
            .filter_map(|p| fingerprints.get(p).and_then(|f| curseforge::parse_fingerprint(f)).map(|f| (p.clone(), f)))
            .collect();
        if !unknown.is_empty() {
            let fps: Vec<u32> = unknown.iter().map(|(_, f)| *f).collect();
            match cf.identify(&fps).await {
                Ok(found) => {
                    for (path, fp) in &unknown {
                        if let Some((id, v)) = found.get(fp) {
                            ident.insert(path.clone(), id.clone());
                            known_versions.insert((Provider::Curseforge, v.id.clone()), v.clone());
                        }
                    }
                }
                Err(e) => warnings.push(e.to_string()),
            }
        }
    }

    // --- projects and versions we do not have yet
    let mut projects: HashMap<(Provider, String), Project> = HashMap::new();
    for provider in [Provider::Modrinth, Provider::Curseforge] {
        let ids: Vec<String> = ident
            .values()
            .filter(|i| i.provider == provider)
            .map(|i| i.project_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let missing_versions: Vec<String> = ident
            .values()
            .filter(|i| i.provider == provider && !known_versions.contains_key(&(provider, i.version_id.clone())))
            .map(|i| i.version_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() {
            continue;
        }
        let result = match provider {
            Provider::Modrinth => match ctx.modrinth() {
                Ok(m) => {
                    let (p, v) = tokio::join!(m.projects(&ids), m.versions_by_id(&missing_versions));
                    p.and_then(|p| v.map(|v| (p, v)))
                }
                Err(e) => Err(e),
            },
            Provider::Curseforge => match ctx.curseforge() {
                Ok(c) => {
                    let (p, v) = tokio::join!(c.projects(&ids), c.versions_by_id(&missing_versions));
                    p.and_then(|p| v.map(|v| (p, v)))
                }
                Err(e) => Err(e),
            },
        };
        match result {
            Ok((p, v)) => {
                for project in p {
                    projects.insert((provider, project.id.clone()), project);
                }
                for version in v {
                    known_versions.insert((provider, version.id.clone()), version);
                }
            }
            Err(e) => warnings.push(e.to_string()),
        }
    }

    // --- updates
    let mut updates: HashMap<String, Version> = HashMap::new(); // path -> newer version
    let modrinth_paths: Vec<(String, String)> = paths
        .iter()
        .filter(|p| ident.get(*p).is_some_and(|i| i.provider == Provider::Modrinth))
        .filter_map(|p| sha1_of(p).map(|h| (p.clone(), h)))
        .collect();
    if !modrinth_paths.is_empty()
        && let Ok(m) = ctx.modrinth()
    {
        let hashes: Vec<String> = modrinth_paths.iter().map(|(_, h)| h.clone()).collect();
        match m.latest_for(&hashes, loader, game_version).await {
            Ok(latest) => {
                for (path, hash) in &modrinth_paths {
                    if let Some(v) = latest.get(hash)
                        && ident.get(path).is_some_and(|i| i.version_id != v.id)
                    {
                        updates.insert(path.clone(), v.clone());
                    }
                }
            }
            Err(e) => warnings.push(format!("Update check: {e}")),
        }
    }
    let cf_paths: Vec<(String, Identified)> = paths
        .iter()
        .filter_map(|p| ident.get(p).filter(|i| i.provider == Provider::Curseforge).map(|i| (p.clone(), i.clone())))
        .collect();
    if !cf_paths.is_empty()
        && let Ok(c) = ctx.curseforge()
    {
        let ids: Vec<String> = cf_paths.iter().map(|(_, i)| i.project_id.clone()).collect();
        // allow betas / alphas only for mods that already run one
        let any_type: HashMap<String, bool> = cf_paths
            .iter()
            .map(|(_, i)| {
                let pre = known_versions
                    .get(&(Provider::Curseforge, i.version_id.clone()))
                    .is_some_and(|v| v.version_type != "release");
                (i.project_id.clone(), pre)
            })
            .collect();
        match c.latest_for(&ids, loader, game_version, &any_type).await {
            Ok(latest) => {
                let newer: Vec<(String, String)> = cf_paths
                    .iter()
                    .filter_map(|(path, i)| {
                        let file = latest.get(&i.project_id)?;
                        let current: u64 = i.version_id.parse().unwrap_or(0);
                        (file.parse::<u64>().unwrap_or(0) > current).then(|| (path.clone(), file.clone()))
                    })
                    .collect();
                let ids: Vec<String> = newer.iter().map(|(_, f)| f.clone()).collect();
                match c.versions_by_id(&ids).await {
                    Ok(versions) => {
                        for (path, file) in newer {
                            if let Some(v) = versions.iter().find(|v| v.id == file) {
                                updates.insert(path, v.clone());
                            }
                        }
                    }
                    Err(e) => warnings.push(format!("Update check: {e}")),
                }
            }
            Err(e) => warnings.push(format!("Update check: {e}")),
        }
    }

    let mods = entries
        .iter()
        .zip(paths.iter())
        .map(|(e, path)| {
            let id = ident.get(path);
            Installed {
                file: e.name.clone(),
                size: e.size,
                modified: e.modified,
                enabled: !e.name.ends_with(DISABLED_SUFFIX),
                sha1: sha1_of(path),
                provider: id.map(|i| i.provider),
                project: id.and_then(|i| projects.get(&(i.provider, i.project_id.clone())).cloned()),
                version: id.and_then(|i| known_versions.get(&(i.provider, i.version_id.clone())).cloned()),
                update: updates.remove(path),
            }
        })
        .collect();
    warnings.dedup();

    InstalledList {
        directory,
        loader: loader.map(String::from),
        game_version: game_version.map(String::from),
        mods,
        warnings,
        generated_at: now,
    }
}
