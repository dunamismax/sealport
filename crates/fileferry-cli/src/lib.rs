use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use fileferry_core::{
    BackupPipeline, BackupPipelineConfig, BackupRequest, CheckReadDataSubset,
    CheckRepositoryOptions, CoreError, MetadataStatus, RestoreDestinationAction,
    RestoreDestinationRequest, RestoreOverwritePolicy, SnapshotEntry, SnapshotSelection,
    create_repository, list_snapshot_entries, open_repository, select_snapshot, snapshot_summaries,
};
use fileferry_crypto::KdfParams;
use fileferry_platform::{EntryKind, MetadataValue};
use fileferry_policy::{
    PolicyError, RetentionAction, RetentionCount, RetentionDecision, RetentionPlan,
    RetentionPolicy, RetentionSnapshot,
};
use fileferry_storage::{
    LocalStore, ObjectKeyPrefix, ObjectStore, PolicyObjectStore, S3Store, S3StoreConfig,
    StorageError, StoragePolicy,
};
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

    /// Create an encrypted repository.
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

    /// Verify an initialized local repository.
    Check {
        /// Read and verify a deterministic subset of referenced chunks, as a count or percent.
        #[arg(
            long = "read-data-subset",
            value_name = "N|PERCENT",
            value_parser = parse_read_data_subset
        )]
        read_data_subset: Option<CheckReadDataSubset>,
    },

    /// Mark snapshots forgotten without deleting repository objects.
    Forget {
        /// Report what would be forgotten without writing forget markers.
        #[arg(long)]
        dry_run: bool,

        /// Keep the newest N snapshots.
        #[arg(long = "keep-last", value_name = "N")]
        keep_last: Option<u32>,

        /// Keep the newest snapshot per hour for N hourly buckets.
        #[arg(long = "keep-hourly", value_name = "N")]
        keep_hourly: Option<u32>,

        /// Keep the newest snapshot per day for N daily buckets.
        #[arg(long = "keep-daily", value_name = "N")]
        keep_daily: Option<u32>,

        /// Keep the newest snapshot per week for N weekly buckets.
        #[arg(long = "keep-weekly", value_name = "N")]
        keep_weekly: Option<u32>,

        /// Keep the newest snapshot per month for N monthly buckets.
        #[arg(long = "keep-monthly", value_name = "N")]
        keep_monthly: Option<u32>,

        /// Keep the newest snapshot per year for N yearly buckets.
        #[arg(long = "keep-yearly", value_name = "N")]
        keep_yearly: Option<u32>,

        /// Keep snapshots carrying this tag. May be repeated.
        #[arg(long = "keep-tag")]
        keep_tags: Vec<String>,
    },

    /// Restore entries from a committed snapshot.
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

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Completion { .. } => "completion",
            Self::Init => "init",
            Self::Backup { .. } => "backup",
            Self::Snapshots => "snapshots",
            Self::Ls { .. } => "ls",
            Self::Check { .. } => "check",
            Self::Forget { .. } => "forget",
            Self::Restore { .. } => "restore",
            Self::Version => "version",
        }
    }
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Repository(#[from] RepositoryError),

    #[error(transparent)]
    Core(Box<CoreError>),

    #[error(transparent)]
    Policy(#[from] PolicyError),

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
            Self::Policy(_) => 2,
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

    #[error("S3 repository URL {value} is invalid: {reason}")]
    InvalidS3RepositoryUrl {
        value: Redacted,
        reason: &'static str,
    },

    #[error("environment variable {name} is required for S3 repository access")]
    MissingS3Environment { name: &'static str },

    #[error("S3 backend configuration is invalid: {reason}")]
    InvalidS3Config { reason: String },

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
            | Self::InvalidFileRepositoryUrl { .. }
            | Self::InvalidS3RepositoryUrl { .. }
            | Self::MissingS3Environment { .. }
            | Self::InvalidS3Config { .. } => 2,
            Self::UnsupportedRepository { .. } => 9,
            Self::Runtime { .. } => 1,
        }
    }
}

fn core_exit_code(error: &CoreError) -> i32 {
    match error {
        CoreError::Storage { source } => storage_exit_code(source),
        CoreError::RepositoryNotInitialized
        | CoreError::UnsupportedRepositoryFormat { .. }
        | CoreError::UnsupportedRepositoryFeatures => 3,
        CoreError::RepositoryUnlock { .. } => 4,
        CoreError::SnapshotNotFound { .. }
        | CoreError::ForgetNoSnapshotsMatched
        | CoreError::SnapshotPathNotFound { .. } => 7,
        CoreError::RepositoryBootstrapDecode { .. }
        | CoreError::InvalidRepositoryBootstrap { .. }
        | CoreError::InvalidSnapshotManifest { .. }
        | CoreError::CommitDecode { .. }
        | CoreError::InvalidCommitMarker { .. }
        | CoreError::ForgetMarkerDecode { .. }
        | CoreError::InvalidForgetMarker { .. }
        | CoreError::MetadataIdentityMismatch { .. }
        | CoreError::ObjectDecode { .. }
        | CoreError::ObjectAuthentication { .. }
        | CoreError::MetadataDecode { .. }
        | CoreError::ChunkIdentityMismatch { .. }
        | CoreError::ChunkIndexMismatch { .. }
        | CoreError::Decompression { .. }
        | CoreError::InvalidChunkLength { .. }
        | CoreError::MissingChunkIndexEntry { .. } => 6,
        CoreError::Encryption { .. } => 6,
        CoreError::ObjectKey { .. }
        | CoreError::Serialization { .. }
        | CoreError::SystemClock { .. }
        | CoreError::InvalidBackupPipelineConfig { .. }
        | CoreError::InvalidChunkingConfig { .. } => 1,
        CoreError::SourceRootNotAbsolute { .. }
        | CoreError::InvalidCheckDataSubset { .. }
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
        | CoreError::RestoreDestinationWrite { .. }
        | CoreError::RestoreVerificationRead { .. } => 5,
        CoreError::InvalidChunkRange { .. }
        | CoreError::RestoreVerificationMismatch { .. }
        | CoreError::RepositoryCheckMissingObject { .. }
        | CoreError::RepositoryReferencedObjectMissing { .. } => 6,
        CoreError::UnsupportedRestoreFeature { .. } => 9,
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
    pub exit_code: i32,
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
    metadata_planned: usize,
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
    S3Compatible,
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
struct ForgetData {
    dry_run: bool,
    snapshots_matched: usize,
    snapshots_forgotten: usize,
    retained_snapshots: usize,
    object_deletion: bool,
    marker_objects_written: usize,
    candidate_snapshots: Vec<ForgetSnapshotItem>,
    kept_snapshots: Vec<ForgetSnapshotItem>,
    forgotten_snapshots: Vec<ForgetSnapshotItem>,
    forgotten_snapshot_ids: Vec<String>,
    policy_summary: RetentionPolicySummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ForgetSnapshotItem {
    snapshot_id: String,
    created_at_unix_seconds: u64,
    tags: Vec<String>,
    action: RetentionAction,
    reasons: Vec<String>,
    marker_object: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct RetentionPolicySummary {
    keep_last: Option<u32>,
    keep_hourly: Option<u32>,
    keep_daily: Option<u32>,
    keep_weekly: Option<u32>,
    keep_monthly: Option<u32>,
    keep_yearly: Option<u32>,
    keep_tags: Vec<String>,
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

#[derive(Debug, Serialize)]
struct FailureData {
    code: String,
    message: String,
    exit_code: i32,
    retryable: bool,
    path: Option<String>,
    object_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finding: Option<fileferry_core::CheckFinding>,
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
        Command::Check { read_data_subset } => {
            let config = resolve_config(&cli.globals)?;
            check(mode, &config, read_data_subset)
        }
        Command::Forget {
            dry_run,
            keep_last,
            keep_hourly,
            keep_daily,
            keep_weekly,
            keep_monthly,
            keep_yearly,
            keep_tags,
        } => {
            let config = resolve_config(&cli.globals)?;
            let policy = retention_policy_from_args(
                keep_last,
                keep_hourly,
                keep_daily,
                keep_weekly,
                keep_monthly,
                keep_yearly,
                keep_tags,
            )?;
            forget(mode, &config, policy, dry_run)
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

pub fn run_with_error_output(cli: Cli) -> (Output, i32) {
    let mode = OutputMode::from_globals(&cli.globals);
    let command = cli.command.name();

    match run(cli) {
        Ok(output) => {
            let exit_code = output.exit_code;
            (output, exit_code)
        }
        Err(error) => {
            let exit_code = error.exit_code();
            match render_error_output(mode, command, &error, exit_code) {
                Ok(output) => (output, exit_code),
                Err(render_error) => (
                    Output {
                        stdout: String::new(),
                        stderr: format!("{render_error}\n"),
                        exit_code: render_error.exit_code(),
                    },
                    render_error.exit_code(),
                ),
            }
        }
    }
}

fn render_error_output(
    mode: OutputMode,
    command: &'static str,
    error: &CliError,
    exit_code: i32,
) -> Result<Output, CliError> {
    let data = failure_data(command, error, exit_code);
    let output = match mode {
        OutputMode::Human => Output {
            stdout: String::new(),
            stderr: format!("{error}\n"),
            exit_code,
        },
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command,
                status: CommandStatus::Failure,
                data,
            };
            Output {
                stdout: format!("{}\n", serde_json::to_string_pretty(&document)?),
                stderr: String::new(),
                exit_code,
            }
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<FailureData> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command,
                status: CommandStatus::Started,
                data: None,
            };
            let failed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandFailed,
                command,
                status: CommandStatus::Failure,
                data: Some(data),
            };
            Output {
                stdout: format!(
                    "{}\n{}\n",
                    serde_json::to_string(&started)?,
                    serde_json::to_string(&failed)?
                ),
                stderr: String::new(),
                exit_code,
            }
        }
    };

    Ok(output)
}

fn failure_data(command: &'static str, error: &CliError, exit_code: i32) -> FailureData {
    FailureData {
        code: failure_code(error).to_owned(),
        message: error.to_string(),
        exit_code,
        retryable: failure_retryable(error),
        path: failure_path(error),
        object_key: failure_object_key(error),
        finding: match error {
            CliError::Core(error) if command == "check" => check_finding_for_core_error(error),
            _ => None,
        },
    }
}

fn failure_code(error: &CliError) -> &'static str {
    match error {
        CliError::Config(error) => match error {
            ConfigError::Read { .. } => "config_read_failed",
            ConfigError::Parse { .. } => "config_parse_failed",
            ConfigError::MissingProfile { .. } => "config_profile_missing",
            ConfigError::InvalidRepositoryUrl { .. } => "config_repository_url_invalid",
            ConfigError::InvalidLogLevel { .. } => "config_log_level_invalid",
            ConfigError::InvalidProgress { .. } => "config_progress_invalid",
        },
        CliError::Repository(error) => match error {
            RepositoryError::MissingRepository => "repository_url_missing",
            RepositoryError::MissingPassword => "repository_password_missing",
            RepositoryError::PasswordFileRead { .. } => "repository_password_file_read_failed",
            RepositoryError::UnsupportedRepository { .. } => "repository_url_unsupported",
            RepositoryError::InvalidFileRepositoryUrl { .. } => "repository_file_url_invalid",
            RepositoryError::InvalidS3RepositoryUrl { .. } => "repository_s3_url_invalid",
            RepositoryError::MissingS3Environment { .. } => "repository_s3_environment_missing",
            RepositoryError::InvalidS3Config { .. } => "repository_s3_config_invalid",
            RepositoryError::Runtime { .. } => "repository_runtime_failed",
        },
        CliError::Core(error) => core_failure_code(error),
        CliError::Policy(error) => match error {
            PolicyError::EmptyPolicy => "retention_policy_empty",
            PolicyError::UnknownRule { .. } => "retention_policy_rule_unknown",
            PolicyError::MissingValue { .. } => "retention_policy_value_missing",
            PolicyError::DuplicateRule { .. } => "retention_policy_rule_duplicated",
            PolicyError::InvalidCount { .. } => "retention_policy_count_invalid",
            PolicyError::InvalidTag { .. } => "retention_policy_tag_invalid",
        },
        CliError::Json(_) => "json_serialization_failed",
        CliError::Completion(_) => "completion_generation_failed",
    }
}

fn core_failure_code(error: &CoreError) -> &'static str {
    match error {
        CoreError::SourceRootNotAbsolute { .. } => "source_root_not_absolute",
        CoreError::SourceRootRead { .. } => "source_root_read_failed",
        CoreError::DirectoryRead { .. } => "directory_read_failed",
        CoreError::DirectoryEntryRead { .. } => "directory_entry_read_failed",
        CoreError::MetadataCapture { .. } => "metadata_capture_failed",
        CoreError::InvalidChunkingConfig { .. } => "chunking_config_invalid",
        CoreError::InvalidBackupPipelineConfig { .. } => "backup_pipeline_config_invalid",
        CoreError::InvalidCheckDataSubset { .. } => "check_data_subset_invalid",
        CoreError::FileRead { .. } => "file_read_failed",
        CoreError::InvalidChunkRange { .. } => "chunk_range_invalid",
        CoreError::Compression { .. } => "chunk_compression_failed",
        CoreError::Decompression { .. } => "chunk_decompression_failed",
        CoreError::InvalidChunkLength { .. } => "chunk_length_invalid",
        CoreError::MissingChunkIndexEntry { .. } => "chunk_index_entry_missing",
        CoreError::ChunkIndexMismatch { .. } => "chunk_index_mismatch",
        CoreError::ChunkIdentityMismatch { .. } => "chunk_identity_mismatch",
        CoreError::Encryption { .. } => "repository_authentication_failed",
        CoreError::Serialization { .. } => "repository_metadata_serialization_failed",
        CoreError::ObjectDecode { .. } => "repository_object_decode_failed",
        CoreError::ObjectAuthentication { .. } => "repository_object_authentication_failed",
        CoreError::MetadataDecode { .. } => "repository_metadata_decode_failed",
        CoreError::MetadataIdentityMismatch { .. } => "repository_metadata_identity_mismatch",
        CoreError::CommitDecode { .. } => "repository_commit_decode_failed",
        CoreError::InvalidCommitMarker { .. } => "repository_commit_marker_invalid",
        CoreError::ForgetNoSnapshotsMatched => "forget_no_snapshots_matched",
        CoreError::ForgetMarkerDecode { .. } => "repository_forget_marker_decode_failed",
        CoreError::InvalidForgetMarker { .. } => "repository_forget_marker_invalid",
        CoreError::RepositoryBootstrapDecode { .. } => "repository_bootstrap_decode_failed",
        CoreError::RepositoryNotInitialized => "repository_not_initialized",
        CoreError::InvalidRepositoryBootstrap { .. } => "repository_bootstrap_invalid",
        CoreError::UnsupportedRepositoryFormat { .. } => "repository_format_unsupported",
        CoreError::UnsupportedRepositoryFeatures => "repository_features_unsupported",
        CoreError::RepositoryUnlock { .. } => "repository_unlock_failed",
        CoreError::SnapshotNotFound { .. } => "snapshot_not_found",
        CoreError::SnapshotPathNotFound { .. } => "snapshot_path_not_found",
        CoreError::InvalidSnapshotManifest { .. } => "snapshot_manifest_invalid",
        CoreError::InvalidRestoreRequest { .. } => "restore_request_invalid",
        CoreError::RestoreDestinationNotAbsolute { .. } => "restore_destination_not_absolute",
        CoreError::RestoreDestinationEscapesRoot { .. } => "restore_destination_escapes_root",
        CoreError::RestoreDestinationSymlink { .. } => "restore_destination_symlink",
        CoreError::RestoreDestinationExists { .. } => "restore_destination_exists",
        CoreError::RestoreDestinationKind { .. } => "restore_destination_kind_mismatch",
        CoreError::RestoreDestinationWrite { .. } => "restore_destination_write_failed",
        CoreError::RestoreVerificationRead { .. } => "restore_verification_read_failed",
        CoreError::RestoreVerificationMismatch { .. } => "restore_verification_mismatch",
        CoreError::UnsupportedRestoreFeature { .. } => "restore_feature_unsupported",
        CoreError::RepositoryCheckMissingObject { .. } => "repository_check_missing_object",
        CoreError::RepositoryReferencedObjectMissing { .. } => {
            "repository_referenced_object_missing"
        }
        CoreError::SystemClock { .. } => "system_clock_invalid",
        CoreError::ObjectKey { .. } => "repository_object_key_invalid",
        CoreError::Storage { source } => storage_failure_code(source),
    }
}

fn storage_failure_code(error: &StorageError) -> &'static str {
    match error {
        StorageError::InvalidObjectKey { .. } => "storage_object_key_invalid",
        StorageError::ObjectAlreadyExists { .. } => "storage_object_already_exists",
        StorageError::ObjectNotFound { .. } => "storage_object_not_found",
        StorageError::Io { .. } => "storage_io_failed",
        StorageError::BackendConfig { .. } => "storage_backend_config_failed",
        StorageError::ObjectIo { .. } => "storage_object_io_failed",
        StorageError::BackendObject { .. } => "storage_backend_object_failed",
        StorageError::Backend { .. } => "storage_backend_failed",
        StorageError::Timeout { .. } => "storage_timeout",
        StorageError::PolicyConfig { .. } => "storage_policy_config_invalid",
    }
}

fn failure_retryable(error: &CliError) -> bool {
    match error {
        CliError::Core(error) => match error.as_ref() {
            CoreError::Storage { source } => storage_retryable(source),
            CoreError::SourceRootRead { .. }
            | CoreError::DirectoryRead { .. }
            | CoreError::DirectoryEntryRead { .. }
            | CoreError::MetadataCapture { .. }
            | CoreError::FileRead { .. }
            | CoreError::RestoreDestinationWrite { .. }
            | CoreError::RestoreVerificationRead { .. } => true,
            _ => false,
        },
        _ => false,
    }
}

fn storage_retryable(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::Io { .. }
            | StorageError::ObjectIo { .. }
            | StorageError::BackendObject { .. }
            | StorageError::Backend { .. }
            | StorageError::Timeout { .. }
    )
}

fn failure_path(error: &CliError) -> Option<String> {
    match error {
        CliError::Config(error) => match error {
            ConfigError::Read { path, .. }
            | ConfigError::Parse { path, .. }
            | ConfigError::MissingProfile { path, .. } => Some(path.to_string()),
            _ => None,
        },
        CliError::Repository(error) => match error {
            RepositoryError::PasswordFileRead { path, .. } => Some(path.to_string()),
            _ => None,
        },
        CliError::Core(error) => core_failure_path(error),
        CliError::Policy(_) => None,
        CliError::Json(_) | CliError::Completion(_) => None,
    }
}

fn core_failure_path(error: &CoreError) -> Option<String> {
    match error {
        CoreError::SourceRootNotAbsolute { path }
        | CoreError::InvalidChunkRange { path }
        | CoreError::SnapshotPathNotFound { path, .. }
        | CoreError::InvalidSnapshotManifest {
            path: Some(path), ..
        }
        | CoreError::MissingChunkIndexEntry { path, .. }
        | CoreError::ChunkIndexMismatch { path, .. }
        | CoreError::RestoreDestinationNotAbsolute { path }
        | CoreError::RestoreDestinationEscapesRoot { path }
        | CoreError::RestoreDestinationExists { path }
        | CoreError::RestoreDestinationKind { path }
        | CoreError::RestoreDestinationWrite { path, .. }
        | CoreError::RestoreVerificationRead { path, .. }
        | CoreError::RestoreVerificationMismatch { path } => {
            Some(redact_for_display(&path.display().to_string()))
        }
        CoreError::SourceRootRead { path, .. }
        | CoreError::DirectoryRead { path, .. }
        | CoreError::DirectoryEntryRead { path, .. }
        | CoreError::MetadataCapture { path, .. }
        | CoreError::FileRead { path, .. }
        | CoreError::Compression { path, .. } => {
            Some(redact_for_display(&path.display().to_string()))
        }
        CoreError::RestoreDestinationSymlink { path, .. } => {
            Some(redact_for_display(&path.display().to_string()))
        }
        CoreError::Decompression {
            path: Some(path), ..
        }
        | CoreError::InvalidChunkLength {
            path: Some(path), ..
        }
        | CoreError::ChunkIdentityMismatch {
            path: Some(path), ..
        } => Some(redact_for_display(&path.display().to_string())),
        _ => None,
    }
}

fn failure_object_key(error: &CliError) -> Option<String> {
    match error {
        CliError::Core(error) => core_failure_object_key(error),
        _ => None,
    }
}

fn core_failure_object_key(error: &CoreError) -> Option<String> {
    match error {
        CoreError::ObjectDecode { key, .. }
        | CoreError::ObjectAuthentication { key, .. }
        | CoreError::MetadataDecode { key, .. }
        | CoreError::MetadataIdentityMismatch {
            object_key: key, ..
        }
        | CoreError::InvalidSnapshotManifest {
            object_key: key, ..
        }
        | CoreError::CommitDecode { key, .. }
        | CoreError::InvalidCommitMarker { key, .. }
        | CoreError::ForgetMarkerDecode { key, .. }
        | CoreError::InvalidForgetMarker { key, .. }
        | CoreError::RepositoryCheckMissingObject { key }
        | CoreError::RepositoryReferencedObjectMissing { key } => Some(key.as_str().to_owned()),
        CoreError::MissingChunkIndexEntry { object_key, .. }
        | CoreError::ChunkIndexMismatch { object_key, .. } => Some(object_key.clone()),
        CoreError::Decompression {
            object_key: Some(object_key),
            ..
        }
        | CoreError::InvalidChunkLength {
            object_key: Some(object_key),
            ..
        }
        | CoreError::ChunkIdentityMismatch {
            object_key: Some(object_key),
            ..
        } => Some(object_key.clone()),
        CoreError::Storage {
            source:
                StorageError::ObjectAlreadyExists { key }
                | StorageError::ObjectNotFound { key }
                | StorageError::ObjectIo { key, .. }
                | StorageError::BackendObject { key, .. },
        } => Some(key.as_str().to_owned()),
        _ => None,
    }
}

fn core_failure_snapshot_id(error: &CoreError) -> Option<String> {
    match error {
        CoreError::MissingChunkIndexEntry { snapshot_id, .. }
        | CoreError::ChunkIndexMismatch { snapshot_id, .. }
        | CoreError::InvalidSnapshotManifest { snapshot_id, .. } => Some(snapshot_id.clone()),
        CoreError::Decompression {
            snapshot_id: Some(snapshot_id),
            ..
        }
        | CoreError::InvalidChunkLength {
            snapshot_id: Some(snapshot_id),
            ..
        }
        | CoreError::ChunkIdentityMismatch {
            snapshot_id: Some(snapshot_id),
            ..
        } => Some(snapshot_id.clone()),
        _ => None,
    }
}

fn check_finding_for_core_error(error: &CoreError) -> Option<fileferry_core::CheckFinding> {
    match error {
        CoreError::RepositoryCheckMissingObject { .. }
        | CoreError::RepositoryReferencedObjectMissing { .. }
        | CoreError::ObjectDecode { .. }
        | CoreError::ObjectAuthentication { .. }
        | CoreError::MetadataDecode { .. }
        | CoreError::InvalidSnapshotManifest { .. }
        | CoreError::MetadataIdentityMismatch { .. }
        | CoreError::CommitDecode { .. }
        | CoreError::InvalidCommitMarker { .. }
        | CoreError::MissingChunkIndexEntry { .. }
        | CoreError::ChunkIndexMismatch { .. }
        | CoreError::InvalidChunkLength { .. }
        | CoreError::Decompression { .. }
        | CoreError::ChunkIdentityMismatch { .. }
        | CoreError::Encryption { .. } => Some(fileferry_core::CheckFinding {
            code: core_failure_code(error).to_owned(),
            severity: fileferry_core::CheckFindingSeverity::Error,
            object_key: core_failure_object_key(error),
            snapshot_id: core_failure_snapshot_id(error),
            path: core_failure_path(error).map(PathBuf::from),
            message: error.to_string(),
        }),
        _ => None,
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
    if value.starts_with("s3://") {
        return "s3://<redacted>".to_owned();
    }

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
        exit_code: 0,
    })
}

fn init_repository(mode: OutputMode, config: &ResolvedConfig) -> Result<Output, CliError> {
    let repository = init_repository_store(config)?;
    let passphrase = repository_passphrase()?;
    let runtime = tokio_runtime()?;
    let result = runtime.block_on(create_repository(
        repository.store.as_ref(),
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

fn check(
    mode: OutputMode,
    config: &ResolvedConfig,
    read_data_subset: Option<CheckReadDataSubset>,
) -> Result<Output, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let runtime = tokio_runtime()?;
    let opened = runtime.block_on(open_repository(&repository.store, &passphrase))?;
    let pipeline = BackupPipeline::new(BackupPipelineConfig::new(opened.repository_id))?;
    let options = read_data_subset
        .map(CheckRepositoryOptions::subset)
        .unwrap_or_else(CheckRepositoryOptions::full);
    let data = runtime.block_on(pipeline.check_repository_with_options(
        &repository.store,
        &opened.master_key,
        options,
    ))?;

    emit_check_command(mode, data)
}

fn forget(
    mode: OutputMode,
    config: &ResolvedConfig,
    policy: RetentionPolicy,
    dry_run: bool,
) -> Result<Output, CliError> {
    let repository = local_repository(config)?;
    let passphrase = repository_passphrase()?;
    let runtime = tokio_runtime()?;
    let opened = runtime.block_on(open_repository(&repository.store, &passphrase))?;
    let pipeline = BackupPipeline::new(BackupPipelineConfig::new(opened.repository_id))?;
    let manifests = runtime.block_on(
        pipeline.read_committed_snapshot_manifests(&repository.store, &opened.master_key),
    )?;
    let snapshots = manifests
        .iter()
        .map(|manifest| RetentionSnapshot {
            snapshot_id: manifest.snapshot_id.clone(),
            created_at_unix_seconds: manifest.body.created_at_unix_seconds,
            tags: manifest.body.tags.clone(),
        })
        .collect::<Vec<_>>();
    let plan = policy.plan(&snapshots);
    let forgotten_snapshot_ids = plan
        .forgotten()
        .into_iter()
        .map(|decision| decision.snapshot_id.clone())
        .collect::<Vec<_>>();

    if forgotten_snapshot_ids.is_empty() {
        return Err(CoreError::ForgetNoSnapshotsMatched.into());
    }

    let marker_writes = if dry_run {
        BTreeMap::new()
    } else {
        runtime
            .block_on(
                pipeline.write_snapshot_forget_markers(&repository.store, &forgotten_snapshot_ids),
            )?
            .markers
            .into_iter()
            .map(|write| (write.snapshot_id, (write.marker_object, write.created)))
            .collect()
    };
    let data = forget_data(policy, plan, dry_run, marker_writes);

    emit_forget_command(mode, data)
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
    let directories_written = result
        .directories
        .iter()
        .filter(|directory| {
            matches!(
                directory.action,
                RestoreDestinationAction::Written | RestoreDestinationAction::WouldWrite
            )
        })
        .count();
    let symlinks_written = result
        .symlinks
        .iter()
        .filter(|symlink| {
            matches!(
                symlink.action,
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
        directories_written,
        symlinks_written,
        metadata_planned: result.metadata_planned,
        metadata_applied: result.metadata_applied,
        metadata_warnings: result
            .metadata_warnings
            .into_iter()
            .map(|warning| RestoreMetadataWarning {
                path: display_snapshot_path(&warning.relative_path),
                field: warning.field.to_owned(),
                reason: warning.reason,
            })
            .collect(),
        bytes_written: result.bytes,
        verified_files: result.verified_files,
    };

    emit_restore_command(mode, data)
}

struct InitRepository {
    url: String,
    backend: CliBackendKind,
    store: Box<dyn ObjectStore>,
}

struct LocalRepository {
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

fn init_repository_store(config: &ResolvedConfig) -> Result<InitRepository, RepositoryError> {
    let url = config
        .repository_url
        .as_deref()
        .ok_or(RepositoryError::MissingRepository)?;

    if url.starts_with("s3://") {
        let s3_config = s3_repository_config(url, &S3RepositoryEnvironment::current())?;
        let store = S3Store::new(s3_config).map_err(|_| RepositoryError::InvalidS3Config {
            reason: "S3 client configuration failed".to_owned(),
        })?;
        let store = PolicyObjectStore::from_store(store, StoragePolicy::default());
        return Ok(InitRepository {
            url: url.to_owned(),
            backend: CliBackendKind::S3Compatible,
            store: Box::new(store),
        });
    }

    let path = local_repository_path(url)?;

    Ok(InitRepository {
        url: url.to_owned(),
        backend: CliBackendKind::Local,
        store: Box::new(LocalStore::new(path)),
    })
}

fn local_repository(config: &ResolvedConfig) -> Result<LocalRepository, RepositoryError> {
    let url = config
        .repository_url
        .as_deref()
        .ok_or(RepositoryError::MissingRepository)?;
    let path = local_repository_path(url)?;

    Ok(LocalRepository {
        store: LocalStore::new(path),
    })
}

#[derive(Clone, Default)]
struct S3RepositoryEnvironment {
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    disable_conditional_create: bool,
}

impl std::fmt::Debug for S3RepositoryEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3RepositoryEnvironment")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field(
                "access_key_id",
                &self.access_key_id.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "disable_conditional_create",
                &self.disable_conditional_create,
            )
            .finish()
    }
}

impl S3RepositoryEnvironment {
    fn current() -> Self {
        Self {
            endpoint: env::var("FILEFERRY_S3_ENDPOINT").ok(),
            region: env::var("FILEFERRY_S3_REGION").ok(),
            access_key_id: env::var("FILEFERRY_S3_ACCESS_KEY_ID").ok(),
            secret_access_key: env::var("FILEFERRY_S3_SECRET_ACCESS_KEY").ok(),
            disable_conditional_create: env::var("FILEFERRY_S3_DISABLE_CONDITIONAL_CREATE")
                .ok()
                .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on")),
        }
    }
}

fn s3_repository_config(
    value: &str,
    environment: &S3RepositoryEnvironment,
) -> Result<S3StoreConfig, RepositoryError> {
    let parsed = parse_s3_repository_url(value)?;
    let endpoint = required_s3_env(environment.endpoint.as_deref(), "FILEFERRY_S3_ENDPOINT")?;
    let region = required_s3_env(environment.region.as_deref(), "FILEFERRY_S3_REGION")?;
    let access_key_id = required_s3_env(
        environment.access_key_id.as_deref(),
        "FILEFERRY_S3_ACCESS_KEY_ID",
    )?;
    let secret_access_key = required_s3_env(
        environment.secret_access_key.as_deref(),
        "FILEFERRY_S3_SECRET_ACCESS_KEY",
    )?;

    S3StoreConfig::new(
        parsed.bucket,
        region,
        endpoint,
        SecretString::from(access_key_id.to_owned()),
        SecretString::from(secret_access_key.to_owned()),
        parsed.root_prefix,
    )
    .map(|config| config.with_conditional_create(!environment.disable_conditional_create))
    .map_err(|error| match error {
        StorageError::BackendConfig { reason, .. } => RepositoryError::InvalidS3Config { reason },
        error => RepositoryError::InvalidS3Config {
            reason: error.to_string(),
        },
    })
}

fn required_s3_env<'a>(
    value: Option<&'a str>,
    name: &'static str,
) -> Result<&'a str, RepositoryError> {
    let Some(value) = value else {
        return Err(RepositoryError::MissingS3Environment { name });
    };
    if value.trim().is_empty() {
        return Err(RepositoryError::MissingS3Environment { name });
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedS3RepositoryUrl {
    bucket: String,
    root_prefix: ObjectKeyPrefix,
}

fn parse_s3_repository_url(value: &str) -> Result<ParsedS3RepositoryUrl, RepositoryError> {
    let Some(rest) = value.strip_prefix("s3://") else {
        return Err(RepositoryError::InvalidS3RepositoryUrl {
            value: Redacted::new(value),
            reason: "expected s3://bucket[/prefix]",
        });
    };

    if rest.contains('?') || rest.contains('#') {
        return Err(RepositoryError::InvalidS3RepositoryUrl {
            value: Redacted::new(value),
            reason: "query strings and fragments are not supported",
        });
    }

    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket.is_empty() {
        return Err(RepositoryError::InvalidS3RepositoryUrl {
            value: Redacted::new(value),
            reason: "bucket must not be empty",
        });
    }

    if bucket.contains('@') || bucket.contains(':') {
        return Err(RepositoryError::InvalidS3RepositoryUrl {
            value: Redacted::new(value),
            reason: "credentials must be supplied through environment variables",
        });
    }

    let root_prefix = if prefix.is_empty() {
        ObjectKeyPrefix::root()
    } else {
        ObjectKeyPrefix::new(prefix.to_owned()).map_err(|_| {
            RepositoryError::InvalidS3RepositoryUrl {
                value: Redacted::new(value),
                reason: "prefix must be relative, non-empty, and use FileFerry object-key characters",
            }
        })?
    };

    Ok(ParsedS3RepositoryUrl {
        bucket: bucket.to_owned(),
        root_prefix,
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

fn parse_read_data_subset(value: &str) -> Result<CheckReadDataSubset, String> {
    if let Some(percent) = value.strip_suffix('%') {
        if percent.is_empty() {
            return Err("percent subset must include a number before '%'".to_owned());
        }
        let percent = percent
            .parse::<u8>()
            .map_err(|_| "percent subset must be an integer from 1% through 100%".to_owned())?;
        return CheckReadDataSubset::percent(percent).map_err(|error| error.to_string());
    }

    let count = value
        .parse::<usize>()
        .map_err(|_| "count subset must be a positive integer or percent like 5%".to_owned())?;
    CheckReadDataSubset::count(count).map_err(|error| error.to_string())
}

fn retention_policy_from_args(
    keep_last: Option<u32>,
    keep_hourly: Option<u32>,
    keep_daily: Option<u32>,
    keep_weekly: Option<u32>,
    keep_monthly: Option<u32>,
    keep_yearly: Option<u32>,
    keep_tags: Vec<String>,
) -> Result<RetentionPolicy, PolicyError> {
    RetentionPolicy {
        keep_last: keep_last.map(RetentionCount::new).transpose()?,
        keep_hourly: keep_hourly.map(RetentionCount::new).transpose()?,
        keep_daily: keep_daily.map(RetentionCount::new).transpose()?,
        keep_weekly: keep_weekly.map(RetentionCount::new).transpose()?,
        keep_monthly: keep_monthly.map(RetentionCount::new).transpose()?,
        keep_yearly: keep_yearly.map(RetentionCount::new).transpose()?,
        keep_tags,
    }
    .validate()
}

fn forget_data(
    policy: RetentionPolicy,
    plan: RetentionPlan,
    dry_run: bool,
    marker_writes: BTreeMap<String, (String, bool)>,
) -> ForgetData {
    let candidate_snapshots = plan
        .candidates()
        .iter()
        .map(|decision| forget_item(decision, &marker_writes))
        .collect::<Vec<_>>();
    let kept_snapshots = candidate_snapshots
        .iter()
        .filter(|item| item.action == RetentionAction::Keep)
        .cloned()
        .collect::<Vec<_>>();
    let forgotten_snapshots = candidate_snapshots
        .iter()
        .filter(|item| item.action == RetentionAction::Forget)
        .cloned()
        .collect::<Vec<_>>();
    let forgotten_snapshot_ids = forgotten_snapshots
        .iter()
        .map(|item| item.snapshot_id.clone())
        .collect::<Vec<_>>();
    let marker_objects_written = marker_writes
        .values()
        .filter(|(_, created)| *created)
        .count();

    ForgetData {
        dry_run,
        snapshots_matched: candidate_snapshots.len(),
        snapshots_forgotten: forgotten_snapshots.len(),
        retained_snapshots: kept_snapshots.len(),
        object_deletion: false,
        marker_objects_written,
        candidate_snapshots,
        kept_snapshots,
        forgotten_snapshots,
        forgotten_snapshot_ids,
        policy_summary: RetentionPolicySummary::from_policy(&policy),
    }
}

fn forget_item(
    decision: &RetentionDecision,
    marker_writes: &BTreeMap<String, (String, bool)>,
) -> ForgetSnapshotItem {
    ForgetSnapshotItem {
        snapshot_id: decision.snapshot_id.clone(),
        created_at_unix_seconds: decision.created_at_unix_seconds,
        tags: decision.tags.clone(),
        action: decision.action,
        reasons: decision.reasons.clone(),
        marker_object: marker_writes
            .get(&decision.snapshot_id)
            .map(|(marker_object, _)| marker_object.clone()),
    }
}

impl RetentionPolicySummary {
    fn from_policy(policy: &RetentionPolicy) -> Self {
        Self {
            keep_last: policy.keep_last.map(RetentionCount::get),
            keep_hourly: policy.keep_hourly.map(RetentionCount::get),
            keep_daily: policy.keep_daily.map(RetentionCount::get),
            keep_weekly: policy.keep_weekly.map(RetentionCount::get),
            keep_monthly: policy.keep_monthly.map(RetentionCount::get),
            keep_yearly: policy.keep_yearly.map(RetentionCount::get),
            keep_tags: policy.keep_tags.clone(),
        }
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
        exit_code: 0,
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
        exit_code: 0,
    })
}

fn emit_restore_command(mode: OutputMode, data: RestoreData) -> Result<Output, CliError> {
    let exit_code = if data.metadata_warnings.is_empty() {
        0
    } else {
        10
    };
    let stderr = match mode {
        OutputMode::Human => restore_warning_stderr(&data.metadata_warnings),
        OutputMode::Json | OutputMode::Jsonl => String::new(),
    };
    let stdout = match mode {
        OutputMode::Human => {
            let action = if data.dry_run {
                "Would restore"
            } else {
                "Restored"
            };
            format!(
                "{} snapshot {} to {}\nentries_selected={} directories={} files={} symlinks={} bytes={} verified_files={}\n",
                action,
                data.snapshot_id,
                data.destination,
                data.entries_selected,
                data.directories_written,
                data.files_written,
                data.symlinks_written,
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
                        items_done: Some(data.entries_selected),
                        items_total: Some(data.entries_selected),
                        bytes_done: Some(data.bytes_written),
                        bytes_total: Some(data.bytes_written),
                        snapshot_id: Some(data.snapshot_id.clone()),
                        object_key: None,
                    }),
                };
                lines.push(serde_json::to_string(&event)?);
            }
            for warning in &data.metadata_warnings {
                let event = CommandEvent {
                    schema_version: OUTPUT_SCHEMA_VERSION,
                    event: EventKind::Warning,
                    command: "restore",
                    status: CommandStatus::Started,
                    data: Some(warning),
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
        stderr,
        exit_code,
    })
}

fn restore_warning_stderr(warnings: &[RestoreMetadataWarning]) -> String {
    warnings
        .iter()
        .map(|warning| {
            format!(
                "warning: restore metadata {} for {}: {}\n",
                warning.field, warning.path, warning.reason
            )
        })
        .collect()
}

fn emit_check_command(
    mode: OutputMode,
    data: fileferry_core::RepositoryCheckResult,
) -> Result<Output, CliError> {
    let stdout = match mode {
        OutputMode::Human => {
            if data.errors.is_empty() {
                format!(
                    "Repository {} checked successfully\nmetadata_objects={} chunk_objects={} bytes_read={} read_data_mode={}\n",
                    data.repository_id,
                    data.metadata_objects_checked,
                    data.chunk_objects_checked,
                    data.bytes_read,
                    display_check_read_data_mode(data.read_data_mode)
                )
            } else {
                format!(
                    "Repository {} check found {} errors and {} warnings\n",
                    data.repository_id,
                    data.errors.len(),
                    data.warnings.len()
                )
            }
        }
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command: "check",
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<fileferry_core::RepositoryCheckResult> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command: "check",
                status: CommandStatus::Started,
                data: None,
            };
            let phases = [
                ("load_commits", "loaded snapshot commit markers"),
                ("verify_metadata", "verified encrypted snapshot manifests"),
                ("verify_indexes", "verified encrypted chunk indexes"),
                ("read_data", "read and verified referenced chunk data"),
                ("complete", "completed repository check"),
            ];
            let mut lines = vec![serde_json::to_string(&started)?];
            for (phase, message) in phases {
                let event = CommandEvent {
                    schema_version: OUTPUT_SCHEMA_VERSION,
                    event: EventKind::Progress,
                    command: "check",
                    status: CommandStatus::Started,
                    data: Some(ProgressData {
                        phase,
                        message,
                        items_done: Some(
                            data.metadata_objects_checked + data.chunk_objects_checked,
                        ),
                        items_total: Some(
                            data.metadata_objects_checked + data.chunk_objects_checked,
                        ),
                        bytes_done: Some(data.bytes_read),
                        bytes_total: Some(data.bytes_read),
                        snapshot_id: None,
                        object_key: None,
                    }),
                };
                lines.push(serde_json::to_string(&event)?);
            }
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command: "check",
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
        exit_code: 0,
    })
}

fn emit_forget_command(mode: OutputMode, data: ForgetData) -> Result<Output, CliError> {
    let stdout = match mode {
        OutputMode::Human => {
            let action = if data.dry_run {
                "Would mark forgotten"
            } else {
                "Marked forgotten"
            };
            format!(
                "{} {} snapshot(s)\nretained_snapshots={} candidate_snapshots={} marker_objects_written={} object_deletion=false\n",
                action,
                data.snapshots_forgotten,
                data.retained_snapshots,
                data.snapshots_matched,
                data.marker_objects_written
            )
        }
        OutputMode::Json => {
            let document = CommandDocument {
                schema_version: OUTPUT_SCHEMA_VERSION,
                command: "forget",
                status: CommandStatus::Success,
                data,
            };
            format!("{}\n", serde_json::to_string_pretty(&document)?)
        }
        OutputMode::Jsonl => {
            let started = CommandEvent::<ForgetData> {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandStarted,
                command: "forget",
                status: CommandStatus::Started,
                data: None,
            };
            let phases = [
                ("load_snapshots", "loaded committed snapshots"),
                ("evaluate_policy", "evaluated retention policy"),
                ("write_forget_state", "wrote snapshot forget markers"),
                ("complete", "completed forget"),
            ];
            let mut lines = vec![serde_json::to_string(&started)?];
            for (phase, message) in phases {
                let event = CommandEvent {
                    schema_version: OUTPUT_SCHEMA_VERSION,
                    event: EventKind::Progress,
                    command: "forget",
                    status: CommandStatus::Started,
                    data: Some(ProgressData {
                        phase,
                        message,
                        items_done: Some(data.snapshots_matched),
                        items_total: Some(data.snapshots_matched),
                        bytes_done: None,
                        bytes_total: None,
                        snapshot_id: None,
                        object_key: None,
                    }),
                };
                lines.push(serde_json::to_string(&event)?);
            }
            let completed = CommandEvent {
                schema_version: OUTPUT_SCHEMA_VERSION,
                event: EventKind::CommandCompleted,
                command: "forget",
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
        exit_code: 0,
    })
}

fn display_check_read_data_mode(mode: fileferry_core::CheckReadDataMode) -> &'static str {
    match mode {
        fileferry_core::CheckReadDataMode::MetadataOnly => "metadata_only",
        fileferry_core::CheckReadDataMode::Subset => "subset",
        fileferry_core::CheckReadDataMode::Full => "full",
    }
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
        exit_code: 0,
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
    fn restore_metadata_warnings_use_partial_success_exit_code() {
        let output = emit_restore_command(
            OutputMode::Human,
            RestoreData {
                snapshot_id: "snapshot".to_owned(),
                destination: "/restore".to_owned(),
                paths: Vec::new(),
                dry_run: false,
                overwrite: CliRestoreOverwritePolicy::FailIfExists,
                entries_selected: 1,
                files_written: 1,
                directories_written: 0,
                symlinks_written: 0,
                metadata_planned: 1,
                metadata_applied: 0,
                metadata_warnings: vec![RestoreMetadataWarning {
                    path: "sample.txt".to_owned(),
                    field: "modified".to_owned(),
                    reason: "modified timestamp was not captured".to_owned(),
                }],
                bytes_written: 6,
                verified_files: 1,
            },
        )
        .expect("restore output");

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.contains("Restored snapshot snapshot"));
        assert!(output.stderr.contains("warning: restore metadata modified"));
    }

    #[test]
    fn restore_jsonl_metadata_warnings_stay_on_stdout() {
        let output = emit_restore_command(
            OutputMode::Jsonl,
            RestoreData {
                snapshot_id: "snapshot".to_owned(),
                destination: "/restore".to_owned(),
                paths: Vec::new(),
                dry_run: false,
                overwrite: CliRestoreOverwritePolicy::FailIfExists,
                entries_selected: 1,
                files_written: 1,
                directories_written: 0,
                symlinks_written: 0,
                metadata_planned: 1,
                metadata_applied: 0,
                metadata_warnings: vec![RestoreMetadataWarning {
                    path: "sample.txt".to_owned(),
                    field: "modified".to_owned(),
                    reason: "modified timestamp was not captured".to_owned(),
                }],
                bytes_written: 6,
                verified_files: 1,
            },
        )
        .expect("restore output");

        let lines = output
            .stdout
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("jsonl event"))
            .collect::<Vec<_>>();

        assert_eq!(output.exit_code, 10);
        assert_eq!(output.stderr, "");
        assert!(lines.iter().any(|event| event["event"] == "warning"));
        assert_eq!(
            lines.last().expect("completed event")["event"],
            "command_completed"
        );
    }

    #[test]
    fn check_failure_finding_preserves_chunk_reference_context() {
        let error = CliError::Core(Box::new(CoreError::ChunkIndexMismatch {
            snapshot_id: "snapshot-123".to_owned(),
            path: PathBuf::from("docs/sample.txt"),
            chunk_id: "chunk-123".to_owned(),
            object_key: "objects/chunk/ab/chunk-123".to_owned(),
            reason: "plaintext length mismatch",
        }));
        let output = render_error_output(OutputMode::Json, "check", &error, 6)
            .expect("render check failure");
        let failure: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("failure json");

        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 6);
        assert_eq!(failure["data"]["code"], "chunk_index_mismatch");
        assert_eq!(failure["data"]["path"], "docs/sample.txt");
        assert_eq!(failure["data"]["object_key"], "objects/chunk/ab/chunk-123");
        assert_eq!(failure["data"]["finding"]["snapshot_id"], "snapshot-123");
        assert_eq!(failure["data"]["finding"]["path"], "docs/sample.txt");
        assert_eq!(
            failure["data"]["finding"]["object_key"],
            "objects/chunk/ab/chunk-123"
        );
    }

    #[test]
    fn check_failure_finding_preserves_invalid_manifest_context() {
        let object_key =
            fileferry_storage::ObjectKey::new("objects/manifest/ab/abcdef").expect("object key");
        let error = CliError::Core(Box::new(CoreError::InvalidSnapshotManifest {
            snapshot_id: "snapshot-abc".to_owned(),
            object_key,
            path: Some(PathBuf::from("docs/sample.txt")),
            reason: "duplicate entry path",
        }));
        let output = render_error_output(OutputMode::Json, "check", &error, 6)
            .expect("render check failure");
        let failure: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("failure json");

        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 6);
        assert_eq!(failure["data"]["code"], "snapshot_manifest_invalid");
        assert_eq!(failure["data"]["path"], "docs/sample.txt");
        assert_eq!(failure["data"]["object_key"], "objects/manifest/ab/abcdef");
        assert_eq!(failure["data"]["finding"]["snapshot_id"], "snapshot-abc");
        assert_eq!(failure["data"]["finding"]["path"], "docs/sample.txt");
        assert_eq!(
            failure["data"]["finding"]["object_key"],
            "objects/manifest/ab/abcdef"
        );
    }

    #[test]
    fn check_failure_finding_preserves_metadata_identity_object_key() {
        let object_key =
            fileferry_storage::ObjectKey::new("objects/index/ab/abcdef").expect("object key");
        let error = CliError::Core(Box::new(CoreError::MetadataIdentityMismatch {
            kind: "chunk index",
            object_key,
            expected: "abcdef".to_owned(),
            actual: "123456".to_owned(),
        }));
        let output = render_error_output(OutputMode::Json, "check", &error, 6)
            .expect("render check failure");
        let failure: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("failure json");

        assert_eq!(output.stderr, "");
        assert_eq!(output.exit_code, 6);
        assert_eq!(
            failure["data"]["code"],
            "repository_metadata_identity_mismatch"
        );
        assert_eq!(failure["data"]["object_key"], "objects/index/ab/abcdef");
        assert_eq!(
            failure["data"]["finding"]["object_key"],
            "objects/index/ab/abcdef"
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
    fn s3_repository_urls_are_parsed_without_leaking_credentials() {
        let parsed = parse_s3_repository_url("s3://company-backups/laptops")
            .expect("valid s3 repository url");

        assert_eq!(parsed.bucket, "company-backups");
        assert_eq!(parsed.root_prefix.as_str(), "laptops");
        assert_eq!(
            redact_for_display("s3://access:secret@example.com/bucket?token=sensitive"),
            "s3://<redacted>"
        );

        let error =
            parse_s3_repository_url("s3://access:secret@example.com/bucket?token=sensitive")
                .expect_err("credentials and query are rejected");
        let rendered = error.to_string();
        assert!(rendered.contains("s3://<redacted>"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("sensitive"));
    }

    #[test]
    fn s3_repository_config_uses_required_environment() {
        let environment = S3RepositoryEnvironment {
            endpoint: Some("https://s3.us-west-001.backblazeb2.com".to_owned()),
            region: Some("us-west-001".to_owned()),
            access_key_id: Some("application-key-id".to_owned()),
            secret_access_key: Some("application-key".to_owned()),
            disable_conditional_create: true,
        };

        let config = s3_repository_config("s3://dunamismax-b2/fileferry/dev", &environment)
            .expect("s3 config");
        let debug = format!("{config:?}");

        assert_eq!(config.bucket(), "dunamismax-b2");
        assert_eq!(config.region(), "us-west-001");
        assert_eq!(config.endpoint(), "https://s3.us-west-001.backblazeb2.com");
        assert_eq!(config.root_prefix().as_str(), "fileferry/dev");
        assert!(!debug.contains("application-key-id"));
        assert!(!debug.contains("application-key"));

        let error = s3_repository_config(
            "s3://dunamismax-b2/fileferry/dev",
            &S3RepositoryEnvironment::default(),
        )
        .expect_err("missing env");
        assert!(matches!(
            error,
            RepositoryError::MissingS3Environment {
                name: "FILEFERRY_S3_ENDPOINT"
            }
        ));
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
