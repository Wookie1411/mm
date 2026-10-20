//! Work out which loader and Minecraft version a server runs, from (in order) the files
//! it has, its latest log, and its egg variables / egg name. Egg variables alone are
//! not enough: most eggs default to "latest".

use crate::{ctx::Ctx, providers::directory_for};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Detected {
    pub loader: Option<String>,
    pub game_version: Option<String>,
    /// mods / plugins, from the loader.
    pub directory: Option<String>,
    /// <level-name>/datapacks, from server.properties.
    #[serde(default)]
    pub datapack_directory: String,
    /// How each value was found, for the page's tooltip.
    pub notes: Vec<String>,
}

static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\.\d+(\.\d+)?$").unwrap());
static LOG_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Starting minecraft server version|Loading Minecraft) (\d+\.\d+(?:\.\d+)?)\b").unwrap()
});
static LOG_LOADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"with (Fabric|Quilt) Loader|\b(NeoForge|MinecraftForge|Forge) v?\d|This server is running (Paper|Purpur|Folia)|Velocity \d|Starting Waterfall|BungeeCord version").unwrap());
static MC_IN_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(MC: (\d+\.\d+(?:\.\d+)?)\)").unwrap());

/// Loader from the egg name, most specific first.
pub(crate) fn loader_from_name(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    [
        ("neoforge", "neoforge"),
        ("forge", "forge"),
        ("quilt", "quilt"),
        ("fabric", "fabric"),
        ("purpur", "purpur"),
        ("folia", "folia"),
        ("paper", "paper"),
        ("spigot", "spigot"),
        ("bukkit", "bukkit"),
        ("sponge", "sponge"),
        ("velocity", "velocity"),
        ("waterfall", "waterfall"),
        ("bungee", "bungeecord"),
    ]
    .into_iter()
    .find(|(needle, _)| n.contains(needle))
    .map(|(_, loader)| loader)
}

fn loader_from_log(log: &str) -> Option<&'static str> {
    let c = LOG_LOADER.captures(log)?;
    let m = c.get(0)?.as_str();
    Some(if m.contains("Fabric") {
        "fabric"
    } else if m.contains("Quilt") {
        "quilt"
    } else if m.contains("NeoForge") {
        "neoforge"
    } else if m.contains("Forge") {
        "forge"
    } else if m.contains("Purpur") {
        "purpur"
    } else if m.contains("Folia") {
        "folia"
    } else if m.contains("Paper") {
        "paper"
    } else if m.contains("Velocity") {
        "velocity"
    } else if m.contains("Waterfall") {
        "waterfall"
    } else {
        "bungeecord"
    })
}

fn version_from_log(log: &str) -> Option<String> {
    LOG_VERSION
        .captures(log)
        .or_else(|| MC_IN_PARENS.captures(log))
        .map(|c| c[1].to_string())
}

/// Newest-looking version directory name (e.g. versions/26.2).
fn highest_version<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    names
        .filter(|n| VERSION.is_match(n))
        .max_by_key(|n| n.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
        .map(String::from)
}

pub async fn detect(ctx: &Ctx) -> Detected {
    let mut d = Detected::default();
    let files = &ctx.files;

    let (root, log, variables, properties) = tokio::join!(
        files.list("/"),
        files.head("logs/latest.log", 400),
        variables(ctx),
        files.head("server.properties", 300),
    );
    d.datapack_directory = format!("{}/datapacks", level_name(properties.as_deref().unwrap_or_default()));
    let root = root.ok().flatten().unwrap_or_default();
    let has = |name: &str| root.iter().any(|e| e.name == name);
    let log = log.unwrap_or_default();

    // --- loader
    let from_files = if has(".fabric") || has("fabric-server-launcher.properties") {
        Some("fabric")
    } else if has(".quilt") || has("quilt-server-launcher.properties") {
        Some("quilt")
    } else if has("purpur.yml") {
        Some("purpur")
    } else if has("velocity.toml") {
        Some("velocity")
    } else if has("waterfall.yml") {
        Some("waterfall")
    } else {
        None
    };
    let from_libraries = if from_files.is_none() && has("libraries") {
        let net = files.list("libraries/net").await.ok().flatten().unwrap_or_default();
        if net.iter().any(|e| e.name == "neoforged") {
            Some("neoforge")
        } else if net.iter().any(|e| e.name == "minecraftforge") {
            Some("forge")
        } else {
            None
        }
    } else {
        None
    };
    let from_paper_files = if from_files.is_none() && from_libraries.is_none() && (has("paper.yml") || has("spigot.yml")) {
        let config = files.list("config").await.ok().flatten().unwrap_or_default();
        if has("paper.yml") || config.iter().any(|e| e.name == "paper-global.yml") {
            Some("paper")
        } else {
            Some("spigot")
        }
    } else {
        None
    };
    let from_vars = variables.iter().find_map(|(k, _)| match k.as_str() {
        "NEOFORGE_VERSION" => Some("neoforge"),
        "FORGE_VERSION" => Some("forge"),
        "FABRIC_VERSION" | "FABRIC_LOADER_VERSION" => Some("fabric"),
        "QUILT_VERSION" | "QUILT_LOADER_VERSION" => Some("quilt"),
        "VELOCITY_VERSION" => Some("velocity"),
        "BUNGEE_VERSION" => Some("bungeecord"),
        "SPONGE_VERSION" => Some("sponge"),
        _ => None,
    });

    let candidates = [
        (from_files, "server files"),
        (from_libraries, "libraries folder"),
        (from_paper_files, "server config files"),
        (loader_from_log(&log), "latest.log"),
        (from_vars, "egg variables"),
        (loader_from_name(&ctx.server.egg.name), "egg name"),
    ];
    if let Some((loader, how)) = candidates.iter().find_map(|(l, how)| l.map(|l| (l, *how))) {
        d.loader = Some(loader.to_string());
        d.directory = directory_for(loader).map(String::from);
        d.notes.push(format!("Loader {loader} from the {how}."));
    } else if has("plugins") {
        d.loader = Some("paper".into());
        d.directory = Some("plugins".into());
        d.notes.push("Guessed Paper from the plugins folder.".into());
    } else if has("mods") {
        d.directory = Some("mods".into());
        d.notes.push("Found a mods folder but could not tell the loader.".into());
    }

    // --- Minecraft version
    let proxy = matches!(d.loader.as_deref(), Some("velocity" | "bungeecord" | "waterfall"));
    if !proxy {
        let var = variables
            .iter()
            .filter(|(k, _)| matches!(k.as_str(), "MC_VERSION" | "MINECRAFT_VERSION" | "VANILLA_VERSION"))
            .find_map(|(_, v)| VERSION.is_match(v.trim()).then(|| v.trim().to_string()));
        let from_log = version_from_log(&log);
        let from_versions_dir = if from_log.is_none() && has("versions") {
            let dirs = files.list("versions").await.ok().flatten().unwrap_or_default();
            highest_version(dirs.iter().filter(|e| e.directory).map(|e| e.name.as_str()))
        } else {
            None
        };
        let from_history = if from_log.is_none() && from_versions_dir.is_none() && has("version_history.json") {
            files.head("version_history.json", 5).await.and_then(|h| version_from_log(&h))
        } else {
            None
        };
        let from_forge_libs = if from_log.is_none() && from_versions_dir.is_none() && from_history.is_none() {
            let path = match d.loader.as_deref() {
                Some("forge") => Some("libraries/net/minecraftforge/forge"),
                _ => None,
            };
            match path {
                Some(p) => files
                    .list(p)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|e| e.name.split_once('-').map(|(mc, _)| mc.to_string()))
                    .find(|mc| VERSION.is_match(mc)),
                None => None,
            }
        } else {
            None
        };
        let candidates = [
            (from_log, "latest.log"),
            (from_versions_dir, "versions folder"),
            (from_history, "version_history.json"),
            (from_forge_libs, "Forge libraries"),
            (var, "egg variables"),
        ];
        if let Some((version, how)) = candidates.into_iter().find_map(|(v, how)| v.map(|v| (v, how))) {
            d.notes.push(format!("Minecraft {version} from the {how}."));
            d.game_version = Some(version);
        }
    }

    d
}

/// The world folder: level-name from server.properties, "world" when unset or unsafe.
fn level_name(properties: &str) -> String {
    properties
        .lines()
        .filter_map(|l| l.trim().strip_prefix("level-name="))
        .map(|v| v.trim().trim_matches('/').to_string())
        .find(|v| !v.is_empty() && !v.contains("..") && !v.contains('\\') && !v.starts_with('.'))
        .unwrap_or_else(|| "world".into())
}

/// The server's egg variables (env name -> value or default).
pub(crate) async fn variables(ctx: &Ctx) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT nest_egg_variables.env_variable, COALESCE(server_variables.value, nest_egg_variables.default_value, '')
        FROM nest_egg_variables
        LEFT JOIN server_variables ON server_variables.variable_uuid = nest_egg_variables.uuid
            AND server_variables.server_uuid = $1
        WHERE nest_egg_variables.egg_uuid = $2",
    )
    .bind(ctx.server.uuid)
    .bind(ctx.server.egg.uuid)
    .fetch_all(ctx.state.database.read())
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs() {
        let fabric = "[17:38:34] [main/INFO]: Loading Minecraft 26.2 with Fabric Loader 0.19.3\n[17:38:53] [Server thread/INFO]: Starting minecraft server version 26.2";
        assert_eq!(loader_from_log(fabric), Some("fabric"));
        assert_eq!(version_from_log(fabric).as_deref(), Some("26.2"));
        let paper = "[INFO]: This server is running Paper version 1.21.4-232-ver/1.21.4@abc (MC: 1.21.4)";
        assert_eq!(loader_from_log(paper), Some("paper"));
        assert_eq!(version_from_log(paper).as_deref(), Some("1.21.4"));
        assert_eq!(loader_from_log("[main/INFO] [net.neoforged.fml.loading.FMLLoader/]: NeoForge 21.1.77"), Some("neoforge"));
        assert_eq!(loader_from_log("Starting minecraft server version 1.21.4"), None);
    }

    #[test]
    fn level_names() {
        assert_eq!(level_name("motd=x\nlevel-name=survival\n"), "survival");
        assert_eq!(level_name("level-name=\n"), "world");
        assert_eq!(level_name("level-name=../etc\n"), "world");
        assert_eq!(level_name(""), "world");
    }

    #[test]
    fn names_and_dirs() {
        assert_eq!(loader_from_name("NeoForge"), Some("neoforge"));
        assert_eq!(loader_from_name("Forge Minecraft"), Some("forge"));
        assert_eq!(loader_from_name("Bungeecord"), Some("bungeecord"));
        assert_eq!(loader_from_name("Vanilla Minecraft"), None);
        assert_eq!(highest_version(["1.20.1", "26.2", "1.21.11", "x"].into_iter()).as_deref(), Some("26.2"));
    }
}
