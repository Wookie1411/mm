//! Crash doctor: read a server log or crash report, recognise the usual reasons a modded
//! server fails (missing / wrong dependencies, incompatible or client-only mods, broken
//! mixins, data packs that do not load, plugin dependencies, Java version, memory …) and
//! turn them into recommendations with one-click actions (install, change version,
//! disable). Rule based: the loaders word these errors in fixed ways, so no guessing.
//!
//! Mod ids in logs are mapped to jars by reading each jar's metadata (fabric.mod.json,
//! quilt.mod.json, mods.toml, plugin.yml) and the Java packages it contains, cached per
//! SHA-1; stack frames are matched to jars by package.

use crate::{
    ctx::{Ctx, display},
    installed::Installed,
    providers::{self, Provider},
    routes::{Resolved, installed_list},
};
use axum::extract::Path;
use futures_util::{StreamExt, stream};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shared::{
    GetState,
    models::{
        server::GetServer,
        user::{GetPermissionManager, GetUser},
    },
    response::{ApiResponse, ApiResponseResult},
};
use std::{
    collections::{BTreeSet, HashMap},
    io::Read,
    sync::LazyLock,
};
use wings_api::Algorithm;

const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_UNZIPPED_LOG: u64 = 64 * 1024 * 1024;
const MAX_JAR_BYTES: u64 = 96 * 1024 * 1024;
const MAX_PASTE: usize = 4 * 1024 * 1024;

// ------------------------------------------------------------------ parsing (pure)

/// One problem recognised in a log, before it is matched to files on the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// Fabric / Quilt: `mod` needs `dep` (a version range, or any); `present` None = missing.
    Requires { name: String, id: String, dep_name: Option<String>, dep: String, wanted: String, present: Option<String>, line: String },
    Incompatible { name: String, id: String, other_name: Option<String>, other: String, line: String },
    /// Forge / NeoForge mandatory dependency.
    ForgeDependency { dep: String, requested_by: String, expected: String, actual: String, line: String },
    ClientClass { class: String, line: String },
    MixinFailed { id: String, line: String },
    EntrypointFailed { id: String, line: String },
    Java { needed: u32, have: u32, line: String },
    OutOfMemory { line: String },
    PortInUse { line: String },
    Eula { line: String },
    Datapack { file: String, line: String },
    DatapacksFailed { line: String },
    PluginMissingDependencies { plugin: String, deps: Vec<String>, line: String },
    PluginLoadFailed { file: String, reason: String, line: String },
    SuspectedMods { mods: Vec<(String, String)>, line: String },
    DuplicateMod { id: Option<String>, line: String },
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub issues: Vec<Issue>,
    /// The server failed to start or crashed (as opposed to errors it survived).
    pub crashed: bool,
    /// Crash report "Description:" and the root "Caused by" lines.
    pub headline: Vec<String>,
    /// Classes from stack frames that are not Minecraft / Java / loader code.
    pub frames: Vec<String>,
    pub lines: usize,
}

macro_rules! re {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

re!(R_REQUIRES, r"Mod '(?P<name>[^']+)' \((?P<id>[\w.\-]+)\) \S+ requires (?P<want>.+?) of (?:mod )?(?:'(?P<depname>[^']+)' \((?P<dep>[\w.\-]+)\)|(?P<dep2>[\w.\-]+)), (?:but only the wrong version is present: (?P<present>[^!\n]+?)!|which is missing!)");
re!(R_INCOMPATIBLE, r"Mod '(?P<name>[^']+)' \((?P<id>[\w.\-]+)\) \S+ is incompatible with .+? of (?:mod )?(?:'(?P<othername>[^']+)' \((?P<other>[\w.\-]+)\)|(?P<other2>[\w.\-]+)), but (?:it|only the wrong version) is present");
re!(R_FORGE_DEP, r"Mod ID: '(?P<dep>[\w.\-]+)', Requested by: '(?P<by>[\w.\-]+)', Expected range: '(?P<range>[^']*)', Actual version: '(?P<actual>[^']*)'");
re!(R_CLIENT_CLASS, r"(?:Cannot load class|[Aa]ttempted to load class|NoClassDefFoundError:|ClassNotFoundException:) ?(?P<cls>net[./]minecraft[./]client[\w./$]*)");
re!(R_MIXIN, r"Mixin apply for mod (?P<id>[\w.\-]+) failed|(?:Critical injection failure|InvalidInjectionException|MixinApplyError|InjectionError).*?from mod (?P<id2>[\w.\-]+)");
re!(R_ENTRYPOINT, r"Could not execute entrypoint stage '\w+' due to errors, provided by '(?P<id>[\w.\-]+)'");
re!(R_JAVA, r"compiled by a more recent version of the Java Runtime \(class file version (?P<need>\d+)(?:\.\d+)?\), this version of the Java Runtime only recognizes class file versions up to (?P<have>\d+)");
re!(R_DATAPACK, r"from pack file/(?P<file>[^\n]+?)\s*$");
re!(R_PLUGIN_DEPS, r"Unknown/missing dependency plugins: \[(?P<deps>[^\]]*)\]\. Please download and install these plugins to run '(?P<plugin>[^']+)'");
re!(R_PLUGIN_LOAD, r"Could not load '?(?:plugins/)?(?P<file>[^'\s]+\.jar)'? in folder '?plugins'?(?P<rest>.*)");
re!(R_SUSPECTED, r"Suspected Mods?: (?P<mods>.+)");
re!(R_SUSPECTED_ONE, r"(?P<name>[^,()]+?) \((?P<id>[\w.\-]+)\)");
re!(R_DUPLICATE, r"(?i)duplicate mods? (?:found|detected)|(?:found|has) (?:more than one|multiple) (?:versions? of|copies of) (?:mod )?'?(?P<id>[\w.\-]+)'?");
re!(R_FRAME, r"^\s*at (?:[\w\-]+//)?(?P<cls>[a-zA-Z_][\w$]*(?:\.[a-zA-Z_][\w$]*)+)\.[\w$<>]+\(");
re!(R_CAUSED, r"^Caused by: (?P<what>.+)$");

/// Packages that belong to Minecraft, Java, the loaders and common libraries.
const NOT_MOD_CODE: [&str; 22] = [
    "java.", "javax.", "jdk.", "sun.", "com.sun.", "net.minecraft.", "com.mojang.", "net.fabricmc.", "org.quiltmc.",
    "net.minecraftforge.", "net.neoforged.", "cpw.mods.", "org.spongepowered.", "com.llamalad7.", "org.objectweb.",
    "io.netty.", "com.google.", "org.apache.", "org.slf4j.", "it.unimi.", "org.bukkit.", "io.papermc.",
];

fn clip(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() > 400 { format!("{}…", line.chars().take(400).collect::<String>()) } else { line.to_string() }
}

pub fn parse(log: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut seen = BTreeSet::new();
    let mut push = |p: &mut Parsed, issue: Issue| {
        let key = format!("{issue:?}").split(", line:").next().unwrap_or_default().to_string();
        if seen.insert(key) {
            p.issues.push(issue);
        }
    };
    let mut frames = Vec::new();
    let mut caused = Vec::new();

    for raw in log.lines() {
        p.lines += 1;
        let line = raw.trim_end();
        // optional-compat mixins that target classes of absent mods are harmless
        if line.contains("@Mixin target") && line.contains("was not found") {
            continue;
        }
        if line.contains("Incompatible mods found")
            || line.contains("Mod resolution failed")
            || line.contains("Missing or unsupported mandatory dependencies")
            || line.contains("Failed to load datapacks, can't proceed")
            || line.contains("---- Minecraft Crash Report ----")
            || line.contains("Exception in server tick loop")
            || line.contains("Encountered an unexpected exception")
            || line.contains("Failed to start the minecraft server")
            || line.contains("This crash report has been saved")
        {
            p.crashed = true;
        }
        if let Some(rest) = line.trim().strip_prefix("Description: ")
            && p.headline.len() < 2
        {
            p.headline.push(clip(rest));
        }

        if let Some(c) = R_REQUIRES.captures(line) {
            let dep = c.name("dep").or(c.name("dep2")).map(|m| m.as_str().to_string()).unwrap_or_default();
            push(&mut p, Issue::Requires {
                name: c["name"].to_string(),
                id: c["id"].to_string(),
                dep_name: c.name("depname").map(|m| m.as_str().to_string()),
                dep,
                wanted: c["want"].to_string(),
                present: c.name("present").map(|m| m.as_str().to_string()),
                line: clip(line),
            });
            p.crashed = true;
        } else if let Some(c) = R_INCOMPATIBLE.captures(line) {
            let other = c.name("other").or(c.name("other2")).map(|m| m.as_str().to_string()).unwrap_or_default();
            push(&mut p, Issue::Incompatible {
                name: c["name"].to_string(),
                id: c["id"].to_string(),
                other_name: c.name("othername").map(|m| m.as_str().to_string()),
                other,
                line: clip(line),
            });
            p.crashed = true;
        } else if let Some(c) = R_FORGE_DEP.captures(line) {
            push(&mut p, Issue::ForgeDependency {
                dep: c["dep"].to_string(),
                requested_by: c["by"].to_string(),
                expected: c["range"].to_string(),
                actual: c["actual"].to_string(),
                line: clip(line),
            });
            p.crashed = true;
        }
        if let Some(c) = R_CLIENT_CLASS.captures(line) {
            push(&mut p, Issue::ClientClass { class: c["cls"].replace('/', "."), line: clip(line) });
        }
        if let Some(c) = R_MIXIN.captures(line) {
            let id = c.name("id").or(c.name("id2")).map(|m| m.as_str().to_string()).unwrap_or_default();
            push(&mut p, Issue::MixinFailed { id, line: clip(line) });
        }
        if let Some(c) = R_ENTRYPOINT.captures(line) {
            push(&mut p, Issue::EntrypointFailed { id: c["id"].to_string(), line: clip(line) });
            p.crashed = true;
        }
        if let Some(c) = R_JAVA.captures(line) {
            let need: u32 = c["need"].parse().unwrap_or(0);
            let have: u32 = c["have"].parse().unwrap_or(0);
            push(&mut p, Issue::Java { needed: need.saturating_sub(44), have: have.saturating_sub(44), line: clip(line) });
            p.crashed = true;
        }
        if line.contains("java.lang.OutOfMemoryError") {
            push(&mut p, Issue::OutOfMemory { line: clip(line) });
            p.crashed = true;
        }
        if line.contains("FAILED TO BIND TO PORT") || line.contains("Address already in use") {
            push(&mut p, Issue::PortInUse { line: clip(line) });
            p.crashed = true;
        }
        if line.contains("You need to agree to the EULA") {
            push(&mut p, Issue::Eula { line: clip(line) });
            p.crashed = true;
        }
        if line.contains("Failed to parse")
            && let Some(c) = R_DATAPACK.captures(line)
        {
            push(&mut p, Issue::Datapack { file: c["file"].trim().to_string(), line: clip(line) });
        }
        if line.contains("Failed to load datapacks") {
            push(&mut p, Issue::DatapacksFailed { line: clip(line) });
        }
        if let Some(c) = R_PLUGIN_DEPS.captures(line) {
            push(&mut p, Issue::PluginMissingDependencies {
                plugin: c["plugin"].to_string(),
                deps: c["deps"].split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect(),
                line: clip(line),
            });
        } else if let Some(c) = R_PLUGIN_LOAD.captures(line) {
            push(&mut p, Issue::PluginLoadFailed {
                file: c["file"].to_string(),
                reason: c["rest"].trim().trim_start_matches(':').trim().to_string(),
                line: clip(line),
            });
        }
        if let Some(c) = R_SUSPECTED.captures(line) {
            let mods: Vec<(String, String)> = R_SUSPECTED_ONE
                .captures_iter(&c["mods"])
                .map(|m| (m["name"].trim().to_string(), m["id"].to_string()))
                .filter(|(_, id)| !matches!(id.as_str(), "minecraft" | "forge" | "neoforge" | "fabricloader" | "java"))
                .collect();
            if !mods.is_empty() {
                push(&mut p, Issue::SuspectedMods { mods, line: clip(line) });
            }
        }
        if let Some(c) = R_DUPLICATE.captures(line) {
            push(&mut p, Issue::DuplicateMod { id: c.name("id").map(|m| m.as_str().to_string()), line: clip(line) });
        }
        if let Some(c) = R_CAUSED.captures(line.trim()) {
            caused.push(clip(&c["what"]));
        }
        if let Some(c) = R_FRAME.captures(line) {
            let cls = c["cls"].to_string();
            if !NOT_MOD_CODE.iter().any(|p| cls.starts_with(p)) && !frames.contains(&cls) && frames.len() < 200 {
                frames.push(cls);
            }
        }
    }
    // the deepest causes say the most
    if p.crashed {
        for c in caused.iter().rev().take(2) {
            if !p.headline.contains(c) {
                p.headline.push(c.clone());
            }
        }
    }
    p.frames = frames;
    p
}

// ------------------------------------------------------------------ jar metadata

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JarMeta {
    pub ids: Vec<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    /// fabric.mod.json environment: client / server / *
    pub environment: Option<String>,
    /// Java package prefixes (3 levels) of the classes in the jar.
    pub packages: Vec<String>,
}

re!(R_TOML_MODID, r#"(?m)^\s*modId\s*=\s*"(?P<id>[^"]+)""#);
re!(R_TOML_NAME, r#"(?m)^\s*displayName\s*=\s*"(?P<name>[^"]+)""#);
re!(R_YML_NAME, r"(?m)^name:\s*'?(?P<name>[^'\r\n]+?)'?\s*$");
re!(R_YML_VERSION, r"(?m)^version:\s*'?(?P<v>[^'\r\n]+?)'?\s*$");

/// Read a jar's mod ids, name, environment and packages (blocking).
pub fn jar_meta(bytes: &[u8]) -> JarMeta {
    let mut meta = JarMeta::default();
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return meta;
    };
    let read = |zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str| -> Option<String> {
        let mut f = zip.by_name(name).ok()?;
        if f.size() > 4 * 1024 * 1024 {
            return None;
        }
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    };
    for file in ["fabric.mod.json", "quilt.mod.json"] {
        if let Some(raw) = read(&mut zip, file)
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
        {
            let root = if file == "quilt.mod.json" { &v["quilt_loader"] } else { &v };
            if let Some(id) = root["id"].as_str() {
                meta.ids.push(id.to_string());
            }
            for p in root["provides"].as_array().into_iter().flatten() {
                if let Some(id) = p.as_str().or(p["id"].as_str()) {
                    meta.ids.push(id.to_string());
                }
            }
            meta.name = meta.name.or(root["name"].as_str().or(root["metadata"]["name"].as_str()).map(String::from));
            meta.version = meta.version.or(root["version"].as_str().map(String::from));
            meta.environment = v["environment"].as_str().map(String::from).or(meta.environment);
        }
    }
    for file in ["META-INF/mods.toml", "META-INF/neoforge.mods.toml"] {
        if let Some(raw) = read(&mut zip, file) {
            meta.ids.extend(R_TOML_MODID.captures_iter(&raw).map(|c| c["id"].to_string()));
            meta.name = meta.name.or(R_TOML_NAME.captures(&raw).map(|c| c["name"].to_string()));
        }
    }
    for file in ["plugin.yml", "paper-plugin.yml", "bungee.yml", "velocity-plugin.json"] {
        if let Some(raw) = read(&mut zip, file) {
            let name = if file.ends_with(".json") {
                serde_json::from_str::<Value>(&raw).ok().and_then(|v| v["id"].as_str().map(String::from))
            } else {
                R_YML_NAME.captures(&raw).map(|c| c["name"].trim().to_string())
            };
            if let Some(name) = name {
                meta.ids.push(name.clone());
                meta.name = meta.name.or(Some(name));
            }
            meta.version = meta.version.or(R_YML_VERSION.captures(&raw).map(|c| c["v"].trim().to_string()));
        }
    }
    let mut packages = BTreeSet::new();
    for i in 0..zip.len() {
        let Ok(name) = zip.name_for_index(i).map(str::to_string).ok_or(()) else { continue };
        if !name.ends_with(".class") || name.starts_with("META-INF/") {
            continue;
        }
        let parts: Vec<&str> = name.split('/').collect();
        if parts.len() >= 4 {
            packages.insert(parts[..3].join("."));
        } else if parts.len() >= 2 {
            packages.insert(parts[..parts.len() - 1].join("."));
        }
        if packages.len() >= 40 {
            break;
        }
    }
    meta.packages = packages.into_iter().collect();
    meta.ids.sort();
    meta.ids.dedup();
    meta
}

/// Metadata of every jar in `dir`, keyed by file name (cached per SHA-1).
async fn jars(ctx: &Ctx, dir: &str, mods: &[Installed]) -> HashMap<String, JarMeta> {
    let jars: Vec<&Installed> = mods.iter().filter(|m| m.file.to_lowercase().contains(".jar")).collect();
    let mut sha1s: HashMap<String, String> = jars.iter().filter_map(|m| m.sha1.clone().map(|h| (m.file.clone(), h))).collect();
    let missing: Vec<String> = jars.iter().filter(|m| m.sha1.is_none()).map(|m| format!("{dir}/{}", m.file)).collect();
    if !missing.is_empty() {
        for (path, h) in ctx.files.fingerprints(&missing, Algorithm::Sha1).await {
            sha1s.insert(path.rsplit('/').next().unwrap_or_default().to_string(), h.to_lowercase());
        }
    }

    stream::iter(sha1s.into_iter())
        .map(|(file, sha1)| async move {
            let meta = ctx
                .cached(&format!("jarmeta:{sha1}"), 30 * 86400, false, async {
                    let bytes = ctx.files.read_bytes(&format!("{dir}/{file}"), MAX_JAR_BYTES).await?.unwrap_or_default();
                    Ok(tokio::task::spawn_blocking(move || jar_meta(&bytes)).await?)
                })
                .await
                .unwrap_or_default();
            (file, meta)
        })
        .buffer_unordered(4)
        .collect()
        .await
}

// ------------------------------------------------------------------ advice

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Switch a file off (adds .disabled).
    Disable { kind: String, file: String, label: String },
    /// Open the version list (update / downgrade).
    Versions { kind: String, provider: Provider, project_id: String, label: String },
    /// Install the newest compatible version.
    Install { kind: String, provider: Provider, project_id: String, label: String },
    /// Nothing matched: search for it.
    Search { kind: String, query: String, label: String },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Finding {
    /// error / warning / info
    pub severity: &'static str,
    pub title: String,
    pub detail: String,
    pub evidence: Vec<String>,
    /// The fix (usually one button).
    pub actions: Vec<Action>,
    /// Other ways to fix it, shown folded away (pick versions by hand, change the dependent mods …).
    pub alternatives: Vec<Action>,
}

struct Server<'a> {
    kind: &'static str,
    mods: &'a [Installed],
    meta: &'a HashMap<String, JarMeta>,
    datapacks: &'a [Installed],
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

impl Server<'_> {
    /// The installed jar that provides a mod id (metadata first, then the project slug / title).
    fn by_id(&self, id: &str) -> Option<&Installed> {
        let n = norm(id);
        self.mods
            .iter()
            .find(|m| self.meta.get(&m.file).is_some_and(|j| j.ids.iter().any(|i| i.eq_ignore_ascii_case(id))))
            .or_else(|| {
                self.mods.iter().find(|m| {
                    m.project.as_ref().is_some_and(|p| norm(&p.slug) == n || norm(&p.title) == n)
                        || self.meta.get(&m.file).and_then(|j| j.name.as_deref()).is_some_and(|name| norm(name) == n)
                })
            })
    }

    fn by_class(&self, class: &str) -> Option<&Installed> {
        self.mods.iter().find(|m| {
            self.meta
                .get(&m.file)
                .is_some_and(|j| j.packages.iter().any(|p| class.starts_with(&format!("{p}."))))
        })
    }

    fn label(&self, m: &Installed) -> String {
        m.project
            .as_ref()
            .map(|p| p.title.clone())
            .or_else(|| self.meta.get(&m.file).and_then(|j| j.name.clone()))
            .unwrap_or_else(|| m.file.clone())
    }

    /// Update / change version and disable, for one installed file.
    fn fix(&self, kind: &str, m: &Installed, versions_label: &str) -> Vec<Action> {
        let mut out = Vec::new();
        if let (Some(provider), Some(project)) = (m.provider, &m.project) {
            out.push(Action::Versions {
                kind: kind.into(),
                provider,
                project_id: project.id.clone(),
                label: format!("{versions_label} {}", project.title),
            });
        }
        if m.enabled {
            out.push(Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {}", self.label(m)) });
        }
        out
    }
}

/// The numbers of the first version in a text: "version 0.92.0 or later" -> [0, 92, 0].
fn version_key(text: &str) -> Option<Vec<u64>> {
    static V: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)*").unwrap());
    let m = V.find(text)?;
    Some(m.as_str().split('.').filter_map(|p| p.parse().ok()).collect())
}

/// Does a requirement ask for a newer (Some(true)) or older (Some(false)) version than `present`?
fn needs_newer(wanted: &str, present: &str) -> Option<bool> {
    let (w, p) = (version_key(wanted)?, version_key(present)?);
    let lower_bound = ["later", "newer", "above", ">="].iter().any(|k| wanted.contains(k))
        || (wanted.starts_with('[') && wanted.trim_end().ends_with(",)"));
    let upper_bound = ["earlier", "older", "below", "<"].iter().any(|k| wanted.contains(k));
    match p.cmp(&w) {
        std::cmp::Ordering::Less if !upper_bound => Some(true),
        std::cmp::Ordering::Greater if !lower_bound => Some(false),
        _ if lower_bound => Some(true),
        _ => None,
    }
}

/// Dependencies the panel can fix by installing or changing one mod (not Minecraft, Java or the loader).
fn groupable(dep: &str) -> bool {
    !matches!(dep, "minecraft" | "java" | "fabricloader" | "quilt_loader" | "forge" | "neoforge")
}

/// A mod that needs a dependency it does not get.
struct Need {
    name: String,
    id: String,
    wanted: String,
    line: String,
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// (dependency id, missing?) -> (label, installed version, the mods that need it)
type Groups = indexmap::IndexMap<(String, bool), (String, Option<String>, Vec<Need>)>;

/// Every mod that needs the same dependency is one problem: fix the dependency once.
fn group_dependencies(parsed: &Parsed, srv: &Server<'_>) -> Groups {
    let mut groups: Groups = indexmap::IndexMap::new();
    for issue in &parsed.issues {
        let (dep, label, present, need) = match issue {
            Issue::Requires { name, id, dep_name, dep, wanted, present, line } if groupable(dep) => (
                dep.clone(),
                dep_name.clone().unwrap_or_else(|| dep.clone()),
                present.clone(),
                Need { name: name.clone(), id: id.clone(), wanted: wanted.clone(), line: line.clone() },
            ),
            Issue::ForgeDependency { dep, requested_by, expected, actual, line } if groupable(dep) => (
                dep.clone(),
                dep.clone(),
                (!actual.contains("MISSING")).then(|| actual.clone()),
                Need {
                    name: srv.by_id(requested_by).map(|m| srv.label(m)).unwrap_or_else(|| requested_by.clone()),
                    id: requested_by.clone(),
                    wanted: expected.clone(),
                    line: line.clone(),
                },
            ),
            _ => continue,
        };
        let entry = groups.entry((dep, present.is_none())).or_insert_with(|| (label, present, Vec::new()));
        if !entry.2.iter().any(|n| n.id == need.id) {
            entry.2.push(need);
        }
    }
    groups
}

/// Resolve mod ids that are not installed to Modrinth projects (ids are usually slugs).
async fn lookup(ctx: &Ctx, ids: &[String]) -> HashMap<String, (Provider, String, String)> {
    let Ok(m) = ctx.modrinth() else { return HashMap::new() };
    let ids: Vec<String> = ids
        .iter()
        .filter(|i| !matches!(i.as_str(), "minecraft" | "java" | "fabricloader" | "forge" | "neoforge" | "quilt_loader"))
        .cloned()
        .collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    let found = m.projects(&ids).await.unwrap_or_default();
    ids.iter()
        .filter_map(|id| {
            found
                .iter()
                .find(|p| p.slug.eq_ignore_ascii_case(id) || p.id == *id)
                .map(|p| (id.clone(), (Provider::Modrinth, p.id.clone(), p.title.clone())))
        })
        .collect()
}

async fn advise(ctx: &Ctx, parsed: &Parsed, srv: &Server<'_>, game_version: Option<&str>) -> Vec<Finding> {
    let kind = srv.kind;
    let mut findings = Vec::new();
    let mut blamed: BTreeSet<String> = BTreeSet::new();

    // ids we may need from Modrinth (missing dependencies)
    let mut wanted: Vec<String> = Vec::new();
    for issue in &parsed.issues {
        match issue {
            Issue::Requires { dep, present: None, .. } if srv.by_id(dep).is_none() => wanted.push(dep.clone()),
            Issue::ForgeDependency { dep, .. } if srv.by_id(dep).is_none() => wanted.push(dep.clone()),
            Issue::PluginMissingDependencies { deps, .. } => wanted.extend(deps.iter().map(|d| d.to_lowercase())),
            _ => {}
        }
    }
    let remote = lookup(ctx, &wanted).await;
    let install_or_search = |id: &str, name: &str, kind: &str| -> Action {
        match remote.get(id).or(remote.get(&id.to_lowercase())) {
            Some((provider, project, title)) => Action::Install {
                kind: kind.into(),
                provider: *provider,
                project_id: project.clone(),
                label: format!("Install {title}"),
            },
            None => Action::Search { kind: kind.into(), query: name.to_string(), label: format!("Search for {name}") },
        }
    };
    let mc = game_version.map(|v| format!("Minecraft {v}")).unwrap_or_else(|| "this Minecraft version".into());

    let groups = group_dependencies(parsed, srv);
    for ((dep, missing), (label, present, needs)) in &groups {
        let installed = srv.by_id(dep);
        let label = installed.map(|m| srv.label(m)).unwrap_or_else(|| label.clone());
        let names: Vec<String> = needs.iter().map(|n| n.name.clone()).collect();
        let who = if needs.len() == 1 { names[0].clone() } else { format!("{} mods", needs.len()) };
        let plural_s = if needs.len() == 1 { "s" } else { "" };
        let evidence: Vec<String> = needs.iter().map(|n| n.line.clone()).take(8).collect();
        // changing a dependent mod instead is the fallback, folded away
        let mut alternatives: Vec<Action> = needs
            .iter()
            .filter_map(|n| srv.by_id(&n.id))
            .filter_map(|m| match (m.provider, &m.project) {
                (Some(provider), Some(project)) => Some(Action::Versions {
                    kind: kind.into(),
                    provider,
                    project_id: project.id.clone(),
                    label: format!("Pick another version of {}", project.title),
                }),
                _ => None,
            })
            .collect();

        if *missing {
            alternatives = needs
                .iter()
                .filter_map(|n| srv.by_id(&n.id))
                .filter(|m| m.enabled)
                .map(|m| Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {}", srv.label(m)) })
                .collect();
            findings.push(Finding {
                severity: "error",
                title: if needs.len() > 1 { format!("{label} is missing (needed by {who})") } else { format!("{label} is missing") },
                detail: format!("{} need{plural_s} {label}, which is not installed.", and_list(&names)),
                evidence,
                actions: vec![install_or_search(dep, &label, kind)],
                alternatives,
            });
            continue;
        }

        let present = present.clone().unwrap_or_default();
        // which way to move: every requirement asks for a newer version -> update
        let directions: Vec<Option<bool>> = needs.iter().map(|n| needs_newer(&n.wanted, &present)).collect();
        let newer = directions.iter().all(|d| *d == Some(true));
        let older = directions.iter().all(|d| *d == Some(false));
        let wants = needs.iter().map(|n| format!("{} needs {}", n.name, n.wanted)).collect::<Vec<_>>().join("; ");
        let project = installed.and_then(|m| m.provider.zip(m.project.clone()));
        let (title, detail, action) = match (project, newer) {
            (Some((provider, project)), true) => (
                format!("{label} is too old for {who}"),
                format!(
                    "{present} is installed. {wants}. Updating {label} fixes {}.",
                    if needs.len() == 1 { "it" } else { "all of them" }
                ),
                Action::Install { kind: kind.into(), provider, project_id: project.id.clone(), label: format!("Update {label}") },
            ),
            (Some((provider, project)), false) => (
                if older { format!("{label} is too new for {who}") } else { format!("{who} need{plural_s} a different {label}") },
                format!("{present} is installed. {wants}. Pick a version of {label} that fits."),
                Action::Versions { kind: kind.into(), provider, project_id: project.id.clone(), label: format!("Pick a version of {label}") },
            ),
            (None, _) => (
                format!("{who} need{plural_s} a different {label}"),
                format!("{present} is installed. {wants}."),
                Action::Search { kind: kind.into(), query: label.clone(), label: format!("Search for {label}") },
            ),
        };
        // the main button updates: picking the dependency's version by hand goes first among the others
        if let Action::Install { kind, provider, project_id, .. } = &action {
            alternatives.insert(0, Action::Versions {
                kind: kind.clone(),
                provider: *provider,
                project_id: project_id.clone(),
                label: format!("Pick a version of {label} by hand"),
            });
        }
        findings.push(Finding { severity: "error", title, detail, evidence, actions: vec![action], alternatives });
    }

    for issue in &parsed.issues {
        match issue {
            Issue::Requires { dep, .. } | Issue::ForgeDependency { dep, .. } if groupable(dep) => {}
            Issue::Requires { name, id, dep_name, dep, wanted, present, line } => {
                let dep_label = dep_name.clone().unwrap_or_else(|| dep.clone());
                if dep == "minecraft" {
                    let mut actions = srv.by_id(id).map(|m| srv.fix(kind, m, "Pick a version of")).unwrap_or_default();
                    if actions.is_empty() {
                        actions.push(Action::Search { kind: kind.into(), query: name.clone(), label: format!("Search for {name}") });
                    }
                    blamed.insert(id.clone());
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: format!("{name} is made for a different Minecraft version"),
                        detail: format!(
                            "{name} needs {wanted} of Minecraft{}. Install a version of {name} made for {mc}, or disable it.",
                            present.as_deref().map(|p| format!(" (this server runs {p})")).unwrap_or_default()
                        ),
                        evidence: vec![line.clone()],
                        actions,
                    });
                } else if dep == "java" {
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: format!("{name} needs a newer Java"),
                        detail: format!(
                            "{name} needs {wanted} of Java{}. Pick a newer Java image under Startup (Docker image).",
                            present.as_deref().map(|p| format!("; the server has {p}")).unwrap_or_default()
                        ),
                        evidence: vec![line.clone()],
                        actions: vec![],
                    });
                } else if matches!(dep.as_str(), "fabricloader" | "quilt_loader") {
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: format!("{name} needs a newer {dep_label}"),
                        detail: format!(
                            "{name} needs {wanted} of {dep_label}{}. Update the loader version in the server's Startup variables, or pick an older {name}.",
                            present.as_deref().map(|p| format!(" (installed: {p})")).unwrap_or_default()
                        ),
                        evidence: vec![line.clone()],
                        actions: srv.by_id(id).map(|m| srv.fix(kind, m, "Pick a version of")).unwrap_or_default(),
                    });
                } else if let Some(present) = present {
                    let mut actions = srv.by_id(dep).map(|m| srv.fix(kind, m, "Change the version of")).unwrap_or_default();
                    actions.retain(|a| !matches!(a, Action::Disable { .. }));
                    if let Some(m) = srv.by_id(id) {
                        actions.extend(srv.fix(kind, m, "Or pick another version of"));
                    }
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: format!("{name} needs a different {dep_label}"),
                        detail: format!("{name} needs {wanted} of {dep_label}, but {present} is installed. Update {dep_label}, or pick a version of {name} that works with {present}."),
                        evidence: vec![line.clone()],
                        actions,
                    });
                } else {
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: format!("{dep_label} is missing"),
                        detail: format!("{name} needs {wanted} of {dep_label}, which is not installed."),
                        evidence: vec![line.clone()],
                        actions: vec![install_or_search(dep, &dep_label, kind)],
                    });
                }
            }
            Issue::Incompatible { name, id, other_name, other, line } => {
                let other_label = other_name.clone().unwrap_or_else(|| other.clone());
                let mut actions = Vec::new();
                for (mod_id, label) in [(id, name.clone()), (other, other_label.clone())] {
                    if let Some(m) = srv.by_id(mod_id)
                        && m.enabled
                    {
                        actions.push(Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {label}") });
                    }
                }
                findings.push(Finding { alternatives: vec![],
                    severity: "error",
                    title: format!("{name} and {other_label} do not work together"),
                    detail: format!("{name} refuses to load next to this version of {other_label}. Disable one of them, or check both for newer versions."),
                    evidence: vec![line.clone()],
                    actions,
                });
            }
            Issue::ForgeDependency { dep, requested_by, expected, actual, line } => {
                let by = srv.by_id(requested_by).map(|m| srv.label(m)).unwrap_or_else(|| requested_by.clone());
                let missing = actual.contains("MISSING");
                let (title, detail, actions) = if dep == "minecraft" || dep == "forge" || dep == "neoforge" {
                    blamed.insert(requested_by.clone());
                    (
                        format!("{by} is made for a different {dep} version"),
                        format!("{by} needs {dep} {expected}, the server has {actual}. Pick a version of {by} for this server, or disable it."),
                        srv.by_id(requested_by).map(|m| srv.fix(kind, m, "Pick a version of")).unwrap_or_default(),
                    )
                } else if missing {
                    (
                        format!("{dep} is missing"),
                        format!("{by} needs {dep} {expected}, which is not installed."),
                        vec![install_or_search(dep, dep, kind)],
                    )
                } else {
                    let mut a = srv.by_id(dep).map(|m| srv.fix(kind, m, "Change the version of")).unwrap_or_default();
                    a.retain(|a| !matches!(a, Action::Disable { .. }));
                    (
                        format!("{by} needs a different {dep}"),
                        format!("{by} needs {dep} {expected}, but {actual} is installed."),
                        a,
                    )
                };
                findings.push(Finding { alternatives: vec![], severity: "error", title, detail, evidence: vec![line.clone()], actions });
            }
            Issue::MixinFailed { id, line } | Issue::EntrypointFailed { id, line } => {
                if !blamed.insert(id.clone()) {
                    continue;
                }
                let client = parsed.issues.iter().any(|i| matches!(i, Issue::ClientClass { .. }));
                let m = srv.by_id(id);
                let name = m.map(|m| srv.label(m)).unwrap_or_else(|| id.clone());
                let client_only = client || m.and_then(|m| srv.meta.get(&m.file)).and_then(|j| j.environment.as_deref()) == Some("client");
                findings.push(Finding { alternatives: vec![],
                    severity: "error",
                    title: if client_only { format!("{name} is a client-side mod") } else { format!("{name} failed to load") },
                    detail: if client_only {
                        format!("{name} tries to use client (graphics / sound) code, which does not exist on a server. Disable it; players can keep it on their own game.")
                    } else {
                        format!("{name} broke while loading. This usually means it does not match {mc} or another mod's version: try another version of {name}, or disable it.")
                    },
                    evidence: vec![line.clone()],
                    actions: m.map(|m| if client_only {
                        vec![Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {name}") }]
                    } else {
                        srv.fix(kind, m, "Pick a version of")
                    }).unwrap_or_default(),
                });
            }
            Issue::Java { needed, have, line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: format!("Java {needed} is needed"),
                detail: format!("Something on this server is built for Java {needed}, but the server runs Java {have}. Choose a Java {needed} (or newer) image under Startup → Docker image."),
                evidence: vec![line.clone()],
                actions: vec![],
            }),
            Issue::OutOfMemory { line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: "The server ran out of memory".into(),
                detail: "Java ran out of memory. Give the server more RAM, or remove heavy mods (world generation, map renderers, pre-generators).".into(),
                evidence: vec![line.clone()],
                actions: vec![],
            }),
            Issue::PortInUse { line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: "The port is already in use".into(),
                detail: "The server could not open its port: another process uses it (often a server that did not shut down). Restart the server, or check server-port in server.properties against the allocation.".into(),
                evidence: vec![line.clone()],
                actions: vec![],
            }),
            Issue::Eula { line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: "The EULA is not accepted".into(),
                detail: "Set eula=true in eula.txt (Files tab) after reading the Minecraft EULA.".into(),
                evidence: vec![line.clone()],
                actions: vec![],
            }),
            Issue::Datapack { file, line } => {
                let name = file.rsplit('/').next().unwrap_or(file);
                let m = srv.datapacks.iter().find(|d| d.file == name || d.file.trim_end_matches(".disabled") == name);
                let title = m.and_then(|m| m.project.as_ref().map(|p| p.title.clone())).unwrap_or_else(|| name.to_string());
                let mut actions = Vec::new();
                if let Some(m) = m {
                    if let (Some(provider), Some(project)) = (m.provider, &m.project) {
                        actions.push(Action::Versions {
                            kind: "datapack".into(),
                            provider,
                            project_id: project.id.clone(),
                            label: format!("Pick a version of {}", project.title),
                        });
                    }
                    if m.enabled {
                        actions.push(Action::Disable { kind: "datapack".into(), file: m.file.clone(), label: format!("Disable {title}") });
                    }
                }
                let fatal = parsed.issues.iter().any(|i| matches!(i, Issue::DatapacksFailed { .. }));
                findings.push(Finding { alternatives: vec![],
                    severity: if fatal { "error" } else { "warning" },
                    title: format!("The data pack {title} does not load"),
                    detail: format!(
                        "Minecraft could not read {title}{}: it is most likely made for another Minecraft version. Install a version made for {mc}, or disable it.",
                        if fatal { ", and refuses to start because of it" } else { "" }
                    ),
                    evidence: parsed.issues.iter().filter_map(|i| match i {
                        Issue::Datapack { file: f, line } if f == file => Some(line.clone()),
                        _ => None,
                    }).take(3).chain(std::iter::once(line.clone())).collect::<BTreeSet<_>>().into_iter().take(4).collect(),
                    actions,
                });
            }
            Issue::DatapacksFailed { line } => {
                if !parsed.issues.iter().any(|i| matches!(i, Issue::Datapack { .. })) {
                    findings.push(Finding { alternatives: vec![],
                        severity: "error",
                        title: "A data pack stops the server from starting".into(),
                        detail: "Minecraft could not load the data packs. Look for \"Failed to parse … from pack\" lines above it, or disable recently added data packs (Content → Data Packs).".into(),
                        evidence: vec![line.clone()],
                        actions: vec![],
                    });
                }
            }
            Issue::PluginMissingDependencies { plugin, deps, line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: format!("{plugin} is missing {}", deps.join(", ")),
                detail: format!("{plugin} only loads when {} {} installed.", deps.join(", "), if deps.len() == 1 { "is" } else { "are" }),
                evidence: vec![line.clone()],
                actions: deps.iter().map(|d| install_or_search(&d.to_lowercase(), d, "plugins")).collect(),
            }),
            Issue::PluginLoadFailed { file, reason, line } => {
                let m = srv.mods.iter().find(|m| m.file == *file);
                findings.push(Finding { alternatives: vec![],
                    severity: "error",
                    title: format!("{} does not load", m.map(|m| srv.label(m)).unwrap_or_else(|| file.clone())),
                    detail: if reason.to_lowercase().contains("api version") {
                        format!("{file} is built for a different server version ({reason}). Pick a version made for {mc}.")
                    } else {
                        format!("{file} failed to load{}.", if reason.is_empty() { String::new() } else { format!(": {reason}") })
                    },
                    evidence: vec![line.clone()],
                    actions: m.map(|m| srv.fix("plugins", m, "Pick a version of")).unwrap_or_default(),
                });
            }
            Issue::SuspectedMods { mods, line } => {
                for (name, id) in mods {
                    if !blamed.insert(id.clone()) {
                        continue;
                    }
                    findings.push(Finding { alternatives: vec![],
                        severity: "warning",
                        title: format!("The crash report suspects {name}"),
                        detail: format!("The crash happened in {name}'s code. Update it, try an older version, or disable it to check."),
                        evidence: vec![line.clone()],
                        actions: srv.by_id(id).map(|m| srv.fix(kind, m, "Pick a version of")).unwrap_or_default(),
                    });
                }
            }
            Issue::DuplicateMod { id, line } => findings.push(Finding { alternatives: vec![],
                severity: "error",
                title: format!("A mod is installed twice{}", id.as_deref().map(|i| format!(" ({i})")).unwrap_or_default()),
                detail: "Two versions of the same mod are in the folder. Remove the older one (Manage tab).".into(),
                evidence: vec![line.clone()],
                actions: vec![],
            }),
            Issue::ClientClass { .. } => {}
        }
    }

    // a client class but nothing named the mod: blame the jar from the stack trace
    let client_line = parsed.issues.iter().find_map(|i| match i {
        Issue::ClientClass { line, .. } => Some(line.clone()),
        _ => None,
    });
    if let Some(line) = client_line
        && !findings.iter().any(|f| f.title.contains("client-side"))
    {
        let culprit = parsed.frames.iter().find_map(|c| srv.by_class(c));
        findings.push(Finding { alternatives: vec![],
            severity: "error",
            title: match culprit {
                Some(m) => format!("{} is a client-side mod", srv.label(m)),
                None => "A client-side mod is installed".into(),
            },
            detail: "A mod tries to use client (graphics / sound) code, which does not exist on a server. Disable it; players can keep it on their own game.".into(),
            evidence: vec![line],
            actions: culprit
                .map(|m| vec![Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {}", srv.label(m)) }])
                .unwrap_or_default(),
        });
    }

    // a crash we have no rule for: point at the mods whose code is in the stack trace
    if parsed.crashed && findings.iter().all(|f| f.severity != "error") {
        let mut seen = BTreeSet::new();
        for class in &parsed.frames {
            let Some(m) = srv.by_class(class) else { continue };
            if !seen.insert(m.file.clone()) || seen.len() > 3 {
                continue;
            }
            findings.push(Finding { alternatives: vec![],
                severity: "warning",
                title: format!("Probably caused by {}", srv.label(m)),
                detail: format!("{}'s code is in the crash's stack trace ({class}). Update it, try an older version, or disable it to check.", srv.label(m)),
                evidence: parsed.headline.clone(),
                actions: srv.fix(kind, m, "Pick a version of"),
            });
        }
    }

    // client-only mods sitting in the folder, crash or not
    for m in srv.mods.iter().filter(|m| m.enabled) {
        if srv.meta.get(&m.file).and_then(|j| j.environment.as_deref()) == Some("client")
            && !findings.iter().any(|f| f.actions.iter().any(|a| matches!(a, Action::Disable { file, .. } if *file == m.file)))
        {
            findings.push(Finding { alternatives: vec![],
                severity: "info",
                title: format!("{} is marked client-only", srv.label(m)),
                detail: format!("{} declares itself client-only (fabric.mod.json), so it does nothing on a server and can break it. Disabling it is safe.", srv.label(m)),
                evidence: vec![format!("{}: environment = client", m.file)],
                actions: vec![Action::Disable { kind: kind.into(), file: m.file.clone(), label: format!("Disable {}", srv.label(m)) }],
            });
        }
    }

    findings
}

// ------------------------------------------------------------------ API

#[derive(Debug, Clone, Serialize)]
struct Source {
    path: String,
    label: String,
    modified: i64,
    size: u64,
}

fn can_read_logs(p: &shared::models::user::PermissionManager) -> bool {
    ["files.read-content", "control.console", crate::ctx::MANAGE_PERMISSION]
        .iter()
        .any(|x| p.has_server_permission(x).is_ok())
}

fn forbidden() -> anyhow::Error {
    shared::response::DisplayError::new("You need \"Read file contents\", \"Console\" or Mod Manager → manage to read the server's logs.")
        .with_status(axum::http::StatusCode::FORBIDDEN)
        .into()
}

/// Server logs and crash reports to pick from, newest first.
pub async fn sources(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    if !can_read_logs(&permissions) {
        return Err(forbidden().into());
    }
    let (logs, crashes) = tokio::join!(ctx.files.list("logs"), ctx.files.list("crash-reports"));
    let mut out: Vec<Source> = Vec::new();
    for e in logs.ok().flatten().unwrap_or_default().into_iter().filter(|e| e.file) {
        if e.name == "latest.log" || e.name.ends_with(".log.gz") || e.name.ends_with(".log") {
            out.push(Source {
                label: if e.name == "latest.log" { "Latest log (latest.log)".into() } else { format!("Log {}", e.name) },
                path: format!("logs/{}", e.name),
                modified: e.modified,
                size: e.size,
            });
        }
    }
    for e in crashes.ok().flatten().unwrap_or_default().into_iter().filter(|e| e.file && e.name.ends_with(".txt")) {
        out.push(Source { label: format!("Crash report {}", e.name), path: format!("crash-reports/{}", e.name), modified: e.modified, size: e.size });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    // latest.log and the newest crash reports first, then a few older logs
    let (mut top, rest): (Vec<Source>, Vec<Source>) =
        out.into_iter().partition(|s| s.path == "logs/latest.log" || s.path.starts_with("crash-reports/"));
    top.truncate(11);
    top.extend(rest.into_iter().take(15));

    ApiResponse::new_serialized(json!({ "sources": top })).ok()
}

fn safe_source(path: &str) -> Result<(), anyhow::Error> {
    let ok = match path.split_once('/') {
        Some(("logs", name)) => name.ends_with(".log") || name.ends_with(".log.gz"),
        Some(("crash-reports", name)) => name.ends_with(".txt"),
        _ => false,
    } && !path.contains("..")
        && path.matches('/').count() == 1;
    if ok { Ok(()) } else { Err(display("Pick a log or crash report from the list.")) }
}

#[derive(Deserialize)]
pub struct AnalyzePayload {
    /// A path from /crash/sources, or …
    source: Option<String>,
    /// … a pasted log.
    text: Option<String>,
}

pub async fn analyze(
    state: GetState,
    permissions: GetPermissionManager,
    user: GetUser,
    server: GetServer,
    Path(_server): Path<String>,
    shared::Payload(data): shared::Payload<AnalyzePayload>,
) -> ApiResponseResult {
    let ctx = Ctx::view(state.0, &permissions, &user, server.0).await?;
    let (text, label) = match (&data.text, &data.source) {
        (Some(t), _) if !t.trim().is_empty() => {
            if t.len() > MAX_PASTE {
                return Err(display("The pasted log is larger than 4 MiB.").into());
            }
            (t.clone(), "pasted log".to_string())
        }
        (_, Some(path)) => {
            if !can_read_logs(&permissions) {
                return Err(forbidden().into());
            }
            safe_source(path)?;
            let Some(bytes) = ctx.files.read_bytes(path, MAX_LOG_BYTES).await? else {
                return Err(display(format!("{path} does not exist any more.")).into());
            };
            let bytes = if path.ends_with(".gz") {
                tokio::task::spawn_blocking(move || -> Result<Vec<u8>, anyhow::Error> {
                    let mut out = Vec::new();
                    flate2::read::GzDecoder::new(&bytes[..]).take(MAX_UNZIPPED_LOG).read_to_end(&mut out)?;
                    Ok(out)
                })
                .await??
            } else {
                bytes
            };
            (String::from_utf8_lossy(&bytes).into_owned(), path.clone())
        }
        _ => return Err(display("Pick a log, or paste one.").into()),
    };

    let parsed = tokio::task::spawn_blocking(move || parse(&text)).await?;

    // what is installed, to turn mod ids into files and projects
    let detected = ctx.detected().await;
    let loader = detected.loader.as_deref().and_then(|l| providers::loader(l).ok());
    let kind: &'static str = if loader.is_some_and(providers::is_plugin_loader) { "plugins" } else { "mods" };
    let mods_target = Resolved { loader, game_version: detected.game_version.clone(), directory: detected.directory.clone() };
    let packs_target = Resolved {
        loader: Some(providers::DATAPACK),
        game_version: detected.game_version.clone(),
        directory: Some(detected.datapack_directory.clone()),
    };
    let (mods, datapacks) = tokio::join!(installed_list(&ctx, &mods_target, false), installed_list(&ctx, &packs_target, false));
    let meta = match &detected.directory {
        Some(dir) => jars(&ctx, dir, &mods.mods).await,
        None => HashMap::new(),
    };
    let srv = Server { kind, mods: &mods.mods, meta: &meta, datapacks: &datapacks.mods };
    let findings = advise(&ctx, &parsed, &srv, detected.game_version.as_deref()).await;

    let summary = match (parsed.crashed, findings.iter().filter(|f| f.severity == "error").count()) {
        (true, 0) => "The server crashed, but none of the known causes matched. Check the highlighted lines below.".to_string(),
        (true, n) => format!("The server crashed. Found {n} likely cause{}.", if n == 1 { "" } else { "s" }),
        (false, 0) if findings.is_empty() => "No crash and no known problems in this log.".to_string(),
        (false, _) => "No crash in this log, but some things need attention.".to_string(),
    };

    ApiResponse::new_serialized(json!({
        "source": label,
        "lines": parsed.lines,
        "crashed": parsed.crashed,
        "summary": summary,
        "headline": parsed.headline,
        "findings": findings,
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 26.2 -> 26.3 upgrade on this panel's Fabric server (logs/2026-10-06-1.log.gz), trimmed.
    const DATAPACK_CRASH: &str = r#"[11:58:25] [main/WARN]: @Mixin target de.siphalor.amecs.key_modifiers.impl.AmecsKeyModifiersEarlyInit was not found jei.mixins.json:AmecsKeyModifiersEarlyInitMixin from mod jei
[11:58:39] [Worker-Main-6/ERROR]: Registry loading errors:
java.lang.IllegalStateException: Failed to parse amplified_nether:barb from pack file/Amplified_Nether_v1.2.15.zip
	at knot//net.minecraft.resources.RegistryDataLoader.loadElementFromResource(RegistryDataLoader.java:404)
Caused by: java.lang.IllegalStateException: Failed to parse either. First: Not a number: {"argument1":{"type":"minecraft:clamp"}}
java.lang.IllegalStateException: Failed to parse amplified_nether:barb2 from pack file/Amplified_Nether_v1.2.15.zip
[11:58:39] [main/WARN]: Failed to load datapacks, can't proceed with server load. You can either fix your datapacks or reset to vanilla with --safeMode
java.util.concurrent.ExecutionException: net.minecraft.ReportedException: Registry Loading
Caused by: java.lang.IllegalStateException: Failed to load registries due to errors"#;

    #[test]
    fn real_datapack_crash() {
        let p = parse(DATAPACK_CRASH);
        assert!(p.crashed);
        assert!(p.issues.contains(&Issue::Datapack {
            file: "Amplified_Nether_v1.2.15.zip".into(),
            line: "java.lang.IllegalStateException: Failed to parse amplified_nether:barb from pack file/Amplified_Nether_v1.2.15.zip".into(),
        }));
        // one finding per file, not per resource
        assert_eq!(p.issues.iter().filter(|i| matches!(i, Issue::Datapack { .. })).count(), 1);
        assert!(p.issues.iter().any(|i| matches!(i, Issue::DatapacksFailed { .. })));
        // the optional-compat mixin warning is not a problem
        assert!(!p.issues.iter().any(|i| matches!(i, Issue::MixinFailed { .. })));
        assert!(p.headline.iter().any(|h| h.contains("Failed to load registries")));
    }

    #[test]
    fn fabric_dependencies() {
        let log = "\
[main/ERROR]: Incompatible mods found!
\t - Mod 'Mod Menu' (modmenu) 9.0.0 requires version 0.92.0 or later of mod 'Fabric API' (fabric-api), but only the wrong version is present: 0.91.0!
\t - Mod 'Lithium' (lithium) 0.12.1 requires any version of fabric-api, which is missing!
\t - Mod 'Sodium' (sodium) 0.5.3 requires version 1.20.1 of 'Minecraft' (minecraft), but only the wrong version is present: 1.20.2!
\t - Mod 'Iris' (iris) 1.6 is incompatible with any version of mod 'OptiFabric' (optifabric), but it is present!";
        let p = parse(log);
        assert!(p.crashed);
        let reqs: Vec<_> = p.issues.iter().filter_map(|i| match i {
            Issue::Requires { id, dep, present, wanted, .. } => Some((id.as_str(), dep.as_str(), present.clone(), wanted.as_str())),
            _ => None,
        }).collect();
        assert_eq!(reqs, vec![
            ("modmenu", "fabric-api", Some("0.91.0".to_string()), "version 0.92.0 or later"),
            ("lithium", "fabric-api", None, "any version"),
            ("sodium", "minecraft", Some("1.20.2".to_string()), "version 1.20.1"),
        ]);
        assert!(p.issues.iter().any(|i| matches!(i, Issue::Incompatible { id, other, .. } if id == "iris" && other == "optifabric")));
    }

    #[test]
    fn forge_and_misc() {
        let log = "\
Missing or unsupported mandatory dependencies:
\tMod ID: 'architectury', Requested by: 'roughlyenoughitems', Expected range: '[9.1.12,)', Actual version: '[MISSING]'
\tMod ID: 'minecraft', Requested by: 'jei', Expected range: '[1.20.1,1.20.2)', Actual version: '1.20.4'
java.lang.UnsupportedClassVersionError: x has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0
java.lang.RuntimeException: Cannot load class net.minecraft.client.gui.screen.Screen in environment type SERVER
Could not execute entrypoint stage 'main' due to errors, provided by 'xaerominimap' at 'xaero.common.Main'!
[Server thread/ERROR]: Unknown/missing dependency plugins: [Vault, ProtocolLib]. Please download and install these plugins to run 'EssentialsX'.
Exception in thread \"main\" java.lang.OutOfMemoryError: Java heap space
**** FAILED TO BIND TO PORT!
Suspected Mods: Create (create), Version: 0.5.1; Minecraft (minecraft)";
        let p = parse(log);
        assert!(p.issues.iter().any(|i| matches!(i, Issue::ForgeDependency { dep, actual, .. } if dep == "architectury" && actual == "[MISSING]")));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::ForgeDependency { dep, requested_by, .. } if dep == "minecraft" && requested_by == "jei")));
        assert!(p.issues.contains(&Issue::Java { needed: 21, have: 17, line: clip("java.lang.UnsupportedClassVersionError: x has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0") }));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::ClientClass { class, .. } if class == "net.minecraft.client.gui.screen.Screen")));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::EntrypointFailed { id, .. } if id == "xaerominimap")));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::PluginMissingDependencies { plugin, deps, .. } if plugin == "EssentialsX" && deps == &vec!["Vault".to_string(), "ProtocolLib".to_string()])));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::OutOfMemory { .. })));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::PortInUse { .. })));
        assert!(p.issues.iter().any(|i| matches!(i, Issue::SuspectedMods { mods, .. } if mods == &vec![("Create".to_string(), "create".to_string())])));
    }

    #[test]
    fn frames_skip_game_code() {
        let p = parse("---- Minecraft Crash Report ----\n\tat net.minecraft.server.Main.main(Main.java:1)\n\tat knot//me.jellysquid.mods.lithium.Foo.bar(Foo.java:2)\n\tat java.base/java.lang.Thread.run(Thread.java:3)");
        assert_eq!(p.frames, vec!["me.jellysquid.mods.lithium.Foo"]);
    }

    #[test]
    fn jar_metadata() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("fabric.mod.json", o).unwrap();
            z.write_all(br#"{"id":"sodium","name":"Sodium","version":"0.9","environment":"client","provides":["indium"]}"#).unwrap();
            z.start_file("net/caffeinemc/mods/sodium/Sodium.class", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let m = jar_meta(&buf.into_inner());
        assert_eq!(m.ids, vec!["indium", "sodium"]);
        assert_eq!(m.environment.as_deref(), Some("client"));
        assert_eq!(m.packages, vec!["net.caffeinemc.mods"]);
    }

    /// MM_LOGS=/dir/with/logs cargo test … -- --ignored real_logs --nocapture
    #[test]
    #[ignore]
    fn real_logs() {
        let dir = std::env::var("MM_LOGS").expect("MM_LOGS");
        let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap();
            let text = if path.extension().is_some_and(|e| e == "gz") {
                let mut out = String::new();
                flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut out).unwrap_or_default();
                out
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            let p = parse(&text);
            let kinds: Vec<String> = p.issues.iter().map(|i| format!("{i:?}").split([' ', '{']).next().unwrap_or_default().to_string()).collect();
            println!("{:<28} lines={:<6} crashed={:<5} issues={:?}", path.file_name().unwrap().to_string_lossy(), p.lines, p.crashed, kinds);
        }
    }

    #[test]
    fn version_direction() {
        assert_eq!(needs_newer("version 0.92.0 or later", "0.91.0"), Some(true));
        assert_eq!(needs_newer("version 0.161.0+26.3 or later", "0.155.2+26.2"), Some(true));
        assert_eq!(needs_newer("[9.1.12,)", "9.0.0"), Some(true));
        assert_eq!(needs_newer("version 0.91.0 or earlier", "0.92.0"), Some(false));
        assert_eq!(needs_newer("version 1.2.0", "1.3.0"), Some(false));
        assert_eq!(needs_newer("any version", "1.0"), None);
    }

    /// The user's test: Fabric API downgraded, seven mods need a newer one -> one problem.
    #[test]
    fn one_problem_per_dependency() {
        let mut log = String::from("[main/ERROR]: Incompatible mods found!\n");
        for (name, id) in [("Lithium", "lithium"), ("Ledger", "ledger"), ("Balm", "balm"), ("Collective", "collective"),
            ("BlueMap", "bluemap"), ("Fixed Anvil Repair Cost", "fixedanvilrepaircost"), ("NetherPortalFix", "netherportalfix")]
        {
            log.push_str(&format!(
                "\t - Mod '{name}' ({id}) 1.0 requires version 0.161.0 or later of mod 'Fabric API' (fabric-api), but only the wrong version is present: 0.155.2+26.2!\n"
            ));
        }
        log.push_str("\t - Mod 'Mod Menu' (modmenu) 9.0 requires any version of cloth-config, which is missing!\n");
        log.push_str("\t - Mod 'Sodium Extra' (sodium-extra) 1.0 requires any version of cloth-config, which is missing!\n");
        let parsed = parse(&log);
        let meta = HashMap::new();
        let srv = Server { kind: "mods", mods: &[], meta: &meta, datapacks: &[] };
        let groups = group_dependencies(&parsed, &srv);
        assert_eq!(groups.len(), 2);
        let (label, present, needs) = &groups[&("fabric-api".to_string(), false)];
        assert_eq!(label, "Fabric API");
        assert_eq!(present.as_deref(), Some("0.155.2+26.2"));
        assert_eq!(needs.len(), 7);
        assert!(needs.iter().all(|n| needs_newer(&n.wanted, present.as_deref().unwrap()) == Some(true)));
        assert_eq!(groups[&("cloth-config".to_string(), true)].2.len(), 2);
        assert_eq!(and_list(&["A".into(), "B".into(), "C".into()]), "A, B and C");
    }

    #[test]
    fn sources_are_confined() {
        assert!(safe_source("logs/latest.log").is_ok());
        assert!(safe_source("logs/2026-10-06-1.log.gz").is_ok());
        assert!(safe_source("crash-reports/crash-2026.txt").is_ok());
        assert!(safe_source("logs/../server.properties").is_err());
        assert!(safe_source("world/level.dat").is_err());
        assert!(safe_source("logs/sub/x.log").is_err());
    }
}
