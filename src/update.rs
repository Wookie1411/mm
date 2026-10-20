//! Self-update from the GitHub releases of this extension: check for a newer release,
//! download its .c7s.zip, verify it, install it like Admin > Extensions > upload does,
//! and start the panel rebuild.
//! Admin API: GET/POST /api/admin/extensions/dev.emrekmek.modmanager/update

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shared::{
    GetState,
    models::{admin_activity::GetAdminActivityLogger, user::GetPermissionManager},
    response::{ApiResponse, ApiResponseResult},
};

/// owner/name of the GitHub repository whose releases carry the extension.
pub const REPO: &str = "Wookie1411/mm";
const PACKAGE: &str = "dev.emrekmek.modmanager";
const ASSET: &str = "dev_emrekmek_modmanager.c7s.zip";
const MAX_ASSET: usize = 64 * 1024 * 1024;
const USER_AGENT: &str = "calagopus-extension-updater";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
struct Release {
    version: String,
    tag: String,
    notes: String,
    published: Option<String>,
    url: String,
    #[serde(skip)]
    asset_url: String,
    #[serde(skip)]
    asset_size: usize,
    /// "sha256:<hex>" as GitHub reports it.
    #[serde(skip)]
    digest: Option<String>,
}

fn version_of(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// The newest published release with the extension attached, `None` when there is none yet.
async fn latest(client: &reqwest::Client) -> Result<Option<Release>, anyhow::Error> {
    let response = client
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("could not reach GitHub: {e}"))?;
    match response.status() {
        reqwest::StatusCode::NOT_FOUND => return Ok(None),
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS => {
            anyhow::bail!("GitHub's rate limit for this server is used up; try again in an hour")
        }
        s if !s.is_success() => anyhow::bail!("GitHub answered HTTP {}", s.as_u16()),
        _ => {}
    }
    let raw: Value = response.json().await?;
    let tag = raw["tag_name"].as_str().unwrap_or_default().to_string();
    let Some(version) = version_of(&tag) else {
        return Ok(None);
    };
    let Some(asset) = raw["assets"].as_array().and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(ASSET))) else {
        return Ok(None);
    };

    Ok(Some(Release {
        version: version.to_string(),
        tag,
        notes: raw["body"].as_str().unwrap_or_default().chars().take(4000).collect(),
        published: raw["published_at"].as_str().map(String::from),
        url: raw["html_url"].as_str().unwrap_or_default().to_string(),
        asset_url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
        asset_size: asset["size"].as_u64().unwrap_or(0) as usize,
        digest: asset["digest"].as_str().map(String::from),
    }))
}

fn newer(release: &Release) -> bool {
    match (semver::Version::parse(&release.version), semver::Version::parse(VERSION)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => false,
    }
}

fn error(message: impl Into<String>) -> ApiResponse {
    ApiResponse::error(message.into())
}

/// Is there a newer release?
pub async fn check(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let release = latest(&state.client).await.map_err(|e| error(e.to_string()))?;

    ApiResponse::new_serialized(json!({
        "current": VERSION,
        "repository": format!("https://github.com/{REPO}"),
        "latest": release,
        "update_available": release.as_ref().is_some_and(newer),
        "can_install": state.ensure_extension_management().is_ok(),
        "panel_version": shared::heavy::build_target_version().await.to_string(),
    }))
    .ok()
}

/// Download, verify and install the newest release, then start the panel rebuild.
pub async fn install(
    state: GetState,
    permissions: GetPermissionManager,
    logger: GetAdminActivityLogger,
) -> ApiResponseResult {
    state.ensure_extension_management()?;
    permissions.has_admin_permission("extensions.manage")?;

    let Some(release) = latest(&state.client).await.map_err(|e| error(e.to_string()))? else {
        return Err(error("No release found on GitHub."));
    };
    if !newer(&release) {
        return Err(error(format!("Mod Manager {VERSION} is already up to date.")));
    }
    if release.asset_size > MAX_ASSET || !release.asset_url.starts_with("https://github.com/") {
        return Err(error("The release file does not look right; not installing it."));
    }

    let mut response = state
        .client
        .get(&release.asset_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| error(format!("Could not download the update: {e}")))?;
    if !response.status().is_success() {
        return Err(error(format!("Downloading the update failed (HTTP {}).", response.status().as_u16())));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| error(format!("Could not download the update: {e}")))? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_ASSET {
            return Err(error("The update is larger than expected; not installing it."));
        }
    }
    // GitHub publishes a SHA-256 for every release file
    if let Some(digest) = release.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(digest) {
            return Err(error("The downloaded update does not match its checksum; not installing it."));
        }
    }

    let mut reader = std::io::Cursor::new(bytes);
    let staged = shared::heavy::stage_extension(&mut reader)
        .await
        .map_err(|e| error(format!("The update archive is not valid: {e}")))?;
    let meta = &staged.distr.metadata_toml;
    if meta.package_name != PACKAGE {
        return Err(error(format!("The release contains {}, not Mod Manager; not installing it.", meta.package_name)));
    }
    if let Err(e) = meta.check_panel_version(Some(&shared::heavy::build_target_version().await)) {
        return Err(error(format!("Mod Manager {} {e}. Update the panel first.", release.version)));
    }
    let version = staged.distr.cargo_toml.package.version.to_string();
    staged.install().await.map_err(|e| error(format!("Could not install the update: {e}")))?;

    let build = match shared::heavy::ask(&shared::heavy::Request::RequestRebuild { force: false }).await {
        Ok(shared::heavy::Response::RebuildAccepted { build_id } | shared::heavy::Response::RebuildAlreadyRunning { build_id }) => {
            Some(build_id)
        }
        Ok(other) => {
            tracing::warn!("modmanager: unexpected rebuild answer {other:?}");
            None
        }
        Err(e) => {
            tracing::warn!("modmanager: could not request a rebuild: {e:?}");
            None
        }
    };
    logger
        .log(
            "extension:upload",
            json!({ "package_name": PACKAGE, "version": version, "from": VERSION, "source": release.url, "auto_update": true }),
        )
        .await;

    ApiResponse::new_serialized(json!({
        "installed": version,
        "build_id": build,
        "message": match build {
            Some(_) => format!("Mod Manager {version} is installed. The panel is rebuilding; it switches to the new version on its own in a few minutes."),
            None => format!("Mod Manager {version} is installed. Click Rebuild under Admin > Extensions to switch to it."),
        },
    }))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(v: &str) -> Release {
        Release {
            version: v.into(),
            tag: format!("v{v}"),
            notes: String::new(),
            published: None,
            url: String::new(),
            asset_url: String::new(),
            asset_size: 0,
            digest: None,
        }
    }

    #[test]
    fn versions() {
        assert_eq!(version_of("v1.11.0").unwrap().to_string(), "1.11.0");
        assert!(version_of("latest").is_none());
        assert!(newer(&release("99.0.0")));
        assert!(!newer(&release(VERSION)));
        assert!(!newer(&release("0.1.0")));
    }
}
