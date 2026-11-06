//! The official Actions runner registers with the fake, runs a job and is removed from it.
//!
//! Runs the unmodified `actions/runner` release binary against the fake. Needs the runner
//! tarball (downloaded once, verified against `RUNNER_SHA256` and cached), `curl`, `tar`, the
//! runner's own system libraries and an unprivileged user and network namespace (`unshare`).
//! Opt in with `NINOT_RUNNER_E2E=1`; the CI workflow sets it.
//!
//! The runner drops the port of its `--url`, so the fake must listen on 443: the test re-runs
//! itself in a user and network namespace, where it may bind 443 and `config.sh` needs
//! `RUNNER_ALLOW_RUNASROOT`. Each test gets its own namespace, even as root, so tests running in
//! parallel never compete for the port. `NINOT_RUNNER_TARBALL` names an already downloaded
//! tarball.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::TempDir;
use ninot_github::Server;
use ninot_github::server::Options;
use serde_json::Value;
use sha2::{Digest, Sha256};

const RUNNER_VERSION: &str = "2.338.2";
const RUNNER_SHA256: &str = "baebf204307bac6193480852732f0aba2ae97b6c5597c244f4d5558390147c40";
const ORG: &str = "fakeorg";
const REPO: &str = "ci";
const RUNNER: &str = "e2e-runner";
const IN_NAMESPACE: &str = "NINOT_RUNNER_E2E_IN_NAMESPACE";
const JOB_TIMEOUT: Duration = Duration::from_secs(120);

/// Whether the test is switched on. Without `NINOT_RUNNER_E2E=1` it reports so and passes.
fn enabled() -> bool {
    let on = std::env::var("NINOT_RUNNER_E2E").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set NINOT_RUNNER_E2E=1 to run the Actions runner against the fake");
    }
    on
}

/// The runner tarball: the one `NINOT_RUNNER_TARBALL` names, else the cached download.
fn tarball() -> PathBuf {
    static TARBALL: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    TARBALL.get_or_init(fetch_tarball).clone()
}

fn fetch_tarball() -> PathBuf {
    let path = match std::env::var_os("NINOT_RUNNER_TARBALL") {
        Some(path) => PathBuf::from(path),
        None => {
            let cache = std::env::var_os("NINOT_RUNNER_CACHE").map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("target/runner-cache"),
                PathBuf::from,
            );
            std::fs::create_dir_all(&cache).unwrap();
            let path = cache.join(format!("actions-runner-linux-x64-{RUNNER_VERSION}.tar.gz"));
            if !path.exists() {
                let partial = path.with_extension("partial");
                let url = format!(
                    "https://github.com/actions/runner/releases/download/v{RUNNER_VERSION}/{}",
                    path.file_name().unwrap().to_str().unwrap()
                );
                let status = Command::new("curl")
                    .args(["-fsSL", "--retry", "3", "-o"])
                    .arg(&partial)
                    .arg(url)
                    .status()
                    .unwrap();
                assert!(status.success(), "downloading the runner failed");
                std::fs::rename(partial, &path).unwrap();
            }
            path
        }
    };
    let digest = Sha256::digest(std::fs::read(&path).unwrap());
    let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        digest,
        RUNNER_SHA256,
        "{} is not the pinned runner",
        path.display()
    );
    path
}

/// Re-runs `test` in a user and network namespace, where it may bind port 443.
///
/// Returns `true` in the outer process once the inner one has passed, and `false` inside the
/// namespace, where the test goes on. Each test gets its own namespace, so tests running in
/// parallel never compete for port 443.
fn outer(test: &str) -> bool {
    if std::env::var_os(IN_NAMESPACE).is_some() {
        return false;
    }
    let tarball = tarball();
    let status = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "sh", "-c"])
        .arg(r#"ip link set lo up && exec "$0" "$@""#)
        .arg(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .env(IN_NAMESPACE, "1")
        .env("NINOT_RUNNER_E2E", "1")
        .env("NINOT_RUNNER_TARBALL", tarball)
        .status()
        .unwrap();
    assert!(status.success(), "the test failed in its namespace");
    true
}

/// Calls the fake's REST API with curl, which trusts the fake's certificate authority.
fn curl(server: &Server, method: &str, path: &str) -> Value {
    let output = Command::new("curl")
        .args(["-sS", "--fail-with-body", "-X", method])
        .arg("--cacert")
        .arg(server.ca_cert_path().unwrap())
        .arg("-H")
        .arg(format!("Authorization: token {}", server.admin_token()))
        .arg(format!("https://localhost{path}"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{method} {path}: {text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn runner_names(server: &Server) -> Vec<String> {
    curl(server, "GET", &format!("/orgs/{ORG}/actions/runners"))["runners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|runner| runner["name"].as_str().unwrap().to_owned())
        .collect()
}

fn runner_token(server: &Server, kind: &str) -> String {
    let path = format!("/orgs/{ORG}/actions/runners/{kind}-token");
    curl(server, "POST", &path)["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The runner's diagnostic logs, for a failure message.
fn diagnostics(dir: &Path) -> String {
    std::fs::read_dir(dir.join("_diag"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            format!(
                "--- {}\n{}\n",
                entry.path().display(),
                std::fs::read_to_string(entry.path()).unwrap_or_default()
            )
        })
        .collect()
}

/// A runner unpacked into a scratch directory, talking to a fake on port 443.
struct Runner {
    server: Server,
    dir: TempDir,
}

impl Runner {
    fn start() -> Self {
        let server = Server::start_with(Options {
            address: ([127, 0, 0, 1], 443).into(),
            tls: true,
            ..Default::default()
        })
        .unwrap();
        server.add_org(ORG, &["admin"]);
        server
            .create_repository(ORG, REPO, true, None, None, "main")
            .unwrap();
        let dir = TempDir::new();
        let unpacked = Command::new("tar")
            .args(["xzf"])
            .arg(tarball())
            .args(["--no-same-owner", "-C"])
            .arg(&dir.0)
            .status()
            .unwrap();
        assert!(unpacked.success());
        Self { server, dir }
    }

    fn command(&self, script: &str) -> Command {
        let mut command = Command::new(self.dir.0.join(script));
        command
            .current_dir(&self.dir.0)
            .env("RUNNER_ALLOW_RUNASROOT", "1")
            .env("DOTNET_SYSTEM_GLOBALIZATION_INVARIANT", "1")
            .env("SSL_CERT_FILE", self.server.ca_cert_path().unwrap());
        command
    }

    /// Runs what `config.sh` runs after its libicu preflight, which the invariant globalization
    /// mode makes moot, failing with its output and the runner's diagnostic logs.
    fn config(&self, args: &[&str]) {
        assert!(self.command("env.sh").status().unwrap().success());
        let (verb, rest) = match args.split_first() {
            Some((&"remove", rest)) => ("remove", rest),
            _ => ("configure", args),
        };
        let output = self
            .command("bin/Runner.Listener")
            .arg(verb)
            .args(rest)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Runner.Listener {args:?} failed\n{}{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            diagnostics(&self.dir.0)
        );
    }

    fn register(&self, extra: &[&str]) {
        let url = format!("https://localhost/{ORG}");
        let token = runner_token(&self.server, "registration");
        let mut args = vec![
            "--unattended",
            "--url",
            &url,
            "--token",
            &token,
            "--name",
            RUNNER,
        ];
        args.extend_from_slice(extra);
        self.config(&args);
    }
}

#[test]
fn config_sh_registers_and_removes_a_runner() {
    if !enabled() || outer("config_sh_registers_and_removes_a_runner") {
        return;
    }
    let runner = Runner::start();
    runner.register(&[]);
    assert_eq!(runner_names(&runner.server), [RUNNER]);

    let token = runner_token(&runner.server, "remove");
    runner.config(&["remove", "--token", &token]);
    assert!(runner_names(&runner.server).is_empty());
}

#[test]
fn runner_runs_a_job_to_completion() {
    if !enabled() || outer("runner_runs_a_job_to_completion") {
        return;
    }
    let runner = Runner::start();
    runner.register(&["--ephemeral"]);
    let marker = runner.dir.join("hello.txt");
    let script = format!("echo hello | tee '{}'", marker.display());
    let job = runner
        .server
        .repository(ORG, REPO)
        .unwrap()
        .queue_script_job(&["self-hosted"], &[&script])
        .unwrap();
    let job_path = format!("/repos/{ORG}/{REPO}/actions/jobs/{}", job.id());
    assert_eq!(curl(&runner.server, "GET", &job_path)["status"], "queued");

    let log = runner.dir.join("run.log");
    let mut listener = runner
        .command("run.sh")
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let started = Instant::now();
    let finished = loop {
        let state = curl(&runner.server, "GET", &job_path);
        if state["status"] == "completed" {
            break state;
        }
        if started.elapsed() > JOB_TIMEOUT {
            let _ = listener.kill();
            panic!(
                "the job did not finish: {state}\n{}\nhits: {:?}\n{}",
                std::fs::read_to_string(&log).unwrap_or_default(),
                runner.server.hits(),
                diagnostics(&runner.dir.0)
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let status = listener.wait().unwrap();
    let output = std::fs::read_to_string(&log).unwrap();
    println!("{output}");

    assert_eq!(finished["conclusion"], "success", "{finished}\n{output}");
    assert_eq!(finished["runner_name"], RUNNER);
    assert!(
        finished["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["conclusion"] == "success"),
        "{finished}"
    );
    assert!(
        output.contains("completed with result: Succeeded"),
        "{output}"
    );
    assert!(
        status.success(),
        "the ephemeral runner exits cleanly: {status}\n{output}"
    );
    let run = curl(
        &runner.server,
        "GET",
        &format!("/repos/{ORG}/{REPO}/actions/runs/{}", job.run_id()),
    );
    assert_eq!(run["status"], "completed");
    assert_eq!(run["conclusion"], "success");
    assert!(
        runner_names(&runner.server).is_empty(),
        "an ephemeral runner is removed"
    );
}
