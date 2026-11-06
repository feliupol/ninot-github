//! Command-line entry point for the GitHub fake.

use std::net::SocketAddr;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// In-memory fake of the GitHub REST API and git hosting.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serves the fake until interrupted, printing its URL and admin token.
    Serve {
        /// Local port; a free one when 0.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Serves HTTPS with a certificate from a fresh local certificate authority.
        #[arg(long)]
        tls: bool,
    },
    /// Lists the operations of the pinned spec and which are implemented.
    Operations,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Operations => {
            print!("{}", operations_report());
            ExitCode::SUCCESS
        }
        Command::Serve { port, tls } => serve(port, tls),
    }
}

fn operations_report() -> String {
    use ninot_github::{IMPLEMENTED, OPERATIONS};

    let mut report = format!(
        "{} / {} operations implemented\n",
        IMPLEMENTED.len(),
        OPERATIONS.len()
    );
    for info in OPERATIONS {
        let mark = if IMPLEMENTED.contains(info) { "x" } else { " " };
        report.push_str(&format!(
            "[{mark}] {} {} {}\n",
            info.id,
            info.method.as_str(),
            info.path
        ));
    }
    report
}

fn serve(port: u16, tls: bool) -> ExitCode {
    let options = ninot_github::server::Options {
        address: SocketAddr::from(([127, 0, 0, 1], port)),
        validate_responses: false,
        tls,
        ..Default::default()
    };
    let server = match ninot_github::Server::start_with(options) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("cannot start: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("url: {}", server.url());
    println!("token: {}", server.admin_token());
    if let Some(ca) = server.ca_cert_path() {
        println!("ca: {}", ca.display());
    }
    server.wait();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_documents_commands_and_rejects_invalid_arguments() {
        let help = Cli::try_parse_from(["ninot-github", "--help"])
            .err()
            .expect("help exits");
        assert!(help.to_string().contains("operations"));
        assert!(help.to_string().contains("serve"));
        assert!(Cli::try_parse_from(["ninot-github", "serve", "--port", "not-a-port"]).is_err());
        assert!(Cli::try_parse_from(["ninot-github", "unknown"]).is_err());
    }
}
