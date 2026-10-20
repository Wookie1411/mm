//! CurseForge for Studios API v1 (https://docs.curseforge.com). Needs an API key from
//! console.curseforge.com. Authors can opt out of third-party downloads: those files
//! come back without a download URL and are shown as "download on CurseForge".

use super::{Dependency, FileInfo, Identified, Link, Project, ProjectDetail, Provider, SearchPage, SearchQuery, Version};
use crate::ctx::display;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;

const API: &str = "https://api.curseforge.com/v1";
const MINECRAFT: u32 = 432;
const CLASS_MODS: u32 = 6;
const CLASS_BUKKIT_PLUGINS: u32 = 5;
const CLASS_DATA_PACKS: u32 = 6945;
pub const CLASS_MODPACKS: u32 = 4471;
/// The API refuses index + pageSize beyond this.
pub const MAX_RESULTS: u64 = 10_000;

pub struct CurseForge<'a> {
    pub client: &'a reqwest::Client,
    pub key: &'a str,
}

/// CurseForge's ModLoaderType for a loader; `None` for plugin platforms.
fn loader_type(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn loader_name(t: u64) -> Option<&'static str> {
    match t {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

/// Which class (mods / Bukkit plugins) to search for a loader; `None` = not on CurseForge.
fn class_for(loader: Option<&str>) -> Result<u32, anyhow::Error> {
    match loader {
        None | Some("fabric" | "quilt" | "forge" | "neoforge") => Ok(CLASS_MODS),
        Some("paper" | "purpur" | "spigot" | "bukkit") => Ok(CLASS_BUKKIT_PLUGINS),
        Some(super::DATAPACK) => Ok(CLASS_DATA_PACKS),
        Some(other) => Err(display(format!(
            "CurseForge has no {other} section. Use Modrinth for {other} plugins."
        ))),
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn project(m: &Value) -> Project {
    let slug = s(&m["slug"]);
    let class = if m["classId"].as_u64() == Some(CLASS_BUKKIT_PLUGINS as u64) {
        "bukkit-plugins"
    } else {
        "mc-mods"
    };
    Project {
        provider: Provider::Curseforge,
        id: m["id"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
        url: m["links"]["websiteUrl"]
            .as_str()
            .filter(|u| u.starts_with("http"))
            .map(String::from)
            .unwrap_or_else(|| format!("https://www.curseforge.com/minecraft/{class}/{slug}")),
        slug,
        title: s(&m["name"]),
        description: s(&m["summary"]),
        author: m["authors"][0]["name"].as_str().map(String::from),
        downloads: m["downloadCount"].as_f64().unwrap_or(0.0) as u64,
        follows: m["thumbsUpCount"].as_u64().unwrap_or(0),
        icon_url: m["logo"]["thumbnailUrl"]
            .as_str()
            .or(m["logo"]["url"].as_str())
            .filter(|u| !u.is_empty())
            .map(String::from),
        categories: m["categories"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| c["name"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        updated: m["dateModified"].as_str().map(String::from),
        server_side: None,
    }
}

pub fn version(f: &Value) -> Version {
    let game_versions: Vec<String> = f["gameVersions"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    // gameVersions mixes Minecraft versions with "Fabric", "Server", "Java 21" …
    let (loaders, game_versions): (Vec<String>, Vec<String>) = game_versions
        .into_iter()
        .partition(|v| !v.starts_with(|c: char| c.is_ascii_digit()));
    let loaders = loaders
        .into_iter()
        .map(|l| l.to_lowercase())
        .filter(|l| super::LOADERS.contains(&l.as_str()))
        .collect();
    let sha1 = f["hashes"]
        .as_array()
        .and_then(|h| h.iter().find(|h| h["algo"].as_u64() == Some(1)))
        .and_then(|h| h["value"].as_str())
        .map(|h| h.to_lowercase());
    Version {
        provider: Provider::Curseforge,
        id: f["id"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
        project_id: f["modId"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
        name: s(&f["displayName"]),
        version_number: s(&f["displayName"]),
        version_type: match f["releaseType"].as_u64() {
            Some(2) => "beta",
            Some(3) => "alpha",
            _ => "release",
        }
        .to_string(),
        game_versions,
        loaders,
        published: f["fileDate"].as_str().map(String::from),
        downloads: f["downloadCount"].as_f64().unwrap_or(0.0) as u64,
        file: Some(FileInfo {
            name: s(&f["fileName"]),
            size: f["fileLength"].as_u64().unwrap_or(0),
            url: f["downloadUrl"].as_str().filter(|u| u.starts_with("http")).map(String::from),
            sha1,
        }),
        dependencies: f["dependencies"]
            .as_array()
            .map(|deps| {
                deps.iter()
                    .map(|d| Dependency {
                        project_id: d["modId"].as_u64().map(|i| i.to_string()),
                        version_id: None,
                        kind: match d["relationType"].as_u64() {
                            Some(3) => "required",
                            Some(5) => "incompatible",
                            Some(1) => "embedded",
                            _ => "optional",
                        }
                        .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl CurseForge<'_> {
    async fn send<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T, anyhow::Error> {
        let request = request
            .header("x-api-key", self.key)
            .header(reqwest::header::ACCEPT, "application/json")
            .build()
            .map_err(|e| display(format!("Could not build the CurseForge request: {e}")))?;
        let url = request.url().clone();
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|e| display(format!("Could not reach CurseForge: {e}")))?;
        let status = response.status();
        match status {
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::UNAUTHORIZED => {
                let blocked = is_cloudflare_block(response.headers());
                let body = response.text().await.unwrap_or_default();
                tracing::warn!(
                    "modmanager: curseforge HTTP {status} for {} (cloudflare block: {blocked}, key {} / {} chars): {}",
                    url.path(),
                    key_id(self.key),
                    self.key.len(),
                    body.chars().take(300).collect::<String>()
                );
                return Err(display(if blocked {
                    "CurseForge's firewall blocked this server's IP address (not a key problem). Try again later."
                } else {
                    "CurseForge rejected the API key. An administrator can check it with \"Test key\" under Admin > Extensions > Mod Manager."
                }));
            }
            reqwest::StatusCode::NOT_FOUND => return Err(display("CurseForge does not know this project or file.")),
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                return Err(display("CurseForge is rate limiting this panel. Try again in a minute."));
            }
            s if !s.is_success() => {
                let body = response.text().await.unwrap_or_default();
                tracing::warn!("modmanager: curseforge HTTP {status}: {}", body.chars().take(300).collect::<String>());
                return Err(display(format!("CurseForge answered HTTP {}.", status.as_u16())));
            }
            _ => {}
        }

        response
            .json()
            .await
            .map_err(|e| display(format!("Unexpected answer from CurseForge: {e}")))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T, anyhow::Error> {
        self.send(self.client.get(format!("{API}{path}")).query(query)).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, anyhow::Error> {
        self.send(self.client.post(format!("{API}{path}")).json(body)).await
    }

    pub async fn search(&self, q: &SearchQuery) -> Result<SearchPage, anyhow::Error> {
        let limit = q.limit.min(50);
        let offset = q.offset.min(MAX_RESULTS.saturating_sub(limit));
        // ModsSearchSortField: 1 featured, 2 popularity, 3 last updated, 6 total downloads, 11 released
        let sort = match q.sort.as_str() {
            "downloads" => "6",
            "follows" => "2",
            "newest" => "11",
            "updated" => "3",
            _ => "1",
        };
        let mut query = vec![
            ("gameId", MINECRAFT.to_string()),
            ("classId", if q.modpacks { CLASS_MODPACKS } else { class_for(q.loader)? }.to_string()),
            ("sortField", sort.to_string()),
            ("sortOrder", "desc".to_string()),
            ("index", offset.to_string()),
            ("pageSize", limit.to_string()),
        ];
        if !q.query.trim().is_empty() {
            query.push(("searchFilter", q.query.trim().to_string()));
        }
        if let Some(t) = q.loader.and_then(loader_type) {
            query.push(("modLoaderType", t.to_string()));
        }
        if let Some(v) = &q.game_version {
            query.push(("gameVersion", v.clone()));
        }
        let raw: Value = self.get("/mods/search", &query).await?;

        Ok(SearchPage {
            hits: raw["data"].as_array().map(|h| h.iter().map(project).collect()).unwrap_or_default(),
            total: raw["pagination"]["totalCount"].as_u64().unwrap_or(0).min(MAX_RESULTS),
            offset,
            limit,
        })
    }

    pub async fn project(&self, id: &str) -> Result<ProjectDetail, anyhow::Error> {
        let raw: Value = self.get(&format!("/mods/{id}"), &[]).await?;
        let m = &raw["data"];
        let mut links = Vec::new();
        for (key, label) in [("sourceUrl", "Source"), ("issuesUrl", "Issues"), ("wikiUrl", "Wiki")] {
            if let Some(url) = m["links"][key].as_str().filter(|u| u.starts_with("http")) {
                links.push(Link { label: label.into(), url: url.into() });
            }
        }
        let indexes = m["latestFilesIndexes"].as_array().cloned().unwrap_or_default();
        let mut game_versions: Vec<String> = Vec::new();
        let mut loaders: Vec<String> = Vec::new();
        for i in &indexes {
            if let Some(v) = i["gameVersion"].as_str()
                && !game_versions.iter().any(|x| x == v)
            {
                game_versions.push(v.to_string());
            }
            if let Some(l) = i["modLoader"].as_u64().and_then(loader_name)
                && !loaders.iter().any(|x| x == l)
            {
                loaders.push(l.to_string());
            }
        }

        Ok(ProjectDetail {
            project: project(m),
            body: None,
            links,
            gallery: m["screenshots"]
                .as_array()
                .map(|g| g.iter().filter_map(|i| i["url"].as_str().map(String::from)).collect())
                .unwrap_or_default(),
            loaders,
            game_versions,
            distribution_blocked: m["allowModDistribution"].as_bool() == Some(false),
        })
    }

    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>, anyhow::Error> {
        let ids: Vec<u64> = ids.iter().filter_map(|i| i.parse().ok()).collect();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let raw: Value = self.post("/mods", &json!({ "modIds": ids, "filterPcOnly": true })).await?;
        Ok(raw["data"].as_array().map(|d| d.iter().map(project).collect()).unwrap_or_default())
    }

    /// Projects with their CurseForge class id (6 mods, 12 resource packs, 6945 data packs …).
    pub async fn projects_with_class(&self, ids: &[String]) -> Result<Vec<(Project, u64)>, anyhow::Error> {
        let ids: Vec<u64> = ids.iter().filter_map(|i| i.parse().ok()).collect();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let raw: Value = self.post("/mods", &json!({ "modIds": ids, "filterPcOnly": true })).await?;
        Ok(raw["data"]
            .as_array()
            .map(|d| d.iter().map(|m| (project(m), m["classId"].as_u64().unwrap_or(0))).collect())
            .unwrap_or_default())
    }

    /// Files of a project, newest first, filtered to what the server can load.
    pub async fn versions(
        &self,
        project: &str,
        loader: Option<&str>,
        game_version: Option<&str>,
    ) -> Result<Vec<Version>, anyhow::Error> {
        let mut out = Vec::new();
        let mut index = 0u64;
        // a few pages are plenty for picking a version; very old files stay reachable on the site
        while index < 400 {
            let mut query = vec![("index", index.to_string()), ("pageSize", "50".to_string())];
            if let Some(t) = loader.and_then(loader_type) {
                query.push(("modLoaderType", t.to_string()));
            }
            if let Some(v) = game_version {
                query.push(("gameVersion", v.to_string()));
            }
            let raw: Value = self.get(&format!("/mods/{project}/files"), &query).await?;
            let page = raw["data"].as_array().cloned().unwrap_or_default();
            let n = page.len() as u64;
            out.extend(page.iter().map(version));
            let total = raw["pagination"]["totalCount"].as_u64().unwrap_or(0);
            index += n;
            if n == 0 || index >= total {
                break;
            }
        }
        out.sort_by(|a, b| b.published.cmp(&a.published));
        Ok(out)
    }

    pub async fn version(&self, project: &str, id: &str) -> Result<Version, anyhow::Error> {
        let raw: Value = self.get(&format!("/mods/{project}/files/{id}"), &[]).await?;
        Ok(version(&raw["data"]))
    }

    pub async fn versions_by_id(&self, ids: &[String]) -> Result<Vec<Version>, anyhow::Error> {
        let ids: Vec<u64> = ids.iter().filter_map(|i| i.parse().ok()).collect();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let raw: Value = self.post("/mods/files", &json!({ "fileIds": ids })).await?;
        Ok(raw["data"].as_array().map(|d| d.iter().map(version).collect()).unwrap_or_default())
    }

    /// Raw file objects (for fields the normalised [`Version`] drops, like Client / Server tags).
    pub async fn files_raw(&self, ids: &[u64]) -> Result<Vec<Value>, anyhow::Error> {
        let mut out = Vec::new();
        for chunk in ids.chunks(200) {
            let raw: Value = self.post("/mods/files", &json!({ "fileIds": chunk })).await?;
            out.extend(raw["data"].as_array().cloned().unwrap_or_default());
        }
        Ok(out)
    }

    /// Identify jars by CurseForge fingerprint (MurmurHash2 of the jar without whitespace).
    pub async fn identify(&self, fingerprints: &[u32]) -> Result<HashMap<u32, (Identified, Version)>, anyhow::Error> {
        if fingerprints.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: Value = self
            .post(&format!("/fingerprints/{MINECRAFT}"), &json!({ "fingerprints": fingerprints }))
            .await?;
        let mut out = HashMap::new();
        for m in raw["data"]["exactMatches"].as_array().cloned().unwrap_or_default() {
            let file = &m["file"];
            let Some(fp) = file["fileFingerprint"].as_u64() else { continue };
            let v = version(file);
            out.insert(
                fp as u32,
                (
                    Identified {
                        provider: Provider::Curseforge,
                        project_id: v.project_id.clone(),
                        version_id: v.id.clone(),
                    },
                    v,
                ),
            );
        }
        Ok(out)
    }

    /// The newest file id per project that matches the loader and Minecraft version,
    /// from the projects' latest-files index. Only releases unless `any_type`.
    pub async fn latest_for(
        &self,
        projects: &[String],
        loader: Option<&str>,
        game_version: Option<&str>,
        any_type: &HashMap<String, bool>,
    ) -> Result<HashMap<String, String>, anyhow::Error> {
        let ids: Vec<u64> = projects.iter().filter_map(|i| i.parse().ok()).collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: Value = self.post("/mods", &json!({ "modIds": ids, "filterPcOnly": true })).await?;
        let wanted_loader = loader.and_then(loader_type);
        let mut out = HashMap::new();
        for m in raw["data"].as_array().cloned().unwrap_or_default() {
            let Some(id) = m["id"].as_u64().map(|i| i.to_string()) else { continue };
            let allow_pre = any_type.get(&id).copied().unwrap_or(false);
            let best = m["latestFilesIndexes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| game_version.is_none_or(|v| i["gameVersion"].as_str() == Some(v)))
                .filter(|i| match (wanted_loader, i["modLoader"].as_u64()) {
                    (Some(want), Some(have)) => want as u64 == have,
                    (Some(_), None) => false,
                    (None, _) => true,
                })
                .filter(|i| allow_pre || i["releaseType"].as_u64() == Some(1))
                .filter_map(|i| i["fileId"].as_u64())
                .max();
            if let Some(file) = best {
                out.insert(id, file.to_string());
            }
        }
        Ok(out)
    }
}

/// A 403 from Cloudflare's bot protection in front of the API, rather than from the API itself.
pub fn is_cloudflare_block(headers: &reqwest::header::HeaderMap) -> bool {
    headers.contains_key("cf-mitigated")
        || headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
}

/// Clean up a pasted key: whitespace and surrounding quotes.
pub fn clean_key(raw: &str) -> String {
    raw.trim().trim_matches(['"', '\'', '`']).trim().to_string()
}

/// What a key looks like, without revealing it.
pub fn key_shape(key: &str) -> serde_json::Value {
    json!({
        "length": key.len(),
        // keys from console.curseforge.com are bcrypt-style: $2a$10$ + 53 characters
        "looks_like_a_key": key.len() == 60 && key.starts_with("$2a$10$"),
        "has_whitespace": key.chars().any(char::is_whitespace),
    })
}

/// Call a harmless endpoint with `key`; (ok, HTTP status, explanation).
/// A short one-way fingerprint of a key, so logs can tell keys apart without revealing them.
pub fn key_id(key: &str) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(key.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Call the game endpoint and the mod search the page uses with `key`; (ok, HTTP status, explanation).
pub async fn test_key(client: &reqwest::Client, key: &str) -> (bool, Option<u16>, String) {
    let (ok, status, message) = test_url(client, key, &format!("{API}/games/{MINECRAFT}")).await;
    let search = format!("{API}/mods/search?gameId={MINECRAFT}&classId={CLASS_MODS}&pageSize=1");
    let (search_ok, search_status, search_message) = test_url(client, key, &search).await;
    tracing::info!(
        "modmanager: curseforge key test (key {} / {} chars): game {status:?}, mod search {search_status:?}",
        key_id(key),
        key.len()
    );
    match (ok, search_ok) {
        (true, true) => (true, status, format!("CurseForge accepted the key (key id {}).", key_id(key))),
        (true, false) => (false, search_status, format!("The key works for game info but not for the mod search: {search_message}")),
        _ => (false, status, message),
    }
}

async fn test_url(client: &reqwest::Client, key: &str, url: &str) -> (bool, Option<u16>, String) {
    let response = match client
        .get(url)
        .header("x-api-key", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (false, None, format!("Could not reach api.curseforge.com: {e}")),
    };
    let status = response.status();
    let blocked = is_cloudflare_block(response.headers());
    let body: String = response.text().await.unwrap_or_default().chars().take(200).collect();
    let message = match status.as_u16() {
        200 => "CurseForge accepted the key.".to_string(),
        _ if blocked => "Cloudflare (in front of CurseForge) blocked this server's IP address; the key itself was not checked.".to_string(),
        401 | 403 => format!(
            "CurseForge rejected the key (HTTP {}). Copy it again from console.curseforge.com → API keys; a regenerated key replaces the old one.",
            status.as_u16()
        ),
        code => format!("CurseForge answered HTTP {code}: {body}"),
    };
    (status.is_success(), Some(status.as_u16()), message)
}

/// Wings reports the CurseForge fingerprint as a decimal (or hex) string.
pub fn parse_fingerprint(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    raw.parse::<u32>()
        .ok()
        .or_else(|| u32::from_str_radix(raw.trim_start_matches("0x"), 16).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_file() {
        let f = json!({
            "id": 5000001, "modId": 238222, "displayName": "JEI 15.2.0.27", "fileName": "jei-1.20.1-forge-15.2.0.27.jar",
            "releaseType": 2, "fileDate": "2026-01-01T00:00:00Z", "fileLength": 1234, "downloadCount": 10.0,
            "downloadUrl": null, "gameVersions": ["1.20.1", "Forge", "Server", "NeoForge"],
            "hashes": [{"value": "ABCDEF", "algo": 1}, {"value": "123", "algo": 2}],
            "dependencies": [{"modId": 1, "relationType": 3}, {"modId": 2, "relationType": 2}],
        });
        let v = version(&f);
        assert_eq!(v.version_type, "beta");
        assert_eq!(v.game_versions, vec!["1.20.1"]);
        assert_eq!(v.loaders, vec!["forge", "neoforge"]);
        let file = v.file.unwrap();
        assert!(file.url.is_none());
        assert_eq!(file.sha1.as_deref(), Some("abcdef"));
        assert_eq!(v.dependencies[0].kind, "required");
        assert_eq!(v.dependencies[1].kind, "optional");
    }

    #[test]
    fn keys() {
        assert_eq!(clean_key("  \"$2a$10$abc\"\n"), "$2a$10$abc");
        let k = format!("$2a$10${}", "x".repeat(53));
        assert_eq!(key_shape(&k)["looks_like_a_key"], true);
        assert_eq!(key_shape("$2a$10$short")["looks_like_a_key"], false);
    }

    #[test]
    fn fingerprints() {
        assert_eq!(parse_fingerprint("3608196219"), Some(3608196219));
        assert_eq!(parse_fingerprint("d711f37b"), Some(0xd711f37b));
    }
}
