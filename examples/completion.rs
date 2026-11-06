//! Writes `completion.json`, the machine-readable completion of this fake, and checks that the
//! committed file is current.
//!
//! ```sh
//! cargo run --example completion -- update   # rewrites completion.json
//! cargo run --example completion -- check    # fails if completion.json is stale
//! ```
//!
//! Matrices:
//! - `rest-operations`: operations of the pinned OpenAPI description, of which those with a
//!   handler rather than the 501 fallback.
//! - `gh-commands`: leaf commands of the pinned `gh`, of which those the `gh_cli` tests
//!   exercise, as recorded in `tests/gh_coverage.txt`.
//!
//! GraphQL fields have no matrix: whether a field is served depends on the keys of the objects
//! the fixtures build, not on a table, so only a run can tell it from the fallback error.

use clap::{Parser, ValueEnum};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::process::{Command, ExitCode};

use ninot_github::{IMPLEMENTED, OPERATIONS};

const OUTPUT: &str = "completion.json";
const GH_COVERAGE: &str = "tests/gh_coverage.txt";

/// Leaf commands among `names`: those no other name extends, as groups such as `repo` head
/// their subcommands and are not commands themselves.
fn leaf_commands(names: &BTreeSet<String>) -> usize {
    names
        .iter()
        .filter(|name| {
            !names
                .iter()
                .any(|other| other.starts_with(&format!("{name} ")))
        })
        .count()
}

/// Command names out of the headings of `gh help reference`, such as `### gh pr create <number>`.
fn reference_commands(reference: &str) -> BTreeSet<String> {
    reference
        .lines()
        .filter(|line| line.starts_with('#'))
        .filter_map(|line| line.split_once(" gh ").map(|(_, rest)| rest))
        .map(|rest| {
            rest.split(' ')
                .take_while(|word| !word.starts_with(['[', '<', '{', '-']))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Number of `command ...` lines of the coverage baseline.
fn exercised_commands(baseline: &str) -> usize {
    baseline
        .lines()
        .filter(|line| line.starts_with("command "))
        .count()
}

/// The version the baseline was measured with, from its `# gh version ...` header.
fn pinned_version(baseline: &str) -> Option<&str> {
    baseline
        .lines()
        .find_map(|line| line.strip_prefix("# gh version "))
        .and_then(|rest| rest.split_whitespace().next())
}

fn matrix(name: &str, done: usize, total: usize) -> Value {
    json!({"name": name, "done": done, "total": total})
}

fn gh(args: &[&str]) -> String {
    let output = Command::new("gh")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run gh: {error}"));
    assert!(output.status.success(), "gh {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// The completion document and the version of `gh` its `gh-commands` total is pinned to.
/// The total comes from the installed `gh`; a different version than the baseline's is
/// reported on stderr.
fn build() -> (Value, bool) {
    let baseline = std::fs::read_to_string(GH_COVERAGE)
        .unwrap_or_else(|error| panic!("cannot read {GH_COVERAGE}: {error}"));
    let pinned = pinned_version(&baseline).expect("baseline lacks a `# gh version` header");
    let installed = gh(&["--version"]);
    let installed = installed
        .split_whitespace()
        .nth(2)
        .expect("unexpected `gh --version` output")
        .to_owned();
    let same_version = installed == pinned;
    if !same_version {
        eprintln!(
            "note: gh {installed} is installed but {GH_COVERAGE} pins {pinned}; the gh-commands total counts the installed one"
        );
    }
    let total = leaf_commands(&reference_commands(&gh(&["help", "reference"])));
    let matrices = vec![
        matrix("rest-operations", IMPLEMENTED.len(), OPERATIONS.len()),
        matrix("gh-commands", exercised_commands(&baseline), total),
    ];
    (json!({"schema": 1, "matrices": matrices}), same_version)
}

fn render(document: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(document).unwrap())
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Update,
    Check,
}

#[derive(Debug, Parser)]
#[command(about = "Write and check completion.json")]
struct Cli {
    /// Rewrite the file or verify it.
    #[arg(value_enum)]
    mode: Mode,
}

fn main() -> ExitCode {
    let (mut document, same_version) = build();
    match Cli::parse().mode {
        Mode::Update => {
            std::fs::write(OUTPUT, render(&document)).unwrap();
            println!("{OUTPUT} written");
        }
        Mode::Check => {
            let committed: Value = serde_json::from_str(
                &std::fs::read_to_string(OUTPUT)
                    .unwrap_or_else(|error| panic!("cannot read {OUTPUT}: {error}")),
            )
            .unwrap_or_else(|error| panic!("{OUTPUT} is not JSON: {error}"));
            if !same_version {
                // Another gh may count other commands; only the exercised count is checked.
                for (fresh, old) in document["matrices"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .zip(committed["matrices"].as_array().into_iter().flatten())
                {
                    if fresh["name"] == "gh-commands" {
                        fresh["total"] = old["total"].clone();
                    }
                }
            }
            if document != committed {
                println!(
                    "{OUTPUT} is stale. Run `cargo run --example completion -- update`.\nexpected: {}\ncommitted: {}",
                    render(&document),
                    render(&committed)
                );
                return ExitCode::FAILURE;
            }
            println!("{OUTPUT} is current");
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_exclude_groups() {
        let names: BTreeSet<String> = ["repo", "repo view", "repo list", "api", "pr", "pr diff"]
            .map(str::to_owned)
            .into();
        assert_eq!(leaf_commands(&names), 4);
    }

    #[test]
    fn reference_headings_yield_command_names() {
        let reference =
            "## gh repo\n\n### gh repo view [<repository>] [flags]\n\n### gh api <endpoint>\n";
        let names = reference_commands(reference);
        assert!(names.contains("repo view") && names.contains("api"));
    }

    #[test]
    fn baseline_lines_are_counted() {
        let baseline = "# x\n# gh version 2.101.0 (2026-09-15)\ncommand api\ncommand pr view\nflag api --jq\nrest foo\n";
        assert_eq!(exercised_commands(baseline), 2);
        assert_eq!(pinned_version(baseline), Some("2.101.0"));
    }

    #[test]
    fn matrix_has_contract_shape() {
        assert_eq!(
            matrix("a-b", 1, 2),
            json!({"name": "a-b", "done": 1, "total": 2})
        );
    }
}
