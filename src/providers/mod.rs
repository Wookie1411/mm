//! Modrinth and CurseForge behind one shape. Everything the page shows is one of the
//! normalised types below; each provider module maps its own API onto them.

use crate::ctx::display;
use serde::{Deserialize, Serialize};

pub mod curseforge;
pub mod modrinth;

pub const LOADERS: [&str; 13] = [
    "fabric",
    "quilt",
    "forge",
    "neoforge",
    "paper",
    "purpur",
    "folia",
    "spigot",
    "bukkit",
    "sponge",
    "velocity",
    "bungeecord",
    "waterfall",
];

/// Data packs are handled as a pseudo-loader (Modrinth's own loader name for them); they go
/// into <level-name>/datapacks instead of a fixed folder.
pub const DATAPACK: &str = "datapack";

/// The file type a loader's content comes as.
pub fn extension_for(loader: Option<&str>) -> &'static str {
    if loader == Some(DATAPACK) { ".zip" } else { ".jar" }
}

/// Where jars for a loader go, relative to the server root.
pub fn directory_for(loader: &str) -> Option<&'static str> {
    match loader {
        "fabric" | "quilt" | "forge" | "neoforge" => Some("mods"),
        "paper" | "purpur" | "folia" | "spigot" | "bukkit" | "sponge" | "velocity" | "bungeecord" | "waterfall" => {
            Some("plugins")
        }
        _ => None,
    }
}

pub fn is_plugin_loader(loader: &str) -> bool {
    directory_for(loader) == Some("plugins")
}

/// Loaders whose jars a server running `loader` can load, best match first.
pub fn compatible_loaders(loader: &str) -> Vec<&'static str> {
    match loader {
        "fabric" => vec!["fabric"],
        "quilt" => vec!["quilt", "fabric"],
        "forge" => vec!["forge"],
        "neoforge" => vec!["neoforge"],
        "paper" => vec!["paper", "spigot", "bukkit"],
        "purpur" => vec!["purpur", "paper", "spigot", "bukkit"],
        "folia" => vec!["folia"],
        "spigot" => vec!["spigot", "bukkit"],
        "bukkit" => vec!["bukkit"],
        "sponge" => vec!["sponge"],
        "velocity" => vec!["velocity"],
        "bungeecord" => vec!["bungeecord"],
        "waterfall" => vec!["waterfall", "bungeecord"],
        DATAPACK => vec![DATAPACK],
        _ => vec![],
    }
}

pub fn loader(raw: &str) -> Result<&'static str, anyhow::Error> {
    if raw.eq_ignore_ascii_case(DATAPACK) {
        return Ok(DATAPACK);
    }
    LOADERS
        .iter()
        .find(|l| l.eq_ignore_ascii_case(raw))
        .copied()
        .ok_or_else(|| display(format!("Unknown loader \"{raw}\".")))
}

/// Minecraft versions as Modrinth and CurseForge spell them: 1.21.4, 26.2, 24w14a, 1.21-pre1 …
pub fn game_version(raw: &str) -> Result<String, anyhow::Error> {
    let v = raw.trim();
    if v.is_empty() || v.len() > 32 || !v.chars().all(|c| c.is_ascii_alphanumeric() || ".-_ ".contains(c)) {
        return Err(display(format!("Invalid Minecraft version \"{raw}\".")));
    }
    Ok(v.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Modrinth,
    Curseforge,
}

impl Provider {
    pub fn parse(raw: &str) -> Result<Self, anyhow::Error> {
        match raw {
            "modrinth" => Ok(Self::Modrinth),
            "curseforge" => Ok(Self::Curseforge),
            other => Err(display(format!("Unknown provider \"{other}\"."))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::Curseforge => "curseforge",
        }
    }
}

/// Provider ids: Modrinth base62 ids / slugs, CurseForge numbers.
pub fn id(raw: &str) -> Result<String, anyhow::Error> {
    if raw.is_empty() || raw.len() > 64 || !raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(display("Invalid project or version id."));
    }
    Ok(raw.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub provider: Provider,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
    pub url: String,
    pub categories: Vec<String>,
    pub updated: Option<String>,
    /// Modrinth's server_side: required / optional / unsupported / unknown (None for CurseForge).
    #[serde(default)]
    pub server_side: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub project: Project,
    /// Markdown description (Modrinth only: CurseForge descriptions are HTML, so the
    /// page shows the summary and links to the project instead).
    pub body: Option<String>,
    pub links: Vec<Link>,
    pub gallery: Vec<String>,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    /// Set when the author does not allow downloads through third-party tools.
    pub distribution_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<Project>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    /// `None` when the author blocks third-party downloads (CurseForge).
    pub url: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    /// required / optional / incompatible / embedded
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub provider: Provider,
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    /// release / beta / alpha
    pub version_type: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub published: Option<String>,
    pub downloads: u64,
    pub file: Option<FileInfo>,
    pub dependencies: Vec<Dependency>,
}

/// What a jar on the server was identified as.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identified {
    pub provider: Provider,
    pub project_id: String,
    pub version_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub query: String,
    pub loader: Option<&'static str>,
    pub game_version: Option<String>,
    /// Modpacks for `loader` instead of mods / plugins.
    pub modpacks: bool,
    /// Leave out projects that do not run on a server (Modrinth only).
    pub server_only: bool,
    /// relevance / downloads / follows / newest / updated
    pub sort: String,
    pub offset: u64,
    pub limit: u64,
}

/// Prefer a release, then a beta, then anything; newest first within each.
pub fn pick_best(mut versions: Vec<Version>) -> Option<Version> {
    versions.retain(|v| v.file.is_some());
    let rank = |v: &Version| match v.version_type.as_str() {
        "release" => 0,
        "beta" => 1,
        _ => 2,
    };
    // providers return newest first; keep that order within a rank
    versions.into_iter().enumerate().min_by_key(|(i, v)| (rank(v), *i)).map(|(_, v)| v)
}

/// A file name Wings may write into the mods / plugins directory.
pub fn safe_file_name(name: &str) -> Result<String, anyhow::Error> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && !name.contains("..")
        && (name.ends_with(".jar") || name.ends_with(".zip"));
    if !ok {
        return Err(display(format!("Refusing to install a file named \"{name}\".")));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, kind: &str) -> Version {
        Version {
            provider: Provider::Modrinth,
            id: id.into(),
            project_id: "p".into(),
            name: id.into(),
            version_number: id.into(),
            version_type: kind.into(),
            game_versions: vec![],
            loaders: vec![],
            published: None,
            downloads: 0,
            file: Some(FileInfo { name: "a.jar".into(), size: 1, url: Some("u".into()), sha1: None }),
            dependencies: vec![],
        }
    }

    #[test]
    fn best_prefers_newest_release() {
        let best = pick_best(vec![v("3", "alpha"), v("2", "release"), v("1", "release")]).unwrap();
        assert_eq!(best.id, "2");
        let best = pick_best(vec![v("3", "alpha"), v("2", "beta")]).unwrap();
        assert_eq!(best.id, "2");
    }

    #[test]
    fn file_names() {
        assert!(safe_file_name("fabric-api-0.161.0+26.2.jar").is_ok());
        assert!(safe_file_name("../x.jar").is_err());
        assert!(safe_file_name("a/b.jar").is_err());
        assert!(safe_file_name(".hidden.jar").is_err());
        assert!(safe_file_name("run.sh").is_err());
    }

    #[test]
    fn loaders() {
        assert_eq!(directory_for("neoforge"), Some("mods"));
        assert_eq!(directory_for("purpur"), Some("plugins"));
        assert_eq!(compatible_loaders("quilt"), vec!["quilt", "fabric"]);
        assert!(loader("Fabric").is_ok());
        assert!(loader("vanilla").is_err());
        assert!(game_version("1.21.4").is_ok());
        assert!(game_version("1.21; rm").is_err());
    }
}
