//! Optional part of a modpack install: switch the server to the Minecraft version and
//! loader the pack is made for, with the Java image Mojang asks for.
//!
//! Two routes. Vanilla and Fabric on a matching egg: download the server jar into the
//! egg's SERVER_JARFILE. Anything else (Forge, NeoForge, or a server on another egg):
//! move the server to the matching egg, set its version variables and reinstall it
//! without wiping files, so the egg's own install script runs the loader's installer.
//! Moving between eggs is off unless an administrator allows it.

use crate::{
    ctx::{Ctx, display},
    detect,
    modpack::Pack,
};
use regex::Regex;
use serde_json::Value;
use shared::models::{
    ByUuid, UpdatableModel,
    nest_egg::NestEgg,
    server::{Server, UpdateServerOptions},
    server_variable::ServerVariable,
    user::PermissionManager,
};
use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions";
/// Egg variables that hold the Minecraft / loader version, across the common eggs.
const MC_VARIABLES: [&str; 3] = ["MC_VERSION", "MINECRAFT_VERSION", "VANILLA_VERSION"];
const LOADER_VARIABLES: [&str; 2] = ["LOADER_VERSION", "FABRIC_LOADER_VERSION"];

static JAVA_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)java[ _-]?(\d+)").unwrap());

const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How the server gets the pack's Minecraft version.
pub enum Route {
    /// Swap the server jar on the current egg (vanilla / Fabric).
    Jar,
    /// Move to (or stay on) this egg and reinstall, so its install script sets the loader up.
    Egg { uuid: uuid::Uuid, name: String },
}

impl Route {
    pub fn describe(&self, ctx: &Ctx) -> serde_json::Value {
        match self {
            Route::Jar => serde_json::json!({ "route": "jar" }),
            Route::Egg { uuid, name } => serde_json::json!({
                "route": "egg", "egg": name, "moves": *uuid != ctx.server.egg.uuid,
            }),
        }
    }
}

/// Whether an egg sets up `loader` (None = vanilla).
fn egg_matches(name: &str, loader: Option<&str>) -> bool {
    match loader {
        None => {
            let n = name.to_lowercase();
            n.contains("vanilla") && !n.contains("bedrock") && detect::loader_from_name(name).is_none()
        }
        Some(l) => detect::loader_from_name(name) == Some(l),
    }
}

/// How the server can be switched to what the pack needs; the reason when it cannot.
pub async fn route(ctx: &Ctx, pack: &Pack) -> Result<Route, String> {
    if pack.minecraft.is_none() {
        return Err("The pack does not say which Minecraft version it needs.".into());
    }
    let loader = pack.loader.as_deref();
    let current = &ctx.server.egg;
    if egg_matches(&current.name, loader) {
        return Ok(match loader {
            None | Some("fabric") => Route::Jar,
            _ => Route::Egg { uuid: current.uuid, name: current.name.to_string() },
        });
    }

    let eggs: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT uuid, name FROM nest_eggs ORDER BY name")
        .fetch_all(ctx.state.database.read())
        .await
        .unwrap_or_default();
    let label = match loader {
        None => "Vanilla Minecraft",
        Some("neoforge") => "NeoForge",
        Some("forge") => "Forge",
        Some("fabric") => "Fabric",
        Some("quilt") => "Quilt",
        Some(other) => other,
    };
    let Some((uuid, name)) = eggs.into_iter().find(|(u, n)| ctx.settings.egg_allowed(*u) && egg_matches(n, loader)) else {
        return Err(format!(
            "No {label} egg is enabled for Mod Manager. An administrator can enable one under Admin > Extensions > Mod Manager."
        ));
    };
    if !ctx.settings.switch_eggs {
        return Err(format!(
            "This server would have to move to the {name} egg. An administrator can allow that under Admin > Extensions > Mod Manager."
        ));
    }
    Ok(Route::Egg { uuid, name })
}

/// Wiping reinstalls the server with the panel's truncate flag: Reinstall plus Delete files.
pub fn check_wipe_permissions(permissions: &PermissionManager) -> Result<(), anyhow::Error> {
    if ["settings.install", "files.delete"].iter().any(|p| permissions.has_server_permission(p).is_err()) {
        return Err(display(
            "Deleting all files first needs the \"Reinstall\" and \"Delete files\" permissions.",
        ));
    }
    Ok(())
}

/// Reinstall the server with its egg's install script, optionally deleting every file
/// first, and wait until Wings is done.
pub async fn reinstall(ctx: &Ctx, server: &Server, wipe: bool) -> Result<(), anyhow::Error> {
    // Wings needs the current egg and variables before it runs the install script
    server.clone().sync(&ctx.state).await?;
    server.install(&ctx.state, wipe, None).await?;
    wait_for_install(ctx).await
}

/// The permissions the panel itself asks for to change startup, the image and reinstall.
pub fn check_permissions(permissions: &PermissionManager) -> Result<(), anyhow::Error> {
    if ["startup.update", "startup.docker-image", "settings.install"]
        .iter()
        .any(|p| permissions.has_server_permission(p).is_err())
    {
        return Err(display(
            "Installing the pack's Minecraft version needs the \"Update startup\", \"Change Docker image\" and \"Reinstall\" permissions.",
        ));
    }
    Ok(())
}

async fn get_json(ctx: &Ctx, url: &str) -> Result<Value, anyhow::Error> {
    let response = ctx
        .state
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| display(format!("Could not reach {url}: {e}")))?;
    if !response.status().is_success() {
        return Err(display(format!("{url} answered HTTP {}.", response.status().as_u16())));
    }
    response.json().await.map_err(|e| display(format!("{url} sent invalid JSON: {e}")))
}

/// Mojang's version file: the vanilla server jar URL and the Java major version it needs.
async fn mojang_version(ctx: &Ctx, minecraft: &str) -> Result<(Option<String>, u64), anyhow::Error> {
    let manifest = get_json(ctx, MOJANG_MANIFEST).await?;
    let Some(url) = manifest["versions"]
        .as_array()
        .and_then(|v| v.iter().find(|v| v["id"].as_str() == Some(minecraft)))
        .and_then(|v| v["url"].as_str())
    else {
        return Err(display(format!("Mojang does not know Minecraft {minecraft}.")));
    };
    let version = get_json(ctx, url).await?;
    Ok((
        version["downloads"]["server"]["url"].as_str().map(String::from),
        version["javaVersion"]["majorVersion"].as_u64().unwrap_or(8),
    ))
}

/// The newest stable entry of a Fabric meta list, else the newest.
fn fabric_pick(list: &Value, get: impl Fn(&Value) -> &Value) -> Option<String> {
    let list = list.as_array()?;
    list.iter()
        .find(|v| get(v)["stable"].as_bool() == Some(true))
        .or(list.first())
        .and_then(|v| get(v)["version"].as_str())
        .map(String::from)
}

/// Pick the egg image for `java`: exactly that version, else the closest newer one.
fn java_image<'a>(images: impl Iterator<Item = (&'a str, &'a str)>, java: u64) -> Option<&'a str> {
    images
        .filter_map(|(key, image)| {
            let n: u64 = JAVA_KEY.captures(key).or_else(|| JAVA_KEY.captures(image))?[1].parse().ok()?;
            (n >= java).then_some((n, image))
        })
        .min_by_key(|(n, _)| *n)
        .map(|(_, image)| image)
}

/// Download `url` next to `path`, then swap it in.
async fn swap(ctx: &Ctx, path: &str, url: &str) -> Result<(), anyhow::Error> {
    let (dir, name) = crate::modpack::split(path);
    let temp = format!("{name}.modmanager-new");
    ctx.files.pull(dir, &temp, url).await?;
    let _ = ctx.files.delete(dir, &[name.to_string()]).await;
    ctx.files.rename(dir, &temp, name).await
}

/// The vanilla jar the Fabric launcher runs, from fabric-server-launcher.properties.
async fn fabric_vanilla_jar(ctx: &Ctx, launcher: &str) -> Result<String, anyhow::Error> {
    const PROPERTIES: &str = "fabric-server-launcher.properties";
    let configured = ctx
        .files
        .read_bytes(PROPERTIES, 64 * 1024)
        .await?
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("serverJar=").map(|v| v.trim().to_string())))
        .and_then(|v| crate::modpack::safe_path(&v));
    match configured {
        Some(jar) if jar != launcher => Ok(jar),
        // the launcher's default is server.jar, which may be the launcher itself
        _ => {
            ctx.files.write_bytes(PROPERTIES, b"serverJar=minecraft-server.jar\n".to_vec()).await?;
            Ok("minecraft-server.jar".into())
        }
    }
}

async fn set_variables(ctx: &Ctx, egg: uuid::Uuid, names: &[&str], value: &str) -> Result<Vec<String>, anyhow::Error> {
    // only variables the user could change themselves on the Startup page
    let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT uuid, env_variable FROM nest_egg_variables
        WHERE egg_uuid = $1 AND env_variable = ANY($2) AND user_editable",
    )
    .bind(egg)
    .bind(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
    .fetch_all(ctx.state.database.read())
    .await?;
    for (uuid, _) in &rows {
        ServerVariable::create(&ctx.state.database, ctx.server.uuid, *uuid, value).await?;
    }
    Ok(rows.into_iter().map(|(_, name)| format!("{name}={value}")).collect())
}

/// The Java image for the server: the closest match among `egg`'s images, unless the
/// server runs a custom image (one its current egg does not offer).
fn pick_image(ctx: &Ctx, egg: &NestEgg, java: u64, changes: &mut Vec<String>) -> Option<compact_str::CompactString> {
    let current = &ctx.server.image;
    if !ctx.server.egg.docker_images.values().any(|i| i == current) {
        changes.push(format!("Kept the custom Docker image {current}; this Minecraft version needs Java {java}"));
        return None;
    }
    match java_image(egg.docker_images.iter().map(|(k, v)| (k.as_str(), v.as_str())), java) {
        Some(image) if image == current => None,
        Some(image) => {
            changes.push(format!("Switched the Docker image to {image} (Java {java})"));
            Some(image.into())
        }
        None => {
            changes.push(format!("The {} egg has no Java {java} image; pick one by hand on the Startup page", egg.name));
            None
        }
    }
}

/// Switch the server to the pack's Minecraft version and loader. Returns what was changed.
/// `wipe`: on the egg route, the reinstall also deletes every file first.
pub async fn apply(ctx: &Ctx, pack: &Pack, route: &Route, wipe: bool) -> Result<Vec<String>, anyhow::Error> {
    let minecraft = pack.minecraft.as_deref().unwrap_or_default();
    let (vanilla_jar, java) = mojang_version(ctx, minecraft).await?;
    let changes = match route {
        Route::Jar => apply_jar(ctx, pack, minecraft, vanilla_jar, java).await?,
        Route::Egg { uuid, name } => apply_egg(ctx, pack, minecraft, java, *uuid, name, wipe).await?,
    };
    ctx.forget("detected").await;
    Ok(changes)
}

/// The egg variables that pin the version, per loader (the eggs' own naming).
fn egg_variables(minecraft: &str, loader: Option<&str>, version: Option<&str>) -> Vec<(&'static [&'static str], String)> {
    let mut out: Vec<(&'static [&'static str], String)> = vec![(&MC_VARIABLES, minecraft.to_string())];
    match loader {
        // the Forge egg downloads forge/<mc>-<forge>/
        Some("forge") => out.push((
            &["FORGE_VERSION"],
            version
                .map(|v| if v.starts_with(&format!("{minecraft}-")) { v.to_string() } else { format!("{minecraft}-{v}") })
                .unwrap_or_default(),
        )),
        // 1.20.1 NeoForge is still published as forge/1.20.1-<version>
        Some("neoforge") => out.push((
            &["NEOFORGE_VERSION"],
            version
                .map(|v| {
                    if minecraft == "1.20.1" && !v.starts_with("1.20.1-") { format!("1.20.1-{v}") } else { v.to_string() }
                })
                .unwrap_or_default(),
        )),
        Some(_) => out.push((&LOADER_VARIABLES, version.unwrap_or("latest").to_string())),
        None => {}
    }
    out
}

/// Wait until Wings reports the reinstall as finished.
async fn wait_for_install(ctx: &Ctx) -> Result<(), anyhow::Error> {
    let started = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let status: Option<String> = sqlx::query_scalar("SELECT status::text FROM servers WHERE uuid = $1")
            .bind(ctx.server.uuid)
            .fetch_one(ctx.state.database.write())
            .await?;
        match status.as_deref() {
            None => return Ok(()),
            Some("INSTALL_FAILED") => {
                return Err(display(
                    "The egg's install script failed; the console shows why. Fix it and reinstall from the Settings tab, then install the modpack again.",
                ));
            }
            _ if started.elapsed() > INSTALL_TIMEOUT => {
                return Err(display("The reinstall is still running after 30 minutes; check the console."));
            }
            _ => {}
        }
    }
}

async fn apply_egg(
    ctx: &Ctx,
    pack: &Pack,
    minecraft: &str,
    java: u64,
    egg_uuid: uuid::Uuid,
    egg_name: &str,
    wipe: bool,
) -> Result<Vec<String>, anyhow::Error> {
    let mut changes = Vec::new();
    let mut server: Server = ctx.server.clone();
    let egg = NestEgg::by_uuid_optional(&ctx.state.database, egg_uuid)
        .await?
        .ok_or_else(|| display(format!("The {egg_name} egg no longer exists.")))?;

    let image = pick_image(ctx, &egg, java, &mut changes);
    if egg_uuid != server.egg.uuid {
        let startup = egg.startup_commands.values().next().cloned();
        server
            .update(
                &ctx.state,
                UpdateServerOptions { egg_uuid: Some(egg_uuid), startup, image, ..Default::default() },
            )
            .await?;
        changes.insert(0, format!("Moved the server from the {} egg to {egg_name}", ctx.server.egg.name));
    } else if image.is_some() {
        server.update(&ctx.state, UpdateServerOptions { image, ..Default::default() }).await?;
    }

    for (names, value) in egg_variables(minecraft, pack.loader.as_deref(), pack.loader_version.as_deref()) {
        changes.extend(set_variables(ctx, egg_uuid, names, &value).await?);
    }

    reinstall(ctx, &server, wipe).await?;
    changes.push(format!(
        "Reinstalled the server with the {egg_name} egg ({})",
        if wipe { "all files deleted first" } else { "files kept" }
    ));
    Ok(changes)
}

async fn apply_jar(
    ctx: &Ctx,
    pack: &Pack,
    minecraft: &str,
    vanilla_jar: Option<String>,
    java: u64,
) -> Result<Vec<String>, anyhow::Error> {
    let mut changes = Vec::new();

    let (jar_url, loader_version) = if pack.loader.as_deref() == Some("fabric") {
        let loader = match &pack.loader_version {
            Some(v) => v.clone(),
            None => fabric_pick(&get_json(ctx, &format!("{FABRIC_META}/loader/{minecraft}")).await?, |v| &v["loader"])
                .ok_or_else(|| display(format!("Fabric has no loader for Minecraft {minecraft}.")))?,
        };
        let installer = fabric_pick(&get_json(ctx, &format!("{FABRIC_META}/installer")).await?, |v| v)
            .ok_or_else(|| display("Could not find a Fabric installer version."))?;
        (format!("{FABRIC_META}/loader/{minecraft}/{loader}/{installer}/server/jar"), Some(loader))
    } else {
        let url = vanilla_jar.clone().ok_or_else(|| display(format!("Minecraft {minecraft} has no server download.")))?;
        (url, None)
    };

    // the jar the egg starts (and for Fabric, the vanilla jar its launcher loads)
    let jarfile = detect::variables(ctx)
        .await
        .into_iter()
        .find(|(k, _)| k == "SERVER_JARFILE")
        .map(|(_, v)| v)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "server.jar".into());
    let jarfile = crate::modpack::safe_path(&jarfile)
        .ok_or_else(|| display(format!("SERVER_JARFILE ({jarfile}) is not a safe path.")))?;
    if loader_version.is_some() {
        let vanilla = vanilla_jar.ok_or_else(|| display(format!("Minecraft {minecraft} has no server download.")))?;
        let path = fabric_vanilla_jar(ctx, &jarfile).await?;
        swap(ctx, &path, &vanilla).await?;
        changes.push(format!("Installed the Minecraft {minecraft} server as {path}"));
    }
    swap(ctx, &jarfile, &jar_url).await?;
    changes.push(format!(
        "Installed the {} server jar for Minecraft {minecraft} as {jarfile}",
        pack.loader.as_deref().unwrap_or("vanilla")
    ));

    let egg = ctx.server.egg.uuid;
    changes.extend(set_variables(ctx, egg, &MC_VARIABLES, minecraft).await?);
    if let Some(loader) = &loader_version {
        changes.extend(set_variables(ctx, egg, &LOADER_VARIABLES, loader).await?);
    }

    let mut server = ctx.server.clone();
    if let Some(image) = pick_image(ctx, &ctx.server.egg, java, &mut changes) {
        server.update(&ctx.state, UpdateServerOptions { image: Some(image), ..Default::default() }).await?;
    }
    server.batch_sync(&ctx.state).await;
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_java() {
        let images = [
            ("java 25", "ghcr.io/pterodactyl/yolks:java_25"),
            ("java 21", "ghcr.io/ptero-eggs/yolks:java_21"),
            ("Java 17", "ghcr.io/ptero-eggs/yolks:java_17"),
            ("Java 8", "ghcr.io/ptero-eggs/yolks:java_8"),
        ];
        assert_eq!(java_image(images.into_iter(), 21), Some("ghcr.io/ptero-eggs/yolks:java_21"));
        assert_eq!(java_image(images.into_iter(), 16), Some("ghcr.io/ptero-eggs/yolks:java_17"));
        assert_eq!(java_image(images.into_iter(), 8), Some("ghcr.io/ptero-eggs/yolks:java_8"));
        assert_eq!(java_image(images.into_iter(), 26), None);
        assert_eq!(java_image([("Temurin", "eclipse-temurin:21-jre")].into_iter(), 21), None);
    }

    #[test]
    fn matches_eggs() {
        assert!(egg_matches("NeoForge", Some("neoforge")));
        assert!(egg_matches("Forge Minecraft", Some("forge")));
        assert!(!egg_matches("NeoForge", Some("forge")));
        assert!(egg_matches("Vanilla Minecraft", None));
        assert!(!egg_matches("Sponge (SpongeVanilla)", None));
        assert!(!egg_matches("Vanilla Bedrock", None));
        assert!(!egg_matches("Minecraft: Java Edition Modpack Installer", Some("fabric")));
    }

    #[test]
    fn version_variables() {
        let v = egg_variables("1.20.1", Some("forge"), Some("47.2.0"));
        assert_eq!(v[1], (&["FORGE_VERSION"][..], "1.20.1-47.2.0".to_string()));
        let v = egg_variables("1.20.1", Some("forge"), Some("1.20.1-47.2.0"));
        assert_eq!(v[1].1, "1.20.1-47.2.0");
        assert_eq!(egg_variables("1.20.1", Some("neoforge"), Some("47.1.106"))[1].1, "1.20.1-47.1.106");
        assert_eq!(egg_variables("1.21.1", Some("neoforge"), Some("21.1.77"))[1].1, "21.1.77");
        assert_eq!(egg_variables("1.21.1", Some("neoforge"), None)[1].1, "");
        assert_eq!(egg_variables("26.3", None, None).len(), 1);
    }
}
