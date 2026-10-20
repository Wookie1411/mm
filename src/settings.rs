use compact_str::ToCompactString;
use serde::{Deserialize, Serialize};
use shared::extensions::settings::{
    ExtensionSettings, SettingsDeserializeExt, SettingsDeserializer, SettingsSerializeExt,
    SettingsSerializer,
};
use utoipa::ToSchema;

/// Minecraft Java eggs (Vanilla for data packs only).
const DEFAULT_EGGS: [&str; 9] = [
    "505cb7b8-36b2-44b0-9b61-f6b13284325a", // Vanilla Minecraft
    "15636215-4c1e-468b-866a-7336e82ef95e", // Sponge
    "3d1f4cd2-bbeb-4958-b3a8-5a6b272f3c61", // Paper
    "bb65e012-ff62-4606-98ec-66ad441ef7a4", // Forge
    "7cb02f70-979f-43a4-b9f4-7069dcea215c", // Bungeecord
    "f3b908ed-aaed-4609-a771-c80e08699af8", // NeoForge
    "d76be2d4-9fcc-44d2-b04e-68d4414a7ffd", // Fabric
    "964d8773-aa6c-4427-bdbd-4ba5e4efbea1", // Velocity
    "f79911f3-e1e8-4199-9df5-30f50e232251", // Java Edition Modpack Installer
];

#[derive(ToSchema, Serialize, Deserialize, Clone)]
pub struct ExtensionSettingsData {
    /// Who may use the page: admins / everyone / users (the selected users, plus admins).
    pub access: compact_str::CompactString,
    /// The selected users when access = users.
    pub users: Vec<uuid::Uuid>,
    /// Eggs the page works on; empty = every egg.
    pub eggs: Vec<uuid::Uuid>,
    pub modrinth_enabled: bool,
    pub curseforge_enabled: bool,
    /// From console.curseforge.com; stored encrypted, never sent to the browser
    /// (admin.rs and routes.rs only expose whether it is set).
    pub curseforge_api_key: compact_str::CompactString,
    /// Let modpack installs move a server to another enabled egg (e.g. Fabric -> NeoForge).
    pub switch_eggs: bool,
}

impl Default for ExtensionSettingsData {
    fn default() -> Self {
        Self {
            access: "everyone".into(),
            users: vec![],
            eggs: DEFAULT_EGGS.iter().filter_map(|u| u.parse().ok()).collect(),
            modrinth_enabled: true,
            curseforge_enabled: true,
            curseforge_api_key: "".into(),
            switch_eggs: false,
        }
    }
}

impl ExtensionSettingsData {
    /// Panel administrators always; others by the access mode.
    pub fn allows(&self, user: &shared::models::user::User) -> bool {
        user.admin
            || match self.access.as_str() {
                "everyone" => true,
                "users" => self.users.contains(&user.uuid),
                _ => false,
            }
    }

    pub fn egg_allowed(&self, egg: uuid::Uuid) -> bool {
        self.eggs.is_empty() || self.eggs.contains(&egg)
    }

    /// The CurseForge key, when CurseForge is enabled and a key is set.
    pub fn curseforge_key(&self) -> Option<&str> {
        (self.curseforge_enabled && !self.curseforge_api_key.trim().is_empty()).then(|| self.curseforge_api_key.trim())
    }

    pub fn providers(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.modrinth_enabled {
            out.push("modrinth");
        }
        if self.curseforge_key().is_some() {
            out.push("curseforge");
        }
        out
    }
}

#[async_trait::async_trait]
impl SettingsSerializeExt for ExtensionSettingsData {
    async fn serialize(&self, serializer: SettingsSerializer) -> Result<SettingsSerializer, anyhow::Error> {
        serializer
            .write_raw_setting("access", self.access.clone())
            .write_serde_setting("users", &self.users)?
            // older versions read this
            .write_raw_setting("admin_only", (self.access == "admins").to_compact_string())
            .write_serde_setting("eggs", &self.eggs)?
            .write_raw_setting("modrinth_enabled", self.modrinth_enabled.to_compact_string())
            .write_raw_setting("curseforge_enabled", self.curseforge_enabled.to_compact_string())
            .write_raw_setting("switch_eggs", self.switch_eggs.to_compact_string())
            .write_raw_encrypted_setting("curseforge_api_key", self.curseforge_api_key.clone())
            .await
    }
}

pub struct ExtensionSettingsDataDeserializer;

#[async_trait::async_trait]
impl SettingsDeserializeExt for ExtensionSettingsDataDeserializer {
    async fn deserialize_boxed(
        &self,
        mut deserializer: SettingsDeserializer<'_>,
    ) -> Result<ExtensionSettings, anyhow::Error> {
        let d = ExtensionSettingsData::default();
        let curseforge_api_key = deserializer
            .read_raw_encrypted_setting("curseforge_api_key")
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        Ok(Box::new(ExtensionSettingsData {
            access: {
                // before the access modes there was only admin_only
                let legacy = deserializer.take_raw_setting("admin_only").and_then(|s| s.parse::<bool>().ok());
                deserializer
                    .take_raw_setting("access")
                    .filter(|a| matches!(a.as_str(), "admins" | "everyone" | "users"))
                    .unwrap_or_else(|| match legacy {
                        Some(true) => "admins".into(),
                        Some(false) => "everyone".into(),
                        None => d.access.clone(),
                    })
            },
            users: deserializer
                .read_serde_setting::<Option<Vec<uuid::Uuid>>>("users")
                .ok()
                .flatten()
                .unwrap_or_default(),
            eggs: deserializer
                .read_serde_setting::<Option<Vec<uuid::Uuid>>>("eggs")
                .ok()
                .flatten()
                .unwrap_or(d.eggs),
            modrinth_enabled: deserializer
                .take_raw_setting("modrinth_enabled")
                .and_then(|s| s.parse().ok())
                .unwrap_or(d.modrinth_enabled),
            curseforge_enabled: deserializer
                .take_raw_setting("curseforge_enabled")
                .and_then(|s| s.parse().ok())
                .unwrap_or(d.curseforge_enabled),
            curseforge_api_key,
            switch_eggs: deserializer
                .take_raw_setting("switch_eggs")
                .and_then(|s| s.parse().ok())
                .unwrap_or(d.switch_eggs),
        }))
    }
}

pub async fn load(state: &shared::State) -> Result<ExtensionSettingsData, anyhow::Error> {
    let settings = state.settings.get().await?;
    let ext: &ExtensionSettingsData = settings.find_extension_settings()?;

    Ok(ext.clone())
}
