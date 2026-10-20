//! Modrinth API v2 (https://docs.modrinth.com/api). No key needed; requests carry a
//! descriptive User-Agent as Modrinth asks.

use super::{Dependency, FileInfo, Identified, Link, Project, ProjectDetail, Provider, SearchPage, SearchQuery, Version};
use crate::ctx::display;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;

const API: &str = "https://api.modrinth.com/v2";
pub struct Modrinth<'a> {
    pub client: &'a reqwest::Client,
    pub user_agent: &'a str,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn project_url(project_type: &str, slug: &str) -> String {
    let kind = match project_type {
        "plugin" | "mod" | "datapack" | "modpack" | "resourcepack" | "shader" => project_type,
        _ => "mod",
    };
    format!("https://modrinth.com/{kind}/{slug}")
}

fn hit(h: &Value) -> Project {
    let slug = s(&h["slug"]);
    Project {
        provider: Provider::Modrinth,
        id: s(&h["project_id"]),
        url: project_url(h["project_type"].as_str().unwrap_or("mod"), &slug),
        slug,
        title: s(&h["title"]),
        description: s(&h["description"]),
        author: h["author"].as_str().map(String::from),
        downloads: h["downloads"].as_u64().unwrap_or(0),
        follows: h["follows"].as_u64().unwrap_or(0),
        icon_url: h["icon_url"].as_str().filter(|u| !u.is_empty()).map(String::from),
        categories: strings(&h["display_categories"]),
        updated: h["date_modified"].as_str().map(String::from),
        server_side: h["server_side"].as_str().map(String::from),
    }
}

fn project(p: &Value) -> Project {
    let slug = s(&p["slug"]);
    Project {
        provider: Provider::Modrinth,
        id: s(&p["id"]),
        url: project_url(p["project_type"].as_str().unwrap_or("mod"), &slug),
        slug,
        title: s(&p["title"]),
        description: s(&p["description"]),
        author: None,
        downloads: p["downloads"].as_u64().unwrap_or(0),
        follows: p["followers"].as_u64().unwrap_or(0),
        icon_url: p["icon_url"].as_str().filter(|u| !u.is_empty()).map(String::from),
        categories: strings(&p["categories"]),
        updated: p["updated"].as_str().map(String::from),
        server_side: p["server_side"].as_str().map(String::from),
    }
}

pub fn version(v: &Value) -> Version {
    version_with(v, false)
}

/// `zip`: pick the version's .zip (data pack) file, never the primary jar of a mod + data
/// pack project; no .zip = no file.
pub fn version_with(v: &Value, zip: bool) -> Version {
    let files: Vec<Value> = v["files"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !zip || f["filename"].as_str().is_some_and(|n| n.to_lowercase().ends_with(".zip")))
        .collect();
    let file = files
        .iter()
        .find(|f| f["primary"].as_bool() == Some(true))
        .or_else(|| files.first())
        .map(|f| FileInfo {
            name: s(&f["filename"]),
            size: f["size"].as_u64().unwrap_or(0),
            url: f["url"].as_str().map(String::from),
            sha1: f["hashes"]["sha1"].as_str().map(String::from),
        });
    Version {
        provider: Provider::Modrinth,
        id: s(&v["id"]),
        project_id: s(&v["project_id"]),
        name: s(&v["name"]),
        version_number: s(&v["version_number"]),
        version_type: v["version_type"].as_str().unwrap_or("release").to_string(),
        game_versions: strings(&v["game_versions"]),
        loaders: strings(&v["loaders"]),
        published: v["date_published"].as_str().map(String::from),
        downloads: v["downloads"].as_u64().unwrap_or(0),
        file,
        dependencies: v["dependencies"]
            .as_array()
            .map(|deps| {
                deps.iter()
                    .map(|d| Dependency {
                        project_id: d["project_id"].as_str().map(String::from),
                        version_id: d["version_id"].as_str().map(String::from),
                        kind: d["dependency_type"].as_str().unwrap_or("optional").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl Modrinth<'_> {
    async fn send<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T, anyhow::Error> {
        let response = request
            .header(reqwest::header::USER_AGENT, self.user_agent)
            .send()
            .await
            .map_err(|e| display(format!("Could not reach Modrinth: {e}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(display("Modrinth does not know this project or version."));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(display("Modrinth is rate limiting this panel. Try again in a minute."));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::warn!("modmanager: modrinth HTTP {status}: {}", body.chars().take(300).collect::<String>());
            return Err(display(format!("Modrinth answered HTTP {}.", status.as_u16())));
        }

        response
            .json()
            .await
            .map_err(|e| display(format!("Unexpected answer from Modrinth: {e}")))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T, anyhow::Error> {
        self.send(self.client.get(format!("{API}{path}")).query(query)).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, anyhow::Error> {
        self.send(self.client.post(format!("{API}{path}")).json(body)).await
    }

    pub async fn search(&self, q: &SearchQuery) -> Result<SearchPage, anyhow::Error> {
        let mut facets: Vec<Vec<String>> = Vec::new();
        match q.loader {
            _ if q.modpacks => {
                facets.push(vec!["project_type:modpack".into()]);
                if let Some(loader) = q.loader {
                    facets.push(super::compatible_loaders(loader).iter().map(|l| format!("categories:{l}")).collect());
                }
            }
            Some(super::DATAPACK) => facets.push(vec!["project_type:datapack".into()]),
            Some(loader) => {
                let kind = if super::is_plugin_loader(loader) { "plugin" } else { "mod" };
                facets.push(vec![format!("project_type:{kind}")]);
                facets.push(super::compatible_loaders(loader).iter().map(|l| format!("categories:{l}")).collect());
            }
            None => facets.push(vec!["project_type:mod".into(), "project_type:plugin".into()]),
        }
        if let Some(v) = &q.game_version {
            facets.push(vec![format!("versions:{v}")]);
        }
        if q.server_only {
            // keeps "unknown" (common for plugins and data packs), drops client-only
            facets.push(vec!["server_side!=unsupported".into()]);
        }
        let index = match q.sort.as_str() {
            "downloads" | "follows" | "newest" | "updated" => q.sort.as_str(),
            _ => "relevance",
        };
        let raw: Value = self
            .get(
                "/search",
                &[
                    ("query", q.query.clone()),
                    ("facets", serde_json::to_string(&facets)?),
                    ("index", index.to_string()),
                    ("offset", q.offset.to_string()),
                    ("limit", q.limit.to_string()),
                ],
            )
            .await?;

        Ok(SearchPage {
            // the search index can lag behind a project's own setting; drop those stragglers too
            hits: raw["hits"]
                .as_array()
                .map(|h| {
                    h.iter()
                        .filter(|h| !q.server_only || h["server_side"].as_str() != Some("unsupported"))
                        .map(hit)
                        .collect()
                })
                .unwrap_or_default(),
            total: raw["total_hits"].as_u64().unwrap_or(0),
            offset: q.offset,
            limit: q.limit,
        })
    }

    pub async fn project(&self, id: &str) -> Result<ProjectDetail, anyhow::Error> {
        let p: Value = self.get(&format!("/project/{id}"), &[]).await?;
        let mut links = Vec::new();
        for (key, label) in [
            ("source_url", "Source"),
            ("issues_url", "Issues"),
            ("wiki_url", "Wiki"),
            ("discord_url", "Discord"),
        ] {
            if let Some(url) = p[key].as_str().filter(|u| u.starts_with("http")) {
                links.push(Link { label: label.into(), url: url.into() });
            }
        }
        let mut project = project(&p);
        // the team owner as author, best effort
        if let Some(team) = p["team"].as_str()
            && let Ok(members) = self.get::<Value>(&format!("/team/{team}/members"), &[]).await
        {
            let members = members.as_array().cloned().unwrap_or_default();
            project.author = members
                .iter()
                .find(|m| m["role"].as_str() == Some("Owner"))
                .or_else(|| members.first())
                .and_then(|m| m["user"]["username"].as_str())
                .map(String::from);
        }

        Ok(ProjectDetail {
            project,
            body: p["body"].as_str().map(String::from),
            links,
            gallery: p["gallery"]
                .as_array()
                .map(|g| g.iter().filter_map(|i| i["url"].as_str().map(String::from)).collect())
                .unwrap_or_default(),
            loaders: strings(&p["loaders"]),
            game_versions: strings(&p["game_versions"]),
            distribution_blocked: false,
        })
    }

    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>, anyhow::Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let raw: Vec<Value> = self.get("/projects", &[("ids", serde_json::to_string(ids)?)]).await?;
        Ok(raw.iter().map(project).collect())
    }

    /// Projects together with their `server_side` value (required / optional / unsupported / unknown).
    pub async fn projects_with_side(&self, ids: &[String]) -> Result<Vec<(Project, String)>, anyhow::Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        // keep the query string short
        for chunk in ids.chunks(100) {
            let raw: Vec<Value> = self.get("/projects", &[("ids", serde_json::to_string(chunk)?)]).await?;
            out.extend(raw.iter().map(|p| (project(p), s(&p["server_side"]))));
        }
        Ok(out)
    }

    /// Versions of a project, newest first, filtered to what the server can load.
    pub async fn versions(
        &self,
        project: &str,
        loader: Option<&str>,
        game_version: Option<&str>,
    ) -> Result<Vec<Version>, anyhow::Error> {
        let mut query = vec![("include_changelog", "false".to_string())];
        if let Some(loader) = loader {
            query.push(("loaders", serde_json::to_string(&super::compatible_loaders(loader))?));
        }
        if let Some(v) = game_version {
            query.push(("game_versions", serde_json::to_string(&[v])?));
        }
        let raw: Vec<Value> = self.get(&format!("/project/{project}/version"), &query).await?;
        let zip = loader == Some(super::DATAPACK);
        Ok(raw.iter().map(|v| version_with(v, zip)).collect())
    }

    /// `loader` decides which file of the version is meant (see [`version_with`]).
    pub async fn version(&self, id: &str, loader: Option<&str>) -> Result<Version, anyhow::Error> {
        let raw: Value = self.get(&format!("/version/{id}"), &[]).await?;
        Ok(version_with(&raw, loader == Some(super::DATAPACK)))
    }

    pub async fn versions_by_id(&self, ids: &[String]) -> Result<Vec<Version>, anyhow::Error> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let raw: Vec<Value> = self.get("/versions", &[("ids", serde_json::to_string(ids)?)]).await?;
        Ok(raw.iter().map(version).collect())
    }

    /// Identify jars by SHA-1.
    pub async fn identify(&self, sha1s: &[String]) -> Result<HashMap<String, (Identified, Version)>, anyhow::Error> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let raw: HashMap<String, Value> = self
            .post("/version_files", &json!({ "hashes": sha1s, "algorithm": "sha1" }))
            .await?;
        Ok(raw
            .into_iter()
            .map(|(hash, v)| {
                let v = version(&v);
                (
                    hash,
                    (
                        Identified {
                            provider: Provider::Modrinth,
                            project_id: v.project_id.clone(),
                            version_id: v.id.clone(),
                        },
                        v,
                    ),
                )
            })
            .collect())
    }

    /// The newest compatible version for each hash (only hashes Modrinth knows).
    pub async fn latest_for(
        &self,
        sha1s: &[String],
        loader: Option<&str>,
        game_version: Option<&str>,
    ) -> Result<HashMap<String, Version>, anyhow::Error> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let mut body = json!({ "hashes": sha1s, "algorithm": "sha1" });
        if let Some(loader) = loader {
            body["loaders"] = json!(super::compatible_loaders(loader));
        }
        if let Some(v) = game_version {
            body["game_versions"] = json!([v]);
        }
        let raw: HashMap<String, Value> = self.post("/version_files/update", &body).await?;
        let zip = loader == Some(super::DATAPACK);
        Ok(raw.into_iter().map(|(hash, v)| (hash, version_with(&v, zip))).collect())
    }

    /// Release Minecraft versions, newest first.
    pub async fn game_versions(&self) -> Result<Vec<String>, anyhow::Error> {
        let raw: Vec<Value> = self.get("/tag/game_version", &[]).await?;
        Ok(raw
            .iter()
            .filter(|v| v["version_type"].as_str() == Some("release"))
            .filter_map(|v| v["version"].as_str().map(String::from))
            .collect())
    }
}
