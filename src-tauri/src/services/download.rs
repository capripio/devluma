//! Package downloads (trd.md §5 step 3, ADR 0004).
//!
//! - HTTPS only, and the initial URL *and every redirect* must be on the package's host
//!   allowlist (max 5 redirects).
//! - Size is exact: a `Content-Length` above the manifest size is refused up front, the
//!   stream is cut off the moment it exceeds it, and a short body is an error.
//! - Connect timeout plus an idle timeout between chunks; cancellable at any point.
//! - SHA-256 is computed while streaming; the file is created with `create_new`.

use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    /// Tests only: permit `http://127.0.0.1`.
    pub allow_loopback_http: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(30),
            allow_loopback_http: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download host \"{0}\" is not allowed for this package")]
    HostNotAllowed(String),
    #[error("could not reach the download server: {0}")]
    Network(String),
    #[error("the download server answered HTTP {0}")]
    HttpStatus(u16),
    #[error("the download is larger than expected ({expected} bytes)")]
    TooLarge { expected: u64 },
    #[error("the download size did not match: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("the download stalled for more than {0:?}")]
    Stalled(Duration),
    #[error("not enough disk space to save the download")]
    DiskFull,
    #[error("the download was cancelled")]
    Cancelled,
    #[error("io: {0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        // ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL
        if matches!(e.raw_os_error(), Some(112) | Some(39)) {
            Self::DiskFull
        } else {
            Self::Io(e)
        }
    }
}

fn url_allowed(url: &url::Url, allowed_hosts: &[String], opts: &DownloadOptions) -> bool {
    let host = url.host_str().unwrap_or_default();
    let scheme_ok = url.scheme() == "https"
        || (opts.allow_loopback_http && url.scheme() == "http" && host == "127.0.0.1");
    scheme_ok
        && url.username().is_empty()
        && url.password().is_none()
        && allowed_hosts.iter().any(|h| h == host)
}

/// Streams `url` into `dest` (which must not exist), calling `on_progress(received, total)`.
pub async fn download(
    url: &str,
    allowed_hosts: &[String],
    expected_size: u64,
    dest: &Path,
    opts: &DownloadOptions,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<Downloaded, DownloadError> {
    let parsed = url::Url::parse(url).map_err(|_| DownloadError::HostNotAllowed(url.to_owned()))?;
    if !url_allowed(&parsed, allowed_hosts, opts) {
        return Err(DownloadError::HostNotAllowed(
            parsed.host_str().unwrap_or_default().to_owned(),
        ));
    }

    let hosts = allowed_hosts.to_vec();
    let redirect_opts = opts.clone();
    // The policy can only signal "error"; remember which host it refused for the message.
    let refused = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let refused_in_policy = refused.clone();
    let client = reqwest::Client::builder()
        .connect_timeout(opts.connect_timeout)
        .user_agent(concat!("Devluma/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if url_allowed(attempt.url(), &hosts, &redirect_opts) {
                attempt.follow()
            } else {
                let host = attempt.url().host_str().unwrap_or_default().to_owned();
                *refused_in_policy.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
                attempt.error("redirect to a host that is not allowed")
            }
        }))
        .build()
        .map_err(|e| DownloadError::Network(e.to_string()))?;

    let send = client.get(parsed).send();
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
        r = send => r.map_err(|e| {
            if e.is_redirect() {
                let host = refused.lock().unwrap_or_else(|e| e.into_inner()).take();
                DownloadError::HostNotAllowed(host.unwrap_or_else(|| "too many redirects".into()))
            } else {
                DownloadError::Network(without_url(&e))
            }
        })?,
    };
    if !response.status().is_success() {
        return Err(DownloadError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > expected_size)
    {
        return Err(DownloadError::TooLarge {
            expected: expected_size,
        });
    }

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .await?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    on_progress(0, expected_size);
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            c = tokio::time::timeout(opts.idle_timeout, response.chunk()) => match c {
                Err(_) => return Err(DownloadError::Stalled(opts.idle_timeout)),
                Ok(Err(e)) => return Err(DownloadError::Network(without_url(&e))),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(bytes))) => bytes,
            },
        };
        received += chunk.len() as u64;
        if received > expected_size {
            return Err(DownloadError::TooLarge {
                expected: expected_size,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        on_progress(received, expected_size);
    }
    file.flush().await?;
    file.sync_all().await?;
    if received != expected_size {
        return Err(DownloadError::SizeMismatch {
            expected: expected_size,
            actual: received,
        });
    }
    Ok(Downloaded {
        bytes: received,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// reqwest errors embed the URL; keep messages short and free of query strings.
fn without_url(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    if let Some(url) = e.url() {
        msg = msg.replace(url.as_str(), "<url>");
    }
    msg.chars().take(200).collect()
}

#[cfg(test)]
pub(crate) mod test_server {
    use std::io::Read;
    use std::sync::Arc;
    use std::time::Duration;

    /// What the in-process server does for a given path.
    #[derive(Clone)]
    pub enum Route {
        Bytes(Vec<u8>),
        /// Streams a file from disk (large real archives in live tests).
        File(std::path::PathBuf),
        Status(u16),
        Redirect(String),
        /// Sends `head`, then stalls for `pause`.
        Stall {
            head: Vec<u8>,
            total_len: usize,
            pause: Duration,
        },
    }

    pub struct TestServer {
        pub base: String,
        _thread: std::thread::JoinHandle<()>,
    }

    struct StallReader {
        head: std::io::Cursor<Vec<u8>>,
        pause: Duration,
        stalled: bool,
    }

    impl Read for StallReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.head.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            if !self.stalled {
                self.stalled = true;
                std::thread::sleep(self.pause);
            }
            Ok(0)
        }
    }

    pub fn serve(routes: Vec<(&'static str, Route)>) -> TestServer {
        serve_owned(routes.into_iter().map(|(p, r)| (p.to_owned(), r)).collect())
    }

    pub fn serve_owned(routes: Vec<(String, Route)>) -> TestServer {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let thread = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let route = routes
                    .iter()
                    .find(|(p, _)| *p == req.url())
                    .map(|(_, r)| r.clone());
                let _ = match route {
                    Some(Route::Bytes(b)) => req.respond(tiny_http::Response::from_data(b)),
                    Some(Route::File(path)) => match std::fs::File::open(&path) {
                        Ok(f) => {
                            // Big files: answer on another thread so requests don't queue.
                            std::thread::spawn(move || {
                                let _ = req.respond(tiny_http::Response::from_file(f));
                            });
                            Ok(())
                        }
                        Err(_) => req.respond(tiny_http::Response::empty(404)),
                    },
                    Some(Route::Status(code)) => req.respond(tiny_http::Response::empty(code)),
                    Some(Route::Redirect(to)) => {
                        req.respond(tiny_http::Response::empty(302).with_header(
                            tiny_http::Header::from_bytes("Location", to.as_bytes()).unwrap(),
                        ))
                    }
                    Some(Route::Stall {
                        head,
                        total_len,
                        pause,
                    }) => {
                        std::thread::spawn(move || {
                            let reader = StallReader {
                                head: std::io::Cursor::new(head),
                                pause,
                                stalled: false,
                            };
                            let _ = req.respond(tiny_http::Response::new(
                                200.into(),
                                vec![],
                                reader,
                                Some(total_len),
                                None,
                            ));
                        });
                        Ok(())
                    }
                    None => req.respond(tiny_http::Response::empty(404)),
                };
            }
        });
        TestServer {
            base,
            _thread: thread,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{Route, serve};
    use super::*;

    fn opts() -> DownloadOptions {
        DownloadOptions {
            allow_loopback_http: true,
            idle_timeout: Duration::from_millis(500),
            ..Default::default()
        }
    }

    fn loopback() -> Vec<String> {
        vec!["127.0.0.1".into()]
    }

    async fn get(
        url: &str,
        expected: u64,
    ) -> (tempfile::TempDir, Result<Downloaded, DownloadError>) {
        let dir = tempfile::tempdir().unwrap();
        let res = download(
            url,
            &loopback(),
            expected,
            &dir.path().join("dl.zip"),
            &opts(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await;
        (dir, res)
    }

    #[tokio::test]
    async fn downloads_and_hashes() {
        let body = b"hello devluma".to_vec();
        let srv = serve(vec![("/a.zip", Route::Bytes(body.clone()))]);
        let dir = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let got = download(
            &format!("{}/a.zip", srv.base),
            &loopback(),
            body.len() as u64,
            &dir.path().join("a.zip"),
            &opts(),
            &CancellationToken::new(),
            |r, t| progress.push((r, t)),
        )
        .await
        .unwrap();
        assert_eq!(got.sha256, hex::encode(Sha256::digest(&body)));
        assert_eq!(std::fs::read(dir.path().join("a.zip")).unwrap(), body);
        assert_eq!(
            progress.last(),
            Some(&(body.len() as u64, body.len() as u64))
        );
    }

    #[tokio::test]
    async fn rejects_hosts_and_schemes_not_on_the_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let res = download(
            "https://evil.example.net/x.zip",
            &["downloads.example.org".into()],
            10,
            &dir.path().join("x"),
            &DownloadOptions::default(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await;
        assert!(matches!(res, Err(DownloadError::HostNotAllowed(_))));

        let res = download(
            "http://127.0.0.1:1/x.zip",
            &loopback(),
            10,
            &dir.path().join("x"),
            &DownloadOptions::default(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await;
        assert!(
            matches!(res, Err(DownloadError::HostNotAllowed(_))),
            "plain http needs the test-only flag"
        );
    }

    #[tokio::test]
    async fn refuses_redirects_to_other_hosts() {
        let srv = serve(vec![]);
        let port = srv.base.rsplit(':').next().unwrap().to_owned();
        let srv = serve(vec![(
            "/a.zip",
            Route::Redirect(format!("http://localhost:{port}/a.zip")),
        )]);
        let (_d, res) = get(&format!("{}/a.zip", srv.base), 10).await;
        assert!(
            matches!(res, Err(DownloadError::HostNotAllowed(_))),
            "{res:?}"
        );
    }

    #[tokio::test]
    async fn enforces_exact_size() {
        let srv = serve(vec![
            ("/big", Route::Bytes(vec![1; 100])),
            ("/small", Route::Bytes(vec![1; 5])),
        ]);
        let (_d, res) = get(&format!("{}/big", srv.base), 10).await;
        assert!(matches!(res, Err(DownloadError::TooLarge { expected: 10 })));
        let (_d, res) = get(&format!("{}/small", srv.base), 10).await;
        assert!(matches!(
            res,
            Err(DownloadError::SizeMismatch {
                expected: 10,
                actual: 5
            })
        ));
    }

    #[tokio::test]
    async fn reports_http_errors() {
        let srv = serve(vec![("/gone", Route::Status(404))]);
        let (_d, res) = get(&format!("{}/gone", srv.base), 10).await;
        assert!(matches!(res, Err(DownloadError::HttpStatus(404))));
    }

    #[tokio::test]
    async fn stalls_time_out() {
        let srv = serve(vec![(
            "/slow",
            Route::Stall {
                head: vec![1; 4],
                total_len: 10,
                pause: Duration::from_secs(5),
            },
        )]);
        let (_d, res) = get(&format!("{}/slow", srv.base), 10).await;
        assert!(matches!(res, Err(DownloadError::Stalled(_))), "{res:?}");
    }

    /// Opt-in check that HTTPS works through the real TLS stack and Windows trust store:
    /// `cargo test live_https -- --ignored`. Not run by default (no network in unit tests).
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn live_https_reaches_an_official_source() {
        let dir = tempfile::tempdir().unwrap();
        let res = download(
            "https://windows.php.net/downloads/releases/sha256sum.txt",
            // windows.php.net redirects to downloads.php.net; both must be allowlisted.
            &["windows.php.net".into(), "downloads.php.net".into()],
            // Deliberately huge so the exact-size check, not TLS, decides the outcome.
            10 * 1024 * 1024,
            &dir.path().join("sums.txt"),
            &DownloadOptions::default(),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await;
        match res {
            Err(DownloadError::SizeMismatch { actual, .. }) => assert!(actual > 0),
            other => {
                panic!("expected a completed TLS download with a size mismatch, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn cancellation_stops_a_stalled_download() {
        let srv = serve(vec![(
            "/slow",
            Route::Stall {
                head: vec![1; 4],
                total_len: 10,
                pause: Duration::from_secs(5),
            },
        )]);
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        let mut o = opts();
        o.idle_timeout = Duration::from_secs(10);
        let started = std::time::Instant::now();
        let res = download(
            &format!("{}/slow", srv.base),
            &loopback(),
            10,
            &dir.path().join("x"),
            &o,
            &cancel,
            |_, _| {},
        )
        .await;
        assert!(matches!(res, Err(DownloadError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
