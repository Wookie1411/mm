//! Everything a request needs: the authorised server, its Wings files, the extension
//! settings and the provider clients, plus a small JSON cache in front of all of them.

use crate::{
    detect::{self, Detected},
    files::ServerFiles,
    providers::{Provider, curseforge::CurseForge, modrinth::Modrinth},
    settings::{self, ExtensionSettingsData},
};
use axum::http::StatusCode;
use serde::{Serialize, de::DeserializeOwned};
use shared::{
    State,
    models::{server::Server, user::PermissionManager},
    response::DisplayError,
};

pub const VIEW_PERMISSION: &str = "mod-manager.read";
pub const MANAGE_PERMISSION: &str = "mod-manager.manage";

pub fn display(message: impl Into<String>) -> anyhow::Error {
    DisplayError::new(message.into()).into()
}

fn forbidden(message: &'static str) -> anyhow::Error {
    DisplayError::new(message).with_status(StatusCode::FORBIDDEN).into()
}

/// Viewing needs the extension's read permission or Read file contents;
/// changes need its manage permission, or both Create and Delete files.
pub fn can_view(p: &PermissionManager) -> bool {
    [VIEW_PERMISSION, MANAGE_PERMISSION, "files.read-content"]
        .iter()
        .any(|x| p.has_server_permission(x).is_ok())
}

pub fn can_manage(p: &PermissionManager) -> bool {
    p.has_server_permission(MANAGE_PERMISSION).is_ok()
        || (p.has_server_permission("files.create").is_ok() && p.has_server_permission("files.delete").is_ok())
}

pub struct Ctx {
    pub state: State,
    pub server: Server,
    pub files: ServerFiles,
    pub settings: ExtensionSettingsData,
    /// Identifies this panel to Modrinth and pack downloads, as Modrinth asks.
    pub user_agent: String,
}

/// `calagopus-modmanager/<version> (<panel host>)`, from the panel's configured URL.
pub fn user_agent(panel_url: &str) -> String {
    let host = panel_url
        .split("://")
        .last()
        .unwrap_or_default()
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    let version = env!("CARGO_PKG_VERSION");
    if host.is_empty() {
        format!("calagopus-modmanager/{version}")
    } else {
        format!("calagopus-modmanager/{version} ({host})")
    }
}

impl Ctx {
    pub async fn view(
        state: State,
        permissions: &PermissionManager,
        user: &shared::models::user::User,
        server: Server,
    ) -> Result<Self, anyhow::Error> {
        let settings = settings::load(&state).await?;
        if !settings.allows(user) {
            return Err(forbidden("Mod Manager is not available to your account on this panel. Ask a panel administrator for access."));
        }
        if !settings.egg_allowed(server.egg.uuid) {
            return Err(display(
                "Mod Manager is not enabled for this server type. An administrator can enable it per egg under Admin > Extensions > Mod Manager.",
            ));
        }
        if !can_view(permissions) {
            return Err(forbidden(
                "You need the \"Read file contents\" or Mod Manager permission to view mods.",
            ));
        }
        let files = ServerFiles::open(&state, &server).await?;
        let user_agent = user_agent(&state.settings.get().await?.app.url);

        Ok(Self {
            state,
            server,
            files,
            settings,
            user_agent,
        })
    }

    pub async fn manage(
        state: State,
        permissions: &PermissionManager,
        user: &shared::models::user::User,
        server: Server,
    ) -> Result<Self, anyhow::Error> {
        let ctx = Self::view(state, permissions, user, server).await?;
        if !can_manage(permissions) {
            return Err(forbidden(
                "You need the Mod Manager manage permission (or both \"Create files\" and \"Delete files\") to change mods.",
            ));
        }

        Ok(ctx)
    }

    pub fn modrinth(&self) -> Result<Modrinth<'_>, anyhow::Error> {
        if !self.settings.modrinth_enabled {
            return Err(display("Modrinth is disabled on this panel."));
        }
        Ok(Modrinth {
            client: &self.state.client,
            user_agent: &self.user_agent,
        })
    }

    pub fn curseforge(&self) -> Result<CurseForge<'_>, anyhow::Error> {
        let Some(key) = self.settings.curseforge_key() else {
            return Err(display(
                "CurseForge is not set up on this panel. An administrator can add a CurseForge API key under Admin > Extensions > Mod Manager.",
            ));
        };
        Ok(CurseForge {
            client: &self.state.client,
            key,
        })
    }

    pub fn check_provider(&self, provider: Provider) -> Result<(), anyhow::Error> {
        match provider {
            Provider::Modrinth => self.modrinth().map(|_| ()),
            Provider::Curseforge => self.curseforge().map(|_| ()),
        }
    }

    fn server_key(&self, what: &str) -> String {
        format!("modmanager:{what}:{}", self.server.uuid)
    }

    pub async fn forget(&self, what: &str) {
        let _ = self.state.cache.invalidate(&self.server_key(what)).await;
    }

    /// Cache a value as JSON under a global key (provider data shared by every server).
    pub async fn cached<T, F>(&self, key: &str, ttl: u64, fresh: bool, compute: F) -> Result<T, anyhow::Error>
    where
        T: Serialize + DeserializeOwned,
        F: Future<Output = Result<T, anyhow::Error>>,
    {
        let key = format!("modmanager:{key}");
        if !fresh
            && let Ok(Some(raw)) = self.state.cache.get::<String>(&key).await
            && let Ok(v) = serde_json::from_str(&raw)
        {
            return Ok(v);
        }
        let v = compute.await?;
        if let Ok(raw) = serde_json::to_string(&v) {
            let _ = self.state.cache.set(&key, ttl, &raw).await;
        }

        Ok(v)
    }

    /// Like `cached`, but per server.
    pub async fn cached_for_server<T, F>(&self, what: &str, ttl: u64, fresh: bool, compute: F) -> Result<T, anyhow::Error>
    where
        T: Serialize + DeserializeOwned,
        F: Future<Output = Result<T, anyhow::Error>>,
    {
        let key = format!("{what}:{}", self.server.uuid);
        self.cached(&key, ttl, fresh, compute).await
    }

    /// A per-server cached value, if present.
    pub async fn peek<T: DeserializeOwned>(&self, what: &str) -> Option<T> {
        let raw = self.state.cache.get::<String>(&self.server_key(what)).await.ok().flatten()?;
        serde_json::from_str(&raw).ok()
    }

    pub async fn store<T: Serialize>(&self, what: &str, ttl: u64, value: &T) {
        if let Ok(raw) = serde_json::to_string(value) {
            let _ = self.state.cache.set(&self.server_key(what), ttl, &raw).await;
        }
    }

    /// The server's loader and Minecraft version (cached for a minute).
    pub async fn detected(&self) -> Detected {
        self.cached_for_server("detected", 60, false, async { Ok(detect::detect(self).await) })
            .await
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::user_agent;

    #[test]
    fn user_agents() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(user_agent("https://panel.example.com"), format!("calagopus-modmanager/{v} (panel.example.com)"));
        assert_eq!(user_agent("http://10.0.0.5:8000/"), format!("calagopus-modmanager/{v} (10.0.0.5:8000)"));
        assert_eq!(user_agent("panel.example.com/x?y"), format!("calagopus-modmanager/{v} (panel.example.com)"));
        assert_eq!(user_agent(""), format!("calagopus-modmanager/{v}"));
    }
}
