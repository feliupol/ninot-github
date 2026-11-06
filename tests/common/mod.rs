//! A small REST client shared by the integration tests.

#![allow(dead_code)]

use ninot_github::Server;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::redirect::Policy;
use serde_json::Value;

/// Calls the API of `server` as the holder of `token`.
pub struct Api<'a> {
    pub server: &'a Server,
    pub token: Option<String>,
    client: Client,
}

/// Status, headers and JSON body of a response.
pub struct Reply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

impl<'a> Api<'a> {
    pub fn new(server: &'a Server, token: Option<&str>) -> Self {
        Self {
            server,
            token: token.map(str::to_owned),
            client: Client::builder().redirect(Policy::none()).build().unwrap(),
        }
    }

    pub fn admin(server: &'a Server) -> Self {
        Self::new(server, Some(server.admin_token()))
    }

    fn send(&self, request: RequestBuilder) -> Reply {
        let request = match &self.token {
            Some(token) => request.header("Authorization", format!("token {token}")),
            None => request,
        };
        let response = request.send().unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().unwrap();
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.url())
    }

    pub fn get(&self, path: &str) -> Reply {
        self.send(self.client.get(self.url(path)))
    }

    pub fn delete(&self, path: &str) -> Reply {
        self.send(self.client.delete(self.url(path)))
    }

    pub fn delete_json(&self, path: &str, body: Value) -> Reply {
        self.send(self.client.delete(self.url(path)).json(&body))
    }

    pub fn post(&self, path: &str, body: Value) -> Reply {
        self.send(self.client.post(self.url(path)).json(&body))
    }

    /// Posts `body` as is, JSON or not.
    pub fn post_raw(&self, path: &str, body: &str) -> Reply {
        self.send(
            self.client
                .post(self.url(path))
                .header("content-type", "application/json")
                .body(body.to_owned()),
        )
    }

    pub fn patch(&self, path: &str, body: Value) -> Reply {
        self.send(self.client.patch(self.url(path)).json(&body))
    }

    pub fn put(&self, path: &str, body: Value) -> Reply {
        self.send(self.client.put(self.url(path)).json(&body))
    }
}

/// A scratch directory removed when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ninot-github-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn join(&self, path: &str) -> std::path::PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs git in `dir`, isolated from the user's configuration and never prompting.
pub fn git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap()
}

/// Runs git and panics with its stderr unless it succeeds; returns trimmed stdout.
pub fn git_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Clone URL of `owner/repo`, authenticated with `token` when given.
pub fn clone_url(server: &Server, owner: &str, repo: &str, token: Option<&str>) -> String {
    let base = server.url().strip_prefix("http://").unwrap();
    match token {
        Some(token) => format!("http://x-access-token:{token}@{base}/{owner}/{repo}.git"),
        None => format!("http://{base}/{owner}/{repo}.git"),
    }
}

/// Commits `file` with `content` in the work tree `dir`.
pub fn commit_file(dir: &std::path::Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    git_ok(dir, &["add", file]);
    git_ok(dir, &["commit", "-q", "-m", &format!("Add {file}")]);
}

/// One HTTP request captured by a [`Receiver`].
pub struct Captured {
    /// Header names lowercased.
    pub headers: std::collections::HashMap<String, String>,
    pub body: Vec<u8>,
}

/// A local HTTP endpoint that records requests and answers with a fixed status.
pub struct Receiver {
    pub url: String,
    requests: std::sync::mpsc::Receiver<Captured>,
}

impl Receiver {
    /// Serves on a free local port from its own thread and runtime, answering every request
    /// with `status`.
    pub fn start(status: u16) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let (sender, requests) = std::sync::mpsc::channel();
        let status = axum::http::StatusCode::from_u16(status).unwrap();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let record = move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let headers = headers
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.as_str().to_owned(),
                                value.to_str().unwrap_or("").to_owned(),
                            )
                        })
                        .collect();
                    let _ = sender.send(Captured {
                        headers,
                        body: body.to_vec(),
                    });
                    std::future::ready(status)
                };
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, axum::Router::new().fallback(record))
                    .await
                    .unwrap();
            });
        });
        Self { url, requests }
    }

    /// The next captured request, or a panic after five seconds.
    pub fn next(&self) -> Captured {
        self.requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("no webhook request arrived")
    }
}
