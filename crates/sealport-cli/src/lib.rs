use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

const CONFIG_CANDIDATES: &[&str] = &["sealport.toml", ".sealport.toml"];
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_PROFILE: &str = "default";
const OUTPUT_SCHEMA_VERSION: u8 = 1;

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

#[derive(Args, Clone, Debug, Default)]
pub struct GlobalArgs {
    /// Repository URL.
    #[arg(long, global = true)]
    pub repo: Option<String>,

    /// Config profile.
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Config file path.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Emit one JSON document on stdout.
    #[arg(long, global = true, conflicts_with = "jsonl")]
    pub json: bool,

    /// Emit newline-delimited JSON events on stdout.
    #[arg(long, global = true, conflicts_with = "json")]
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
    /// Generate shell completions.
    Completion {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Print version information.
    Version,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("JSON serialization failed")]
    Json(#[from] serde_json::Error),

    #[error("completion generation failed")]
    Completion(#[from] io::Error),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::Json(_) | Self::Completion(_) => 1,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file {path} could not be read: {source}")]
    Read {
        path: Redacted,
        #[source]
        source: io::Error,
    },

    #[error("config file {path} is invalid: {source}")]
    Parse {
        path: Redacted,
        #[source]
        source: toml::de::Error,
    },

    #[error("profile {profile} was requested but is not defined in {path}")]
    MissingProfile { profile: String, path: Redacted },

    #[error("repository URL {value} is not a supported v1 target")]
    InvalidRepositoryUrl { value: Redacted },

    #[error("log level {value} is invalid; expected trace, debug, info, warn, or error")]
    InvalidLogLevel { value: String },

    #[error("output progress value {value} is invalid; expected auto, always, or never")]
    InvalidProgress { value: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    fn new(value: impl AsRef<str>) -> Self {
        Self(redact_for_display(value.as_ref()))
    }
}

impl std::fmt::Display for Redacted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
    Jsonl,
}

impl OutputMode {
    fn from_globals(globals: &GlobalArgs) -> Self {
        if globals.json {
            Self::Json
        } else if globals.jsonl {
            Self::Jsonl
        } else {
            Self::Human
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub config_path: Option<PathBuf>,
    pub profile: String,
    pub repository_url: Option<String>,
    pub log_level: LogLevel,
    pub progress: ProgressMode,
    pub quiet: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            other => Err(ConfigError::InvalidLogLevel {
                value: other.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMode {
    Auto,
    Always,
    Never,
}

impl ProgressMode {
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(ConfigError::InvalidProgress {
                value: other.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    repository: Option<RepositoryConfig>,
    output: Option<OutputConfig>,
    profiles: Option<BTreeMap<String, ProfileConfig>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ProfileConfig {
    repository: Option<RepositoryConfig>,
    output: Option<OutputConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RepositoryConfig {
    url: Option<String>,
    profile: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OutputConfig {
    progress: Option<String>,
    log_level: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
struct CommandDocument<T> {
    schema_version: u8,
    command: &'static str,
    status: CommandStatus,
    data: T,
}

#[derive(Debug, Serialize)]
struct CommandEvent<T> {
    schema_version: u8,
    event: EventKind,
    command: &'static str,
    status: CommandStatus,
    data: Option<T>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    CommandStarted,
    Progress,
    Warning,
    CommandCompleted,
    CommandFailed,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Started,
    Success,
    Failure,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct VersionData<'a> {
    command: &'a str,
    version: &'a str,
}

pub fn run(cli: Cli) -> Result<Output, CliError> {
    let mode = OutputMode::from_globals(&cli.globals);

    match cli.command {
        Command::Completion { shell } => completion(shell),
        Command::Version => {
            let _config = resolve_config(&cli.globals)?;
            version(mode)
        }
    }
}

pub fn resolve_config(globals: &GlobalArgs) -> Result<ResolvedConfig, ConfigError> {
    resolve_config_in(globals, env::current_dir().ok().as_deref())
}

fn resolve_config_in(
    globals: &GlobalArgs,
    working_dir: Option<&Path>,
) -> Result<ResolvedConfig, ConfigError> {
    resolve_config_with_env(globals, working_dir, EnvConfig::current())
}

fn resolve_config_with_env(
    globals: &GlobalArgs,
    working_dir: Option<&Path>,
    env_config: EnvConfig,
) -> Result<ResolvedConfig, ConfigError> {
    let config_path = globals
        .config
        .clone()
        .or(env_config.config)
        .or_else(|| discover_config(working_dir));
    let file_config = match config_path.as_deref() {
        Some(path) => Some(read_config(path)?),
        None => None,
    };

    let configured_profile = file_config
        .as_ref()
        .and_then(|config| config.repository.as_ref())
        .and_then(|repository| repository.profile.as_deref());
    let profile = globals
        .profile
        .as_deref()
        .or(env_config.profile.as_deref())
        .or(configured_profile)
        .unwrap_or(DEFAULT_PROFILE)
        .to_owned();
    let profile_config = match (&file_config, config_path.as_ref()) {
        (Some(config), Some(path)) if profile != DEFAULT_PROFILE => {
            let profiles = config.profiles.as_ref();
            match profiles.and_then(|profiles| profiles.get(&profile)) {
                Some(profile_config) => Some(profile_config),
                None => {
                    return Err(ConfigError::MissingProfile {
                        profile,
                        path: Redacted::new(path.display().to_string()),
                    });
                }
            }
        }
        (Some(config), _) => config
            .profiles
            .as_ref()
            .and_then(|profiles| profiles.get(&profile)),
        _ => None,
    };

    let root_repository = file_config
        .as_ref()
        .and_then(|config| config.repository.as_ref())
        .and_then(|repository| repository.url.as_deref());
    let profile_repository = profile_config
        .and_then(|profile| profile.repository.as_ref())
        .and_then(|repository| repository.url.as_deref());
    let repository_url = globals
        .repo
        .as_deref()
        .or(env_config.repository.as_deref())
        .or(profile_repository)
        .or(root_repository)
        .map(str::to_owned);

    if let Some(repository_url) = repository_url.as_deref() {
        validate_repository_url(repository_url)?;
    }

    let root_output = file_config
        .as_ref()
        .and_then(|config| config.output.as_ref());
    let profile_output = profile_config.and_then(|profile| profile.output.as_ref());
    let log_level = globals
        .log_level
        .as_deref()
        .or(env_config.log.as_deref())
        .or_else(|| profile_output.and_then(|output| output.log_level.as_deref()))
        .or_else(|| root_output.and_then(|output| output.log_level.as_deref()))
        .unwrap_or(DEFAULT_LOG_LEVEL);
    let log_level = LogLevel::parse(log_level)?;

    let progress = if globals.no_progress {
        ProgressMode::Never
    } else {
        let value = profile_output
            .and_then(|output| output.progress.as_deref())
            .or_else(|| root_output.and_then(|output| output.progress.as_deref()))
            .unwrap_or("auto");
        ProgressMode::parse(value)?
    };

    Ok(ResolvedConfig {
        config_path,
        profile,
        repository_url,
        log_level,
        progress,
        quiet: globals.quiet,
    })
}

#[derive(Clone, Debug, Default)]
struct EnvConfig {
    config: Option<PathBuf>,
    profile: Option<String>,
    repository: Option<String>,
    log: Option<String>,
}

impl EnvConfig {
    fn current() -> Self {
        Self {
            config: env::var_os("SEALPORT_CONFIG").map(PathBuf::from),
            profile: env::var("SEALPORT_PROFILE").ok(),
            repository: env::var("SEALPORT_REPOSITORY").ok(),
            log: env::var("SEALPORT_LOG").ok(),
        }
    }
}

fn discover_config(working_dir: Option<&Path>) -> Option<PathBuf> {
    let working_dir = working_dir?;

    CONFIG_CANDIDATES
        .iter()
        .map(|candidate| working_dir.join(candidate))
        .find(|candidate| candidate.is_file())
}

fn read_config(path: &Path) -> Result<FileConfig, ConfigError> {
    let content = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: Redacted::new(path.display().to_string()),
        source,
    })?;

    toml::from_str(&content).map_err(|source| ConfigError::Parse {
        path: Redacted::new(path.display().to_string()),
        source,
    })
}

fn validate_repository_url(value: &str) -> Result<(), ConfigError> {
    let supported = value.starts_with("s3://")
        || value.starts_with("file://")
        || value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../");

    if supported {
        Ok(())
    } else {
        Err(ConfigError::InvalidRepositoryUrl {
            value: Redacted::new(value),
        })
    }
}

pub fn redact_for_display(value: &str) -> String {
    let mut redacted = value.to_owned();

    if let Some(scheme_end) = redacted.find("://") {
        let authority_start = scheme_end + 3;
        let authority_end = redacted[authority_start..]
            .find(['/', '?', '#'])
            .map(|offset| authority_start + offset)
            .unwrap_or(redacted.len());
        if let Some(relative_at) = redacted[authority_start..authority_end].rfind('@') {
            let userinfo_end = authority_start + relative_at + 1;
            redacted.replace_range(authority_start..userinfo_end, "<redacted>@");
        }
    }

    if let Some(query_start) = redacted.find('?') {
        redacted.truncate(query_start);
        redacted.push_str("?<redacted>");
    }

    if let Some(fragment_start) = redacted.find('#') {
        redacted.truncate(fragment_start);
        redacted.push_str("#<redacted>");
    }

    redacted
}

fn completion(shell: Shell) -> Result<Output, CliError> {
    let mut command = Cli::command();
    let mut stdout = Vec::new();
    generate(shell, &mut command, "sealport", &mut stdout);
    let stdout = String::from_utf8(stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

fn version(mode: OutputMode) -> Result<Output, CliError> {
    let data = VersionData {
        command: "sealport",
        version: env!("CARGO_PKG_VERSION"),
    };

    let stdout = match mode {
        OutputMode::Human => format!("sealport {}\n", data.version),
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command: "version",
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<VersionData<'_>> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command: "version",
                status: CommandStatus::Started,
                data: None,
            };
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command: "version",
                status: CommandStatus::Success,
                data: Some(data),
            };
            format!(
                "{}\n{}\n",
                serde_json::to_string(&started)?,
                serde_json::to_string(&completed)?
            )
        }
    };

    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globals() -> GlobalArgs {
        GlobalArgs::default()
    }

    fn version_cli(json: bool, jsonl: bool) -> Cli {
        Cli {
            globals: GlobalArgs {
                json,
                jsonl,
                ..GlobalArgs::default()
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

        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["command"], "version");
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["data"]["command"], "sealport");
        assert_eq!(parsed["data"]["version"], "0.0.0");
    }

    #[test]
    fn version_jsonl_output_is_an_event_stream() {
        let output = run(version_cli(false, true)).expect("version output");
        let lines: Vec<_> = output.stdout.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(output.stderr, "");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(lines[0]).expect("start event")["event"],
            "command_started"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(lines[1]).expect("complete event")["event"],
            "command_completed"
        );
    }

    #[test]
    fn config_discovery_loads_default_profile() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("sealport.toml"),
            r#"
[repository]
url = "s3://backups/sealport"

[output]
log_level = "warn"
progress = "always"
"#,
        )
        .expect("write config");

        let resolved = resolve_config_with_env(&globals(), Some(temp.path()), EnvConfig::default())
            .expect("config");

        assert_eq!(resolved.profile, "default");
        assert_eq!(
            resolved.repository_url.as_deref(),
            Some("s3://backups/sealport")
        );
        assert_eq!(resolved.log_level, LogLevel::Warn);
        assert_eq!(resolved.progress, ProgressMode::Always);
    }

    #[test]
    fn explicit_profile_overrides_root_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.toml");
        fs::write(
            &path,
            r#"
[repository]
url = "s3://root/repo"

[profiles.laptop.repository]
url = "file:///tmp/sealport"

[profiles.laptop.output]
log_level = "debug"
"#,
        )
        .expect("write config");
        let globals = GlobalArgs {
            config: Some(path),
            profile: Some("laptop".to_owned()),
            ..GlobalArgs::default()
        };

        let resolved = resolve_config_with_env(&globals, Some(temp.path()), EnvConfig::default())
            .expect("config");

        assert_eq!(resolved.profile, "laptop");
        assert_eq!(
            resolved.repository_url.as_deref(),
            Some("file:///tmp/sealport")
        );
        assert_eq!(resolved.log_level, LogLevel::Debug);
    }

    #[test]
    fn cli_values_take_precedence_over_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sealport.toml");
        fs::write(
            &path,
            r#"
[repository]
url = "s3://root/repo"

[output]
log_level = "warn"
progress = "always"
"#,
        )
        .expect("write config");
        let globals = GlobalArgs {
            config: Some(path),
            repo: Some("/var/backups/sealport".to_owned()),
            log_level: Some("error".to_owned()),
            no_progress: true,
            ..GlobalArgs::default()
        };

        let resolved = resolve_config_with_env(&globals, Some(temp.path()), EnvConfig::default())
            .expect("config");

        assert_eq!(
            resolved.repository_url.as_deref(),
            Some("/var/backups/sealport")
        );
        assert_eq!(resolved.log_level, LogLevel::Error);
        assert_eq!(resolved.progress, ProgressMode::Never);
    }

    #[test]
    fn invalid_repository_diagnostic_is_redacted() {
        let globals = GlobalArgs {
            repo: Some("https://user:secret@example.com/repo?token=sensitive".to_owned()),
            ..GlobalArgs::default()
        };

        let error = resolve_config_with_env(&globals, None, EnvConfig::default())
            .expect_err("invalid repository");
        let rendered = error.to_string();

        assert!(rendered.contains("https://<redacted>@example.com/repo?<redacted>"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("sensitive"));
    }

    #[test]
    fn environment_values_take_precedence_over_config_profiles() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("sealport.toml"),
            r#"
[repository]
url = "s3://root/repo"

[profiles.work.repository]
url = "s3://profile/repo"

[profiles.work.output]
log_level = "debug"
"#,
        )
        .expect("write config");
        let env_config = EnvConfig {
            profile: Some("work".to_owned()),
            repository: Some("file:///env/repo".to_owned()),
            log: Some("error".to_owned()),
            ..EnvConfig::default()
        };

        let resolved =
            resolve_config_with_env(&globals(), Some(temp.path()), env_config).expect("config");

        assert_eq!(resolved.profile, "work");
        assert_eq!(resolved.repository_url.as_deref(), Some("file:///env/repo"));
        assert_eq!(resolved.log_level, LogLevel::Error);
    }

    #[test]
    fn completion_generation_writes_shell_script() {
        let output = completion(Shell::Bash).expect("completion");

        assert!(output.stdout.contains("_sealport"));
        assert!(output.stdout.contains("version"));
        assert!(output.stdout.contains("completion"));
        assert_eq!(output.stderr, "");
    }
}
