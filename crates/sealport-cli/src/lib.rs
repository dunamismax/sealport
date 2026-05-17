use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "sealport",
    version,
    about = "Encrypted backups. Same everywhere.",
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug)]
pub struct GlobalArgs {
    /// Repository URL.
    #[arg(long, global = true)]
    pub repo: Option<String>,

    /// Config profile.
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Config file path.
    #[arg(long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Emit one JSON document on stdout.
    #[arg(long, global = true, conflicts_with = "jsonl")]
    pub json: bool,

    /// Emit newline-delimited JSON events on stdout.
    #[arg(long, global = true)]
    pub jsonl: bool,

    /// Reduce human output.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Set log level.
    #[arg(long, global = true)]
    pub log_level: Option<String>,

    /// Disable progress UI.
    #[arg(long, global = true)]
    pub no_progress: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print version information.
    Version,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("JSON serialization failed")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
struct VersionDocument<'a> {
    command: &'a str,
    version: &'a str,
}

pub fn run(cli: Cli) -> Result<Output, CliError> {
    match cli.command {
        Command::Version => version(cli.globals),
    }
}

fn version(globals: GlobalArgs) -> Result<Output, CliError> {
    let command = "sealport";
    let version = env!("CARGO_PKG_VERSION");

    let stdout = if globals.json {
        serde_json::to_string_pretty(&VersionDocument { command, version })?
    } else if globals.jsonl {
        serde_json::to_string(&VersionDocument { command, version })?
    } else {
        format!("{command} {version}")
    };

    Ok(Output {
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version_cli(json: bool, jsonl: bool) -> Cli {
        Cli {
            globals: GlobalArgs {
                repo: None,
                profile: None,
                config: None,
                json,
                jsonl,
                quiet: false,
                log_level: None,
                no_progress: false,
            },
            command: Command::Version,
        }
    }

    #[test]
    fn version_human_output_is_stable() {
        let output = run(version_cli(false, false)).expect("version output");

        assert_eq!(output.stdout, "sealport 0.0.0\n");
        assert_eq!(output.stderr, "");
    }

    #[test]
    fn version_json_output_is_one_document() {
        let output = run(version_cli(true, false)).expect("version output");
        let parsed: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("valid version JSON");

        assert_eq!(parsed["command"], "sealport");
        assert_eq!(parsed["version"], "0.0.0");
    }

    #[test]
    fn version_jsonl_output_is_one_event_line() {
        let output = run(version_cli(false, true)).expect("version output");
        let lines: Vec<_> = output.stdout.lines().collect();

        assert_eq!(lines.len(), 1);
        assert_eq!(output.stderr, "");
    }
}
