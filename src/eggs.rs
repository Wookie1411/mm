//! The "Minecraft: Java (any loader)" egg: one egg whose install script sets up vanilla,
//! Fabric, Quilt, Forge, NeoForge, Paper or Purpur, picked with MINECRAFT_LOADER. A server
//! on it changes loader by setting variables and reinstalling, without moving to another
//! egg. Admins add it with one click (standard Pterodactyl egg format, so it can also be
//! downloaded and imported by hand on any panel).
//! Admin API: /api/admin/extensions/dev.emrekmek.modmanager/egg{,/download}

use axum::http::header;
use serde::Deserialize;
use serde_json::{Value, json};
use shared::{
    GetState,
    models::{
        ByUuid,
        admin_activity::GetAdminActivityLogger,
        nest_egg::{ExportedNestEgg, NestEgg},
        user::GetPermissionManager,
    },
    response::{ApiResponse, ApiResponseResult},
};

pub const NAME: &str = "Minecraft: Java (any loader)";
/// The variable that marks the egg (and picks the loader).
pub const LOADER_VARIABLE: &str = "MINECRAFT_LOADER";
/// Bumped whenever the egg below changes, so panels can tell theirs is outdated.
pub const REVISION: u32 = 1;
const INSTALL_SCRIPT: &str = include_str!("egg/install.sh");
pub const LOADERS: [&str; 7] = ["vanilla", "fabric", "quilt", "forge", "neoforge", "paper", "purpur"];

/// The egg in Pterodactyl's export format.
pub fn exported() -> Value {
    let java: Vec<(String, Value)> = [25, 21, 17, 16, 11, 8]
        .iter()
        .map(|v| (format!("Java {v}"), Value::String(format!("ghcr.io/pterodactyl/yolks:java_{v}"))))
        .collect();
    let server_properties = json!({
        "server.properties": {
            "parser": "properties",
            "find": {
                "server-ip": "0.0.0.0",
                "server-port": "{{server.build.default.port}}",
                "query.port": "{{server.build.default.port}}"
            }
        }
    });
    json!({
        "_comment": "Made by Mod Manager for Calagopus. Also works on Pterodactyl.",
        "meta": { "version": "PTDL_v2", "update_url": null },
        "exported_at": "2026-10-27T00:00:00+00:00",
        "name": NAME,
        "author": "mod-manager@users.noreply.github.com",
        "description": format!(
            "Minecraft: Java Edition with any loader: vanilla, Fabric, Quilt, Forge, NeoForge, Paper or Purpur. \
             Change the loader or Minecraft version under Startup and reinstall; worlds, mods and configs are kept. \
             Mod Manager uses it to switch servers to what a modpack needs. (revision {REVISION})"
        ),
        "features": ["eula", "java_version", "pid_limit"],
        "docker_images": java.into_iter().collect::<serde_json::Map<String, Value>>(),
        "file_denylist": [],
        "startup": "java -Xms128M -XX:MaxRAMPercentage=95.0 -Dterminal.jline=false -Dterminal.ansi=true $( [[ -f unix_args.txt ]] && printf %s \"@unix_args.txt\" || printf %s \"-jar server.jar\" )",
        "config": {
            "files": server_properties.to_string(),
            "startup": json!({ "done": ")! For help, type " }).to_string(),
            "logs": "{}",
            "stop": "stop"
        },
        "scripts": {
            "installation": {
                "script": INSTALL_SCRIPT,
                "container": "eclipse-temurin:21-jdk-jammy",
                "entrypoint": "bash"
            }
        },
        "variables": [
            {
                "name": "Loader",
                "description": "vanilla, fabric, quilt, forge, neoforge, paper or purpur. Reinstall the server after changing it.",
                "env_variable": LOADER_VARIABLE,
                "default_value": "vanilla",
                "user_viewable": true,
                "user_editable": true,
                "rules": format!("required|string|in:{}", LOADERS.join(",")),
                "field_type": "text"
            },
            {
                "name": "Minecraft Version",
                "description": "For example 1.20.1, or latest. Reinstall the server after changing it.",
                "env_variable": "MC_VERSION",
                "default_value": "latest",
                "user_viewable": true,
                "user_editable": true,
                "rules": "required|string|max:30",
                "field_type": "text"
            },
            {
                "name": "Loader Version",
                "description": "The Fabric / Quilt / Forge / NeoForge version or Paper / Purpur build, or latest.",
                "env_variable": "LOADER_VERSION",
                "default_value": "latest",
                "user_viewable": true,
                "user_editable": true,
                "rules": "required|string|max:60",
                "field_type": "text"
            }
        ]
    })
}

fn parsed() -> Result<ExportedNestEgg, anyhow::Error> {
    Ok(serde_json::from_value(exported())?)
}

/// The any-loader egg on this panel (egg, nest), found by its loader variable.
pub async fn find(database: &shared::database::Database) -> Option<(uuid::Uuid, uuid::Uuid)> {
    sqlx::query_as(
        "SELECT nest_eggs.uuid, nest_eggs.nest_uuid FROM nest_eggs
        JOIN nest_egg_variables ON nest_egg_variables.egg_uuid = nest_eggs.uuid
        WHERE nest_egg_variables.env_variable = $1
        ORDER BY nest_eggs.created LIMIT 1",
    )
    .bind(LOADER_VARIABLE)
    .fetch_optional(database.read())
    .await
    .ok()
    .flatten()
}

/// Is this egg the any-loader egg?
pub async fn is_any_loader(database: &shared::database::Database, egg: uuid::Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM nest_egg_variables WHERE egg_uuid = $1 AND env_variable = $2")
        .bind(egg)
        .bind(LOADER_VARIABLE)
        .fetch_one(database.read())
        .await
        .is_ok_and(|n| n > 0)
}

/// Revision of an installed copy, from its description.
fn revision_of(description: &str) -> u32 {
    description
        .rsplit("(revision ")
        .next()
        .and_then(|r| r.trim_end_matches(')').trim().parse().ok())
        .filter(|_| description.contains("(revision "))
        .unwrap_or(0)
}

/// Is the egg on this panel, and where; plus the nests to put it in.
pub async fn status(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let installed = match find(&state.database).await {
        Some((egg, nest)) => {
            let row: Option<(String, Option<String>, String)> = sqlx::query_as(
                "SELECT nest_eggs.name, nest_eggs.description, nests.name FROM nest_eggs
                JOIN nests ON nests.uuid = nest_eggs.nest_uuid WHERE nest_eggs.uuid = $1",
            )
            .bind(egg)
            .fetch_optional(state.database.read())
            .await?;
            row.map(|(name, description, nest_name)| {
                let revision = revision_of(description.as_deref().unwrap_or_default());
                json!({ "uuid": egg, "name": name, "nest": nest, "nest_name": nest_name, "revision": revision, "outdated": revision < REVISION })
            })
        }
        None => None,
    };
    let nests: Vec<(uuid::Uuid, String, i64)> = sqlx::query_as(
        "SELECT nests.uuid, nests.name,
            (SELECT count(*) FROM nest_eggs WHERE nest_eggs.nest_uuid = nests.uuid AND nest_eggs.name ILIKE '%minecraft%')
        FROM nests ORDER BY nests.name",
    )
    .fetch_all(state.database.read())
    .await?;

    ApiResponse::new_serialized(json!({
        "installed": installed,
        "revision": REVISION,
        "nests": nests.into_iter().map(|(uuid, name, minecraft)| json!({ "uuid": uuid, "name": name, "minecraft_eggs": minecraft })).collect::<Vec<_>>(),
    }))
    .ok()
}

#[derive(Deserialize)]
pub struct AddPayload {
    nest: uuid::Uuid,
}

/// Add the egg to a nest, or bring an existing copy up to date. It is also put on Mod Manager's egg list.
pub async fn add(
    state: GetState,
    permissions: GetPermissionManager,
    logger: GetAdminActivityLogger,
    shared::Payload(data): shared::Payload<AddPayload>,
) -> ApiResponseResult {
    permissions.has_admin_permission("eggs.create")?;
    let egg = parsed().map_err(|e| ApiResponse::error(format!("The bundled egg is invalid: {e}")))?;
    let uuid = match find(&state.database).await {
        Some((existing, _)) => {
            let current = NestEgg::by_uuid_optional(&state.database, existing)
                .await?
                .ok_or_else(|| ApiResponse::error("The egg disappeared; try again."))?;
            current.import_update(&state.database, egg, false).await?;
            existing
        }
        None => {
            let nest_exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM nests WHERE uuid = $1)")
                .bind(data.nest)
                .fetch_one(state.database.read())
                .await?;
            if !nest_exists {
                return Err(ApiResponse::error("That nest does not exist."));
            }
            match NestEgg::import(&state, data.nest, None, egg).await {
                Ok(created) => created.uuid,
                Err(e) if e.is_unique_violation() => {
                    return Err(ApiResponse::error(format!(
                        "That nest already has an egg called \"{NAME}\" without Mod Manager's loader variable. Rename or remove it first."
                    )));
                }
                Err(e) => return Err(e.into()),
            }
        }
    };

    // show the Mods page on servers that use it
    {
        let mut settings = state.settings.get_mut().await?;
        let ext: &mut crate::settings::ExtensionSettingsData = settings.find_mut_extension_settings()?;
        if !ext.eggs.is_empty() && !ext.eggs.contains(&uuid) {
            ext.eggs.push(uuid);
        }
        settings.save().await?;
    }
    logger
        .log("extension:modmanager.egg", json!({ "egg": uuid, "name": NAME, "revision": REVISION }))
        .await;

    ApiResponse::new_serialized(json!({ "uuid": uuid, "revision": REVISION })).ok()
}

/// The egg as a file, to import by hand on another panel.
pub async fn download(permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    let body = serde_json::to_string_pretty(&exported()).unwrap_or_default();
    ApiResponse::new(axum::body::Body::from(body))
        .with_header(header::CONTENT_TYPE.as_str(), "application/json")
        .with_header(header::CONTENT_DISPOSITION.as_str(), "attachment; filename=\"egg-minecraft-any-loader.json\"")
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's own importer accepts the egg.
    #[test]
    fn egg_parses() {
        let egg = parsed().unwrap();
        assert_eq!(egg.name, NAME);
        assert_eq!(egg.variables.len(), 3);
        assert!(egg.variables.iter().any(|v| v.env_variable == LOADER_VARIABLE));
        assert!(egg.scripts.installation.content.contains("MINECRAFT_LOADER"));
        assert_eq!(egg.docker_images.len(), 6);
        assert!(egg.config.files.contains_key("server.properties"));
    }

    #[test]
    fn revisions() {
        assert_eq!(revision_of("Minecraft … (revision 3)"), 3);
        assert_eq!(revision_of("some egg"), 0);
    }
}
