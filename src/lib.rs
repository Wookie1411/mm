//! Mod Manager for Calagopus: browse Modrinth and CurseForge, install mods / plugins
//! into a Minecraft server, and update, downgrade, enable, disable or remove them.
//! Files are downloaded by Wings straight from the provider's CDN; installed jars are
//! identified by their SHA-1 (Modrinth) or CurseForge fingerprint, so mods that were
//! uploaded by hand are recognised too.

use indexmap::IndexMap;
use shared::{
    State,
    extensions::{Extension, ExtensionPermissionsBuilder, ExtensionRouteBuilder},
    permissions::PermissionGroup,
};
use std::sync::Arc;

mod admin;
mod crash;
mod ctx;
mod detect;
mod files;
mod installed;
mod modlist;
mod modpack;
mod pack;
mod providers;
mod routes;
mod runtime;
mod serverpack;
mod staging;
mod settings;
mod update;

#[derive(Default)]
pub struct ExtensionStruct;

#[async_trait::async_trait]
impl Extension for ExtensionStruct {
    async fn initialize_router(&mut self, state: State, builder: ExtensionRouteBuilder) -> ExtensionRouteBuilder {
        builder
            .add_client_server_api_router(|router| router.nest("/mod-manager", routes::router(&state)))
            .add_admin_api_router(|router| router.nest("/extensions/dev.emrekmek.modmanager", admin::router(&state)))
    }

    async fn initialize_permissions(
        &mut self,
        _state: State,
        builder: ExtensionPermissionsBuilder,
    ) -> ExtensionPermissionsBuilder {
        builder.add_server_permission_group(
            "mod-manager",
            PermissionGroup {
                description: "Mod Manager: browse and manage the server's Minecraft mods and plugins.",
                permissions: IndexMap::from([
                    ("read", "Allows browsing Modrinth / CurseForge and viewing the installed mods and plugins."),
                    ("manage", "Allows installing, updating, downgrading, disabling and removing mods and plugins."),
                ]),
            },
        )
    }

    async fn settings_deserializer(&self, _state: State) -> shared::extensions::settings::ExtensionSettingsDeserializer {
        Arc::new(settings::ExtensionSettingsDataDeserializer)
    }
}
