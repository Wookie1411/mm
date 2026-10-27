//! Pack archives are downloaded and unpacked on the node that hosts the server, never on
//! the panel: Wings pulls the zip into a hidden folder in the server, checks its SHA-1,
//! unpacks it there, and files are moved into place from it. The panel only reads the
//! pack's index and a listing of the unpacked files, through Wings.
//!
//! A marker file records which archive a folder holds, so the install can reuse what the
//! preview already unpacked. Callers delete the folder when they are done with it.

use crate::{
    ctx::{Ctx, display},
    files::ServerFiles,
};
use serde_json::Value;
use wings_api::Algorithm;

/// The regular pack (also the client pack a server pack belongs to).
pub const PACK_DIR: &str = ".modmanager-pack";
/// A CurseForge server pack.
pub const SERVER_PACK_DIR: &str = ".modmanager-serverpack";
const ARCHIVE: &str = "pack.zip";
const MARKER: &str = ".modmanager-source";
/// Paths per delete / move request.
const BATCH: usize = 500;

/// Whether `path` is inside one of the staging folders (never a pack target).
pub fn is_staging(path: &str) -> bool {
    [PACK_DIR, SERVER_PACK_DIR].iter().any(|d| path == *d || path.starts_with(&format!("{d}/")))
}

fn marker_for(url: &str, sha1: Option<&str>) -> String {
    format!("{url}\n{}", sha1.unwrap_or_default())
}

/// Make sure `dir` holds the unpacked archive at `url`, downloading and unpacking it on
/// the node when it does not yet. Returns every unpacked file (path in `dir`, size).
pub async fn stage(ctx: &Ctx, dir: &str, url: &str, sha1: Option<&str>) -> Result<Vec<(String, u64)>, anyhow::Error> {
    let marker = marker_for(url, sha1);
    let marker_path = format!("{dir}/{MARKER}");
    let current = ctx.files.read_bytes(&marker_path, 64 * 1024).await.ok().flatten();
    if current.as_deref() != Some(marker.as_bytes()) {
        clear(&ctx.files, dir).await;
        ctx.files.create_directory(dir).await?;
        ctx.files.pull(dir, ARCHIVE, url).await?;
        let archive = format!("{dir}/{ARCHIVE}");
        if let Some(want) = sha1 {
            let got = ctx.files.fingerprints(std::slice::from_ref(&archive), Algorithm::Sha1).await;
            if got.get(&archive).map(|h| h.to_lowercase()).as_deref() != Some(want) {
                clear(&ctx.files, dir).await;
                return Err(display("The downloaded modpack is damaged (SHA-1 mismatch). Try again."));
            }
        }
        ctx.files.decompress(dir, ARCHIVE).await?;
        ctx.files.delete(dir, &[ARCHIVE.to_string()]).await?;
        // some authors wrap the real pack in another zip (with a readme next to it)
        for _ in 0..2 {
            let Some(inner) = wrapped_archive(&ctx.files.walk(dir).await?) else { break };
            let (parent, name) = match inner.rsplit_once('/') {
                Some((parent, name)) => (format!("{dir}/{parent}"), name.to_string()),
                None => (dir.to_string(), inner.clone()),
            };
            ctx.files.decompress(&parent, &name).await?;
            ctx.files.delete(&parent, &[name]).await?;
        }
        // last, so a half-unpacked folder is never reused
        ctx.files.write_bytes(&marker_path, marker.into_bytes()).await?;
    }
    let mut files = ctx.files.walk(dir).await?;
    files.retain(|(path, _)| path != MARKER);
    Ok(files)
}

/// Files that can sit next to a wrapped pack.
const NOTE_FILES: [&str; 4] = [".txt", ".md", ".url", ".html"];

/// The one zip an unpacked archive holds when that zip is the actual pack: no other
/// files but a few notes (a readme, a link). The marker file is ignored.
fn wrapped_archive(files: &[(String, u64)]) -> Option<String> {
    let files: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).filter(|p| *p != MARKER).collect();
    let mut zips = files.iter().filter(|p| p.to_lowercase().ends_with(".zip"));
    let zip = zips.next()?;
    if zips.next().is_some() || files.len() > 6 {
        return None;
    }
    let notes = files.iter().filter(|p| *p != zip).all(|p| NOTE_FILES.iter().any(|e| p.to_lowercase().ends_with(e)));
    notes.then(|| zip.to_string())
}

/// The pack's index file, if the unpacked folder has it.
pub async fn read_json(ctx: &Ctx, dir: &str, name: &str) -> Option<Value> {
    let bytes = ctx.files.read_bytes(&format!("{dir}/{name}"), 32 * 1024 * 1024).await.ok().flatten()?;
    serde_json::from_slice(&bytes).ok()
}

/// Delete a staging folder (nothing happens when it is not there).
pub async fn clear(files: &ServerFiles, dir: &str) {
    let _ = files.delete("/", &[dir.to_string()]).await;
}

/// Move unpacked files (path in `dir`, target) into place, replacing what is there.
/// Returns how many were moved.
pub async fn move_into_place(
    ctx: &Ctx,
    dir: &str,
    entries: &[(String, String)],
    mut progress: impl FnMut(usize),
) -> Result<usize, anyhow::Error> {
    // Wings never moves over an existing file, so clear the targets first
    let targets: Vec<String> = entries.iter().map(|(_, to)| to.clone()).collect();
    for chunk in targets.chunks(BATCH) {
        ctx.files.delete("/", chunk).await?;
    }
    let mut moved = 0;
    for chunk in entries.chunks(BATCH) {
        let pairs: Vec<(String, String)> = chunk.iter().map(|(from, to)| (format!("{dir}/{from}"), to.clone())).collect();
        moved += ctx.files.move_many("/", &pairs).await? as usize;
        progress(chunk.len());
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_paths() {
        assert!(is_staging(".modmanager-pack"));
        assert!(is_staging(".modmanager-pack/overrides/a"));
        assert!(is_staging(".modmanager-serverpack/x"));
        assert!(!is_staging(".modmanager-packs/x"));
        assert!(!is_staging("config/.modmanager-pack"));
    }

    fn files(n: &[&str]) -> Vec<(String, u64)> {
        n.iter().map(|s| (s.to_string(), 1)).collect()
    }

    #[test]
    fn wrapped_zip() {
        let w = |n: &[&str]| wrapped_archive(&files(n));
        assert_eq!(w(&["readme.txt", "Pack_server_pack.zip"]).as_deref(), Some("Pack_server_pack.zip"));
        assert_eq!(w(&["Pack/Server.ZIP", MARKER]).as_deref(), Some("Pack/Server.ZIP"));
        assert_eq!(w(&["mods/a.jar", "server.zip"]), None);
        assert_eq!(w(&["a.zip", "b.zip"]), None);
        assert_eq!(w(&["readme.txt"]), None);
        assert_eq!(w(&["config/x.txt", "start.sh", "pack.zip"]), None);
    }
}
