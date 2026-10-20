//! CurseForge server packs: a zip of the ready-to-run server that a modpack file can link
//! to (`serverPackFileId`), with client-only mods already left out by the author.
//!
//! Like every pack archive it is downloaded, checked and unpacked on the node (see
//! [`staging`]); its files are then moved into place without the zip's top-level folder.

use crate::{
    ctx::{Ctx, display},
    modpack::{Job, MAX_PACK_BYTES, safe_path, step},
    providers::{Provider, Version, curseforge},
    staging::{self, SERVER_PACK_DIR},
};
use serde::Serialize;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct ServerPack {
    pub name: String,
    pub size: u64,
    /// `None` when the author blocks third-party downloads.
    pub url: Option<String>,
    pub sha1: Option<String>,
}

/// The server pack CurseForge links to this modpack file, if any.
pub async fn find(ctx: &Ctx, version: &Version) -> Result<Option<ServerPack>, anyhow::Error> {
    if version.provider != Provider::Curseforge {
        return Ok(None);
    }
    let Ok(id) = version.id.parse::<u64>() else { return Ok(None) };
    let cf = ctx.curseforge()?;
    let Some(pack_id) = cf.files_raw(&[id]).await?.first().and_then(|f| f["serverPackFileId"].as_u64()) else {
        return Ok(None);
    };
    let Some(raw) = cf.files_raw(&[pack_id]).await?.pop() else { return Ok(None) };
    let Some(file) = curseforge::version(&raw).file else { return Ok(None) };
    Ok(Some(ServerPack { name: file.name, size: file.size, url: file.url, sha1: file.sha1 }))
}

struct Listing {
    /// (path inside the zip, path on the server)
    entries: Vec<(String, String)>,
    rejected: Vec<String>,
}

/// Folders a server root has; a zip whose single top folder holds these wraps the server.
const SERVER_FOLDERS: [&str; 5] = ["mods/", "config/", "defaultconfigs/", "kubejs/", "libraries/"];

/// The folder every entry sits in ("ServerFiles-1.2/"), or "" when there is none or it is
/// itself a server folder (a zip of just mods/).
fn common_folder(names: &[String]) -> String {
    let Some(first) = names.first().and_then(|n| n.split_once('/')).map(|(dir, _)| format!("{dir}/")) else {
        return String::new();
    };
    let wraps = names.iter().all(|n| n.starts_with(&first) && n.len() > first.len())
        && names.iter().any(|n| SERVER_FOLDERS.iter().any(|f| n[first.len()..].starts_with(f)));
    if wraps { first } else { String::new() }
}

/// Map zip entries to server paths, leaving out unsafe ones (blocking).
fn entries(names: Vec<String>) -> Listing {
    let prefix = common_folder(&names);
    let mut listing = Listing { entries: Vec::new(), rejected: Vec::new() };
    for name in names {
        match safe_path(&name[prefix.len()..]) {
            Some(path) if !staging::is_staging(&path) => listing.entries.push((name, path)),
            _ => listing.rejected.push(name),
        }
    }
    listing
}

/// Put the server pack's files on the server, replacing files with the same path.
pub async fn install(ctx: &Ctx, pack: &ServerPack, job: &Mutex<Job>) -> Result<(), anyhow::Error> {
    let url = pack
        .url
        .as_deref()
        .ok_or_else(|| display("The server pack cannot be downloaded through third-party apps."))?;
    if pack.size > MAX_PACK_BYTES {
        return Err(display(format!("The server pack is larger than {} MiB.", MAX_PACK_BYTES >> 20)));
    }
    step(ctx, job, |j| j.step = format!("Downloading and unpacking the server pack {} on the node", pack.name)).await;
    let names: Vec<String> = staging::stage(ctx, SERVER_PACK_DIR, url, pack.sha1.as_deref())
        .await?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    if names.is_empty() {
        return Err(display("The server pack is empty."));
    }
    let listing = entries(names);
    let total = listing.entries.len();
    step(ctx, job, |j| {
        j.total = total;
        j.warnings.extend(listing.rejected.iter().map(|r| format!("Left out (unsafe path): {r}")));
        j.step = format!("Moving {total} files into place");
    })
    .await;

    let mut done = 0;
    let moved = staging::move_into_place(ctx, SERVER_PACK_DIR, &listing.entries, |n| done += n).await?;
    step(ctx, job, |j| {
        j.done += done;
        j.installed = moved;
        if moved < total {
            j.failed.push(format!("{} of {total} server pack files could not be moved into place", total - moved));
        }
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn strips_one_top_folder() {
        assert_eq!(common_folder(&names(&["Pack 1.2/mods/a.jar", "Pack 1.2/config/b.toml"])), "Pack 1.2/");
        assert_eq!(common_folder(&names(&["mods/a.jar", "config/b.toml"])), "");
        assert_eq!(common_folder(&names(&["mods/a.jar", "server.properties"])), "");
        assert_eq!(common_folder(&names(&["mods/a.jar", "mods/b.jar"])), "");
        assert_eq!(common_folder(&names(&["Pack/readme.txt"])), "");
        assert_eq!(common_folder(&names(&[])), "");
    }

    #[test]
    fn maps_entries() {
        let l = entries(names(&["S/mods/a.jar", "S/../evil", "S/.modmanager-serverpack/x", "S/start.sh"]));
        assert_eq!(
            l.entries,
            vec![("S/mods/a.jar".to_string(), "mods/a.jar".to_string()), ("S/start.sh".into(), "start.sh".into())]
        );
        assert_eq!(l.rejected, names(&["S/../evil", "S/.modmanager-serverpack/x"]));
    }
}
