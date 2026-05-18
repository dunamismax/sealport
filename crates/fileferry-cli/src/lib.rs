use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use fileferry_core::{
    BackupPipeline, BackupPipelineConfig, BackupRequest, CoreError, MetadataStatus,
    RestoreDestinationAction, RestoreDestinationRequest, RestoreOverwritePolicy, SnapshotEntry,
    SnapshotSelection, create_repository, list_snapshot_entries, open_repository, select_snapshot,
    snapshot_summaries,
};
use fileferry_crypto::KdfParams;
use fileferry_platform::{EntryKind, MetadataValue};
use fileferry_storage::{LocalStore, StorageError};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const CONFIG_CANDIDATES: &[&str] = &["fileferry.toml", ".fileferry.toml"];
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_PROFILE: &str = "default";
const OUTPUT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "ferry",
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

    /// Create an encrypted local repository.
    Init,

    /// Create an encrypted snapshot from local source paths.
    Backup {
        /// Tag to attach to the snapshot. May be repeated.
        #[arg(long = "tag")]
        tags: Vec<String>,

        /// Source paths to include in the snapshot.
        #[arg(required = true, value_name = "SOURCE")]
        sources: Vec<PathBuf>,
    },

    /// List committed snapshots.
    Snapshots,

    /// List entries in a committed snapshot.
    Ls {
        /// Snapshot id to list.
        #[arg(long, conflicts_with_all = ["tag", "latest"])]
        snapshot: Option<String>,

        /// Select the newest snapshot with this tag.
        #[arg(long, conflicts_with_all = ["snapshot", "latest"])]
        tag: Option<String>,

        /// Select the newest committed snapshot.
        #[arg(long, conflicts_with_all = ["snapshot", "tag"])]
        latest: bool,

        /// Snapshot-relative path to list.
        path: Option<PathBuf>,
    },

    /// Restore regular-file contents from a committed snapshot.
    Restore {
        /// Snapshot id to restore.
        #[arg(long, conflicts_with_all = ["tag", "latest"])]
        snapshot: Option<String>,

        /// Select the newest snapshot with this tag.
        #[arg(long, conflicts_with_all = ["snapshot", "latest"])]
        tag: Option<String>,

        /// Select the newest committed snapshot.
        #[arg(long, conflicts_with_all = ["snapshot", "tag"])]
        latest: bool,

        /// Snapshot-relative path to restore. May be repeated.
        #[arg(long = "path")]
        paths: Vec<PathBuf>,

        /// Report what would be restored without writing files.
        #[arg(long)]
        dry_run: bool,

        /// Overwrite existing destination files.
        #[arg(long)]
        overwrite: bool,

        /// Destination directory for restored files.
        #[arg(value_name = "DESTINATION")]
        destination: PathBuf,
    },

    /// Print version information.
    Version,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Repository(#[from] RepositoryError),

    #[error(transparent)]
    Core(Box<CoreError>),

    #[error("JSON serialization failed")]
    Json(#[from] serde_json::Error),

    #[error("completion generation failed")]
    Completion(#[from] io::Error),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::Repository(error) => error.exit_code(),
            Self::Core(error) => core_exit_code(error),
            Self::Json(_) | Self::Completion(_) => 1,
        }
    }
}

impl From<CoreError> for CliError {
    fn from(error: CoreError) -> Self {
        Self::Core(Box::new(error))
    }
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository URL is required; pass --repo or set FILEFERRY_REPOSITORY")]
    MissingRepository,

    #[error("FILEFERRY_PASSWORD or FILEFERRY_PASSWORD_FILE is required for repository access")]
    MissingPassword,

    #[error("password file {path} could not be read: {source}")]
    PasswordFileRead {
        path: Redacted,
        #[source]
        source: io::Error,
    },

    #[error("repository URL {value} is not supported by this command yet")]
    UnsupportedRepository { value: Redacted },

    #[error("file repository URL {value} is invalid; expected file:///absolute/path")]
    InvalidFileRepositoryUrl { value: Redacted },

    #[error("repository runtime could not be started: {source}")]
    Runtime {
        #[source]
        source: io::Error,
    },
}

impl RepositoryError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::MissingRepository
            | Self::MissingPassword
            | Self::PasswordFileRead { .. }
            | Self::InvalidFileRepositoryUrl { .. } => 2,
            Self::UnsupportedRepository { .. } => 9,
            Self::Runtime { .. } => 1,
        }
    }
}

fn core_exit_code(error: &CoreError) -> i32 {
    match error {
        CoreError::Storage { source } => storage_exit_code(source),
        CoreError::RepositoryUnlock { .. } => 4,
        CoreError::SnapshotNotFound { .. } | CoreError::SnapshotPathNotFound { .. } => 7,
        CoreError::RepositoryBootstrapDecode { .. }
        | CoreError::InvalidRepositoryBootstrap { .. }
        | CoreError::CommitDecode { .. }
        | CoreError::InvalidCommitMarker { .. }
        | CoreError::MetadataIdentityMismatch { .. }
        | CoreError::ObjectDecode { .. }
        | CoreError::MetadataDecode { .. }
        | CoreError::ChunkIdentityMismatch { .. } => 6,
        CoreError::Encryption { .. } => 6,
        CoreError::ObjectKey { .. }
        | CoreError::Serialization { .. }
        | CoreError::SystemClock { .. }
        | CoreError::InvalidBackupPipelineConfig { .. }
        | CoreError::InvalidChunkingConfig { .. } => 1,
        CoreError::SourceRootNotAbsolute { .. }
        | CoreError::InvalidRestoreRequest { .. }
        | CoreError::RestoreDestinationNotAbsolute { .. }
        | CoreError::RestoreDestinationEscapesRoot { .. }
        | CoreError::RestoreDestinationSymlink { .. }
        | CoreError::RestoreDestinationExists { .. }
        | CoreError::RestoreDestinationKind { .. } => 2,
        CoreError::SourceRootRead { .. }
        | CoreError::DirectoryRead { .. }
        | CoreError::DirectoryEntryRead { .. }
        | CoreError::MetadataCapture { .. }
        | CoreError::FileRead { .. }
        | CoreError::Compression { .. }
        | CoreError::Decompression { .. }
        | CoreError::RestoreDestinationWrite { .. }
        | CoreError::RestoreVerificationRead { .. } => 5,
        CoreError::InvalidChunkRange { .. }
        | CoreError::InvalidChunkLength { .. }
        | CoreError::MissingChunkIndexEntry { .. }
        | CoreError::RestoreVerificationMismatch { .. } => 6,
    }
}

fn storage_exit_code(error: &StorageError) -> i32 {
    match error {
        StorageError::ObjectNotFound { .. } => 3,
        StorageError::InvalidObjectKey { .. } | StorageError::PolicyConfig { .. } => 1,
        StorageError::BackendConfig { .. } => 9,
        StorageError::Io { .. }
        | StorageError::ObjectIo { .. }
        | StorageError::BackendObject { .. }
        | StorageError::Backend { .. }
        | StorageError::Timeout { .. }
        | StorageError::ObjectAlreadyExists { .. } => 5,
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

#[derive(Debug, PartialEq, Eq, Serialize)]
struct InitData {
    repository_id: String,
    repository_url: String,
    format_version: u16,
    backend: CliBackendKind,
    created: bool,
    key_slots: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct BackupData {
    snapshot_id: String,
    repository_id: String,
    started_at_unix_seconds: u64,
    completed_at_unix_seconds: u64,
    sources: Vec<String>,
    tags: Vec<String>,
    entries_scanned: usize,
    files_backed_up: usize,
    directories_backed_up: usize,
    symlinks_backed_up: usize,
    special_entries_seen: usize,
    bytes_scanned: u64,
    bytes_uploaded: u64,
    chunks_seen: usize,
    chunks_written: usize,
    chunks_reused: usize,
    index_ids: Vec<String>,
    manifest_id: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct RestoreData {
    snapshot_id: String,
    destination: String,
    paths: Vec<String>,
    dry_run: bool,
    overwrite: CliRestoreOverwritePolicy,
    entries_selected: usize,
    files_written: usize,
    directories_written: usize,
    symlinks_written: usize,
    metadata_applied: usize,
    metadata_warnings: Vec<RestoreMetadataWarning>,
    bytes_written: u64,
    verified_files: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CliRestoreOverwritePolicy {
    FailIfExists,
    OverwriteFiles,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct RestoreMetadataWarning {
    path: String,
    field: String,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CliBackendKind {
    Local,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct SnapshotsData {
    snapshots: Vec<fileferry_core::SnapshotSummary>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct LsData {
    snapshot_id: String,
    path: String,
    entries: Vec<CliSnapshotEntry>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CliSnapshotEntry {
    path: String,
    kind: EntryKind,
    size_bytes: Option<u64>,
    modified: CliTimestampValue,
    metadata_status: MetadataStatus,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CliTimestampValue {
    status: CliTimestampStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nanoseconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    denial_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CliTimestampStatus {
    Captured,
    Unsupported,
    Denied,
}

#[derive(Debug, Serialize)]
struct ProgressData {
    phase: &'static str,
    message: &'static str,
    items_done: Option<usize>,
    items_total: Option<usize>,
    bytes_done: Option<u64>,
    bytes_total: Option<u64>,
    snapshot_id: Option<String>,
    object_key: Option<String>,
}

pub fn run(cli: Cli) -> Result<Output, CliError> {
    let mode = OutputMode::from_globals(&cli.globals);

    match cli.command {
        Command::Completion { shell } => completion(shell),
        Command::Init => {
            let config = resolve_config(&cli.globals)?;
            init_repository(mode, &config)
        }
        Command::Backup { tags, sources } => {
            let config = resolve_config(&cli.globals)?;
            backup(mode, &config, sources, tags)
        }
        Command::Snapshots => {
            let config = resolve_config(&cli.globals)?;
            snapshots(mode, &config)
        }
        Command::Ls {
            snapshot,
            tag,
            latest,
            path,
        } => {
            let config = resolve_config(&cli.globals)?;
            ls(
                mode,
                &config,
                snapshot_selection(snapshot, tag, latest),
                path.unwrap_or_default(),
            )
        }
        Command::Restore {
            snapshot,
            tag,
            latest,
            paths,
            dry_run,
            overwrite,
            destination,
        } => {
            let config = resolve_config(&cli.globals)?;
            restore(
                mode,
                &config,
                snapshot_selection(snapshot, tag, latest),
                paths,
                destination,
                dry_run,
                overwrite,
            )
        }
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
            config: env::var_os("FILEFERRY_CONFIG").map(PathBuf::from),
            profile: env::var("FILEFERRY_PROFILE").ok(),
            repository: env::var("FILEFERRY_REPOSITORY").ok(),
            log: env::var("FILEFERRY_LOG").ok(),
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
    generate(shell, &mut command, "ferry", &mut stdout);
    let stdout = String::from_utf8(stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

fn init_repository(mode: OutputMode, config: &ResolvedConfig) -> Result<Output, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let runtime = tokio_runtime()?;
    let result = runtime.block_on(create_repository(
        &repository.store,
        &passphrase,
        KdfParams::default(),
    ))?;
    let data = InitData {
        repository_id: result.repository.repository_id,
        repository_url: redact_for_display(&repository.url),
        format_version: result.format_version,
        backend: repository.backend,
        created: result.created,
        key_slots: result.key_slots,
    };

    emit_command(mode, "init", data, |data| {
        if data.created {
            format!(
                "Initialized repository {} at {}\n",
                data.repository_id, data.repository_url
            )
        } else {
            format!(
                "Repository {} already initialized at {}\n",
                data.repository_id, data.repository_url
            )
        }
    })
}

fn backup(
    mode: OutputMode,
    config: &ResolvedConfig,
    sources: Vec<PathBuf>,
    tags: Vec<String>,
) -> Result<Output, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let roots = sources
        .iter()
        .map(|source| absolute_source_path(source))
        .collect::<Result<Vec<_>, _>>()?;
    let source_display = roots
        .iter()
        .map(|source| redact_for_display(&source.display().to_string()))
        .collect::<Vec<_>>();
    let started_at_unix_seconds = unix_seconds_now()?;
    let runtime = tokio_runtime()?;
    let opened = runtime.block_on(open_repository(&repository.store, &passphrase))?;
    let pipeline = BackupPipeline::new(BackupPipelineConfig::new(opened.repository_id.clone()))?;
    let result = runtime.block_on(pipeline.write_snapshot(
        &repository.store,
        &opened.master_key,
        BackupRequest {
            roots,
            exclusion_rules: Vec::new(),
            tags: tags.clone(),
        },
    ))?;
    let completed_at_unix_seconds = unix_seconds_now()?;
    let data = BackupData {
        snapshot_id: result.snapshot_id.clone(),
        repository_id: opened.repository_id,
        started_at_unix_seconds,
        completed_at_unix_seconds,
        sources: source_display,
        tags,
        entries_scanned: result.entries_scanned,
        files_backed_up: result.files_backed_up,
        directories_backed_up: result.directories_backed_up,
        symlinks_backed_up: result.symlinks_backed_up,
        special_entries_seen: result.special_entries_seen,
        bytes_scanned: result.bytes_scanned,
        bytes_uploaded: result.bytes_uploaded,
        chunks_seen: result.chunks_seen,
        chunks_written: result.chunks_written,
        chunks_reused: result.chunks_reused,
        index_ids: result.index_ids,
        manifest_id: result.snapshot_id,
    };

    emit_backup_command(mode, data)
}

fn snapshots(mode: OutputMode, config: &ResolvedConfig) -> Result<Output, CliError> {
    let loaded = load_repository_snapshots(config)?;
    let data = SnapshotsData {
        snapshots: snapshot_summaries(&loaded.manifests),
    };

    emit_command(mode, "snapshots", data, |data| {
        if data.snapshots.is_empty() {
            return "No snapshots found.\n".to_owned();
        }

        data.snapshots
            .iter()
            .map(|snapshot| {
                let tags = if snapshot.tags.is_empty() {
                    "-".to_owned()
                } else {
                    snapshot.tags.join(",")
                };
                format!(
                    "{} {} entries={} sources={} tags={}",
                    snapshot.snapshot_id,
                    snapshot.created_at_unix_seconds,
                    snapshot.entry_count,
                    snapshot.source_count,
                    tags
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    })
}

fn ls(
    mode: OutputMode,
    config: &ResolvedConfig,
    selection: SnapshotSelection,
    path: PathBuf,
) -> Result<Output, CliError> {
    let loaded = load_repository_snapshots(config)?;
    let manifest = select_snapshot(&loaded.manifests, &selection)?;
    let listing = list_snapshot_entries(manifest, &path)?;
    let data = LsData {
        snapshot_id: listing.snapshot_id,
        path: display_snapshot_path(&listing.path),
        entries: listing
            .entries
            .iter()
            .map(CliSnapshotEntry::from_snapshot_entry)
            .collect(),
    };

    emit_command(mode, "ls", data, |data| {
        if data.entries.is_empty() {
            return String::new();
        }

        data.entries
            .iter()
            .map(|entry| {
                format!(
                    "{}\t{}\t{}",
                    display_entry_kind(&entry.kind),
                    entry
                        .size_bytes
                        .map(|size| size.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    entry.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    })
}

fn restore(
    mode: OutputMode,
    config: &ResolvedConfig,
    selection: SnapshotSelection,
    paths: Vec<PathBuf>,
    destination: PathBuf,
    dry_run: bool,
    overwrite: bool,
) -> Result<Output, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let destination = absolute_source_path(&destination)?;
    let display_destination = redact_for_display(&destination.display().to_string());
    let display_paths = paths
        .iter()
        .map(|path| display_snapshot_path(path))
        .collect::<Vec<_>>();
    let overwrite_policy = if overwrite {
        RestoreOverwritePolicy::OverwriteFiles
    } else {
        RestoreOverwritePolicy::FailIfExists
    };
    let cli_overwrite = if overwrite {
        CliRestoreOverwritePolicy::OverwriteFiles
    } else {
        CliRestoreOverwritePolicy::FailIfExists
    };

    let runtime = tokio_runtime()?;
    let opened = runtime.block_on(open_repository(&repository.store, &passphrase))?;
    let pipeline = BackupPipeline::new(BackupPipelineConfig::new(opened.repository_id))?;
    let manifests = runtime.block_on(
        pipeline.read_committed_snapshot_manifests(&repository.store, &opened.master_key),
    )?;
    let snapshot_id = select_snapshot(&manifests, &selection)?.snapshot_id.clone();
    let result = runtime.block_on(pipeline.restore_snapshot_to_destination(
        &repository.store,
        &opened.master_key,
        RestoreDestinationRequest {
            snapshot_id,
            paths,
            destination,
            overwrite: overwrite_policy,
            dry_run,
            verify: true,
        },
    ))?;
    let files_written = result
        .files
        .iter()
        .filter(|file| {
            matches!(
                file.action,
                RestoreDestinationAction::Written | RestoreDestinationAction::WouldWrite
            )
        })
        .count();
    let data = RestoreData {
        snapshot_id: result.snapshot_id,
        destination: display_destination,
        paths: display_paths,
        dry_run: result.dry_run,
        overwrite: cli_overwrite,
        entries_selected: result.selected_entries,
        files_written,
        directories_written: 0,
        symlinks_written: 0,
        metadata_applied: 0,
        metadata_warnings: Vec::new(),
        bytes_written: result.bytes,
        verified_files: result.verified_files,
    };

    emit_restore_command(mode, data)
}

struct LocalRepository {
    url: String,
    backend: CliBackendKind,
    store: LocalStore,
}

struct LoadedRepositorySnapshots {
    manifests: Vec<fileferry_core::SnapshotManifest>,
}

fn load_repository_snapshots(
    config: &ResolvedConfig,
) -> Result<LoadedRepositorySnapshots, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let runtime = tokio_runtime()?;
    let opened = runtime.block_on(open_repository(&repository.store, &passphrase))?;
    let pipeline = BackupPipeline::new(BackupPipelineConfig::new(opened.repository_id))?;
    let manifests = runtime.block_on(
        pipeline.read_committed_snapshot_manifests(&repository.store, &opened.master_key),
    )?;

    Ok(LoadedRepositorySnapshots { manifests })
}

fn local_repository(config: &ResolvedConfig) -> Result<LocalRepository, RepositoryError> {
    let url = config
        .repository_url
        .as_deref()
        .ok_or(RepositoryError::MissingRepository)?;
    let path = local_repository_path(url)?;

    Ok(LocalRepository {
        url: url.to_owned(),
        backend: CliBackendKind::Local,
        store: LocalStore::new(path),
    })
}

fn local_repository_path(value: &str) -> Result<PathBuf, RepositoryError> {
    if value.starts_with("s3://") {
        return Err(RepositoryError::UnsupportedRepository {
            value: Redacted::new(value),
        });
    }

    if let Some(path) = value.strip_prefix("file://") {
        if path.starts_with('/') {
            return Ok(PathBuf::from(path));
        }
        return Err(RepositoryError::InvalidFileRepositoryUrl {
            value: Redacted::new(value),
        });
    }

    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .map_err(|source| RepositoryError::Runtime { source })
    }
}

fn repository_passphrase() -> Result<SecretString, RepositoryError> {
    if let Ok(value) = env::var("FILEFERRY_PASSWORD") {
        return Ok(SecretString::from(value));
    }

    if let Some(path) = env::var_os("FILEFERRY_PASSWORD_FILE").map(PathBuf::from) {
        let content =
            fs::read_to_string(&path).map_err(|source| RepositoryError::PasswordFileRead {
                path: Redacted::new(path.display().to_string()),
                source,
            })?;
        return Ok(SecretString::from(
            content.trim_end_matches(['\r', '\n']).to_owned(),
        ));
    }

    Err(RepositoryError::MissingPassword)
}

fn tokio_runtime() -> Result<tokio::runtime::Runtime, RepositoryError> {
    tokio::runtime::Runtime::new().map_err(|source| RepositoryError::Runtime { source })
}

fn absolute_source_path(path: &Path) -> Result<PathBuf, RepositoryError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .map_err(|source| RepositoryError::Runtime { source })
    }
}

fn unix_seconds_now() -> Result<u64, CliError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| CliError::Core(Box::new(CoreError::SystemClock { source })))
}

fn snapshot_selection(
    snapshot: Option<String>,
    tag: Option<String>,
    latest: bool,
) -> SnapshotSelection {
    match (snapshot, tag, latest) {
        (Some(snapshot), None, false) => SnapshotSelection::Id(snapshot),
        (None, Some(tag), false) => SnapshotSelection::Tag(tag),
        _ => SnapshotSelection::Latest,
    }
}

fn emit_command<T>(
    mode: OutputMode,
    command: &'static str,
    data: T,
    human: impl FnOnce(&T) -> String,
) -> Result<Output, CliError>
where
    T: Serialize,
{
    let stdout = match mode {
        OutputMode::Human => human(&data),
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command,
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<T> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command,
                status: CommandStatus::Started,
                data: None,
            };
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command,
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

fn emit_backup_command(mode: OutputMode, data: BackupData) -> Result<Output, CliError> {
    let stdout = match mode {
        OutputMode::Human => format!(
            "Created snapshot {}\nentries={} files={} directories={} symlinks={} bytes_scanned={} chunks_seen={} chunks_written={} chunks_reused={}\n",
            data.snapshot_id,
            data.entries_scanned,
            data.files_backed_up,
            data.directories_backed_up,
            data.symlinks_backed_up,
            data.bytes_scanned,
            data.chunks_seen,
            data.chunks_written,
            data.chunks_reused
        ),
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command: "backup",
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<BackupData> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command: "backup",
                status: CommandStatus::Started,
                data: None,
            };
            let phases = [
                ("walk_sources", "walked source paths"),
                ("plan_chunks", "planned content chunks"),
                ("write_chunks", "wrote encrypted chunks"),
                ("write_index", "wrote encrypted chunk index"),
                ("write_manifest", "wrote encrypted snapshot manifest"),
                ("write_commit", "wrote snapshot commit marker"),
                ("complete", "completed backup"),
            ];
            let mut lines = vec![serde_json::to_string(&started)?];
            for (phase, message) in phases {
                let event = CommandEvent {
                    schema_version: OUTPUT_SCHEMA_VERSION,
                    event: EventKind::Progress,
                    command: "backup",
                    status: CommandStatus::Started,
                    data: Some(ProgressData {
                        phase,
                        message,
                        items_done: Some(data.entries_scanned),
                        items_total: Some(data.entries_scanned),
                        bytes_done: Some(data.bytes_scanned),
                        bytes_total: Some(data.bytes_scanned),
                        snapshot_id: Some(data.snapshot_id.clone()),
                        object_key: None,
                    }),
                };
                lines.push(serde_json::to_string(&event)?);
            }
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command: "backup",
                status: CommandStatus::Success,
                data: Some(data),
            };
            lines.push(serde_json::to_string(&completed)?);
            lines.join("\n") + "\n"
        }
    };

    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

fn emit_restore_command(mode: OutputMode, data: RestoreData) -> Result<Output, CliError> {
    let stdout = match mode {
        OutputMode::Human => {
            let action = if data.dry_run {
                "Would restore"
            } else {
                "Restored"
            };
            format!(
                "{} snapshot {} to {}\nentries_selected={} files={} bytes={} verified_files={}\n",
                action,
                data.snapshot_id,
                data.destination,
                data.entries_selected,
                data.files_written,
                data.bytes_written,
                data.verified_files
            )
        }
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command: "restore",
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<RestoreData> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command: "restore",
                status: CommandStatus::Started,
                data: None,
            };
            let phases = [
                ("load_manifest", "loaded snapshot manifest"),
                ("read_chunks", "read and verified encrypted chunks"),
                ("write_entries", "processed restore entries"),
                ("apply_metadata", "recorded metadata restore status"),
                ("verify", "recorded restore verification status"),
                ("complete", "completed restore"),
            ];
            let mut lines = vec![serde_json::to_string(&started)?];
            for (phase, message) in phases {
                let event = CommandEvent {
                    schema_version: OUTPUT_SCHEMA_VERSION,
                    event: EventKind::Progress,
                    command: "restore",
                    status: CommandStatus::Started,
                    data: Some(ProgressData {
                        phase,
                        message,
                        items_done: Some(data.files_written),
                        items_total: Some(data.files_written),
                        bytes_done: Some(data.bytes_written),
                        bytes_total: Some(data.bytes_written),
                        snapshot_id: Some(data.snapshot_id.clone()),
                        object_key: None,
                    }),
                };
                lines.push(serde_json::to_string(&event)?);
            }
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command: "restore",
                status: CommandStatus::Success,
                data: Some(data),
            };
            lines.push(serde_json::to_string(&completed)?);
            lines.join("\n") + "\n"
        }
    };

    Ok(Output {
        stdout,
        stderr: String::new(),
    })
}

impl CliSnapshotEntry {
    fn from_snapshot_entry(entry: &SnapshotEntry) -> Self {
        Self {
            path: display_snapshot_path(&entry.relative_path),
            kind: entry.kind.clone(),
            size_bytes: entry.size_bytes,
            modified: timestamp_value(&entry.modified),
            metadata_status: entry.metadata_status,
        }
    }
}

fn timestamp_value(value: &MetadataValue<fileferry_platform::Timestamp>) -> CliTimestampValue {
    match value {
        MetadataValue::Captured(timestamp) => CliTimestampValue {
            status: CliTimestampStatus::Captured,
            seconds: Some(timestamp.seconds),
            nanoseconds: Some(timestamp.nanoseconds),
            denial_reason: None,
        },
        MetadataValue::Unsupported => CliTimestampValue {
            status: CliTimestampStatus::Unsupported,
            seconds: None,
            nanoseconds: None,
            denial_reason: None,
        },
        MetadataValue::Denied(reason) => CliTimestampValue {
            status: CliTimestampStatus::Denied,
            seconds: None,
            nanoseconds: None,
            denial_reason: Some(reason.clone()),
        },
    }
}

fn display_snapshot_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.display().to_string()
    }
}

fn display_entry_kind(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::RegularFile => "file",
        EntryKind::Directory => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

fn version(mode: OutputMode) -> Result<Output, CliError> {
    let data = VersionData {
        command: "ferry",
        version: env!("CARGO_PKG_VERSION"),
    };

    let stdout = match mode {
        OutputMode::Human => format!("ferry {}\n", data.version),
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

        assert_eq!(output.stdout, "ferry 0.0.0\n");
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
        assert_eq!(parsed["data"]["command"], "ferry");
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
            temp.path().join("fileferry.toml"),
            r#"
[repository]
url = "s3://backups/fileferry"

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
            Some("s3://backups/fileferry")
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
url = "file:///tmp/fileferry"

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
            Some("file:///tmp/fileferry")
        );
        assert_eq!(resolved.log_level, LogLevel::Debug);
    }

    #[test]
    fn cli_values_take_precedence_over_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("fileferry.toml");
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
            repo: Some("/var/backups/fileferry".to_owned()),
            log_level: Some("error".to_owned()),
            no_progress: true,
            ..GlobalArgs::default()
        };

        let resolved = resolve_config_with_env(&globals, Some(temp.path()), EnvConfig::default())
            .expect("config");

        assert_eq!(
            resolved.repository_url.as_deref(),
            Some("/var/backups/fileferry")
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
            temp.path().join("fileferry.toml"),
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

        assert!(output.stdout.contains("_ferry"));
        assert!(output.stdout.contains("version"));
        assert!(output.stdout.contains("completion"));
        assert_eq!(output.stderr, "");
    }
}
