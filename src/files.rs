//! Thin wrapper around the Wings file API for one server.

use axum::http::StatusCode;
use compact_str::{CompactString, ToCompactString};
use indexmap::IndexMap;
use shared::response::DisplayError;
use wings_api::{
    Algorithm,
    client::{ApiHttpError, WingsClient},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub file: bool,
    pub directory: bool,
    /// Unix seconds.
    pub modified: i64,
}

pub struct ServerFiles {
    pub client: WingsClient,
    pub server: uuid::Uuid,
}

fn is_missing(e: &ApiHttpError) -> bool {
    matches!(e, ApiHttpError::Http(StatusCode::NOT_FOUND, _))
}

fn wings_error(what: &str, path: &str, e: ApiHttpError) -> anyhow::Error {
    let message = match &e {
        ApiHttpError::Http(status, err) => describe_http(*status, &err.error),
        _ => "the node did not answer".to_string(),
    };
    tracing::warn!(path, "modmanager: wings {what} failed: {:?}", anyhow::Error::from(e));

    DisplayError::new(format!("Could not {what} {path} through Wings ({message})."))
    .with_status(StatusCode::BAD_GATEWAY)
    .into()
}

/// Wings answers with an `ApiError`. When the body is something else (the client then
/// reports it could not decode it), a proxy in front of Wings answered, usually with an
/// HTML error page, so say that instead of the decoder's message.
fn describe_http(status: StatusCode, error: &str) -> String {
    let code = status.as_u16();
    if !error.contains("expected struct ApiError") {
        return format!("HTTP {code}: {error}");
    }
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => format!(
            "HTTP {code}: the file is too large for a proxy in front of Wings; raise its upload limit, e.g. client_max_body_size in nginx"
        ),
        _ => format!("HTTP {code} from a proxy in front of Wings, not from Wings itself"),
    }
}

/// Worth another try: the node was busy or briefly unreachable, or the server is at its
/// concurrent-pull limit (Wings' `limits.server_concurrent_pulls`, 3 by default).
fn transient(e: &ApiHttpError) -> bool {
    match e {
        ApiHttpError::Reqwest(_) => true,
        ApiHttpError::Http(status, err) => {
            matches!(
                *status,
                StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
            ) || (*status == StatusCode::EXPECTATION_FAILED && err.error.contains("too many concurrent"))
        }
        _ => false,
    }
}

const RETRIES: u32 = 6;
const BACKOFF: std::time::Duration =
    if cfg!(test) { std::time::Duration::from_millis(1) } else { std::time::Duration::from_secs(1) };

/// Run a Wings call, retrying transient failures with backoff (1, 2, 4, 8, 16 s), long
/// enough to ride out a Wings restart.
async fn retry<T, F, Fut>(mut call: F) -> Result<T, ApiHttpError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiHttpError>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Err(e) if attempt + 1 < RETRIES && transient(&e) => {
                tokio::time::sleep(BACKOFF * (1 << attempt)).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// Most files [`ServerFiles::walk`] lists.
const WALK_LIMIT: u64 = 200_000;

/// Paths per fingerprint request, so the query string stays short.
const FINGERPRINT_BATCH: usize = 40;

impl ServerFiles {
    pub async fn open(
        state: &shared::State,
        server: &shared::models::server::Server,
    ) -> Result<Self, anyhow::Error> {
        let client = server
            .node
            .fetch_cached(&state.database)
            .await?
            .api_client(&state.database)
            .await?
            .ignoring(server.subuser_ignored_files.clone().unwrap_or_default());

        Ok(Self {
            client,
            server: server.uuid,
        })
    }

    /// List a directory, `None` when it does not exist.
    pub async fn list(&self, directory: &str) -> Result<Option<Vec<Entry>>, anyhow::Error> {
        match self
            .client
            .get_servers_server_files_list_directory(
                self.server,
                &wings_api::servers_server_files_list_directory::get::Query {
                    directory: Some(directory.to_compact_string()),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(entries) => Ok(Some(
                entries
                    .into_iter()
                    .map(|e| Entry {
                        name: e.name.to_string(),
                        size: e.size,
                        file: e.file,
                        directory: e.directory,
                        modified: e.modified.timestamp(),
                    })
                    .collect(),
            )),
            Err(e) if is_missing(&e) => Ok(None),
            // a path that is a file, or a missing parent, is "not a directory" too
            Err(ApiHttpError::Http(StatusCode::BAD_REQUEST | StatusCode::EXPECTATION_FAILED, _)) => Ok(None),
            Err(e) => Err(wings_error("list", directory, e)),
        }
    }

    /// A whole file, `None` when it does not exist; an error when it is larger than `max_size`.
    pub async fn read_bytes(&self, path: &str, max_size: u64) -> Result<Option<Vec<u8>>, anyhow::Error> {
        use tokio::io::AsyncReadExt;
        let mut reader = match self
            .client
            .get_servers_server_files_contents(
                self.server,
                &wings_api::servers_server_files_contents::get::Query {
                    file: Some(path.to_compact_string()),
                    max_size: Some(max_size),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(reader) => reader,
            Err(e) if is_missing(&e) => return Ok(None),
            Err(ApiHttpError::Http(StatusCode::PAYLOAD_TOO_LARGE, _)) => {
                return Err(DisplayError::new(format!("{path} is larger than {} MiB.", max_size >> 20))
                    .with_status(StatusCode::PAYLOAD_TOO_LARGE)
                    .into());
            }
            Err(e) => return Err(wings_error("read", path, e)),
        };
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await?;

        Ok(Some(out))
    }

    /// The first `lines` lines of a text file, `None` when it is missing or unreadable.
    pub async fn head(&self, path: &str, lines: u64) -> Option<String> {
        self.client
            .get_servers_server_files_lines(
                self.server,
                &wings_api::servers_server_files_lines::get::Query {
                    file: Some(path.to_compact_string()),
                    start_line: Some(1),
                    end_line: Some(lines),
                    max_size: Some(1024 * 1024),
                    ..Default::default()
                },
            )
            .await
            .ok()
            .map(|r| r.content.to_string())
    }

    /// Hashes of files (paths relative to the server root), keyed by the path as given.
    /// Missing or unreadable files are simply absent from the result.
    pub async fn fingerprints(&self, paths: &[String], algorithm: Algorithm) -> IndexMap<String, String> {
        let mut out = IndexMap::new();
        for chunk in paths.chunks(FINGERPRINT_BATCH) {
            let result = self
                .client
                .get_servers_server_files_fingerprints(
                    self.server,
                    &wings_api::servers_server_files_fingerprints::get::Query {
                        algorithm: Some(algorithm),
                        files: Some(chunk.iter().map(|p| p.to_compact_string()).collect()),
                        ..Default::default()
                    },
                )
                .await;
            match result {
                Ok(r) => {
                    for (path, hash) in r.fingerprints {
                        out.insert(path.trim_start_matches('/').to_string(), hash.to_string());
                    }
                }
                Err(e) => {
                    tracing::warn!("modmanager: wings fingerprints ({algorithm}) failed: {:?}", anyhow::Error::from(e));
                }
            }
        }

        out
    }

    /// Download `url` into `directory/file_name` and wait until it is there.
    pub async fn pull(&self, directory: &str, file_name: &str, url: &str) -> Result<(), anyhow::Error> {
        let body = wings_api::servers_server_files_pull::post::RequestBody {
            root: directory.to_compact_string(),
            url: url.to_compact_string(),
            file_name: Some(file_name.to_compact_string()),
            use_header: false,
            foreground: true,
        };
        retry(|| self.client.post_servers_server_files_pull(self.server, &body))
            .await
            .map_err(|e| wings_error("download", &format!("{directory}/{file_name}"), e))?;

        Ok(())
    }

    /// Every file under `root`, recursively: (path relative to `root`, size).
    pub async fn walk(&self, root: &str) -> Result<Vec<(String, u64)>, anyhow::Error> {
        let body = wings_api::servers_server_files_search::post::RequestBody {
            root: root.to_compact_string(),
            path_filter: None,
            size_filter: None,
            content_filter: None,
            per_page: WALK_LIMIT,
        };
        let found = retry(|| self.client.post_servers_server_files_search(self.server, &body))
            .await
            .map_err(|e| wings_error("list", root, e))?;

        Ok(found
            .results
            .into_iter()
            .filter(|e| e.file)
            .map(|e| (e.name.trim_start_matches('/').to_string(), e.size))
            .collect())
    }

    /// Unpack `directory/file` (a zip) into `directory`, on the node, and wait for it.
    pub async fn decompress(&self, directory: &str, file: &str) -> Result<(), anyhow::Error> {
        let body = wings_api::servers_server_files_decompress::post::RequestBody {
            root: directory.to_compact_string(),
            file: file.to_compact_string(),
            foreground: true,
        };
        retry(|| self.client.post_servers_server_files_decompress(self.server, &body))
            .await
            .map_err(|e| wings_error("unpack", &format!("{directory}/{file}"), e))?;

        Ok(())
    }

    /// Move files (from, to) relative to `root`, creating parent folders. Wings skips a
    /// move whose target exists; returns how many were moved.
    pub async fn move_many(&self, root: &str, pairs: &[(String, String)]) -> Result<u64, anyhow::Error> {
        let body = wings_api::servers_server_files_rename::put::RequestBody {
            root: root.to_compact_string(),
            files: pairs
                .iter()
                .map(|(from, to)| wings_api::servers_server_files_rename::put::RequestBodyFiles {
                    from: from.to_compact_string(),
                    to: to.to_compact_string(),
                })
                .collect(),
        };
        let extra = wings_api::servers_server_files_rename::put::Extra {
            create_directories: true,
            ..Default::default()
        };
        let moved = retry(|| self.client.put_servers_server_files_rename_with(self.server, &body, &extra))
            .await
            .map_err(|e| wings_error("move", &format!("{} files in {root}", pairs.len()), e))?;

        Ok(moved.renamed)
    }

    /// Write (create or overwrite) a file.
    pub async fn write_bytes(&self, path: &str, content: Vec<u8>) -> Result<(), anyhow::Error> {
        let query = wings_api::servers_server_files_write::post::Query {
            file: Some(path.to_compact_string()),
            ..Default::default()
        };
        retry(|| {
            self.client.post_servers_server_files_write(
                self.server,
                wings_api::client::AsyncRequestReader::new(std::io::Cursor::new(content.clone())),
                &query,
            )
        })
        .await
        .map_err(|e| wings_error("write", path, e))?;

        Ok(())
    }

    pub async fn create_directory(&self, directory: &str) -> Result<(), anyhow::Error> {
        let (root, name) = match directory.rsplit_once('/') {
            Some((root, name)) => (root, name),
            None => ("/", directory),
        };
        match self
            .client
            .post_servers_server_files_create_directory(
                self.server,
                &wings_api::servers_server_files_create_directory::post::RequestBody {
                    root: root.to_compact_string(),
                    name: name.to_compact_string(),
                },
            )
            .await
        {
            Ok(_) => Ok(()),
            // already there
            Err(ApiHttpError::Http(StatusCode::EXPECTATION_FAILED | StatusCode::CONFLICT, _)) => Ok(()),
            Err(e) => Err(wings_error("create", directory, e)),
        }
    }

    pub async fn delete(&self, directory: &str, names: &[String]) -> Result<(), anyhow::Error> {
        self.client
            .post_servers_server_files_delete(
                self.server,
                &wings_api::servers_server_files_delete::post::RequestBody {
                    root: directory.to_compact_string(),
                    files: names.iter().map(CompactString::from).collect(),
                },
            )
            .await
            .map_err(|e| wings_error("delete", &format!("{directory}/{}", names.join(", ")), e))?;

        Ok(())
    }

    pub async fn rename(&self, directory: &str, from: &str, to: &str) -> Result<(), anyhow::Error> {
        self.client
            .put_servers_server_files_rename(
                self.server,
                &wings_api::servers_server_files_rename::put::RequestBody {
                    root: directory.to_compact_string(),
                    files: vec![wings_api::servers_server_files_rename::put::RequestBodyFiles {
                        from: from.to_compact_string(),
                        to: to.to_compact_string(),
                    }],
                },
            )
            .await
            .map_err(|e| wings_error("rename", &format!("{directory}/{from}"), e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: StatusCode, error: &str) -> ApiHttpError {
        ApiHttpError::Http(status, wings_api::ApiError { error: error.into() })
    }

    #[test]
    fn proxy_errors() {
        let decode = "invalid type: integer 60, expected struct ApiError";
        assert!(describe_http(StatusCode::PAYLOAD_TOO_LARGE, decode).contains("client_max_body_size"));
        assert!(describe_http(StatusCode::BAD_GATEWAY, decode).contains("proxy"));
        assert_eq!(describe_http(StatusCode::EXPECTATION_FAILED, "failed to pull file"), "HTTP 417: failed to pull file");
    }

    #[test]
    fn transient_errors() {
        assert!(transient(&http(StatusCode::EXPECTATION_FAILED, "too many concurrent pulls")));
        assert!(transient(&http(StatusCode::BAD_GATEWAY, "")));
        assert!(transient(&http(StatusCode::TOO_MANY_REQUESTS, "")));
        assert!(!transient(&http(StatusCode::EXPECTATION_FAILED, "remote pulling is disabled")));
        assert!(!transient(&http(StatusCode::EXPECTATION_FAILED, "failed to pull file")));
        assert!(!transient(&http(StatusCode::NOT_FOUND, "path not found")));
    }

    #[tokio::test]
    async fn retries_then_gives_up() {
        let mut calls = 0;
        let r: Result<(), _> = retry(|| {
            calls += 1;
            async { Err(http(StatusCode::BAD_GATEWAY, "")) }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(calls, RETRIES);

        let mut calls = 0;
        let r = retry(|| {
            calls += 1;
            let n = calls;
            async move { if n < 3 { Err(http(StatusCode::EXPECTATION_FAILED, "too many concurrent pulls")) } else { Ok(n) } }
        })
        .await;
        assert_eq!(r.unwrap(), 3);
    }
}
