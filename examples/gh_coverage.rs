//! Reports what the `gh_cli` tests exercise, of the GitHub CLI and of the fake, and ratchets it
//! against `tests/gh_coverage.txt`.
//!
//! ```sh
//! cargo test --test gh_cli
//! cargo run --example gh_coverage -- report   # Markdown on stdout
//! cargo run --example gh_coverage -- check    # fails if coverage changed from the baseline
//! cargo run --example gh_coverage -- update   # rewrites the baseline
//! ```

use clap::{Parser, ValueEnum};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::process::{Command, ExitCode};

use ninot_github::{IMPLEMENTED, graphql_fields};
use serde_json::Value;

const LOG: &str = "target/tmp/gh-coverage/gh_cli.jsonl";
const BASELINE: &str = "tests/gh_coverage.txt";

/// A leaf command of `gh help reference`, such as `pr create`.
#[derive(Default)]
struct GhCommand {
    /// Long flags, such as `--draft`.
    flags: BTreeSet<String>,
    /// Long flag of each short one.
    short: HashMap<char, String>,
}

/// What one test exercised.
#[derive(Default)]
struct Usage {
    commands: BTreeSet<String>,
    /// `command --flag`.
    flags: BTreeSet<String>,
    /// Arguments that match no command or flag of the reference.
    unknown: BTreeSet<String>,
    /// [`ninot_github::server::Hit`]s as displayed.
    hits: BTreeSet<String>,
}

impl Usage {
    /// The lines this usage contributes to the baseline: commands, flags, and what the fake
    /// served without an error.
    fn ratchet(&self) -> BTreeSet<String> {
        let mut items: BTreeSet<String> = self
            .commands
            .iter()
            .map(|command| format!("command {command}"))
            .chain(self.flags.iter().map(|flag| format!("flag {flag}")))
            .collect();
        for hit in &self.hits {
            let words: Vec<&str> = hit.split(' ').collect();
            match words.as_slice() {
                ["graphql", _] => {
                    items.insert(hit.clone());
                }
                [kind @ ("rest" | "git"), name, status]
                    if status.parse::<u16>().is_ok_and(|status| status < 400) =>
                {
                    items.insert(format!("{kind} {name}"));
                }
                _ => {}
            }
        }
        items
    }

    fn record(&mut self, reference: &BTreeMap<String, GhCommand>, args: &[String]) {
        let words = args.iter().take_while(|arg| !arg.starts_with('-')).count();
        let Some((name, command)) = (1..=words).rev().find_map(|count| {
            let name = args[..count].join(" ");
            reference.get_key_value(&name)
        }) else {
            self.unknown.insert(format!("gh {}", args.join(" ")));
            return;
        };
        self.commands.insert(name.clone());
        for arg in args {
            if arg == "--" {
                break;
            }
            let long = if let Some(flag) = arg.strip_prefix("--") {
                Some(format!("--{}", flag.split('=').next().unwrap_or_default()))
            } else if let Some(short) = arg.strip_prefix('-').and_then(|flag| flag.chars().next()) {
                command.short.get(&short).cloned()
            } else {
                continue;
            };
            match long.filter(|long| command.flags.contains(long)) {
                Some(long) => self.flags.insert(format!("{name} {long}")),
                None => self.unknown.insert(format!("{name} {arg}")),
            };
        }
    }
}

fn run(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    assert!(output.status.success(), "{program} {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// Parses the leaf commands out of `gh help reference`, and each one's flags, own and
/// inherited, out of its `gh help`: the reference leaves out persistent flags such as `--repo`.
fn gh_reference() -> BTreeMap<String, GhCommand> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for line in run("gh", &["help", "reference"]).lines() {
        if line.starts_with('#')
            && let Some((_, rest)) = line.split_once(" gh ")
        {
            names.insert(
                rest.split(' ')
                    .take_while(|word| !word.starts_with(['[', '<', '{', '-']))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }
    // Groups such as `repo` head their subcommands and are not commands themselves.
    let groups: Vec<String> = names
        .iter()
        .filter(|name| {
            names
                .iter()
                .any(|other| other.starts_with(&format!("{name} ")))
        })
        .cloned()
        .collect();
    for group in groups {
        names.remove(&group);
    }
    let names: Vec<String> = names.into_iter().collect();
    let chunks = names.chunks(names.len().div_ceil(16).max(1));
    std::thread::scope(|scope| {
        let workers: Vec<_> = chunks
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|name| {
                            let mut args = vec!["help"];
                            args.extend(name.split(' '));
                            (name.clone(), parse_flags(&run("gh", &args)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    })
}

/// Parses the `FLAGS` and `INHERITED FLAGS` sections of a command's `gh help`, leaving out
/// `--help` itself.
fn parse_flags(help: &str) -> GhCommand {
    let mut command = GhCommand::default();
    let mut in_flags = false;
    for line in help.lines() {
        if !line.is_empty() && !line.starts_with(' ') {
            in_flags = matches!(line, "FLAGS" | "INHERITED FLAGS");
            continue;
        }
        let Some(flag) = line.trim_start().strip_prefix('-').filter(|_| in_flags) else {
            continue;
        };
        let mut words = flag.split_whitespace();
        let (short, long) = match words.next() {
            Some(short) if short.ends_with(',') => (short.chars().next(), words.next()),
            first => (None, first.map(|word| word.trim_start_matches('-'))),
        };
        let Some(long) = long.map(|long| format!("--{}", long.trim_start_matches('-'))) else {
            continue;
        };
        if long == "--help" {
            continue;
        }
        if let Some(short) = short {
            command.short.insert(short, long.clone());
        }
        command.flags.insert(long);
    }
    command
}

fn read_log(reference: &BTreeMap<String, GhCommand>) -> BTreeMap<String, Usage> {
    let text = std::fs::read_to_string(LOG).unwrap_or_else(|error| {
        panic!("cannot read {LOG}; run `cargo test --test gh_cli`: {error}")
    });
    let mut tests: BTreeMap<String, Usage> = BTreeMap::new();
    for line in text.lines() {
        let entry: Value = serde_json::from_str(line).unwrap();
        let usage = tests
            .entry(entry["test"].as_str().unwrap().to_owned())
            .or_default();
        if let Some(args) = entry["gh"].as_array() {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.as_str().unwrap().to_owned())
                .collect();
            usage.record(reference, &args);
        } else if let Some(hit) = entry["hit"].as_str() {
            usage.hits.insert(hit.to_owned());
        }
    }
    tests
}

fn read_baseline() -> BTreeSet<String> {
    std::fs::read_to_string(BASELINE)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "-".to_owned();
    }
    format!("{:.1}%", 100.0 * part as f64 / whole as f64)
}

fn report(
    version: &str,
    reference: &BTreeMap<String, GhCommand>,
    tests: &BTreeMap<String, Usage>,
) -> String {
    let all = |pick: fn(&Usage) -> &BTreeSet<String>| -> BTreeSet<String> {
        tests.values().flat_map(pick).cloned().collect()
    };
    let commands = all(|usage| &usage.commands);
    let flags = all(|usage| &usage.flags);
    let unknown = all(|usage| &usage.unknown);
    let hits = all(|usage| &usage.hits);
    let served: BTreeSet<String> = tests.values().flat_map(Usage::ratchet).collect();
    let implemented: BTreeSet<&str> = IMPLEMENTED.iter().map(|info| info.id).collect();
    let rest_ok: BTreeSet<&str> = served
        .iter()
        .filter_map(|item| item.strip_prefix("rest "))
        .collect();
    let graphql_fields = graphql_fields();
    let graphql_ok = served
        .iter()
        .filter(|item| item.starts_with("graphql "))
        .count();
    let total_flags: usize = reference.values().map(|command| command.flags.len()).sum();

    let mut out = String::new();
    let _ = writeln!(out, "## gh coverage\n");
    let _ = writeln!(
        out,
        "Measured with `{version}` over {} tests.\n",
        tests.len()
    );
    let _ = writeln!(out, "| | Exercised | Of | |\n|---|---:|---:|---:|");
    for (label, part, whole) in [
        ("gh commands", commands.len(), reference.len()),
        ("gh flags", flags.len(), total_flags),
        (
            "Implemented REST operations",
            rest_ok.intersection(&implemented).count(),
            implemented.len(),
        ),
        ("GraphQL fields", graphql_ok, graphql_fields.len()),
    ] {
        let _ = writeln!(
            out,
            "| {label} | {part} | {whole} | {} |",
            percent(part, whole)
        );
    }

    let _ = writeln!(out, "\n### Validation\n");
    let _ = writeln!(
        out,
        "- {} REST operations answered successfully, each response checked against the OpenAPI spec.",
        rest_ok.len()
    );
    let _ = writeln!(
        out,
        "- {graphql_ok} GraphQL fields resolved; GraphQL responses are not checked against GitHub's schema."
    );
    let failures: Vec<&String> = hits
        .iter()
        .filter(|hit| {
            ["unrouted ", "graphql-error ", "panic "]
                .iter()
                .any(|kind| hit.starts_with(kind))
                || hit
                    .rsplit(' ')
                    .next()
                    .and_then(|status| status.parse::<u16>().ok())
                    .is_some_and(|status| status >= 500)
        })
        .collect();
    let _ = writeln!(
        out,
        "- {} server errors, panics, unimplemented operations, unrouted requests or rejected GraphQL requests{}",
        failures.len(),
        if failures.is_empty() { "." } else { ":" }
    );
    for hit in failures {
        let _ = writeln!(out, "  - `{hit}`");
    }
    if !unknown.is_empty() {
        let _ = writeln!(out, "- Arguments missing from `gh help reference`:");
        for arg in &unknown {
            let _ = writeln!(out, "  - `{arg}`");
        }
    }

    let _ = writeln!(
        out,
        "\n### By test\n\n| Test | Commands | Flags | REST operations | GraphQL fields |\n|---|---:|---:|---:|---:|"
    );
    for (name, usage) in tests {
        let items = usage.ratchet();
        let count = |prefix: &str| items.iter().filter(|item| item.starts_with(prefix)).count();
        let _ = writeln!(
            out,
            "| `{name}` | {} | {} | {} | {} |",
            usage.commands.len(),
            usage.flags.len(),
            count("rest "),
            count("graphql ")
        );
    }
    for (name, usage) in tests {
        let _ = writeln!(out, "\n<details><summary><code>{name}</code></summary>\n");
        for item in usage.ratchet() {
            let _ = writeln!(out, "- `{item}`");
        }
        let _ = writeln!(out, "\n</details>");
    }

    let _ = writeln!(
        out,
        "\n### By gh command group\n\n| Group | Commands | Flags |\n|---|---:|---:|"
    );
    let mut groups: BTreeMap<&str, (usize, usize, usize, usize)> = BTreeMap::new();
    for (name, command) in reference {
        let group = groups.entry(name.split(' ').next().unwrap()).or_default();
        group.1 += 1;
        group.3 += command.flags.len();
        if commands.contains(name) {
            group.0 += 1;
        }
        group.2 += command
            .flags
            .iter()
            .filter(|flag| flags.contains(&format!("{name} {flag}")))
            .count();
    }
    for (group, (commands, all_commands, flags, all_flags)) in groups {
        let _ = writeln!(
            out,
            "| {group} | {commands}/{all_commands} | {flags}/{all_flags} |"
        );
    }
    out
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum Mode {
    #[default]
    Report,
    Update,
    Check,
}

#[derive(Debug, Parser)]
#[command(about = "Report and check gh CLI test coverage")]
struct Cli {
    /// What to do with the gh CLI coverage baseline.
    #[arg(value_enum, default_value_t = Mode::Report)]
    mode: Mode,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Report => "report",
            Self::Update => "update",
            Self::Check => "check",
        })
    }
}

fn main() -> ExitCode {
    let mode = Cli::parse().mode;
    let version = run("gh", &["--version"])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let reference = gh_reference();
    let tests = read_log(&reference);
    let current: BTreeSet<String> = tests.values().flat_map(Usage::ratchet).collect();
    match mode {
        Mode::Report => print!("{}", report(&version, &reference, &tests)),
        Mode::Update => {
            let mut text = format!(
                "# What the gh_cli tests exercise; `cargo run --example gh_coverage -- update` rewrites it.\n# {version}\n"
            );
            for item in &current {
                text.push_str(item);
                text.push('\n');
            }
            std::fs::write(BASELINE, text).unwrap();
            println!("{} items written to {BASELINE}", current.len());
        }
        Mode::Check => {
            let baseline = read_baseline();
            let lost: Vec<&String> = baseline.difference(&current).collect();
            let gained: Vec<&String> = current.difference(&baseline).collect();
            for item in &lost {
                println!("lost: {item}");
            }
            for item in &gained {
                println!("new:  {item}");
            }
            if !lost.is_empty() {
                println!(
                    "\nCoverage dropped. Run the full `cargo test --test gh_cli`, or restore the tests."
                );
                return ExitCode::FAILURE;
            }
            if !gained.is_empty() {
                println!(
                    "\nCoverage grew. Record it with `cargo run --example gh_coverage -- update`."
                );
                return ExitCode::FAILURE;
            }
            println!("{} items match {BASELINE}", current.len());
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_to_report_and_rejects_unknown_commands() {
        assert!(matches!(
            Cli::try_parse_from(["gh_coverage"]).unwrap().mode,
            Mode::Report
        ));
        assert!(matches!(
            Cli::try_parse_from(["gh_coverage", "update"]).unwrap().mode,
            Mode::Update
        ));
        assert!(Cli::try_parse_from(["gh_coverage", "invalid"]).is_err());
        let help = Cli::try_parse_from(["gh_coverage", "--help"])
            .err()
            .expect("help exits");
        assert!(help.to_string().contains("report"));
    }
}
