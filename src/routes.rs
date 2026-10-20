//! Client API behind the Mod Manager page, mounted at
//! /api/client/servers/{server}/mod-manager.

use crate::{
    ctx::{self, Ctx, display},
    installed::{self, DISABLED_SUFFIX, Installed, InstalledList},
    providers::{self, Project, Provider, SearchQuery, Version, curseforge, modrinth::Modrinth},
};
use axum::{
    extract::{Path, Query},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use shared::{
    GetState, State,
    models::{
        server::{GetServer, GetServerActivityLogger, ServerActivityLogger},
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::collections::VecDeque;
use utoipa_axum::router::OpenApiRouter;

type ServerPath = Path<String>;
type ProjectPath = Path<(String, String, String)>;

/// Installs one request may do, dependencies included.
const MAX_INSTALLS: usize = 25;
const INSTALLED_TTL: u64 = 120;

fn ok(v: Value) -> ApiResponseResult {
    ApiResponse::new_serialized(v).ok()
}

async fn log(logger: &ServerActivityLogger, event: &str, data: Value) {
    logger.log(format!("server:modmanager.{event}"), data).await;
}

/// Loader / Minecraft version as the page asks for them, falling back to what was detected.
#[derive(Deserialize, Default)]
pub(crate) struct Target {
    /// mods (mods or plugins, by loader) / datapack
    pub kind: Option<String>,
    pub loader: Option<String>,
    pub game_version: Option<String>,
}

pub(crate) struct Resolved {
    pub loader: Option<&'static str>,
    pub game_version: Option<String>,
    pub directory: Option<String>,
}

impl Target {
    pub async fn resolve(&self, ctx: &Ctx) -> Result<Resolved, anyhow::Error> {
        let detected = ctx.detected().await;
        let datapacks = self.kind.as_deref() == Some(providers::DATAPACK);
        let loader = match self.loader.as_deref().filter(|l| !l.is_empty()) {
            Some("any") => None,
            Some(l) => Some(providers::loader(l)?),
            None => detected.loader.as_deref().and_then(|l| providers::loader(l).ok()),
        };
        let game_version = match self.game_version.as_deref() {
            Some("") | Some("any") => None,
            Some(v) => Some(providers::game_version(v)?),
            None => detected.game_version.clone(),
        };
        let (loader, directory) = if datapacks {
            (Some(providers::DATAPACK), Some(detected.datapack_directory.clone()))
        } else {
            let directory = loader
                .and_then(providers::directory_for)
                .map(String::from)
                .or(detected.directory);
            (loader, directory)
        };

        Ok(Resolved {
            loader,
            game_version,
            directory,
        })
    }
}

// ------------------------------------------------------------------ reads

async fn settings(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): ServerPath,
) -> ApiResponseResult {
    let s = crate::settings::load(&state.0).await?;

    ok(json!({
        "access": s.access,
        "restricted": !s.allows(&user),
        "egg_allowed": s.egg_allowed(server.egg.uuid),
        "can_view": ctx::can_view(&permissions),
        "can_manage": ctx::can_manage(&permissions),
        "providers": s.providers(),
        "curseforge_disabled_reason": if s.curseforge_key().is_some() { Value::Null } else if !s.curseforge_enabled {
            json!("CurseForge is disabled on this panel.")
        } else {
            json!("CurseForge needs an API key: an administrator can add one under Admin > Extensions > Mod Manager.")
        },
    }))
}

/// Detected loader / version / folder, plus the lists for the filter dropdowns.
async fn context(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): ServerPath,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let (detected, game_versions) = tokio::join!(
        ctx.detected(),
        ctx.cached("game-versions", 6 * 3600, false, async {
            Modrinth { client: &ctx.state.client, user_agent: &ctx.user_agent }.game_versions().await
        }),
    );
    let mut game_versions = game_versions.unwrap_or_default();
    if let Some(v) = &detected.game_version
        && !game_versions.contains(v)
    {
        game_versions.insert(0, v.clone());
    }

    ok(json!({
        "detected": detected,
        "loaders": providers::LOADERS,
        "game_versions": game_versions,
    }))
}

#[derive(Deserialize)]
struct SearchParams {
    provider: String,
    kind: Option<String>,
    server_only: Option<String>,
    query: Option<String>,
    loader: Option<String>,
    game_version: Option<String>,
    sort: Option<String>,
    page: Option<u64>,
    per_page: Option<u64>,
}

async fn search(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): ServerPath,
    Query(p): Query<SearchParams>,
) -> ApiResponseResult {
    let provider = Provider::parse(&p.provider)?;
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    ctx.check_provider(provider)?;
    let kind = p.kind.clone();
    let target = Target {
        kind: p.kind,
        loader: p.loader,
        game_version: p.game_version,
    }
    .resolve(&ctx)
    .await?;
    let per_page = p.per_page.unwrap_or(24).clamp(6, 50);
    let page = p.page.unwrap_or(1).max(1);
    let query = SearchQuery {
        query: p.query.unwrap_or_default().chars().take(100).collect(),
        loader: target.loader,
        game_version: target.game_version.clone(),
        modpacks: kind.as_deref() == Some("modpack"),
        server_only: !matches!(p.server_only.as_deref(), Some("false" | "0")),
        sort: p.sort.unwrap_or_else(|| "relevance".into()),
        offset: ((page - 1) * per_page).min(curseforge::MAX_RESULTS),
        limit: per_page,
    };
    let key = format!(
        "search:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        provider.as_str(),
        if query.server_only { "server" } else { "all" },
        if query.modpacks { "modpack" } else { "content" },
        query.loader.unwrap_or("any"),
        query.game_version.as_deref().unwrap_or("any"),
        query.sort,
        query.offset,
        query.limit,
        query.query.to_lowercase(),
    );
    let result = ctx
        .cached(&key, 600, false, async {
            match provider {
                Provider::Modrinth => ctx.modrinth()?.search(&query).await,
                Provider::Curseforge => ctx.curseforge()?.search(&query).await,
            }
        })
        .await?;

    ok(json!({
        "page": result,
        "loader": target.loader,
        "game_version": target.game_version,
    }))
}

async fn project(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path((_server, provider, id)): ProjectPath,
) -> ApiResponseResult {
    let provider = Provider::parse(&provider)?;
    let id = providers::id(&id)?;
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let detail = ctx
        .cached(&format!("project:{}:{id}", provider.as_str()), 600, false, async {
            match provider {
                Provider::Modrinth => ctx.modrinth()?.project(&id).await,
                Provider::Curseforge => ctx.curseforge()?.project(&id).await,
            }
        })
        .await?;

    ok(json!({ "project": detail }))
}

pub(crate) async fn fetch_versions(
    ctx: &Ctx,
    provider: Provider,
    project: &str,
    loader: Option<&str>,
    game_version: Option<&str>,
) -> Result<Vec<Version>, anyhow::Error> {
    let key = format!(
        "versions:{}:{project}:{}:{}",
        provider.as_str(),
        loader.unwrap_or("any"),
        game_version.unwrap_or("any")
    );
    ctx.cached(&key, 300, false, async {
        match provider {
            Provider::Modrinth => ctx.modrinth()?.versions(project, loader, game_version).await,
            Provider::Curseforge => ctx.curseforge()?.versions(project, loader, game_version).await,
        }
    })
    .await
}

async fn versions(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path((_server, provider, id)): ProjectPath,
    Query(target): Query<Target>,
) -> ApiResponseResult {
    let provider = Provider::parse(&provider)?;
    let id = providers::id(&id)?;
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let t = target.resolve(&ctx).await?;
    let versions = fetch_versions(&ctx, provider, &id, t.loader, t.game_version.as_deref()).await?;

    ok(json!({ "versions": versions, "loader": t.loader, "game_version": t.game_version }))
}

#[derive(Deserialize, Default)]
struct InstalledParams {
    #[serde(flatten)]
    target: Target,
    fresh: Option<String>,
}

pub(crate) async fn installed_list(ctx: &Ctx, t: &Resolved, fresh: bool) -> InstalledList {
    if !fresh
        && let Some(cached) = ctx.peek::<InstalledList>("installed").await
        && cached.directory == t.directory
        && cached.loader.as_deref() == t.loader
        && cached.game_version == t.game_version
    {
        return cached;
    }
    let list = installed::scan(ctx, t.directory.clone(), t.loader, t.game_version.as_deref()).await;
    ctx.store("installed", INSTALLED_TTL, &list).await;

    list
}

async fn installed(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): ServerPath,
    Query(p): Query<InstalledParams>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let fresh = matches!(p.fresh.as_deref(), Some("1" | "true"));
    if fresh {
        ctx.forget("detected").await;
    }
    let t = p.target.resolve(&ctx).await?;
    let list = installed_list(&ctx, &t, fresh).await;

    ok(serde_json::to_value(list)?)
}

// ------------------------------------------------------------------ changes

#[derive(Deserialize)]
struct InstallPayload {
    provider: String,
    project_id: String,
    /// None = newest compatible version.
    version_id: Option<String>,
    /// File in the folder this replaces (update / downgrade); found automatically when
    /// the same project is already installed.
    replace: Option<String>,
    #[serde(default = "yes")]
    dependencies: bool,
    #[serde(flatten)]
    target: Target,
}

fn yes() -> bool {
    true
}

struct Job {
    provider: Provider,
    project_id: String,
    version_id: Option<String>,
    replace: Option<String>,
    /// Title of the mod that needed this one.
    required_by: Option<String>,
}

/// A slug-ish key for matching the same mod across providers and file names.
fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Is this project (from any provider) already in the folder?
pub(crate) fn already_installed<'a>(mods: &'a [Installed], provider: Provider, project: &Project) -> Option<&'a Installed> {
    let slug = norm(&project.slug);
    let title = norm(&project.title);
    mods.iter().find(|m| {
        if let Some(p) = &m.project {
            if m.provider == Some(provider) && p.id == project.id {
                return true;
            }
            if norm(&p.slug) == slug || norm(&p.title) == title {
                return true;
            }
        }
        m.project.is_none() && file_names_project(&m.file, &slug)
    })
}

/// Does an unidentified jar's name look like this project? "fabric-api-0.161.0+26.2.jar"
/// is fabric-api, "lithium-fabric-0.25.2.jar" is lithium; "sodium-extra-…" is not sodium.
fn file_names_project(file: &str, slug: &str) -> bool {
    if slug.len() < 4 {
        return false;
    }
    let lower = file.to_lowercase();
    let is_version = |w: &str| {
        let digits = w.strip_prefix("mc").or_else(|| w.strip_prefix('v')).unwrap_or(w);
        digits.starts_with(|c: char| c.is_ascii_digit())
    };
    // the name words before the first version-ish word
    let words: Vec<&str> = lower
        .split(['-', '_', '+', ' ', '.'])
        .take_while(|w| !w.is_empty() && !is_version(w))
        .collect();
    let all: String = words.concat();
    let without_loaders: String = words.iter().filter(|w| !providers::LOADERS.contains(*w)).copied().collect();
    all == slug || without_loaders == slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_names() {
        assert!(file_names_project("fabric-api-0.161.0+26.2.jar", &norm("fabric-api")));
        assert!(file_names_project("lithium-fabric-0.25.2.jar", &norm("lithium")));
        assert!(file_names_project("c2me-fabric-mc26.2-0.4.2.jar", &norm("c2me")));
        assert!(!file_names_project("abc-1.0.jar", &norm("abc"))); // slug too short to trust
        assert!(file_names_project("ferritecore-9.0.0-fabric.jar", &norm("ferrite-core")));
        assert!(!file_names_project("sodium-extra-0.6.jar", &norm("sodium")));
        assert!(!file_names_project("sodium-fabric-0.9.1.jar", &norm("sodium-extra")));
    }
}

pub(crate) async fn load_version(
    ctx: &Ctx,
    provider: Provider,
    project: &str,
    version: &str,
    loader: Option<&str>,
) -> Result<Version, anyhow::Error> {
    let v = match provider {
        Provider::Modrinth => ctx.modrinth()?.version(version, loader).await?,
        Provider::Curseforge => ctx.curseforge()?.version(project, version).await?,
    };
    if v.project_id != project {
        return Err(display("That version belongs to a different project."));
    }
    Ok(v)
}

async fn load_project(ctx: &Ctx, provider: Provider, id: &str) -> Result<Project, anyhow::Error> {
    let mut found = match provider {
        Provider::Modrinth => ctx.modrinth()?.projects(&[id.to_string()]).await?,
        Provider::Curseforge => ctx.curseforge()?.projects(&[id.to_string()]).await?,
    };
    found.pop().ok_or_else(|| display("The project does not exist."))
}

async fn install(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    logger: GetServerActivityLogger,
    Path(_server): ServerPath,
    shared::Payload(data): shared::Payload<InstallPayload>,
) -> ApiResponseResult {
    let provider = Provider::parse(&data.provider)?;
    let project_id = providers::id(&data.project_id)?;
    let version_id = data.version_id.as_deref().map(providers::id).transpose()?;
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    ctx.check_provider(provider)?;
    let t = data.target.resolve(&ctx).await?;
    let Some(dir) = t.directory.clone() else {
        return Err(display(
            "Could not tell whether this server uses mods or plugins. Pick a loader in the filters first.",
        )
        .into());
    };
    if ctx.files.list(&dir).await?.is_none() {
        ctx.files.create_directory(&dir).await?;
    }
    let mut mods = installed_list(&ctx, &t, true).await.mods;

    let mut queue = VecDeque::from([Job {
        provider,
        project_id,
        version_id,
        replace: data.replace.clone(),
        required_by: None,
    }]);
    let mut done: Vec<Value> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut first = true;

    while let Some(job) = queue.pop_front() {
        if done.len() >= MAX_INSTALLS {
            warnings.push(format!("Stopped after {MAX_INSTALLS} installs; install the remaining dependencies by hand."));
            break;
        }
        let result = install_one(&ctx, &t, &dir, &mut mods, &job, first).await;
        let is_root = first;
        first = false;
        match result {
            Ok(Some((project, version, file, replaced))) => {
                log(
                    &logger,
                    if replaced.is_some() { "update" } else { "install" },
                    json!({
                        "provider": version.provider, "project": project.title, "project_id": project.id,
                        "version": version.version_number, "version_id": version.id,
                        "file": format!("{dir}/{file}"), "replaced": replaced, "required_by": job.required_by,
                    }),
                )
                .await;
                if data.dependencies {
                    for dep in version.dependencies.iter().filter(|d| d.kind == "required") {
                        let (dep_project, dep_version) = match (&dep.project_id, &dep.version_id) {
                            (Some(p), v) => (p.clone(), v.clone()),
                            // Modrinth may pin only a version
                            (None, Some(v)) if version.provider == Provider::Modrinth => {
                                match ctx.modrinth()?.version(v, t.loader).await {
                                    Ok(dv) => (dv.project_id, Some(v.clone())),
                                    Err(e) => {
                                        warnings.push(format!("A dependency of {}: {e}", project.title));
                                        continue;
                                    }
                                }
                            }
                            _ => continue,
                        };
                        if queue.iter().any(|j| j.provider == version.provider && j.project_id == dep_project) {
                            continue;
                        }
                        queue.push_back(Job {
                            provider: version.provider,
                            project_id: dep_project,
                            version_id: dep_version,
                            replace: None,
                            required_by: Some(project.title.clone()),
                        });
                    }
                }
                done.push(json!({
                    "title": project.title, "file": file, "version": version.version_number,
                    "replaced": replaced, "required_by": job.required_by,
                }));
            }
            Ok(None) => {}
            Err(e) if is_root => {
                ctx.forget("installed").await;
                return Err(e.into());
            }
            Err(e) => warnings.push(format!(
                "Dependency of {}: {e}",
                job.required_by.as_deref().unwrap_or("the mod")
            )),
        }
    }
    ctx.forget("installed").await;

    ok(json!({ "installed": done, "warnings": warnings }))
}

/// Install (or replace) one project. `Ok(None)` = a dependency that is already there.
async fn install_one(
    ctx: &Ctx,
    t: &Resolved,
    dir: &str,
    mods: &mut Vec<Installed>,
    job: &Job,
    root: bool,
) -> Result<Option<(Project, Version, String, Option<String>)>, anyhow::Error> {
    ctx.check_provider(job.provider)?;
    let project = load_project(ctx, job.provider, &job.project_id).await?;

    let existing = already_installed(mods, job.provider, &project).map(|m| m.file.clone());
    if !root && existing.is_some() {
        return Ok(None);
    }

    let version = match &job.version_id {
        Some(v) => load_version(ctx, job.provider, &job.project_id, v, t.loader).await?,
        None => {
            let candidates =
                fetch_versions(ctx, job.provider, &job.project_id, t.loader, t.game_version.as_deref()).await?;
            providers::pick_best(candidates).ok_or_else(|| {
                display(format!(
                    "{} has no version for {}{}.",
                    project.title,
                    t.loader.unwrap_or("this server"),
                    t.game_version.as_deref().map(|v| format!(" {v}")).unwrap_or_default()
                ))
            })?
        }
    };
    let Some(file) = version.file.clone() else {
        return Err(display(format!("{} {} has no downloadable file.", project.title, version.version_number)));
    };
    let Some(url) = file.url.clone() else {
        return Err(display(format!(
            "The author of {} does not allow downloads through third-party apps. Download it from {} and upload it to /{dir} in the Files tab.",
            project.title, project.url
        )));
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(display("The provider returned an unusable download URL."));
    }
    let name = providers::safe_file_name(&file.name)?;
    let ext = providers::extension_for(t.loader);
    if !name.to_lowercase().ends_with(ext) {
        return Err(display(format!(
            "{} {} comes as {name}, not a {ext} file, so it does not belong in /{dir}.",
            project.title, version.version_number
        )));
    }

    let replace = match job.replace.clone().or(existing) {
        Some(old) => {
            if !mods.iter().any(|m| m.file == old) {
                return Err(display(format!("{old} is not in /{dir} any more. Refresh the list.")));
            }
            Some(old)
        }
        None => None,
    };
    let keep_disabled = replace.as_deref().is_some_and(|o| o.ends_with(DISABLED_SUFFIX));
    let target = if keep_disabled { format!("{name}{DISABLED_SUFFIX}") } else { name.clone() };
    if replace.as_deref() != Some(target.as_str()) && mods.iter().any(|m| m.file == target) {
        return Err(display(format!("/{dir}/{target} already exists.")));
    }

    // Same file name as the jar being replaced (reinstall): move the old one aside first,
    // so a failed download can put it back.
    let backup = match &replace {
        Some(old) if *old == target => {
            let bak = format!("{old}.mmbak");
            ctx.files.rename(dir, old, &bak).await?;
            Some(bak)
        }
        _ => None,
    };
    if let Err(e) = ctx.files.pull(dir, &target, &url).await {
        if let (Some(bak), Some(old)) = (&backup, &replace) {
            let _ = ctx.files.rename(dir, bak, old).await;
        }
        return Err(e);
    }
    match (&backup, &replace) {
        (Some(bak), _) => ctx.files.delete(dir, std::slice::from_ref(bak)).await?,
        (None, Some(old)) => ctx.files.delete(dir, std::slice::from_ref(old)).await?,
        _ => {}
    }

    if let Some(sha1) = &file.sha1 {
        installed::record(ctx, sha1, &version).await;
    }
    mods.retain(|m| Some(&m.file) != replace.as_ref());
    mods.push(Installed {
        file: target.clone(),
        size: file.size,
        modified: chrono::Utc::now().timestamp(),
        enabled: !keep_disabled,
        sha1: file.sha1.clone(),
        provider: Some(job.provider),
        project: Some(project.clone()),
        version: Some(version.clone()),
        update: None,
    });

    Ok(Some((project, version, target, replace)))
}

#[derive(Deserialize)]
struct TogglePayload {
    file: String,
    enabled: bool,
    #[serde(flatten)]
    target: Target,
}

/// Enable / disable a jar by adding or removing ".disabled".
async fn toggle(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    logger: GetServerActivityLogger,
    Path(_server): ServerPath,
    shared::Payload(data): shared::Payload<TogglePayload>,
) -> ApiResponseResult {
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    let t = data.target.resolve(&ctx).await?;
    let Some(dir) = t.directory.clone() else {
        return Err(display("No mods or plugins folder for this server.").into());
    };
    let entries = ctx.files.list(&dir).await?.unwrap_or_default();
    if !entries.iter().any(|e| e.name == data.file && installed::is_content_file(e, t.loader)) {
        return Err(display(format!("{} is not in /{dir}.", data.file)).into());
    }
    let base = data.file.strip_suffix(DISABLED_SUFFIX).unwrap_or(&data.file).to_string();
    let to = if data.enabled { base.clone() } else { format!("{base}{DISABLED_SUFFIX}") };
    if to != data.file {
        if entries.iter().any(|e| e.name == to) {
            return Err(display(format!("/{dir}/{to} already exists.")).into());
        }
        ctx.files.rename(&dir, &data.file, &to).await?;
        log(
            &logger,
            if data.enabled { "enable" } else { "disable" },
            json!({ "file": format!("{dir}/{base}") }),
        )
        .await;
    }
    ctx.forget("installed").await;

    ok(json!({ "file": to }))
}

#[derive(Deserialize)]
struct RemovePayload {
    files: Vec<String>,
    #[serde(flatten)]
    target: Target,
}

async fn remove(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    logger: GetServerActivityLogger,
    Path(_server): ServerPath,
    shared::Payload(data): shared::Payload<RemovePayload>,
) -> ApiResponseResult {
    if data.files.is_empty() || data.files.len() > 500 {
        return Err(display("Nothing to remove.").into());
    }
    let ctx = Ctx::manage(state.0, &permissions, &user, server.0).await?;
    let t = data.target.resolve(&ctx).await?;
    let Some(dir) = t.directory.clone() else {
        return Err(display("No mods or plugins folder for this server.").into());
    };
    let entries = ctx.files.list(&dir).await?.unwrap_or_default();
    for f in &data.files {
        if !entries.iter().any(|e| &e.name == f && installed::is_content_file(e, t.loader)) {
            return Err(display(format!("{f} is not in /{dir}.")).into());
        }
    }
    ctx.files.delete(&dir, &data.files).await?;
    log(
        &logger,
        "remove",
        json!({ "files": data.files.iter().map(|f| format!("{dir}/{f}")).collect::<Vec<_>>() }),
    )
    .await;
    ctx.forget("installed").await;

    ok(json!({ "removed": data.files }))
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .route("/export", get(crate::modlist::export))
        .route("/import/preview", post(crate::modlist::preview))
        .route("/import/pack", post(crate::pack::from_pack))
        .route("/crash/sources", get(crate::crash::sources))
        .route("/crash/analyze", post(crate::crash::analyze))
        .route("/modpack/plan", post(crate::modpack::plan))
        .route("/modpack/install", post(crate::modpack::install))
        .route("/modpack/job", get(crate::modpack::job))
        .route("/modpack/current", get(crate::modpack::current).merge(delete(crate::modpack::forget)))
        .route("/settings", get(settings))
        .route("/context", get(context))
        .route("/search", get(search))
        .route("/projects/{provider}/{id}", get(project))
        .route("/projects/{provider}/{id}/versions", get(versions))
        .route("/installed", get(installed))
        .route("/install", post(install))
        .route("/toggle", post(toggle))
        .route("/remove", post(remove))
        .with_state(state.clone())
}
