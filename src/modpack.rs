//! Modpacks: set a server up from a Modrinth `.mrpack` or a CurseForge modpack.
//!
//! The panel downloads the pack archive (to a temp file), reads its index and overrides, and
//! leaves the heavy lifting to Wings: every mod is pulled straight from its CDN into the
//! server, override files (configs, scripts, …) are written, and downloads are checked
//! against their SHA-1. Client-only mods are left out (pack env flags, CurseForge
//! Client/Server tags, Modrinth `server_side`). The install runs as a background job the
//! page polls; `/modpack/plan` shows what would happen first.

use crate::{
    ctx::{Ctx, display},
    files::ServerFiles,
    installed,
    providers::{self, Provider, Version},
    routes::{Target, fetch_versions, load_version},
    runtime, serverpack,
    staging::{self, PACK_DIR},
};
use axum::extract::Path;
use futures_util::{StreamExt, stream};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shared::{
    GetState,
    models::{
        server::{GetServer, GetServerActivityLogger},
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use tokio::sync::Mutex;
use wings_api::Algorithm;

/// Packs are downloaded and unpacked on the node, so this only bounds its disk use.
pub(crate) const MAX_PACK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OVERRIDE_FILES: usize = 50_000;
/// Hosts a .mrpack may download from (the format's own allow-list).
const MRPACK_HOSTS: [&str; 4] = ["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];
const JOB: &str = "modpack-job";
/// Folders only clients use; never written to a server.
const CLIENT_FOLDERS: [&str; 3] = ["resourcepacks/", "shaderpacks/", "screenshots/"];

fn client_folder(path: &str) -> bool {
    CLIENT_FOLDERS.iter().any(|f| path.starts_with(f))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackFile {
    /// Destination relative to the server root.
    pub path: String,
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: u64,
    pub title: Option<String>,
    /// Where to get it by hand (CurseForge files whose authors block third-party downloads).
    pub page: Option<String>,
    /// provider, project id, version id
    pub ident: Option<(Provider, String, String)>,
}

pub struct Pack {
    pub minecraft: Option<String>,
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    pub files: Vec<PackFile>,
    pub manual: Vec<PackFile>,
    pub skipped: Vec<String>,
    pub rejected: Vec<String>,
    /// Target path -> the unpacked file on the node; later layers (server-overrides)
    /// already applied.
    pub overrides: IndexMap<String, Override>,
    /// Override jars identified by hash: path -> (sha1, ident).
    pub override_idents: HashMap<String, (String, (Provider, String, String))>,
}

/// An override file, unpacked on the node.
#[derive(Debug, Clone)]
pub struct Override {
    /// Path inside the staging folder.
    pub from: String,
    pub size: u64,
}

/// A relative path that stays inside the server folder.
pub(crate) fn safe_path(raw: &str) -> Option<String> {
    let p = raw.replace('\\', "/");
    let p = p.trim_start_matches("./");
    if p.is_empty() || p.starts_with('/') || p.contains('\0') || p.contains(':') {
        return None;
    }
    let parts: Vec<&str> = p.split('/').collect();
    if parts.iter().any(|c| c.is_empty() || *c == "." || *c == "..") {
        return None;
    }
    Some(parts.join("/"))
}

pub(crate) fn split(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("/", path),
    }
}

fn pack_loader(raw: &str) -> Option<(&'static str, Option<String>)> {
    // mrpack: "fabric-loader" -> 0.16.5 (version is the value); CurseForge: "fabric-0.16.5"
    let (name, rest) = raw.split_once('-').unwrap_or((raw, ""));
    let loader = match name {
        "fabric" => "fabric",
        "quilt" => "quilt",
        "forge" => "forge",
        "neoforge" => "neoforge",
        _ => return None,
    };
    let version = (!rest.is_empty() && rest != "loader").then(|| rest.to_string());
    Some((loader, version))
}

/// The override layers of a pack, later ones winning.
fn layers(kind: &str, index: &Value) -> Vec<String> {
    match kind {
        "mrpack" => vec!["overrides/".into(), "server-overrides/".into()],
        _ => vec![format!("{}/", index["overrides"].as_str().unwrap_or("overrides").trim_matches('/'))],
    }
}

/// Override files from the unpacked pack's listing: target path -> file; unsafe paths
/// are rejected, client-only folders left out.
fn overrides_from(
    layers: &[String],
    listing: &[(String, u64)],
) -> Result<(IndexMap<String, Override>, Vec<String>), anyhow::Error> {
    let mut overrides = IndexMap::new();
    let mut rejected = Vec::new();
    for layer in layers {
        for (name, size) in listing {
            let Some(rel) = name.strip_prefix(layer.as_str()) else { continue };
            let Some(path) = safe_path(rel).filter(|p| !staging::is_staging(p)) else {
                rejected.push(name.clone());
                continue;
            };
            if client_folder(&path) {
                continue;
            }
            overrides.insert(path, Override { from: name.clone(), size: *size });
            if overrides.len() > MAX_OVERRIDE_FILES {
                return Err(display(format!("The modpack has over {MAX_OVERRIDE_FILES} override files.")));
            }
        }
    }
    Ok((overrides, rejected))
}

/// Download a pack version and work out exactly what installing it means.
/// The archive of a pack version: (url, sha1).
fn archive(version: &Version) -> Result<(String, Option<String>), anyhow::Error> {
    let file = version.file.as_ref();
    let Some(url) = file.and_then(|f| f.url.clone()) else {
        return Err(display(
            "This modpack version cannot be downloaded through third-party apps. Download it from the provider instead.",
        ));
    };
    if file.is_some_and(|f| f.size > MAX_PACK_BYTES) {
        return Err(display(format!("The modpack archive is larger than {} MiB.", MAX_PACK_BYTES >> 20)));
    }
    Ok((url, file.and_then(|f| f.sha1.clone())))
}

/// Unpack the pack version on the node (reusing what is already there).
async fn stage(ctx: &Ctx, version: &Version) -> Result<Vec<(String, u64)>, anyhow::Error> {
    let (url, sha1) = archive(version)?;
    staging::stage(ctx, PACK_DIR, &url, sha1.as_deref()).await
}

/// Download and unpack a pack version on the node and work out exactly what installing
/// it means. The unpacked files stay in the staging folder.
pub async fn load(ctx: &Ctx, version: &Version) -> Result<Pack, anyhow::Error> {
    let listing = stage(ctx, version).await?;
    let (kind, index) = if let Some(index) = staging::read_json(ctx, PACK_DIR, "modrinth.index.json").await {
        ("mrpack", index)
    } else if let Some(index) = staging::read_json(ctx, PACK_DIR, "manifest.json").await {
        ("curseforge", index)
    } else {
        return Err(display("The modpack has no modrinth.index.json or manifest.json."));
    };
    let (overrides, rejected) = overrides_from(&layers(kind, &index), &listing)?;
    let index = &index;
    let mut pack = Pack {
        minecraft: None,
        loader: None,
        loader_version: None,
        files: vec![],
        manual: vec![],
        skipped: vec![],
        rejected,
        overrides,
        override_idents: HashMap::new(),
    };

    match kind {
        "mrpack" => {
            let deps = index["dependencies"].as_object().cloned().unwrap_or_default();
            pack.minecraft = deps.get("minecraft").and_then(Value::as_str).map(String::from);
            if let Some((key, v)) = deps.iter().find(|(k, _)| pack_loader(k).is_some()) {
                pack.loader = pack_loader(key).map(|(l, _)| l.to_string());
                pack.loader_version = v.as_str().map(String::from);
            }
            for f in index["files"].as_array().cloned().unwrap_or_default() {
                let raw_path = f["path"].as_str().unwrap_or_default();
                let Some(path) = safe_path(raw_path) else {
                    pack.rejected.push(raw_path.to_string());
                    continue;
                };
                if f["env"]["server"].as_str() == Some("unsupported") || client_folder(&path) {
                    pack.skipped.push(split(&path).1.to_string());
                    continue;
                }
                let url = f["downloads"].as_array().and_then(|d| {
                    d.iter().filter_map(Value::as_str).find(|u| {
                        reqwest::Url::parse(u).is_ok_and(|u| {
                            u.scheme() == "https" && u.host_str().is_some_and(|h| MRPACK_HOSTS.contains(&h))
                        })
                    })
                });
                let Some(url) = url else {
                    pack.rejected.push(path);
                    continue;
                };
                pack.files.push(PackFile {
                    path,
                    url: Some(url.to_string()),
                    sha1: f["hashes"]["sha1"].as_str().map(|h| h.to_lowercase()),
                    size: f["fileSize"].as_u64().unwrap_or(0),
                    title: None,
                    page: None,
                    ident: None,
                });
            }
        }
        _ => {
            pack.minecraft = index["minecraft"]["version"].as_str().map(String::from);
            let loaders = index["minecraft"]["modLoaders"].as_array().cloned().unwrap_or_default();
            if let Some((l, v)) = loaders
                .iter()
                .find(|l| l["primary"].as_bool() == Some(true))
                .or(loaders.first())
                .and_then(|l| l["id"].as_str())
                .and_then(pack_loader)
            {
                pack.loader = Some(l.to_string());
                pack.loader_version = v;
            }
            let cf = ctx.curseforge()?;
            let entries: Vec<(u64, u64)> = index["files"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|f| {
                    if f["required"].as_bool() == Some(false) {
                        return None; // optional, shipped switched off
                    }
                    Some((f["projectID"].as_u64()?, f["fileID"].as_u64()?))
                })
                .collect();
            let file_ids: Vec<u64> = entries.iter().map(|(_, f)| *f).collect();
            let project_ids: Vec<String> = entries.iter().map(|(p, _)| p.to_string()).collect();
            let (files, projects) = tokio::join!(cf.files_raw(&file_ids), cf.projects_with_class(&project_ids));
            let (files, projects) = (files?, projects?);
            let datapacks = ctx.detected().await.datapack_directory;
            for (project_id, file_id) in entries {
                let Some(f) = files.iter().find(|f| f["id"].as_u64() == Some(file_id)) else {
                    pack.rejected.push(format!("CurseForge file {file_id}"));
                    continue;
                };
                let (project, class) = match projects.iter().find(|(p, _)| p.id == project_id.to_string()) {
                    Some((p, c)) => (Some(p), *c),
                    None => (None, 6),
                };
                let name = f["fileName"].as_str().unwrap_or_default().to_string();
                let title = project.map(|p| p.title.clone()).unwrap_or_else(|| name.clone());
                let tags: Vec<&str> = f["gameVersions"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let client_only = tags.contains(&"Client") && !tags.contains(&"Server");
                let dir = match class {
                    6 => "mods".to_string(),
                    5 => "plugins".to_string(),
                    6945 => datapacks.clone(),
                    _ => String::new(), // resource packs, shaders, worlds: client side
                };
                if client_only || dir.is_empty() {
                    pack.skipped.push(title);
                    continue;
                }
                let Ok(name) = providers::safe_file_name(&name) else {
                    pack.rejected.push(name);
                    continue;
                };
                let file = PackFile {
                    path: format!("{dir}/{name}"),
                    url: f["downloadUrl"].as_str().filter(|u| u.starts_with("https://")).map(String::from),
                    sha1: f["hashes"]
                        .as_array()
                        .and_then(|h| h.iter().find(|h| h["algo"].as_u64() == Some(1)))
                        .and_then(|h| h["value"].as_str())
                        .map(|h| h.to_lowercase()),
                    size: f["fileLength"].as_u64().unwrap_or(0),
                    page: project.map(|p| format!("{}/files/{file_id}", p.url.trim_end_matches('/'))),
                    title: Some(title),
                    ident: Some((Provider::Curseforge, project_id.to_string(), file_id.to_string())),
                };
                if file.url.is_some() {
                    pack.files.push(file);
                } else {
                    pack.manual.push(file);
                }
            }
        }
    }

    // Modrinth knows which projects do not run on servers; ask it about every jar we have a
    // hash for (override jars are hashed by Wings, where they were unpacked)
    let jar_sources: Vec<String> = pack
        .overrides
        .values()
        .filter(|o| o.from.ends_with(".jar"))
        .map(|o| format!("{PACK_DIR}/{}", o.from))
        .collect();
    let jar_hashes = ctx.files.fingerprints(&jar_sources, Algorithm::Sha1).await;
    let override_jars: Vec<(String, String)> = pack
        .overrides
        .iter()
        .filter_map(|(path, o)| {
            let hash = jar_hashes.get(&format!("{PACK_DIR}/{}", o.from))?;
            Some((path.clone(), hash.to_lowercase()))
        })
        .collect();
    if let Ok(m) = ctx.modrinth() {
        let mut hashes: Vec<String> = pack.files.iter().filter_map(|f| f.sha1.clone()).collect();
        hashes.extend(override_jars.iter().map(|(_, h)| h.clone()));
        let mut found = HashMap::new();
        for chunk in hashes.chunks(200) {
            if let Ok(r) = m.identify(chunk).await {
                found.extend(r);
            }
        }
        let ids: Vec<String> =
            found.values().map(|(i, _)| i.project_id.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        let sides: HashMap<String, (String, String)> = m
            .projects_with_side(&ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(p, side)| (p.id.clone(), (p.title, side)))
            .collect();
        let client_only = |hash: &str| -> Option<String> {
            let (id, _) = found.get(hash)?;
            let (title, side) = sides.get(&id.project_id)?;
            (side == "unsupported").then(|| title.clone())
        };

        let mut kept = Vec::new();
        for mut f in std::mem::take(&mut pack.files) {
            if let Some(hash) = f.sha1.clone() {
                if let Some(title) = client_only(&hash) {
                    pack.skipped.push(title);
                    continue;
                }
                if let Some((id, _)) = found.get(&hash) {
                    f.title = f.title.or_else(|| sides.get(&id.project_id).map(|(t, _)| t.clone()));
                    f.ident = f.ident.or(Some((Provider::Modrinth, id.project_id.clone(), id.version_id.clone())));
                }
            }
            kept.push(f);
        }
        pack.files = kept;
        for (path, hash) in override_jars {
            if let Some(title) = client_only(&hash) {
                pack.overrides.shift_remove(&path);
                pack.skipped.push(title);
            } else if let Some((id, _)) = found.get(&hash) {
                pack.override_idents
                    .insert(path, (hash, (Provider::Modrinth, id.project_id.clone(), id.version_id.clone())));
            }
        }
    }
    pack.skipped.sort();
    pack.skipped.dedup();

    Ok(pack)
}

// ------------------------------------------------------------------ plan

#[derive(Deserialize)]
pub struct PackRequest {
    provider: String,
    project_id: String,
    version_id: String,
    #[serde(default)]
    clean: bool,
    /// Also switch the server to the pack's Minecraft version and loader.
    #[serde(default)]
    switch_version: bool,
    /// Reinstall the server and delete every file before installing the pack.
    #[serde(default)]
    wipe: bool,
    /// Install the pack's server pack instead, when the provider has one (CurseForge).
    #[serde(default = "yes")]
    server_pack: bool,
}

fn yes() -> bool {
    true
}

async fn resolve(ctx: &Ctx, data: &PackRequest) -> Result<(Provider, Version), anyhow::Error> {
    let provider = Provider::parse(&data.provider)?;
    let project_id = providers::id(&data.project_id)?;
    let version_id = providers::id(&data.version_id)?;
    ctx.check_provider(provider)?;
    let version = load_version(ctx, provider, &project_id, &version_id, None).await?;
    Ok((provider, version))
}

/// Requirement mismatches between the pack and the server, as warnings.
fn mismatches(pack: &Pack, loader: Option<&str>, game_version: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(want), Some(have)) = (pack.minecraft.as_deref(), game_version)
        && want != have
    {
        out.push(format!("The pack is made for Minecraft {want}; this server runs {have}."));
    }
    if let Some(want) = pack.loader.as_deref() {
        match loader {
            Some(have) if have != want && !(want == "fabric" && have == "quilt") => out.push(format!(
                "The pack needs {want}{}; this server runs {have}.",
                pack.loader_version.as_deref().map(|v| format!(" {v}")).unwrap_or_default()
            )),
            None => out.push(format!(
                "The pack needs {want}{}; could not tell which loader this server runs.",
                pack.loader_version.as_deref().map(|v| format!(" {v}")).unwrap_or_default()
            )),
            _ => {}
        }
    }
    out
}

pub async fn plan(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    shared::Payload(data): shared::Payload<PackRequest>,
) -> ApiResponseResult {
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    // the install job uses the same staging folder on the node
    if running(&ctx).await {
        return Err(display("A modpack install is running on this server.").into());
    }
    let (provider, version) = resolve(&ctx, &data).await?;
    let pack = load(&ctx, &version).await;
    // the preview unpacked the pack on the node only to read it
    staging::clear(&ctx.files, PACK_DIR).await;
    let pack = pack?;
    let detected = ctx.detected().await;
    let existing = ctx
        .files
        .list("mods")
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .iter()
        .filter(|e| installed::is_content_file(e, None))
        .count();

    ApiResponse::new_serialized(json!({
        "provider": provider,
        "version": version,
        "requires": { "minecraft": pack.minecraft, "loader": pack.loader, "loader_version": pack.loader_version },
        "server": { "loader": detected.loader, "game_version": detected.game_version },
        "warnings": mismatches(&pack, detected.loader.as_deref(), detected.game_version.as_deref()),
        "server_pack": serverpack::find(&ctx, &version).await.ok().flatten(),
        "switch_version": match runtime::route(&ctx, &pack).await {
            Ok(route) => json!({ "supported": true, "reason": null, "how": route.describe(&ctx) }),
            Err(reason) => json!({ "supported": false, "reason": reason, "how": null }),
        },
        "downloads": pack.files.iter().map(|f| json!({ "path": f.path, "title": f.title, "size": f.size })).collect::<Vec<_>>(),
        "download_bytes": pack.files.iter().map(|f| f.size).sum::<u64>(),
        "manual": pack.manual.iter().map(|f| json!({ "path": f.path, "title": f.title, "page": f.page })).collect::<Vec<_>>(),
        "skipped": pack.skipped,
        "rejected": pack.rejected,
        "overrides": pack.overrides.len(),
        "override_bytes": pack.overrides.values().map(|o| o.size).sum::<u64>(),
        "existing_mods": existing,
    }))
    .ok()
}

// ------------------------------------------------------------------ install job

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Job {
    /// running / done / failed
    pub state: String,
    pub title: String,
    pub version: String,
    pub step: String,
    pub total: usize,
    pub done: usize,
    pub installed: usize,
    pub written: usize,
    pub removed: usize,
    pub failed: Vec<String>,
    pub manual: Vec<Value>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    /// What switching the Minecraft version changed.
    #[serde(default)]
    pub switched: Vec<String>,
    pub started: i64,
    pub finished: Option<i64>,
}

/// Whether an install job is running on this server (one stuck for an hour does not count).
async fn running(ctx: &Ctx) -> bool {
    ctx.peek::<Job>(JOB)
        .await
        .is_some_and(|job| job.state == "running" && chrono::Utc::now().timestamp() - job.started < 3600)
}

async fn save(ctx: &Ctx, job: &Job) {
    ctx.store(JOB, 6 * 3600, job).await;
}

pub async fn install(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    logger: GetServerActivityLogger,
    Path(_server): Path<String>,
    shared::Payload(data): shared::Payload<PackRequest>,
) -> ApiResponseResult {
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    if data.switch_version {
        runtime::check_permissions(&permissions)?;
    }
    if data.wipe {
        runtime::check_wipe_permissions(&permissions)?;
    }
    if running(&ctx).await {
        return Err(display("A modpack install is already running on this server.").into());
    }
    let (provider, version) = resolve(&ctx, &data).await?;
    let title = match provider {
        Provider::Modrinth => ctx.modrinth()?.projects(std::slice::from_ref(&version.project_id)).await?,
        Provider::Curseforge => ctx.curseforge()?.projects(std::slice::from_ref(&version.project_id)).await?,
    }
    .pop()
    .map(|p| p.title)
    .unwrap_or_else(|| version.name.clone());

    let job = Job {
        state: "running".into(),
        title: title.clone(),
        version: version.version_number.clone(),
        step: "Downloading the modpack".into(),
        started: chrono::Utc::now().timestamp(),
        ..Default::default()
    };
    save(&ctx, &job).await;
    logger
        .log(
            "server:modmanager.modpack",
            json!({
                "provider": provider, "project": title, "project_id": version.project_id,
                "version": version.version_number, "version_id": version.id, "clean": data.clean,
                "switch_version": data.switch_version, "wipe": data.wipe, "server_pack": data.server_pack,
            }),
        )
        .await;

    let options = RunOptions { clean: data.clean, switch_version: data.switch_version, wipe: data.wipe, server_pack: data.server_pack };
    tokio::spawn(async move {
        let job = Arc::new(Mutex::new(job));
        if let Err(e) = run(&ctx, &version, &title, options, job.clone()).await {
            let mut j = job.lock().await;
            j.state = "failed".into();
            j.step = e.to_string();
            j.finished = Some(chrono::Utc::now().timestamp());
            save(&ctx, &j).await;
        }
        // the unpacked archives are not needed any more, whether or not this worked
        staging::clear(&ctx.files, PACK_DIR).await;
        staging::clear(&ctx.files, staging::SERVER_PACK_DIR).await;
        ctx.forget("installed").await;
        ctx.forget("detected").await;
    });

    ApiResponse::new_serialized(json!({ "started": true })).ok()
}

pub(crate) async fn step(ctx: &Ctx, job: &Mutex<Job>, f: impl FnOnce(&mut Job)) {
    let mut j = job.lock().await;
    f(&mut j);
    save(ctx, &j).await;
}

/// Every directory (and parent) the files go into, parents first.
fn directories<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for p in paths {
        let mut acc = String::new();
        let parts: Vec<&str> = p.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            acc = if acc.is_empty() { part.to_string() } else { format!("{acc}/{part}") };
            dirs.insert(acc.clone());
        }
    }
    let mut dirs: Vec<String> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| d.matches('/').count());
    dirs
}

async fn make_directories(files: &ServerFiles, dirs: &[String]) {
    // one level at a time, so parents exist before their children
    let max_depth = dirs.iter().map(|d| d.matches('/').count()).max().unwrap_or(0);
    for depth in 0..=max_depth {
        let level: Vec<&String> = dirs.iter().filter(|d| d.matches('/').count() == depth).collect();
        stream::iter(level)
            .for_each_concurrent(8, |d| async move {
                let (root, name) = split(d);
                let _ = files
                    .client
                    .post_servers_server_files_create_directory(
                        files.server,
                        &wings_api::servers_server_files_create_directory::post::RequestBody {
                            root: root.into(),
                            name: name.into(),
                        },
                    )
                    .await;
            })
            .await;
    }
}

struct RunOptions {
    clean: bool,
    switch_version: bool,
    wipe: bool,
    server_pack: bool,
}

async fn run(ctx: &Ctx, version: &Version, title: &str, options: RunOptions, job: Arc<Mutex<Job>>) -> Result<(), anyhow::Error> {
    let RunOptions { clean, switch_version, wipe, server_pack } = options;
    // download and read the pack before anything on the server changes; the client pack
    // still says which Minecraft and loader the server pack needs
    let pack = load(ctx, version).await?;
    let mut notes = Vec::new();
    let server_pack = match server_pack {
        true => match serverpack::find(ctx, version).await? {
            Some(sp) if sp.url.is_some() => Some(sp),
            Some(sp) => {
                notes.push(format!(
                    "The server pack {} cannot be downloaded through third-party apps; installed the regular pack instead.",
                    sp.name
                ));
                None
            }
            None => None,
        },
        false => None,
    };
    let route = match switch_version {
        true => Some(runtime::route(ctx, &pack).await.map_err(display)?),
        false => None,
    };
    let mut switched = Vec::new();
    if let Some(sp) = &server_pack {
        switched.push(format!("Using the server pack {}", sp.name));
    }
    // the egg route wipes as part of its own reinstall
    if wipe && !matches!(route, Some(runtime::Route::Egg { .. })) {
        step(ctx, &job, |j| j.step = "Reinstalling the server and deleting all its files (can take a few minutes)".into())
            .await;
        runtime::reinstall(ctx, &ctx.server, true).await?;
        switched.push(format!("Reinstalled the server with the {} egg (all files deleted first)", ctx.server.egg.name));
    }
    // before the mods, so a failed switch leaves them untouched
    if let Some(route) = &route {
        let minecraft = pack.minecraft.as_deref().unwrap_or_default();
        step(ctx, &job, |j| {
            j.step = match route {
                runtime::Route::Jar => format!("Installing Minecraft {minecraft}"),
                runtime::Route::Egg { name, .. } => {
                    format!("Reinstalling the server with the {name} egg for Minecraft {minecraft} (can take a few minutes)")
                }
            }
        })
        .await;
        switched.extend(runtime::apply(ctx, &pack, route, wipe).await?);
    }
    step(ctx, &job, |j| {
        j.warnings = notes;
        if server_pack.is_none() {
            j.total = pack.files.len() + pack.overrides.len();
            j.skipped = pack.skipped.clone();
            j.manual = pack.manual.iter().map(|f| json!({ "path": f.path, "title": f.title, "page": f.page })).collect();
            j.warnings.extend(pack.rejected.iter().map(|r| format!("Left out (unsafe path or download host): {r}")));
        }
        j.switched = switched;
        j.step = "Preparing folders".into();
    })
    .await;

    // nothing left to clean after a wipe
    if clean && !wipe {
        let old: Vec<String> = ctx
            .files
            .list("mods")
            .await?
            .unwrap_or_default()
            .iter()
            .filter(|e| installed::is_content_file(e, None))
            .map(|e| e.name.clone())
            .collect();
        if !old.is_empty() {
            ctx.files.delete("mods", &old).await?;
        }
        step(ctx, &job, |j| j.removed = old.len()).await;
    }

    let mut mismatched = Vec::new();
    if let Some(sp) = &server_pack {
        // the author's ready server: unpacked on the node, no per-mod downloads
        serverpack::install(ctx, sp, &job).await?;
    } else {
        // override files get their folders when they are moved into place
        let dirs = directories(pack.files.iter().map(|f| f.path.as_str()));
        make_directories(&ctx.files, &dirs).await;

        // mods: Wings downloads them straight from the CDN; stay under its per-server
        // concurrent-pull limit (3 by default) so other pulls still have room
        step(ctx, &job, |j| j.step = format!("Downloading {} files", pack.files.len())).await;
        stream::iter(pack.files.iter())
            .for_each_concurrent(2, |f| {
                let job = job.clone();
                async move {
                    let (dir, name) = split(&f.path);
                    let result = ctx.files.pull(dir, name, f.url.as_deref().unwrap_or_default()).await;
                    step(ctx, &job, |j| {
                        j.done += 1;
                        match result {
                            Ok(()) => j.installed += 1,
                            Err(e) => j.failed.push(format!("{}: {e}", f.path)),
                        }
                    })
                    .await;
                }
            })
            .await;

        // configs and everything else from the overrides
        step(ctx, &job, |j| j.step = format!("Moving {} override files into place", pack.overrides.len())).await;
        // unpacked on the node: moved into place there, nothing goes through the panel.
        // A reinstall above may have deleted the unpacked pack; this unpacks it again then.
        let total = pack.overrides.len();
        if total > 0 {
            stage(ctx, version).await?;
            let entries: Vec<(String, String)> =
                pack.overrides.iter().map(|(path, o)| (o.from.clone(), path.clone())).collect();
            let mut done = 0;
            let moved = staging::move_into_place(ctx, PACK_DIR, &entries, |n| done += n).await?;
            step(ctx, &job, |j| {
                j.done += done;
                j.written = moved;
                if moved < total {
                    j.failed.push(format!("{} of {total} override files could not be moved into place", total - moved));
                }
            })
            .await;
        }

        // check what Wings downloaded, and remember where each jar came from
        step(ctx, &job, |j| j.step = "Verifying downloads".into()).await;
        let paths: Vec<String> = pack.files.iter().map(|f| f.path.clone()).collect();
        let hashes = ctx.files.fingerprints(&paths, Algorithm::Sha1).await;
        for f in &pack.files {
            let Some(got) = hashes.get(&f.path).map(|h| h.to_lowercase()) else { continue };
            if let Some(want) = &f.sha1
                && *want != got
            {
                mismatched.push(f.path.clone());
                continue;
            }
            if let Some((provider, project, version)) = &f.ident {
                installed::record_ids(ctx, &got, *provider, project, version).await;
            }
        }
        for (sha1, (provider, project, version)) in pack.override_idents.values() {
            installed::record_ids(ctx, sha1, *provider, project, version).await;
        }
        if !mismatched.is_empty() {
            let _ = ctx.files.delete("/", &mismatched).await;
        }
    }

    if let Err(e) = sqlx::query(
        "INSERT INTO dev_emrekmek_modmanager_modpacks (server_uuid, provider, project_id, version_id, title, version_number, installed)
        VALUES ($1, $2, $3, $4, $5, $6, now())
        ON CONFLICT (server_uuid) DO UPDATE SET provider = EXCLUDED.provider, project_id = EXCLUDED.project_id,
            version_id = EXCLUDED.version_id, title = EXCLUDED.title, version_number = EXCLUDED.version_number,
            installed = EXCLUDED.installed",
    )
    .bind(ctx.server.uuid)
    .bind(version.provider.as_str())
    .bind(&version.project_id)
    .bind(&version.id)
    .bind(title)
    .bind(&version.version_number)
    .execute(ctx.state.database.write())
    .await
    {
        tracing::warn!("modmanager: could not record the modpack: {e:?}");
    }

    let warnings = if switch_version { Vec::new() } else { mismatches(&pack, ctx.detected().await.loader.as_deref(), None) };
    step(ctx, &job, |j| {
        j.failed.extend(mismatched.iter().map(|p| format!("{p}: checksum mismatch, deleted")));
        j.warnings.extend(warnings);
        j.state = "done".into();
        j.step = "Done".into();
        j.finished = Some(chrono::Utc::now().timestamp());
    })
    .await;

    Ok(())
}

// ------------------------------------------------------------------ status

pub async fn job(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    ApiResponse::new_serialized(json!({ "job": ctx.peek::<Job>(JOB).await })).ok()
}

/// Forget which modpack this server was set up from (files are left alone), e.g. after
/// the server was wiped. Also clears the last install job.
pub async fn forget(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    logger: GetServerActivityLogger,
    Path(_server): Path<String>,
) -> ApiResponseResult {
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    if ctx.peek::<Job>(JOB).await.is_some_and(|j| j.state == "running") {
        return Err(display("A modpack install is running on this server.").into());
    }
    let removed: Option<String> =
        sqlx::query_scalar("DELETE FROM dev_emrekmek_modmanager_modpacks WHERE server_uuid = $1 RETURNING title")
            .bind(ctx.server.uuid)
            .fetch_optional(ctx.state.database.write())
            .await?;
    ctx.forget(JOB).await;
    staging::clear(&ctx.files, PACK_DIR).await;
    staging::clear(&ctx.files, staging::SERVER_PACK_DIR).await;
    if let Some(title) = &removed {
        logger.log("server:modmanager.modpack-forget", json!({ "project": title })).await;
    }
    ApiResponse::new_serialized(json!({ "removed": removed.is_some() })).ok()
}

/// The modpack this server was set up from, with the newest compatible version.
pub async fn current(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    axum::extract::Query(target): axum::extract::Query<Target>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let row: Option<(String, String, String, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT provider, project_id, version_id, title, version_number, installed
        FROM dev_emrekmek_modmanager_modpacks WHERE server_uuid = $1",
    )
    .bind(ctx.server.uuid)
    .fetch_optional(ctx.state.database.read())
    .await?;
    let Some((provider, project_id, version_id, title, version_number, installed)) = row else {
        return ApiResponse::new_serialized(json!({ "modpack": null })).ok();
    };
    let provider = Provider::parse(&provider)?;
    let t = target.resolve(&ctx).await?;
    let (project, latest, installed_version) = match ctx.check_provider(provider) {
        Ok(()) => {
            let project = match provider {
                Provider::Modrinth => ctx.modrinth()?.projects(std::slice::from_ref(&project_id)).await,
                Provider::Curseforge => ctx.curseforge()?.projects(std::slice::from_ref(&project_id)).await,
            }
            .ok()
            .and_then(|mut p| p.pop());
            let latest = fetch_versions(&ctx, provider, &project_id, t.loader, t.game_version.as_deref())
                .await
                .ok()
                .and_then(providers::pick_best)
                .filter(|v| v.id != version_id);
            let installed_version = load_version(&ctx, provider, &project_id, &version_id, None).await.ok();
            (project, latest, installed_version)
        }
        Err(_) => (None, None, None),
    };

    ApiResponse::new_serialized(json!({
        "modpack": {
            "provider": provider, "project_id": project_id, "version_id": version_id, "title": title,
            "version_number": version_number, "installed": installed.to_rfc3339(), "project": project, "update": latest,
            "installed_version": installed_version,
        }
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(safe_path("config/sodium.json").as_deref(), Some("config/sodium.json"));
        assert_eq!(safe_path("./mods/a.jar").as_deref(), Some("mods/a.jar"));
        assert_eq!(safe_path("config\\x.toml").as_deref(), Some("config/x.toml"));
        assert!(safe_path("../etc/passwd").is_none());
        assert!(safe_path("/etc/passwd").is_none());
        assert!(safe_path("config/../../x").is_none());
        assert!(safe_path("C:/x").is_none());
        assert!(safe_path("a//b").is_none());
    }

    #[test]
    fn loaders() {
        assert_eq!(pack_loader("fabric-loader"), Some(("fabric", None)));
        assert_eq!(pack_loader("forge-47.2.0"), Some(("forge", Some("47.2.0".into()))));
        assert_eq!(pack_loader("neoforge"), Some(("neoforge", None)));
        assert_eq!(pack_loader("minecraft"), None);
    }

    #[test]
    fn dirs_parents_first() {
        let d = directories(["config/a/b.toml", "mods/x.jar", "server.properties", "config/c.json"].into_iter());
        assert_eq!(d, vec!["config", "mods", "config/a"]);
    }

    #[test]
    fn layers_overrides() {
        let listing: Vec<(String, u64)> = [
            ("modrinth.index.json", 10),
            ("overrides/config/a.toml", 3),
            ("server-overrides/config/a.toml", 4),
            ("overrides/resourcepacks/big.zip", 1),
            ("client-overrides/options.txt", 1),
            ("overrides/.modmanager-pack/x", 1),
        ]
        .iter()
        .map(|(n, s)| (n.to_string(), *s))
        .collect();
        let index = json!({});
        let (o, rejected) = overrides_from(&layers("mrpack", &index), &listing).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o["config/a.toml"].from, "server-overrides/config/a.toml");
        assert_eq!(o["config/a.toml"].size, 4);
        assert_eq!(rejected, vec!["overrides/.modmanager-pack/x"]);

        let cf = json!({ "overrides": "/extra/" });
        assert_eq!(layers("curseforge", &cf), vec!["extra/"]);
        assert_eq!(layers("curseforge", &json!({})), vec!["overrides/"]);
    }
}
