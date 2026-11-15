//! Logs every successful JSON response a test binary's fakes serve, for
//! `cargo xtask example-shapes`, which compares them with the spec's examples.
//!
//! Each process of a test binary writes `<target>/tmp/example-shapes/<binary>.<pid>.jsonl`, one
//! `{"operation", "status", "body"}` line per response, and removes the logs of earlier builds
//! of the same binary. `<binary>` keeps cargo's hash suffix, so tests that re-run their own
//! binary in parallel processes never share a file. Executables outside
//! `<target>/<profile>/deps` log nothing.

use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

/// Creates this process's log, if it is a test binary.
fn open() -> Option<File> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    if deps.file_name()? != "deps" {
        return None;
    }
    let binary = exe.file_stem()?.to_str()?;
    let (name, _hash) = binary.rsplit_once('-')?;
    let dir: PathBuf = deps.parent()?.parent()?.join("tmp/example-shapes");
    std::fs::create_dir_all(&dir).expect("cannot create the response log directory");
    for entry in std::fs::read_dir(&dir).expect("cannot list the response logs") {
        let path = entry.expect("cannot list the response logs").path();
        let Some(logged) = path.file_name().and_then(|file| file.to_str()) else {
            continue;
        };
        let Some((other, _)) = logged.split_once('.') else {
            continue;
        };
        if other != binary && other.rsplit_once('-').map(|(other, _)| other) == Some(name) {
            // Another process of the old build may race to remove it too.
            let _ = std::fs::remove_file(&path);
        }
    }
    let path = dir.join(format!("{binary}.{}.jsonl", std::process::id()));
    Some(File::create(path).expect("cannot create the response log"))
}

/// Appends a response of `operation` to the log.
pub(crate) fn record(operation: &str, status: u16, body: &Value) {
    static LOG: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    let log = LOG.get_or_init(|| open().map(Mutex::new));
    if let Some(log) = log {
        let line = json!({ "operation": operation, "status": status, "body": body });
        writeln!(log.lock().unwrap(), "{line}").expect("cannot write the response log");
    }
}
