//! Core repository, snapshot, backup, restore, and check orchestration.

use fastcdc::v2020::{
    AVERAGE_MAX, AVERAGE_MIN, FastCDC, MAXIMUM_MAX, MAXIMUM_MIN, MINIMUM_MAX, MINIMUM_MIN,
};
use fileferry_crypto::{
    AeadAlgorithm, CryptoError, EncryptedObject, KdfAlgorithm, KdfParams, KeyPurpose, KeySlot,
    MasterKey, ObjectContext, ObjectKind, create_master_key, decrypt_object, encrypt_object,
    keyed_content_id, random_bytes, unlock_master_key,
};
use fileferry_platform::{
    EntryKind, EntryMetadata, MetadataValue, PlatformError, Timestamp, capture_metadata,
};
use fileferry_storage::{ObjectKey, ObjectKeyPrefix, ObjectStore, PutStatus, StorageError};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs, io,
    path::Component,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("source root {path} is not absolute")]
    SourceRootNotAbsolute { path: PathBuf },

    #[error("source root {path} could not be read")]
    SourceRootRead {
        path: PathBuf,
        #[source]
        source: PlatformError,
    },

    #[error("directory {path} could not be read")]
    DirectoryRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("directory entry in {path} could not be read")]
    DirectoryEntryRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("metadata for {path} could not be captured")]
    MetadataCapture {
        path: PathBuf,
        #[source]
        source: PlatformError,
    },

    #[error("chunking configuration is invalid: {reason}")]
    InvalidChunkingConfig { reason: &'static str },

    #[error("backup pipeline configuration is invalid: {reason}")]
    InvalidBackupPipelineConfig { reason: &'static str },

    #[error("repository check data subset is invalid: {reason}")]
    InvalidCheckDataSubset { reason: &'static str },

    #[error("file {path} could not be read")]
    FileRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("chunk range for {path} is invalid")]
    InvalidChunkRange { path: PathBuf },

    #[error("chunk for {path} could not be compressed")]
    Compression {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("chunk {chunk_id} could not be decompressed")]
    Decompression {
        chunk_id: String,
        snapshot_id: Option<String>,
        path: Option<PathBuf>,
        object_key: Option<String>,
        #[source]
        source: io::Error,
    },

    #[error("chunk {chunk_id} has an invalid length")]
    InvalidChunkLength {
        chunk_id: String,
        snapshot_id: Option<String>,
        path: Option<PathBuf>,
        object_key: Option<String>,
    },

    #[error(
        "chunk {chunk_id} referenced by {path} in snapshot {snapshot_id} is missing from the loaded indexes"
    )]
    MissingChunkIndexEntry {
        snapshot_id: String,
        path: PathBuf,
        chunk_id: String,
        object_key: String,
    },

    #[error(
        "chunk {chunk_id} referenced by {path} in snapshot {snapshot_id} does not match the loaded index: {reason}"
    )]
    ChunkIndexMismatch {
        snapshot_id: String,
        path: PathBuf,
        chunk_id: String,
        object_key: String,
        reason: &'static str,
    },

    #[error("restored chunk identity mismatch: expected {expected}, found {actual}")]
    ChunkIdentityMismatch {
        expected: String,
        actual: String,
        snapshot_id: Option<String>,
        path: Option<PathBuf>,
        object_key: Option<String>,
    },

    #[error("repository object could not be encrypted")]
    Encryption {
        #[source]
        source: CryptoError,
    },

    #[error("repository metadata could not be serialized")]
    Serialization {
        #[source]
        source: serde_json::Error,
    },

    #[error("repository object {key} framing could not be decoded")]
    ObjectDecode {
        key: ObjectKey,
        #[source]
        source: serde_json::Error,
    },

    #[error("repository object {key} failed authentication")]
    ObjectAuthentication {
        key: ObjectKey,
        #[source]
        source: CryptoError,
    },

    #[error("repository metadata object {key} could not be decoded")]
    MetadataDecode {
        key: ObjectKey,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "{kind} metadata object {object_key} identity mismatch: expected {expected}, found {actual}"
    )]
    MetadataIdentityMismatch {
        kind: &'static str,
        object_key: ObjectKey,
        expected: String,
        actual: String,
    },

    #[error("snapshot commit marker {key} could not be decoded")]
    CommitDecode {
        key: ObjectKey,
        #[source]
        source: serde_json::Error,
    },

    #[error("snapshot commit marker {key} is invalid: {reason}")]
    InvalidCommitMarker {
        key: ObjectKey,
        reason: &'static str,
    },

    #[error("repository bootstrap could not be decoded")]
    RepositoryBootstrapDecode {
        #[source]
        source: serde_json::Error,
    },

    #[error("repository is not initialized; run ferry init first")]
    RepositoryNotInitialized,

    #[error("repository bootstrap is invalid: {reason}")]
    InvalidRepositoryBootstrap { reason: &'static str },

    #[error("repository format version {format_version} is not supported")]
    UnsupportedRepositoryFormat { format_version: u16 },

    #[error("repository uses unsupported features")]
    UnsupportedRepositoryFeatures,

    #[error("repository could not be unlocked")]
    RepositoryUnlock {
        #[source]
        source: CryptoError,
    },

    #[error("snapshot selection {selection} did not match any loaded snapshot")]
    SnapshotNotFound { selection: String },

    #[error("forget policy did not select any snapshots to forget")]
    ForgetNoSnapshotsMatched,

    #[error("snapshot forget marker {key} could not be decoded")]
    ForgetMarkerDecode {
        key: ObjectKey,
        #[source]
        source: serde_json::Error,
    },

    #[error("snapshot forget marker {key} is invalid: {reason}")]
    InvalidForgetMarker {
        key: ObjectKey,
        reason: &'static str,
    },

    #[error("snapshot path {path} was not found in snapshot {snapshot_id}")]
    SnapshotPathNotFound { snapshot_id: String, path: PathBuf },

    #[error("snapshot manifest {object_key} for snapshot {snapshot_id} is invalid: {reason}")]
    InvalidSnapshotManifest {
        snapshot_id: String,
        object_key: ObjectKey,
        path: Option<PathBuf>,
        reason: &'static str,
    },

    #[error("restore request is invalid: {reason}")]
    InvalidRestoreRequest { reason: &'static str },

    #[error("restore destination {path} is not absolute")]
    RestoreDestinationNotAbsolute { path: PathBuf },

    #[error("restore destination {path} escapes the requested destination root")]
    RestoreDestinationEscapesRoot { path: PathBuf },

    #[error("restore destination {path} contains a symlink at {symlink}")]
    RestoreDestinationSymlink { path: PathBuf, symlink: PathBuf },

    #[error("restore destination {path} already exists")]
    RestoreDestinationExists { path: PathBuf },

    #[error("restore destination {path} has the wrong entry kind")]
    RestoreDestinationKind { path: PathBuf },

    #[error("restore destination {path} could not be written")]
    RestoreDestinationWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("restore destination {path} could not be read for verification")]
    RestoreVerificationRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("restore verification failed for {path}")]
    RestoreVerificationMismatch { path: PathBuf },

    #[error("restore feature {feature} is not supported on this platform yet")]
    UnsupportedRestoreFeature { feature: &'static str },

    #[error("repository check failed: object {key} referenced by repository metadata is missing")]
    RepositoryCheckMissingObject { key: ObjectKey },

    #[error("repository object {key} referenced by repository metadata is missing")]
    RepositoryReferencedObjectMissing { key: ObjectKey },

    #[error("system clock is before the Unix epoch")]
    SystemClock {
        #[source]
        source: SystemTimeError,
    },

    #[error("repository object key could not be created")]
    ObjectKey {
        #[source]
        source: StorageError,
    },

    #[error("repository object write failed")]
    Storage {
        #[source]
        source: StorageError,
    },
}

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SourceEntry {
    pub root: PathBuf,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub metadata: EntryMetadata,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceWalker {
    exclusion_rules: Vec<ExclusionRule>,
}

impl SourceWalker {
    pub fn new(exclusion_rules: Vec<ExclusionRule>) -> Self {
        Self { exclusion_rules }
    }

    pub fn walk(&self, roots: &[PathBuf]) -> CoreResult<Vec<SourceEntry>> {
        let mut entries = Vec::new();

        for root in roots {
            self.walk_root(root, &mut entries)?;
        }

        Ok(entries)
    }

    fn walk_root(&self, root: &Path, entries: &mut Vec<SourceEntry>) -> CoreResult<()> {
        if !root.is_absolute() {
            return Err(CoreError::SourceRootNotAbsolute {
                path: root.to_path_buf(),
            });
        }

        let root_metadata = capture_metadata(root).map_err(|source| CoreError::SourceRootRead {
            path: root.to_path_buf(),
            source,
        })?;
        let root = root.to_path_buf();
        entries.push(SourceEntry {
            root: root.clone(),
            path: root.clone(),
            relative_path: PathBuf::new(),
            metadata: root_metadata.clone(),
        });

        if root_metadata.kind != EntryKind::Directory {
            return Ok(());
        }

        let mut pending = VecDeque::from([root.clone()]);
        while let Some(directory) = pending.pop_front() {
            let mut children = read_sorted_children(&directory)?;

            for child in children.drain(..) {
                let relative_path = child
                    .strip_prefix(&root)
                    .expect("walked children must stay under root")
                    .to_path_buf();
                if self.is_excluded(&relative_path) {
                    continue;
                }

                let metadata =
                    capture_metadata(&child).map_err(|source| CoreError::MetadataCapture {
                        path: child.clone(),
                        source,
                    })?;
                if metadata.kind == EntryKind::Directory {
                    pending.push_back(child.clone());
                }

                entries.push(SourceEntry {
                    root: root.clone(),
                    path: child,
                    relative_path,
                    metadata,
                });
            }
        }

        Ok(())
    }

    fn is_excluded(&self, relative_path: &Path) -> bool {
        self.exclusion_rules
            .iter()
            .any(|rule| rule.matches(relative_path))
    }
}

pub const DEFAULT_MIN_CHUNK_SIZE: usize = 512 * 1024;
pub const DEFAULT_AVG_CHUNK_SIZE: usize = 1024 * 1024;
pub const DEFAULT_MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChunkingConfig {
    pub min_size: usize,
    pub avg_size: usize,
    pub max_size: usize,
}

impl ChunkingConfig {
    pub const fn new(min_size: usize, avg_size: usize, max_size: usize) -> Self {
        Self {
            min_size,
            avg_size,
            max_size,
        }
    }

    pub fn validate(self) -> CoreResult<()> {
        if self.min_size < MINIMUM_MIN {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "minimum chunk size is below the FastCDC lower bound",
            });
        }
        if self.min_size > MINIMUM_MAX {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "minimum chunk size is above the FastCDC upper bound",
            });
        }
        if self.avg_size < AVERAGE_MIN {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "average chunk size is below the FastCDC lower bound",
            });
        }
        if self.avg_size > AVERAGE_MAX {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "average chunk size is above the FastCDC upper bound",
            });
        }
        if self.max_size < MAXIMUM_MIN {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "maximum chunk size is below the FastCDC lower bound",
            });
        }
        if self.max_size > MAXIMUM_MAX {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "maximum chunk size is above the FastCDC upper bound",
            });
        }
        if self.min_size > self.avg_size {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "minimum chunk size must be less than or equal to average chunk size",
            });
        }
        if self.avg_size > self.max_size {
            return Err(CoreError::InvalidChunkingConfig {
                reason: "average chunk size must be less than or equal to maximum chunk size",
            });
        }

        Ok(())
    }
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self::new(
            DEFAULT_MIN_CHUNK_SIZE,
            DEFAULT_AVG_CHUNK_SIZE,
            DEFAULT_MAX_CHUNK_SIZE,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ContentChunk {
    pub offset: u64,
    pub length: u64,
    pub gear_hash: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentChunker {
    config: ChunkingConfig,
}

impl ContentChunker {
    pub fn new(config: ChunkingConfig) -> CoreResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> ChunkingConfig {
        self.config
    }

    pub fn chunk_bytes(&self, bytes: &[u8]) -> Vec<ContentChunk> {
        FastCDC::new(
            bytes,
            self.config.min_size,
            self.config.avg_size,
            self.config.max_size,
        )
        .map(|chunk| ContentChunk {
            offset: chunk.offset as u64,
            length: chunk.length as u64,
            gear_hash: chunk.hash,
        })
        .collect()
    }
}

pub const DEFAULT_ZSTD_COMPRESSION_LEVEL: i32 = 3;
pub const REPOSITORY_MAGIC: &str = "fileferry";
pub const REPOSITORY_FORMAT_VERSION_V0: u16 = fileferry_crypto::FORMAT_VERSION_V0;
const REPOSITORY_ID_BYTES: usize = 32;

#[derive(Debug)]
pub struct OpenedRepository {
    pub repository_id: String,
    pub master_key: MasterKey,
}

#[derive(Debug)]
pub struct RepositoryInitResult {
    pub repository: OpenedRepository,
    pub format_version: u16,
    pub key_slots: usize,
    pub created: bool,
}

pub async fn create_repository(
    store: &dyn ObjectStore,
    passphrase: &SecretString,
    kdf: KdfParams,
) -> CoreResult<RepositoryInitResult> {
    if store
        .exists(&bootstrap_object_key()?)
        .await
        .map_err(|source| CoreError::Storage { source })?
    {
        let opened = open_repository(store, passphrase).await?;
        return Ok(RepositoryInitResult {
            repository: opened,
            format_version: REPOSITORY_FORMAT_VERSION_V0,
            key_slots: 1,
            created: false,
        });
    }

    let repository_id = hex_bytes(&random_bytes::<REPOSITORY_ID_BYTES>());
    let (master_key, key_slot) =
        create_master_key(passphrase, kdf).map_err(|source| CoreError::Encryption { source })?;
    let bootstrap = RepositoryBootstrap {
        magic: REPOSITORY_MAGIC.to_owned(),
        format_version: REPOSITORY_FORMAT_VERSION_V0,
        repository_id: repository_id.clone(),
        key_slots: vec![StoredKeySlot::from_key_slot(&key_slot)],
        features: Vec::new(),
    };
    let bytes = serde_json::to_vec_pretty(&bootstrap)
        .map_err(|source| CoreError::Serialization { source })?;
    let key = bootstrap_object_key()?;
    let created = match store
        .put_if_absent(&key, &bytes)
        .await
        .map_err(|source| CoreError::Storage { source })?
    {
        PutStatus::Created => true,
        PutStatus::AlreadyPresent => false,
    };

    if created {
        Ok(RepositoryInitResult {
            repository: OpenedRepository {
                repository_id,
                master_key,
            },
            format_version: REPOSITORY_FORMAT_VERSION_V0,
            key_slots: 1,
            created: true,
        })
    } else {
        let opened = open_repository(store, passphrase).await?;
        Ok(RepositoryInitResult {
            repository: opened,
            format_version: REPOSITORY_FORMAT_VERSION_V0,
            key_slots: 1,
            created: false,
        })
    }
}

pub async fn open_repository(
    store: &dyn ObjectStore,
    passphrase: &SecretString,
) -> CoreResult<OpenedRepository> {
    let key = bootstrap_object_key()?;
    let bytes = store.get(&key).await.map_err(|source| match source {
        StorageError::ObjectNotFound { .. } => CoreError::RepositoryNotInitialized,
        source => CoreError::Storage { source },
    })?;
    let bootstrap: RepositoryBootstrap = serde_json::from_slice(&bytes)
        .map_err(|source| CoreError::RepositoryBootstrapDecode { source })?;
    bootstrap.validate()?;

    for stored_slot in &bootstrap.key_slots {
        let key_slot = stored_slot.to_key_slot()?;
        match unlock_master_key(passphrase, &key_slot) {
            Ok(master_key) => {
                return Ok(OpenedRepository {
                    repository_id: bootstrap.repository_id,
                    master_key,
                });
            }
            Err(CryptoError::Decryption) => {}
            Err(source) => return Err(CoreError::RepositoryUnlock { source }),
        }
    }

    Err(CoreError::RepositoryUnlock {
        source: CryptoError::Decryption,
    })
}

fn bootstrap_object_key() -> CoreResult<ObjectKey> {
    ObjectKey::new("bootstrap").map_err(|source| CoreError::ObjectKey { source })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RepositoryBootstrap {
    magic: String,
    format_version: u16,
    repository_id: String,
    key_slots: Vec<StoredKeySlot>,
    features: Vec<String>,
}

impl RepositoryBootstrap {
    fn validate(&self) -> CoreResult<()> {
        if self.magic != REPOSITORY_MAGIC {
            return Err(CoreError::InvalidRepositoryBootstrap {
                reason: "repository magic is not recognized",
            });
        }
        if self.format_version != REPOSITORY_FORMAT_VERSION_V0 {
            return Err(CoreError::UnsupportedRepositoryFormat {
                format_version: self.format_version,
            });
        }
        if self.repository_id.len() != REPOSITORY_ID_BYTES * 2
            || !self
                .repository_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CoreError::InvalidRepositoryBootstrap {
                reason: "repository id is invalid",
            });
        }
        if self.key_slots.is_empty() {
            return Err(CoreError::InvalidRepositoryBootstrap {
                reason: "repository has no key slots",
            });
        }
        if !self.features.is_empty() {
            return Err(CoreError::UnsupportedRepositoryFeatures);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredKeySlot {
    kdf: StoredKdfParams,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    wrapped_master_key: Vec<u8>,
}

impl StoredKeySlot {
    fn from_key_slot(key_slot: &KeySlot) -> Self {
        Self {
            kdf: StoredKdfParams::from_kdf_params(key_slot.kdf),
            salt: key_slot.salt.to_vec(),
            nonce: key_slot.nonce.to_vec(),
            wrapped_master_key: key_slot.wrapped_master_key.clone(),
        }
    }

    fn to_key_slot(&self) -> CoreResult<KeySlot> {
        let salt = fixed_bytes::<{ fileferry_crypto::KDF_SALT_LEN }>(
            &self.salt,
            "key slot salt has an invalid length",
        )?;
        let nonce = fixed_bytes::<{ fileferry_crypto::XCHACHA20_POLY1305_NONCE_LEN }>(
            &self.nonce,
            "key slot nonce has an invalid length",
        )?;

        Ok(KeySlot {
            kdf: self.kdf.to_kdf_params()?,
            salt,
            nonce,
            wrapped_master_key: self.wrapped_master_key.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct StoredKdfParams {
    algorithm: StoredKdfAlgorithm,
    memory_cost_kib: u32,
    time_cost: u32,
    parallelism: u32,
}

impl StoredKdfParams {
    fn from_kdf_params(params: KdfParams) -> Self {
        Self {
            algorithm: match params.algorithm {
                KdfAlgorithm::Argon2idV19 => StoredKdfAlgorithm::Argon2idV19,
            },
            memory_cost_kib: params.memory_cost_kib,
            time_cost: params.time_cost,
            parallelism: params.parallelism,
        }
    }

    fn to_kdf_params(self) -> CoreResult<KdfParams> {
        Ok(KdfParams {
            algorithm: match self.algorithm {
                StoredKdfAlgorithm::Argon2idV19 => KdfAlgorithm::Argon2idV19,
            },
            memory_cost_kib: self.memory_cost_kib,
            time_cost: self.time_cost,
            parallelism: self.parallelism,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredKdfAlgorithm {
    Argon2idV19,
}

fn fixed_bytes<const N: usize>(bytes: &[u8], reason: &'static str) -> CoreResult<[u8; N]> {
    bytes
        .try_into()
        .map_err(|_| CoreError::InvalidRepositoryBootstrap { reason })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct BackupPipelineConfig {
    pub chunking: ChunkingConfig,
    pub compression_level: i32,
    pub repository_id: String,
}

impl BackupPipelineConfig {
    pub fn new(repository_id: impl Into<String>) -> Self {
        Self {
            chunking: ChunkingConfig::default(),
            compression_level: DEFAULT_ZSTD_COMPRESSION_LEVEL,
            repository_id: repository_id.into(),
        }
    }

    pub fn validate(&self) -> CoreResult<()> {
        self.chunking.validate()?;
        if self.repository_id.is_empty() {
            return Err(CoreError::InvalidBackupPipelineConfig {
                reason: "repository id must not be empty",
            });
        }
        if self.repository_id.as_bytes().contains(&0) {
            return Err(CoreError::InvalidBackupPipelineConfig {
                reason: "repository id must not contain NUL",
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupRequest {
    pub roots: Vec<PathBuf>,
    pub exclusion_rules: Vec<ExclusionRule>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BackupPipeline {
    config: BackupPipelineConfig,
    chunker: ContentChunker,
}

impl BackupPipeline {
    pub fn new(config: BackupPipelineConfig) -> CoreResult<Self> {
        config.validate()?;
        let chunker = ContentChunker::new(config.chunking)?;
        Ok(Self { config, chunker })
    }

    pub fn config(&self) -> &BackupPipelineConfig {
        &self.config
    }

    pub async fn write_snapshot(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        request: BackupRequest,
    ) -> CoreResult<SnapshotWriteResult> {
        let entries = SourceWalker::new(request.exclusion_rules)
            .walk(&request.roots)?
            .into_iter()
            .map(ManifestEntry::from_source_entry)
            .collect::<Vec<_>>();

        let repository_context = self.config.repository_id.as_bytes();
        let chunk_key = master_key
            .derive_subkey(KeyPurpose::ChunkData, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let index_key = master_key
            .derive_subkey(KeyPurpose::Index, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let manifest_key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;

        let mut manifest_entries = Vec::with_capacity(entries.len());
        let mut index_entries = Vec::new();
        let mut chunk_objects_written = 0_usize;
        let mut chunk_objects_reused = 0_usize;
        let mut chunks_seen = 0_usize;
        let mut bytes_scanned = 0_u64;
        let mut bytes_uploaded = 0_u64;
        let mut files_backed_up = 0_usize;
        let mut directories_backed_up = 0_usize;
        let mut symlinks_backed_up = 0_usize;
        let mut special_entries_seen = 0_usize;

        for mut entry in entries {
            match entry.metadata.kind {
                EntryKind::RegularFile => {
                    files_backed_up += 1;
                }
                EntryKind::Directory => {
                    directories_backed_up += 1;
                }
                EntryKind::Symlink => {
                    symlinks_backed_up += 1;
                }
                EntryKind::Other => {
                    special_entries_seen += 1;
                }
            }

            if entry.metadata.kind == EntryKind::RegularFile {
                let file_bytes = fs::read(&entry.path).map_err(|source| CoreError::FileRead {
                    path: entry.path.clone(),
                    source,
                })?;
                bytes_scanned += file_bytes.len() as u64;
                for chunk in self.chunker.chunk_bytes(&file_bytes) {
                    chunks_seen += 1;
                    let start = usize::try_from(chunk.offset).map_err(|_| {
                        CoreError::InvalidChunkRange {
                            path: entry.path.clone(),
                        }
                    })?;
                    let length = usize::try_from(chunk.length).map_err(|_| {
                        CoreError::InvalidChunkRange {
                            path: entry.path.clone(),
                        }
                    })?;
                    let end = start
                        .checked_add(length)
                        .filter(|end| *end <= file_bytes.len())
                        .ok_or_else(|| CoreError::InvalidChunkRange {
                            path: entry.path.clone(),
                        })?;
                    let plaintext = &file_bytes[start..end];
                    let chunk_id = hex_bytes(
                        &keyed_content_id(
                            master_key,
                            KeyPurpose::ChunkIdentity,
                            repository_context,
                            plaintext,
                        )
                        .map_err(|source| CoreError::Encryption { source })?,
                    );
                    let object_key = object_key_for_id("objects/chunk", &chunk_id)?;
                    let compressed = zstd::bulk::compress(plaintext, self.config.compression_level)
                        .map_err(|source| CoreError::Compression {
                            path: entry.path.clone(),
                            source,
                        })?;
                    let encrypted = encrypt_repository_object(
                        &chunk_key,
                        ObjectKind::Chunk,
                        &object_key,
                        &compressed,
                    )?;

                    if !store
                        .exists(&object_key)
                        .await
                        .map_err(|source| CoreError::Storage { source })?
                    {
                        match store
                            .put_if_absent(&object_key, &encrypted)
                            .await
                            .map_err(|source| CoreError::Storage { source })?
                        {
                            PutStatus::Created => {
                                chunk_objects_written += 1;
                                bytes_uploaded += encrypted.len() as u64;
                            }
                            PutStatus::AlreadyPresent => {
                                chunk_objects_reused += 1;
                            }
                        }
                    } else {
                        chunk_objects_reused += 1;
                    }

                    let chunk_ref = ManifestChunkRef {
                        chunk_id: chunk_id.clone(),
                        object_key: object_key.as_str().to_owned(),
                        offset: chunk.offset,
                        length: chunk.length,
                    };
                    entry.chunks.push(chunk_ref.clone());
                    index_entries.push(ChunkIndexEntry {
                        chunk_id,
                        object_key: object_key.as_str().to_owned(),
                        plaintext_length: chunk.length,
                        compressed_length: compressed.len() as u64,
                        stored_length: encrypted.len() as u64,
                        compression: CompressionAlgorithm::Zstd,
                        aead: RepositoryAeadAlgorithm::XChaCha20Poly1305,
                    });
                }
            }
            manifest_entries.push(entry);
        }

        index_entries.sort_by(|left, right| left.chunk_id.cmp(&right.chunk_id));
        index_entries.dedup_by(|left, right| left.chunk_id == right.chunk_id);

        let index_id = content_id_for_metadata(
            master_key,
            KeyPurpose::Index,
            repository_context,
            &index_entries,
        )?;
        let index = ChunkIndex {
            schema_version: 0,
            index_id: index_id.clone(),
            chunks: index_entries,
        };
        let index_object = object_key_for_id("objects/index", &index_id)?;
        bytes_uploaded += write_encrypted_json_object(
            store,
            &index_key,
            ObjectKind::Index,
            &index_object,
            &index,
        )
        .await?;

        let manifest_body = SnapshotManifestBody {
            created_at_unix_seconds: current_unix_seconds()?,
            tags: request.tags,
            entries: manifest_entries,
            index_ids: vec![index_id.clone()],
        };
        let snapshot_id = content_id_for_metadata(
            master_key,
            KeyPurpose::SnapshotMetadata,
            repository_context,
            &manifest_body,
        )?;
        let manifest = SnapshotManifest {
            schema_version: 0,
            snapshot_id: snapshot_id.clone(),
            body: manifest_body,
        };
        let manifest_object = object_key_for_id("objects/manifest", &snapshot_id)?;
        let manifest_bytes_written = write_encrypted_json_object(
            store,
            &manifest_key,
            ObjectKind::SnapshotManifest,
            &manifest_object,
            &manifest,
        )
        .await?;
        bytes_uploaded += manifest_bytes_written;

        let commit_object = object_key_for_commit(&snapshot_id)?;
        let commit = SnapshotCommit {
            schema_version: 0,
            snapshot_id: snapshot_id.clone(),
            manifest_object: manifest_object.as_str().to_owned(),
        };
        let commit_bytes =
            serde_json::to_vec(&commit).map_err(|source| CoreError::Serialization { source })?;
        let commit_status = store
            .put_if_absent(&commit_object, &commit_bytes)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        if commit_status == PutStatus::Created {
            bytes_uploaded += commit_bytes.len() as u64;
        }

        Ok(SnapshotWriteResult {
            snapshot_id,
            created_at_unix_seconds: manifest.body.created_at_unix_seconds,
            manifest_object,
            index_object,
            index_ids: manifest.body.index_ids,
            commit_object,
            chunk_objects_written,
            chunk_objects_reused,
            entries: manifest.body.entries.len(),
            entries_scanned: manifest.body.entries.len(),
            files_backed_up,
            directories_backed_up,
            symlinks_backed_up,
            special_entries_seen,
            bytes_scanned,
            bytes_uploaded,
            chunks_seen,
            chunks_written: chunk_objects_written,
            chunks_reused: chunk_objects_reused,
            chunks: index.chunks.len(),
        })
    }

    pub async fn read_snapshot_manifest(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        snapshot_id: &str,
    ) -> CoreResult<SnapshotManifest> {
        let repository_context = self.config.repository_id.as_bytes();
        let manifest_key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let object_key = object_key_for_id("objects/manifest", snapshot_id)?;
        let manifest: SnapshotManifest = read_encrypted_json_object(
            store,
            &manifest_key,
            ObjectKind::SnapshotManifest,
            &object_key,
        )
        .await?;
        let actual = content_id_for_metadata(
            master_key,
            KeyPurpose::SnapshotMetadata,
            repository_context,
            &manifest.body,
        )?;

        if manifest.snapshot_id != snapshot_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "snapshot manifest",
                object_key: object_key.clone(),
                expected: snapshot_id.to_owned(),
                actual: manifest.snapshot_id,
            });
        }
        if actual != snapshot_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "snapshot manifest",
                object_key: object_key.clone(),
                expected: snapshot_id.to_owned(),
                actual,
            });
        }
        validate_snapshot_manifest(&manifest, &object_key)?;

        Ok(manifest)
    }

    pub async fn read_committed_snapshot_manifests(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
    ) -> CoreResult<Vec<SnapshotManifest>> {
        let forgotten_snapshot_ids = self.read_forgotten_snapshot_ids(store).await?;
        let prefix =
            ObjectKeyPrefix::new("commits").map_err(|source| CoreError::ObjectKey { source })?;
        let mut commit_keys = store
            .list_prefix(&prefix)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        commit_keys.sort();

        let mut manifests = Vec::with_capacity(commit_keys.len());
        for commit_key in commit_keys {
            let commit = self.read_snapshot_commit(store, &commit_key).await?;
            if forgotten_snapshot_ids.contains(&commit.snapshot_id) {
                continue;
            }
            let expected_commit_key = object_key_for_commit(&commit.snapshot_id)?;
            if expected_commit_key != commit_key {
                return Err(CoreError::InvalidCommitMarker {
                    key: commit_key,
                    reason: "commit object key does not match committed snapshot id",
                });
            }

            let expected_manifest_object =
                object_key_for_id("objects/manifest", &commit.snapshot_id)?;
            if commit.manifest_object != expected_manifest_object.as_str() {
                return Err(CoreError::InvalidCommitMarker {
                    key: expected_commit_key,
                    reason: "commit manifest object does not match committed snapshot id",
                });
            }

            manifests.push(
                self.read_snapshot_manifest(store, master_key, &commit.snapshot_id)
                    .await
                    .map_err(|error| {
                        referenced_object_read_error(error, expected_manifest_object)
                    })?,
            );
        }
        manifests.sort_by(compare_snapshot_manifests);

        Ok(manifests)
    }

    pub async fn read_forgotten_snapshot_ids(
        &self,
        store: &dyn ObjectStore,
    ) -> CoreResult<BTreeSet<String>> {
        let prefix =
            ObjectKeyPrefix::new("forgets").map_err(|source| CoreError::ObjectKey { source })?;
        let mut marker_keys = store
            .list_prefix(&prefix)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        marker_keys.sort();

        let mut snapshot_ids = BTreeSet::new();
        for marker_key in marker_keys {
            let marker = self.read_snapshot_forget_marker(store, &marker_key).await?;
            snapshot_ids.insert(marker.snapshot_id);
        }

        Ok(snapshot_ids)
    }

    pub async fn write_snapshot_forget_markers(
        &self,
        store: &dyn ObjectStore,
        snapshot_ids: &[String],
    ) -> CoreResult<SnapshotForgetWriteResult> {
        if snapshot_ids.is_empty() {
            return Err(CoreError::ForgetNoSnapshotsMatched);
        }

        let forgotten_at_unix_seconds = current_unix_seconds()?;
        let mut markers = Vec::with_capacity(snapshot_ids.len());
        let mut unique_snapshot_ids = snapshot_ids.to_vec();
        unique_snapshot_ids.sort();
        unique_snapshot_ids.dedup();

        for snapshot_id in unique_snapshot_ids {
            let marker_object = object_key_for_forget_marker(&snapshot_id)?;
            let marker = SnapshotForgetMarker {
                schema_version: 0,
                snapshot_id: snapshot_id.clone(),
                manifest_object: object_key_for_id("objects/manifest", &snapshot_id)?
                    .as_str()
                    .to_owned(),
                commit_object: object_key_for_commit(&snapshot_id)?.as_str().to_owned(),
                forgotten_at_unix_seconds,
            };
            let marker_bytes = serde_json::to_vec(&marker)
                .map_err(|source| CoreError::Serialization { source })?;
            let created = store
                .put_if_absent(&marker_object, &marker_bytes)
                .await
                .map_err(|source| CoreError::Storage { source })?
                == PutStatus::Created;

            markers.push(SnapshotForgetWrite {
                snapshot_id,
                marker_object: marker_object.as_str().to_owned(),
                created,
            });
        }

        Ok(SnapshotForgetWriteResult { markers })
    }

    async fn read_snapshot_commit(
        &self,
        store: &dyn ObjectStore,
        commit_key: &ObjectKey,
    ) -> CoreResult<SnapshotCommit> {
        let bytes = store
            .get(commit_key)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        let commit: SnapshotCommit =
            serde_json::from_slice(&bytes).map_err(|source| CoreError::CommitDecode {
                key: commit_key.clone(),
                source,
            })?;

        if commit.schema_version != 0 {
            return Err(CoreError::InvalidCommitMarker {
                key: commit_key.clone(),
                reason: "unsupported commit marker schema version",
            });
        }

        Ok(commit)
    }

    async fn read_snapshot_forget_marker(
        &self,
        store: &dyn ObjectStore,
        marker_key: &ObjectKey,
    ) -> CoreResult<SnapshotForgetMarker> {
        let bytes = store
            .get(marker_key)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        let marker: SnapshotForgetMarker =
            serde_json::from_slice(&bytes).map_err(|source| CoreError::ForgetMarkerDecode {
                key: marker_key.clone(),
                source,
            })?;

        if marker.schema_version != 0 {
            return Err(CoreError::InvalidForgetMarker {
                key: marker_key.clone(),
                reason: "unsupported forget marker schema version",
            });
        }

        let expected_marker_key = object_key_for_forget_marker(&marker.snapshot_id)?;
        if expected_marker_key != *marker_key {
            return Err(CoreError::InvalidForgetMarker {
                key: marker_key.clone(),
                reason: "forget marker object key does not match snapshot id",
            });
        }

        let expected_manifest_object = object_key_for_id("objects/manifest", &marker.snapshot_id)?;
        if marker.manifest_object != expected_manifest_object.as_str() {
            return Err(CoreError::InvalidForgetMarker {
                key: marker_key.clone(),
                reason: "forget marker manifest object does not match snapshot id",
            });
        }

        let expected_commit_object = object_key_for_commit(&marker.snapshot_id)?;
        if marker.commit_object != expected_commit_object.as_str() {
            return Err(CoreError::InvalidForgetMarker {
                key: marker_key.clone(),
                reason: "forget marker commit object does not match snapshot id",
            });
        }

        Ok(marker)
    }

    pub async fn read_chunk_index(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        index_id: &str,
    ) -> CoreResult<ChunkIndex> {
        let repository_context = self.config.repository_id.as_bytes();
        let index_key = master_key
            .derive_subkey(KeyPurpose::Index, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let object_key = object_key_for_id("objects/index", index_id)?;
        let index: ChunkIndex =
            read_encrypted_json_object(store, &index_key, ObjectKind::Index, &object_key).await?;
        let actual = content_id_for_metadata(
            master_key,
            KeyPurpose::Index,
            repository_context,
            &index.chunks,
        )?;

        if index.index_id != index_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                object_key: object_key.clone(),
                expected: index_id.to_owned(),
                actual: index.index_id,
            });
        }
        if actual != index_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                object_key: object_key.clone(),
                expected: index_id.to_owned(),
                actual,
            });
        }

        Ok(index)
    }

    pub async fn check_repository(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
    ) -> CoreResult<RepositoryCheckResult> {
        self.check_repository_with_options(store, master_key, CheckRepositoryOptions::full())
            .await
    }

    pub async fn check_repository_with_options(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        options: CheckRepositoryOptions,
    ) -> CoreResult<RepositoryCheckResult> {
        let repository_context = self.config.repository_id.as_bytes();
        let chunk_key = master_key
            .derive_subkey(KeyPurpose::ChunkData, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let prefix =
            ObjectKeyPrefix::new("commits").map_err(|source| CoreError::ObjectKey { source })?;
        let mut commit_keys = store
            .list_prefix(&prefix)
            .await
            .map_err(|source| CoreError::Storage { source })?;
        commit_keys.sort();

        let mut metadata_objects_checked = 0_usize;
        let mut chunk_objects_checked = 0_usize;
        let mut bytes_read = 0_u64;
        let mut chunk_targets = BTreeMap::new();

        for commit_key in commit_keys {
            let commit_bytes = store
                .get(&commit_key)
                .await
                .map_err(|source| CoreError::Storage { source })?;
            bytes_read += commit_bytes.len() as u64;
            metadata_objects_checked += 1;
            let commit: SnapshotCommit =
                serde_json::from_slice(&commit_bytes).map_err(|source| {
                    CoreError::CommitDecode {
                        key: commit_key.clone(),
                        source,
                    }
                })?;
            if commit.schema_version != 0 {
                return Err(CoreError::InvalidCommitMarker {
                    key: commit_key,
                    reason: "unsupported commit marker schema version",
                });
            }
            let expected_commit_key = object_key_for_commit(&commit.snapshot_id)?;
            if expected_commit_key != commit_key {
                return Err(CoreError::InvalidCommitMarker {
                    key: commit_key,
                    reason: "commit object key does not match committed snapshot id",
                });
            }
            let expected_manifest_object =
                object_key_for_id("objects/manifest", &commit.snapshot_id)?;
            if commit.manifest_object != expected_manifest_object.as_str() {
                return Err(CoreError::InvalidCommitMarker {
                    key: expected_commit_key,
                    reason: "commit manifest object does not match committed snapshot id",
                });
            }

            let (manifest, manifest_bytes) = self
                .read_snapshot_manifest_with_bytes(store, master_key, &commit.snapshot_id)
                .await
                .map_err(|error| check_read_error(error, expected_manifest_object.clone()))?;
            bytes_read += manifest_bytes;
            metadata_objects_checked += 1;

            let mut index_entries = BTreeMap::new();
            let mut chunk_contexts = BTreeMap::new();
            for index_id in &manifest.body.index_ids {
                let expected_index_object = object_key_for_id("objects/index", index_id)?;
                let (index, index_bytes) = self
                    .read_chunk_index_with_bytes(store, master_key, index_id)
                    .await
                    .map_err(|error| check_read_error(error, expected_index_object.clone()))?;
                bytes_read += index_bytes;
                metadata_objects_checked += 1;
                for entry in index.chunks {
                    index_entries.insert(entry.chunk_id.clone(), entry);
                }
            }

            for entry in &manifest.body.entries {
                if entry.metadata.kind != EntryKind::RegularFile {
                    continue;
                }
                for chunk in &entry.chunks {
                    let indexed = index_entries.get(&chunk.chunk_id).ok_or_else(|| {
                        CoreError::MissingChunkIndexEntry {
                            snapshot_id: manifest.snapshot_id.clone(),
                            path: entry.relative_path.clone(),
                            chunk_id: chunk.chunk_id.clone(),
                            object_key: chunk.object_key.clone(),
                        }
                    })?;
                    validate_indexed_chunk_reference(
                        &manifest.snapshot_id,
                        &entry.relative_path,
                        chunk,
                        indexed,
                    )?;
                    chunk_contexts
                        .entry(chunk.chunk_id.clone())
                        .or_insert_with(|| ChunkReferenceContext {
                            snapshot_id: manifest.snapshot_id.clone(),
                            path: entry.relative_path.clone(),
                            object_key: chunk.object_key.clone(),
                        });
                }
            }

            for (chunk_id, context) in chunk_contexts {
                let indexed = index_entries.get(&chunk_id).ok_or_else(|| {
                    CoreError::MissingChunkIndexEntry {
                        snapshot_id: context.snapshot_id.clone(),
                        path: context.path.clone(),
                        chunk_id: chunk_id.clone(),
                        object_key: context.object_key.clone(),
                    }
                })?;
                chunk_targets
                    .entry(chunk_id)
                    .or_insert_with(|| ChunkCheckTarget {
                        indexed: indexed.clone(),
                        reference_context: context,
                    });
            }
        }

        let selected_chunk_ids = select_check_chunk_ids(&chunk_targets, options.read_data);
        for chunk_id in selected_chunk_ids {
            let target = chunk_targets
                .get(&chunk_id)
                .expect("selected chunk id must come from chunk target map");
            let indexed = &target.indexed;
            let reference_context = &target.reference_context;
            let object_key = ObjectKey::new(indexed.object_key.clone())
                .map_err(|source| CoreError::ObjectKey { source })?;
            let encrypted = store
                .get(&object_key)
                .await
                .map_err(|source| match source {
                    StorageError::ObjectNotFound { .. } => {
                        CoreError::RepositoryCheckMissingObject {
                            key: object_key.clone(),
                        }
                    }
                    source => CoreError::Storage { source },
                })?;
            bytes_read += encrypted.len() as u64;
            let compressed =
                decrypt_repository_object(&chunk_key, ObjectKind::Chunk, &object_key, &encrypted)?;
            let expected_len = usize::try_from(indexed.plaintext_length).map_err(|_| {
                CoreError::InvalidChunkLength {
                    chunk_id: chunk_id.clone(),
                    snapshot_id: Some(reference_context.snapshot_id.clone()),
                    path: Some(reference_context.path.clone()),
                    object_key: Some(reference_context.object_key.clone()),
                }
            })?;
            let plaintext =
                zstd::bulk::decompress(&compressed, expected_len).map_err(|source| {
                    CoreError::Decompression {
                        chunk_id: chunk_id.clone(),
                        snapshot_id: Some(reference_context.snapshot_id.clone()),
                        path: Some(reference_context.path.clone()),
                        object_key: Some(reference_context.object_key.clone()),
                        source,
                    }
                })?;
            if plaintext.len() != expected_len {
                return Err(CoreError::InvalidChunkLength {
                    chunk_id,
                    snapshot_id: Some(reference_context.snapshot_id.clone()),
                    path: Some(reference_context.path.clone()),
                    object_key: Some(reference_context.object_key.clone()),
                });
            }
            let actual = hex_bytes(
                &keyed_content_id(
                    master_key,
                    KeyPurpose::ChunkIdentity,
                    repository_context,
                    &plaintext,
                )
                .map_err(|source| CoreError::Encryption { source })?,
            );
            if actual != chunk_id {
                return Err(CoreError::ChunkIdentityMismatch {
                    expected: chunk_id,
                    actual,
                    snapshot_id: Some(reference_context.snapshot_id.clone()),
                    path: Some(reference_context.path.clone()),
                    object_key: Some(reference_context.object_key.clone()),
                });
            }

            chunk_objects_checked += 1;
        }

        Ok(RepositoryCheckResult {
            repository_id: self.config.repository_id.clone(),
            checked_at_unix_seconds: current_unix_seconds()?,
            metadata_objects_checked,
            chunk_objects_checked,
            bytes_read,
            read_data_mode: options.read_data.mode(),
            read_data_subset: options.read_data.subset_label(),
            errors: Vec::new(),
            warnings: Vec::new(),
        })
    }

    async fn read_snapshot_manifest_with_bytes(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        snapshot_id: &str,
    ) -> CoreResult<(SnapshotManifest, u64)> {
        let repository_context = self.config.repository_id.as_bytes();
        let manifest_key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let object_key = object_key_for_id("objects/manifest", snapshot_id)?;
        let (manifest, bytes_read): (SnapshotManifest, u64) =
            read_encrypted_json_object_with_bytes(
                store,
                &manifest_key,
                ObjectKind::SnapshotManifest,
                &object_key,
            )
            .await?;
        let actual = content_id_for_metadata(
            master_key,
            KeyPurpose::SnapshotMetadata,
            repository_context,
            &manifest.body,
        )?;

        if manifest.snapshot_id != snapshot_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "snapshot manifest",
                object_key: object_key.clone(),
                expected: snapshot_id.to_owned(),
                actual: manifest.snapshot_id,
            });
        }
        if actual != snapshot_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "snapshot manifest",
                object_key: object_key.clone(),
                expected: snapshot_id.to_owned(),
                actual,
            });
        }
        validate_snapshot_manifest(&manifest, &object_key)?;

        Ok((manifest, bytes_read))
    }

    async fn read_chunk_index_with_bytes(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        index_id: &str,
    ) -> CoreResult<(ChunkIndex, u64)> {
        let repository_context = self.config.repository_id.as_bytes();
        let index_key = master_key
            .derive_subkey(KeyPurpose::Index, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let object_key = object_key_for_id("objects/index", index_id)?;
        let (index, bytes_read): (ChunkIndex, u64) = read_encrypted_json_object_with_bytes(
            store,
            &index_key,
            ObjectKind::Index,
            &object_key,
        )
        .await?;
        let actual = content_id_for_metadata(
            master_key,
            KeyPurpose::Index,
            repository_context,
            &index.chunks,
        )?;

        if index.index_id != index_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                object_key: object_key.clone(),
                expected: index_id.to_owned(),
                actual: index.index_id,
            });
        }
        if actual != index_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                object_key: object_key.clone(),
                expected: index_id.to_owned(),
                actual,
            });
        }

        Ok((index, bytes_read))
    }

    pub async fn restore_snapshot_contents(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        request: RestoreContentRequest,
    ) -> CoreResult<RestoreContentResult> {
        let restore_paths = normalize_restore_paths(&request.paths)?;
        let expected_manifest_object = object_key_for_id("objects/manifest", &request.snapshot_id)?;
        let manifest = self
            .read_snapshot_manifest(store, master_key, &request.snapshot_id)
            .await
            .map_err(|error| referenced_object_read_error(error, expected_manifest_object))?;
        let scoped_entries = scoped_manifest_entries(&manifest, &restore_paths);
        ensure_restore_paths_exist(&manifest, &restore_paths)?;
        let selected_entries = scoped_entries.len();
        let chunk_index = self
            .load_chunk_index_entries(store, master_key, &manifest)
            .await?;
        let repository_context = self.config.repository_id.as_bytes();
        let chunk_key = master_key
            .derive_subkey(KeyPurpose::ChunkData, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let mut directories = Vec::new();
        let mut files = Vec::new();
        let mut symlinks = Vec::new();
        let mut metadata_warnings = Vec::new();

        for entry in scoped_entries {
            match entry.metadata.kind {
                EntryKind::Directory => {
                    directories.push(RestoredDirectory {
                        relative_path: entry.relative_path.clone(),
                        modified: entry.metadata.modified.clone(),
                    });
                    continue;
                }
                EntryKind::Symlink => {
                    match &entry.metadata.symlink_target {
                        MetadataValue::Captured(target) => {
                            symlinks.push(RestoredSymlink {
                                relative_path: entry.relative_path.clone(),
                                target: target.clone(),
                            });
                        }
                        MetadataValue::Unsupported => {
                            metadata_warnings.push(RestoreMetadataWarning {
                                relative_path: entry.relative_path.clone(),
                                field: "symlink_target",
                                reason: "symlink target was not captured".to_owned(),
                            });
                        }
                        MetadataValue::Denied(reason) => {
                            metadata_warnings.push(RestoreMetadataWarning {
                                relative_path: entry.relative_path.clone(),
                                field: "symlink_target",
                                reason: format!("symlink target was denied: {reason}"),
                            });
                        }
                    }
                    continue;
                }
                EntryKind::RegularFile => {}
                EntryKind::Other => continue,
            }

            let mut contents = Vec::new();
            for chunk in &entry.chunks {
                let indexed = chunk_index.get(&chunk.chunk_id).ok_or_else(|| {
                    CoreError::MissingChunkIndexEntry {
                        snapshot_id: manifest.snapshot_id.clone(),
                        path: entry.relative_path.clone(),
                        chunk_id: chunk.chunk_id.clone(),
                        object_key: chunk.object_key.clone(),
                    }
                })?;
                validate_indexed_chunk_reference(
                    &manifest.snapshot_id,
                    &entry.relative_path,
                    chunk,
                    indexed,
                )?;

                let object_key = ObjectKey::new(chunk.object_key.clone())
                    .map_err(|source| CoreError::ObjectKey { source })?;
                let encrypted = store
                    .get(&object_key)
                    .await
                    .map_err(|source| match source {
                        StorageError::ObjectNotFound { .. } => {
                            CoreError::RepositoryReferencedObjectMissing {
                                key: object_key.clone(),
                            }
                        }
                        source => CoreError::Storage { source },
                    })?;
                let compressed = decrypt_repository_object(
                    &chunk_key,
                    ObjectKind::Chunk,
                    &object_key,
                    &encrypted,
                )?;
                let expected_len = usize::try_from(indexed.plaintext_length).map_err(|_| {
                    CoreError::InvalidChunkLength {
                        chunk_id: chunk.chunk_id.clone(),
                        snapshot_id: Some(manifest.snapshot_id.clone()),
                        path: Some(entry.relative_path.clone()),
                        object_key: Some(indexed.object_key.clone()),
                    }
                })?;
                let plaintext =
                    zstd::bulk::decompress(&compressed, expected_len).map_err(|source| {
                        CoreError::Decompression {
                            chunk_id: chunk.chunk_id.clone(),
                            snapshot_id: Some(manifest.snapshot_id.clone()),
                            path: Some(entry.relative_path.clone()),
                            object_key: Some(indexed.object_key.clone()),
                            source,
                        }
                    })?;
                if plaintext.len() != expected_len {
                    return Err(CoreError::InvalidChunkLength {
                        chunk_id: chunk.chunk_id.clone(),
                        snapshot_id: Some(manifest.snapshot_id.clone()),
                        path: Some(entry.relative_path.clone()),
                        object_key: Some(indexed.object_key.clone()),
                    });
                }
                let actual = hex_bytes(
                    &keyed_content_id(
                        master_key,
                        KeyPurpose::ChunkIdentity,
                        repository_context,
                        &plaintext,
                    )
                    .map_err(|source| CoreError::Encryption { source })?,
                );
                if actual != chunk.chunk_id {
                    return Err(CoreError::ChunkIdentityMismatch {
                        expected: chunk.chunk_id.clone(),
                        actual,
                        snapshot_id: Some(manifest.snapshot_id.clone()),
                        path: Some(entry.relative_path.clone()),
                        object_key: Some(indexed.object_key.clone()),
                    });
                }

                contents.extend_from_slice(&plaintext);
            }

            files.push(RestoredFile {
                relative_path: entry.relative_path.clone(),
                contents,
                modified: entry.metadata.modified.clone(),
            });
        }

        Ok(RestoreContentResult {
            snapshot_id: manifest.snapshot_id,
            selected_entries,
            directories,
            files,
            symlinks,
            metadata_warnings,
        })
    }

    pub async fn restore_snapshot_to_destination(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        request: RestoreDestinationRequest,
    ) -> CoreResult<RestoreDestinationResult> {
        validate_restore_destination_root(&request.destination)?;

        let contents = self
            .restore_snapshot_contents(
                store,
                master_key,
                RestoreContentRequest {
                    snapshot_id: request.snapshot_id,
                    paths: request.paths,
                },
            )
            .await?;
        let file_count = contents.files.len();
        let mut prepared_directories = Vec::with_capacity(contents.directories.len());
        let mut prepared_files = Vec::with_capacity(contents.files.len());
        let mut prepared_symlinks = Vec::with_capacity(contents.symlinks.len());
        let mut metadata_warnings = contents.metadata_warnings;
        let mut metadata_applied = 0_usize;

        for directory in contents.directories {
            let destination_path =
                safe_destination_path(&request.destination, &directory.relative_path)?;
            ensure_restore_directory_destination_safe(&request.destination, &destination_path)?;
            prepared_directories.push((directory, destination_path));
        }

        for file in contents.files {
            let destination_path =
                safe_destination_path(&request.destination, &file.relative_path)?;
            ensure_restore_destination_safe(
                &request.destination,
                &destination_path,
                request.overwrite,
            )?;
            prepared_files.push((file, destination_path));
        }

        for symlink in contents.symlinks {
            let destination_path =
                safe_destination_path(&request.destination, &symlink.relative_path)?;
            ensure_restore_symlink_destination_safe(&request.destination, &destination_path)?;
            prepared_symlinks.push((symlink, destination_path));
        }

        let mut planned_directories = Vec::with_capacity(prepared_directories.len());
        let mut planned_files = Vec::with_capacity(prepared_files.len());
        let mut planned_symlinks = Vec::with_capacity(prepared_symlinks.len());

        for (directory, destination_path) in prepared_directories {
            if !request.dry_run {
                create_restored_directory(&destination_path)?;
            }
            planned_directories.push(RestoreDestinationDirectory {
                relative_path: directory.relative_path,
                destination_path,
                modified: directory.modified,
                action: if request.dry_run {
                    RestoreDestinationAction::WouldWrite
                } else {
                    RestoreDestinationAction::Written
                },
            });
        }

        for (file, destination_path) in prepared_files {
            let byte_len = file.contents.len() as u64;

            if !request.dry_run {
                write_restored_file(
                    &destination_path,
                    &file.contents,
                    request.overwrite,
                    request.verify,
                )?;
            }

            planned_files.push(RestoreDestinationFile {
                relative_path: file.relative_path,
                destination_path,
                bytes: byte_len,
                modified: file.modified,
                action: if request.dry_run {
                    RestoreDestinationAction::WouldWrite
                } else {
                    RestoreDestinationAction::Written
                },
                verified: request.verify && !request.dry_run,
            });
        }

        for (symlink, destination_path) in prepared_symlinks {
            if !request.dry_run {
                create_restored_symlink(&symlink.target, &destination_path)?;
            }
            planned_symlinks.push(RestoreDestinationSymlink {
                relative_path: symlink.relative_path,
                destination_path,
                target: symlink.target,
                action: if request.dry_run {
                    RestoreDestinationAction::WouldWrite
                } else {
                    RestoreDestinationAction::Written
                },
            });
        }

        let metadata_planned = planned_files.len() + planned_directories.len();
        if request.dry_run {
            for file in &planned_files {
                plan_restored_modified_timestamp(
                    &file.relative_path,
                    &file.modified,
                    &mut metadata_warnings,
                );
            }

            for directory in &planned_directories {
                plan_restored_modified_timestamp(
                    &directory.relative_path,
                    &directory.modified,
                    &mut metadata_warnings,
                );
            }
        } else {
            for file in &planned_files {
                metadata_applied += apply_restored_modified_timestamp(
                    &file.destination_path,
                    &file.relative_path,
                    &file.modified,
                    RestoredMetadataTarget::RegularFile,
                    &mut metadata_warnings,
                );
            }

            for directory in &planned_directories {
                metadata_applied += apply_restored_modified_timestamp(
                    &directory.destination_path,
                    &directory.relative_path,
                    &directory.modified,
                    RestoredMetadataTarget::Directory,
                    &mut metadata_warnings,
                );
            }
        }

        let bytes = planned_files.iter().map(|file| file.bytes).sum();
        Ok(RestoreDestinationResult {
            snapshot_id: contents.snapshot_id,
            selected_entries: contents.selected_entries,
            directories: planned_directories,
            files: planned_files,
            symlinks: planned_symlinks,
            metadata_planned,
            metadata_applied,
            metadata_warnings,
            bytes,
            dry_run: request.dry_run,
            verified_files: if request.verify && !request.dry_run {
                file_count
            } else {
                0
            },
        })
    }

    async fn load_chunk_index_entries(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        manifest: &SnapshotManifest,
    ) -> CoreResult<BTreeMap<String, ChunkIndexEntry>> {
        let mut entries = BTreeMap::new();
        for index_id in &manifest.body.index_ids {
            let expected_index_object = object_key_for_id("objects/index", index_id)?;
            let index = self
                .read_chunk_index(store, master_key, index_id)
                .await
                .map_err(|error| referenced_object_read_error(error, expected_index_object))?;
            for entry in index.chunks {
                entries.insert(entry.chunk_id.clone(), entry);
            }
        }
        Ok(entries)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotWriteResult {
    pub snapshot_id: String,
    pub created_at_unix_seconds: u64,
    pub manifest_object: ObjectKey,
    pub index_object: ObjectKey,
    pub index_ids: Vec<String>,
    pub commit_object: ObjectKey,
    pub chunk_objects_written: usize,
    pub chunk_objects_reused: usize,
    pub entries: usize,
    pub entries_scanned: usize,
    pub files_backed_up: usize,
    pub directories_backed_up: usize,
    pub symlinks_backed_up: usize,
    pub special_entries_seen: usize,
    pub bytes_scanned: u64,
    pub bytes_uploaded: u64,
    pub chunks_seen: usize,
    pub chunks_written: usize,
    pub chunks_reused: usize,
    pub chunks: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SnapshotCommit {
    pub schema_version: u16,
    pub snapshot_id: String,
    pub manifest_object: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SnapshotForgetMarker {
    pub schema_version: u16,
    pub snapshot_id: String,
    pub manifest_object: String,
    pub commit_object: String,
    pub forgotten_at_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotForgetWrite {
    pub snapshot_id: String,
    pub marker_object: String,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotForgetWriteResult {
    pub markers: Vec<SnapshotForgetWrite>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotSummary {
    pub snapshot_id: String,
    pub created_at_unix_seconds: u64,
    pub tags: Vec<String>,
    pub source_count: usize,
    pub entry_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotEntryListing {
    pub snapshot_id: String,
    pub path: PathBuf,
    pub entries: Vec<SnapshotEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotEntry {
    pub relative_path: PathBuf,
    pub kind: EntryKind,
    pub size_bytes: Option<u64>,
    pub modified: MetadataValue<Timestamp>,
    pub metadata_status: MetadataStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckRepositoryOptions {
    pub read_data: CheckReadDataSelection,
}

impl CheckRepositoryOptions {
    pub const fn full() -> Self {
        Self {
            read_data: CheckReadDataSelection::Full,
        }
    }

    pub const fn subset(subset: CheckReadDataSubset) -> Self {
        Self {
            read_data: CheckReadDataSelection::Subset(subset),
        }
    }
}

impl Default for CheckRepositoryOptions {
    fn default() -> Self {
        Self::full()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckReadDataSelection {
    Full,
    Subset(CheckReadDataSubset),
}

impl CheckReadDataSelection {
    fn mode(self) -> CheckReadDataMode {
        match self {
            Self::Full => CheckReadDataMode::Full,
            Self::Subset(_) => CheckReadDataMode::Subset,
        }
    }

    fn subset_label(self) -> Option<String> {
        match self {
            Self::Full => None,
            Self::Subset(subset) => Some(subset.label()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckReadDataSubset {
    Count(usize),
    Percent(u8),
}

impl CheckReadDataSubset {
    pub fn count(count: usize) -> CoreResult<Self> {
        if count == 0 {
            return Err(CoreError::InvalidCheckDataSubset {
                reason: "count must be greater than zero",
            });
        }

        Ok(Self::Count(count))
    }

    pub fn percent(percent: u8) -> CoreResult<Self> {
        if !(1..=100).contains(&percent) {
            return Err(CoreError::InvalidCheckDataSubset {
                reason: "percent must be between 1 and 100",
            });
        }

        Ok(Self::Percent(percent))
    }

    fn label(self) -> String {
        match self {
            Self::Count(count) => count.to_string(),
            Self::Percent(percent) => format!("{percent}%"),
        }
    }

    fn selected_count(self, total_chunks: usize) -> usize {
        match self {
            Self::Count(count) => count.min(total_chunks),
            Self::Percent(percent) => {
                if total_chunks == 0 {
                    0
                } else {
                    total_chunks
                        .saturating_mul(percent as usize)
                        .div_ceil(100)
                        .min(total_chunks)
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepositoryCheckResult {
    pub repository_id: String,
    pub checked_at_unix_seconds: u64,
    pub metadata_objects_checked: usize,
    pub chunk_objects_checked: usize,
    pub bytes_read: u64,
    pub read_data_mode: CheckReadDataMode,
    pub read_data_subset: Option<String>,
    pub errors: Vec<CheckFinding>,
    pub warnings: Vec<CheckFinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckReadDataMode {
    MetadataOnly,
    Subset,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckFinding {
    pub code: String,
    pub severity: CheckFindingSeverity,
    pub object_key: Option<String>,
    pub snapshot_id: Option<String>,
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChunkReferenceContext {
    snapshot_id: String,
    path: PathBuf,
    object_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChunkCheckTarget {
    indexed: ChunkIndexEntry,
    reference_context: ChunkReferenceContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckFindingSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Complete,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotSelection {
    Id(String),
    Tag(String),
    Latest,
}

impl SnapshotSelection {
    fn label(&self) -> String {
        match self {
            Self::Id(snapshot_id) => format!("id:{snapshot_id}"),
            Self::Tag(tag) => format!("tag:{tag}"),
            Self::Latest => "latest".to_owned(),
        }
    }
}

pub fn select_snapshot<'a>(
    manifests: &'a [SnapshotManifest],
    selection: &SnapshotSelection,
) -> CoreResult<&'a SnapshotManifest> {
    let selected = match selection {
        SnapshotSelection::Id(snapshot_id) => manifests
            .iter()
            .find(|manifest| manifest.snapshot_id == *snapshot_id),
        SnapshotSelection::Tag(tag) => manifests
            .iter()
            .filter(|manifest| manifest.body.tags.iter().any(|candidate| candidate == tag))
            .max_by(snapshot_order),
        SnapshotSelection::Latest => manifests.iter().max_by(snapshot_order),
    };

    selected.ok_or_else(|| CoreError::SnapshotNotFound {
        selection: selection.label(),
    })
}

pub fn snapshot_summaries(manifests: &[SnapshotManifest]) -> Vec<SnapshotSummary> {
    let mut summaries = manifests
        .iter()
        .map(|manifest| SnapshotSummary {
            snapshot_id: manifest.snapshot_id.clone(),
            created_at_unix_seconds: manifest.body.created_at_unix_seconds,
            tags: manifest.body.tags.clone(),
            source_count: manifest
                .body
                .entries
                .iter()
                .filter(|entry| entry.relative_path.as_os_str().is_empty())
                .count(),
            entry_count: manifest.body.entries.len(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .created_at_unix_seconds
            .cmp(&left.created_at_unix_seconds)
            .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
    });
    summaries
}

pub fn list_snapshot_entries(
    manifest: &SnapshotManifest,
    path: impl AsRef<Path>,
) -> CoreResult<SnapshotEntryListing> {
    let path = normalize_restore_path(path.as_ref())?;
    let exact_entry = if path.as_os_str().is_empty() {
        None
    } else {
        manifest
            .body
            .entries
            .iter()
            .find(|entry| entry.relative_path == path)
    };

    let entries = match exact_entry {
        Some(entry) if entry.metadata.kind != EntryKind::Directory => {
            vec![snapshot_entry_from_manifest(entry)]
        }
        Some(_) => manifest
            .body
            .entries
            .iter()
            .filter(|entry| {
                !entry.relative_path.as_os_str().is_empty()
                    && entry_parent(&entry.relative_path) == path
            })
            .map(snapshot_entry_from_manifest)
            .collect(),
        None if path.as_os_str().is_empty() => manifest
            .body
            .entries
            .iter()
            .filter(|entry| {
                !entry.relative_path.as_os_str().is_empty()
                    && entry_parent(&entry.relative_path) == path
            })
            .map(snapshot_entry_from_manifest)
            .collect(),
        None => {
            return Err(CoreError::SnapshotPathNotFound {
                snapshot_id: manifest.snapshot_id.clone(),
                path,
            });
        }
    };

    Ok(SnapshotEntryListing {
        snapshot_id: manifest.snapshot_id.clone(),
        path,
        entries,
    })
}

fn snapshot_order(left: &&SnapshotManifest, right: &&SnapshotManifest) -> std::cmp::Ordering {
    compare_snapshot_manifests(left, right)
}

fn compare_snapshot_manifests(
    left: &SnapshotManifest,
    right: &SnapshotManifest,
) -> std::cmp::Ordering {
    left.body
        .created_at_unix_seconds
        .cmp(&right.body.created_at_unix_seconds)
        .then_with(|| left.snapshot_id.cmp(&right.snapshot_id))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreContentRequest {
    pub snapshot_id: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreContentResult {
    pub snapshot_id: String,
    pub selected_entries: usize,
    pub directories: Vec<RestoredDirectory>,
    pub files: Vec<RestoredFile>,
    pub symlinks: Vec<RestoredSymlink>,
    pub metadata_warnings: Vec<RestoreMetadataWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredDirectory {
    pub relative_path: PathBuf,
    pub modified: MetadataValue<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredFile {
    pub relative_path: PathBuf,
    pub contents: Vec<u8>,
    pub modified: MetadataValue<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredSymlink {
    pub relative_path: PathBuf,
    pub target: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestoreOverwritePolicy {
    #[default]
    FailIfExists,
    OverwriteFiles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationRequest {
    pub snapshot_id: String,
    pub paths: Vec<PathBuf>,
    pub destination: PathBuf,
    pub overwrite: RestoreOverwritePolicy,
    pub dry_run: bool,
    pub verify: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationResult {
    pub snapshot_id: String,
    pub selected_entries: usize,
    pub directories: Vec<RestoreDestinationDirectory>,
    pub files: Vec<RestoreDestinationFile>,
    pub symlinks: Vec<RestoreDestinationSymlink>,
    pub metadata_planned: usize,
    pub metadata_applied: usize,
    pub metadata_warnings: Vec<RestoreMetadataWarning>,
    pub bytes: u64,
    pub dry_run: bool,
    pub verified_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationDirectory {
    pub relative_path: PathBuf,
    pub destination_path: PathBuf,
    pub modified: MetadataValue<Timestamp>,
    pub action: RestoreDestinationAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationFile {
    pub relative_path: PathBuf,
    pub destination_path: PathBuf,
    pub bytes: u64,
    pub modified: MetadataValue<Timestamp>,
    pub action: RestoreDestinationAction,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationSymlink {
    pub relative_path: PathBuf,
    pub destination_path: PathBuf,
    pub target: PathBuf,
    pub action: RestoreDestinationAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreDestinationAction {
    WouldWrite,
    Written,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreMetadataWarning {
    pub relative_path: PathBuf,
    pub field: &'static str,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SnapshotManifest {
    pub schema_version: u16,
    pub snapshot_id: String,
    pub body: SnapshotManifestBody,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SnapshotManifestBody {
    pub created_at_unix_seconds: u64,
    pub tags: Vec<String>,
    pub entries: Vec<ManifestEntry>,
    pub index_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ManifestEntry {
    pub root: PathBuf,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub metadata: EntryMetadata,
    pub chunks: Vec<ManifestChunkRef>,
}

impl ManifestEntry {
    fn from_source_entry(entry: SourceEntry) -> Self {
        Self {
            root: entry.root,
            path: entry.path,
            relative_path: entry.relative_path,
            metadata: entry.metadata,
            chunks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ManifestChunkRef {
    pub chunk_id: String,
    pub object_key: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChunkIndex {
    pub schema_version: u16,
    pub index_id: String,
    pub chunks: Vec<ChunkIndexEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ChunkIndexEntry {
    pub chunk_id: String,
    pub object_key: String,
    pub plaintext_length: u64,
    pub compressed_length: u64,
    pub stored_length: u64,
    pub compression: CompressionAlgorithm,
    pub aead: RepositoryAeadAlgorithm,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionAlgorithm {
    Zstd,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAeadAlgorithm {
    XChaCha20Poly1305,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredEncryptedObject {
    algorithm: RepositoryAeadAlgorithm,
    nonce: [u8; fileferry_crypto::XCHACHA20_POLY1305_NONCE_LEN],
    ciphertext: Vec<u8>,
}

fn encrypt_repository_object(
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
    plaintext: &[u8],
) -> CoreResult<Vec<u8>> {
    let context = ObjectContext::new(kind, object_key.as_str())
        .map_err(|source| CoreError::Encryption { source })?;
    let encrypted = encrypt_object(key, &context, plaintext)
        .map_err(|source| CoreError::Encryption { source })?;
    encode_encrypted_object(encrypted)
}

async fn write_encrypted_json_object<T: Serialize>(
    store: &dyn ObjectStore,
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
    value: &T,
) -> CoreResult<u64> {
    let plaintext =
        serde_json::to_vec(value).map_err(|source| CoreError::Serialization { source })?;
    let encrypted = encrypt_repository_object(key, kind, object_key, &plaintext)?;
    let encrypted_len = encrypted.len() as u64;
    let status = store
        .put_if_absent(object_key, &encrypted)
        .await
        .map_err(|source| CoreError::Storage { source })?;
    Ok(match status {
        PutStatus::Created => encrypted_len,
        PutStatus::AlreadyPresent => 0,
    })
}

async fn read_encrypted_json_object<T: for<'de> Deserialize<'de>>(
    store: &dyn ObjectStore,
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
) -> CoreResult<T> {
    read_encrypted_json_object_with_bytes(store, key, kind, object_key)
        .await
        .map(|(value, _)| value)
}

async fn read_encrypted_json_object_with_bytes<T: for<'de> Deserialize<'de>>(
    store: &dyn ObjectStore,
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
) -> CoreResult<(T, u64)> {
    let encrypted = store
        .get(object_key)
        .await
        .map_err(|source| CoreError::Storage { source })?;
    let bytes_read = encrypted.len() as u64;
    let plaintext = decrypt_repository_object(key, kind, object_key, &encrypted)?;
    let value = serde_json::from_slice(&plaintext).map_err(|source| CoreError::MetadataDecode {
        key: object_key.clone(),
        source,
    })?;
    Ok((value, bytes_read))
}

fn decrypt_repository_object(
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
    bytes: &[u8],
) -> CoreResult<Vec<u8>> {
    let stored: StoredEncryptedObject =
        serde_json::from_slice(bytes).map_err(|source| CoreError::ObjectDecode {
            key: object_key.clone(),
            source,
        })?;
    let algorithm = match stored.algorithm {
        RepositoryAeadAlgorithm::XChaCha20Poly1305 => AeadAlgorithm::XChaCha20Poly1305,
    };
    let object = EncryptedObject {
        algorithm,
        nonce: stored.nonce,
        ciphertext: stored.ciphertext,
    };
    let context = ObjectContext::new(kind, object_key.as_str())
        .map_err(|source| CoreError::Encryption { source })?;

    decrypt_object(key, &context, &object).map_err(|source| CoreError::ObjectAuthentication {
        key: object_key.clone(),
        source,
    })
}

fn encode_encrypted_object(encrypted: EncryptedObject) -> CoreResult<Vec<u8>> {
    let algorithm = match encrypted.algorithm {
        AeadAlgorithm::XChaCha20Poly1305 => RepositoryAeadAlgorithm::XChaCha20Poly1305,
    };
    serde_json::to_vec(&StoredEncryptedObject {
        algorithm,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    })
    .map_err(|source| CoreError::Serialization { source })
}

fn current_unix_seconds() -> CoreResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| CoreError::SystemClock { source })
}

fn normalize_restore_paths(paths: &[PathBuf]) -> CoreResult<Vec<PathBuf>> {
    paths
        .iter()
        .map(|path| normalize_restore_path(path))
        .collect()
}

fn normalize_restore_path(path: &Path) -> CoreResult<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                return Err(CoreError::InvalidRestoreRequest {
                    reason: "restore paths must not contain parent directory components",
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::InvalidRestoreRequest {
                    reason: "restore paths must be relative to the snapshot root",
                });
            }
        }
    }

    Ok(normalized)
}

fn validate_restore_destination_root(destination: &Path) -> CoreResult<()> {
    if !destination.is_absolute() {
        return Err(CoreError::RestoreDestinationNotAbsolute {
            path: destination.to_path_buf(),
        });
    }

    Ok(())
}

fn safe_destination_path(destination: &Path, relative_path: &Path) -> CoreResult<PathBuf> {
    let relative_path = normalize_restore_path(relative_path)?;
    let destination_path = destination.join(relative_path);

    if !destination_path.starts_with(destination) {
        return Err(CoreError::RestoreDestinationEscapesRoot {
            path: destination_path,
        });
    }

    Ok(destination_path)
}

fn ensure_restore_destination_safe(
    root: &Path,
    destination_path: &Path,
    overwrite: RestoreOverwritePolicy,
) -> CoreResult<()> {
    ensure_no_symlink_ancestor(root, destination_path)?;

    match fs::symlink_metadata(destination_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CoreError::RestoreDestinationSymlink {
                path: destination_path.to_path_buf(),
                symlink: destination_path.to_path_buf(),
            })
        }
        Ok(metadata) if metadata.is_file() => match overwrite {
            RestoreOverwritePolicy::FailIfExists => Err(CoreError::RestoreDestinationExists {
                path: destination_path.to_path_buf(),
            }),
            RestoreOverwritePolicy::OverwriteFiles => Ok(()),
        },
        Ok(_) => Err(CoreError::RestoreDestinationKind {
            path: destination_path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_restore_directory_destination_safe(
    root: &Path,
    destination_path: &Path,
) -> CoreResult<()> {
    ensure_no_symlink_ancestor(root, destination_path)?;

    match fs::symlink_metadata(destination_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CoreError::RestoreDestinationSymlink {
                path: destination_path.to_path_buf(),
                symlink: destination_path.to_path_buf(),
            })
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(CoreError::RestoreDestinationKind {
            path: destination_path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_restore_symlink_destination_safe(root: &Path, destination_path: &Path) -> CoreResult<()> {
    ensure_no_symlink_ancestor(root, destination_path)?;

    match fs::symlink_metadata(destination_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CoreError::RestoreDestinationSymlink {
                path: destination_path.to_path_buf(),
                symlink: destination_path.to_path_buf(),
            })
        }
        Ok(_) => Err(CoreError::RestoreDestinationExists {
            path: destination_path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_no_symlink_ancestor(root: &Path, destination_path: &Path) -> CoreResult<()> {
    let parent = destination_path
        .parent()
        .ok_or(CoreError::RestoreDestinationEscapesRoot {
            path: destination_path.to_path_buf(),
        })?;
    let mut cursor = PathBuf::new();

    for component in parent.components() {
        cursor.push(component.as_os_str());
        if !cursor.starts_with(root) {
            continue;
        }

        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CoreError::RestoreDestinationSymlink {
                    path: destination_path.to_path_buf(),
                    symlink: cursor,
                });
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(CoreError::RestoreDestinationKind { path: cursor });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(CoreError::RestoreDestinationWrite {
                    path: cursor,
                    source,
                });
            }
        }
    }

    Ok(())
}

fn create_restored_directory(destination_path: &Path) -> CoreResult<()> {
    fs::create_dir_all(destination_path).map_err(|source| CoreError::RestoreDestinationWrite {
        path: destination_path.to_path_buf(),
        source,
    })
}

fn write_restored_file(
    destination_path: &Path,
    contents: &[u8],
    overwrite: RestoreOverwritePolicy,
    verify: bool,
) -> CoreResult<()> {
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent).map_err(|source| CoreError::RestoreDestinationWrite {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut options = fs::OpenOptions::new();
    options.write(true);
    match overwrite {
        RestoreOverwritePolicy::FailIfExists => {
            options.create_new(true);
        }
        RestoreOverwritePolicy::OverwriteFiles => {
            options.create(true).truncate(true);
        }
    }
    let mut file =
        options
            .open(destination_path)
            .map_err(|source| CoreError::RestoreDestinationWrite {
                path: destination_path.to_path_buf(),
                source,
            })?;
    io::Write::write_all(&mut file, contents).map_err(|source| {
        CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        }
    })?;
    file.sync_all()
        .map_err(|source| CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        })?;

    if verify {
        let restored =
            fs::read(destination_path).map_err(|source| CoreError::RestoreVerificationRead {
                path: destination_path.to_path_buf(),
                source,
            })?;
        if restored != contents {
            return Err(CoreError::RestoreVerificationMismatch {
                path: destination_path.to_path_buf(),
            });
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoredMetadataTarget {
    RegularFile,
    Directory,
}

fn apply_restored_modified_timestamp(
    destination_path: &Path,
    relative_path: &Path,
    modified: &MetadataValue<Timestamp>,
    target: RestoredMetadataTarget,
    warnings: &mut Vec<RestoreMetadataWarning>,
) -> usize {
    let Some(modified_time) = restored_modified_time_or_warn(relative_path, modified, warnings)
    else {
        return 0;
    };

    match set_restored_modified_timestamp(destination_path, target, modified_time) {
        Ok(()) => 1,
        Err(source) => {
            warnings.push(RestoreMetadataWarning {
                relative_path: relative_path.to_path_buf(),
                field: "modified",
                reason: format!("modified timestamp could not be applied: {source}"),
            });
            0
        }
    }
}

fn plan_restored_modified_timestamp(
    relative_path: &Path,
    modified: &MetadataValue<Timestamp>,
    warnings: &mut Vec<RestoreMetadataWarning>,
) {
    let _ = restored_modified_time_or_warn(relative_path, modified, warnings);
}

fn restored_modified_time_or_warn(
    relative_path: &Path,
    modified: &MetadataValue<Timestamp>,
    warnings: &mut Vec<RestoreMetadataWarning>,
) -> Option<SystemTime> {
    let timestamp = match modified {
        MetadataValue::Captured(timestamp) => timestamp,
        MetadataValue::Unsupported => {
            warnings.push(RestoreMetadataWarning {
                relative_path: relative_path.to_path_buf(),
                field: "modified",
                reason: "modified timestamp was not captured".to_owned(),
            });
            return None;
        }
        MetadataValue::Denied(reason) => {
            warnings.push(RestoreMetadataWarning {
                relative_path: relative_path.to_path_buf(),
                field: "modified",
                reason: format!("modified timestamp was denied during backup: {reason}"),
            });
            return None;
        }
    };

    let Some(modified_time) = system_time_from_timestamp(*timestamp) else {
        warnings.push(RestoreMetadataWarning {
            relative_path: relative_path.to_path_buf(),
            field: "modified",
            reason: "modified timestamp is outside the supported system time range".to_owned(),
        });
        return None;
    };

    Some(modified_time)
}

fn system_time_from_timestamp(timestamp: Timestamp) -> Option<SystemTime> {
    if timestamp.nanoseconds >= 1_000_000_000 {
        return None;
    }

    if timestamp.seconds >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(timestamp.seconds as u64))?
            .checked_add(Duration::from_nanos(u64::from(timestamp.nanoseconds)))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(timestamp.seconds.unsigned_abs()))?
            .checked_add(Duration::from_nanos(u64::from(timestamp.nanoseconds)))
    }
}

fn set_restored_modified_timestamp(
    destination_path: &Path,
    target: RestoredMetadataTarget,
    modified_time: SystemTime,
) -> io::Result<()> {
    let file = match target {
        RestoredMetadataTarget::RegularFile => {
            fs::OpenOptions::new().write(true).open(destination_path)?
        }
        RestoredMetadataTarget::Directory => fs::File::open(destination_path)?,
    };
    file.set_times(fs::FileTimes::new().set_modified(modified_time))
}

#[cfg(unix)]
fn create_restored_symlink(target: &Path, destination_path: &Path) -> CoreResult<()> {
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent).map_err(|source| CoreError::RestoreDestinationWrite {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    std::os::unix::fs::symlink(target, destination_path).map_err(|source| {
        CoreError::RestoreDestinationWrite {
            path: destination_path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn create_restored_symlink(_target: &Path, _destination_path: &Path) -> CoreResult<()> {
    Err(CoreError::UnsupportedRestoreFeature {
        feature: "symlink restore",
    })
}

fn scoped_manifest_entries<'a>(
    manifest: &'a SnapshotManifest,
    restore_paths: &[PathBuf],
) -> Vec<&'a ManifestEntry> {
    manifest
        .body
        .entries
        .iter()
        .filter(|entry| {
            restore_paths.is_empty()
                || restore_paths.iter().any(|path| {
                    path.as_os_str().is_empty()
                        || entry.relative_path == *path
                        || entry.relative_path.starts_with(path)
                })
        })
        .collect()
}

fn validate_snapshot_manifest(
    manifest: &SnapshotManifest,
    object_key: &ObjectKey,
) -> CoreResult<()> {
    let mut entry_kinds = BTreeMap::new();

    for entry in &manifest.body.entries {
        validate_manifest_entry_path(manifest, object_key, &entry.relative_path)?;

        if entry.relative_path.as_os_str().is_empty() && entry.metadata.kind != EntryKind::Directory
        {
            return Err(CoreError::InvalidSnapshotManifest {
                snapshot_id: manifest.snapshot_id.clone(),
                object_key: object_key.clone(),
                path: Some(entry.relative_path.clone()),
                reason: "root entry is not a directory",
            });
        }

        if entry_kinds
            .insert(entry.relative_path.clone(), entry.metadata.kind.clone())
            .is_some()
        {
            return Err(CoreError::InvalidSnapshotManifest {
                snapshot_id: manifest.snapshot_id.clone(),
                object_key: object_key.clone(),
                path: Some(entry.relative_path.clone()),
                reason: "duplicate entry path",
            });
        }

        if entry.metadata.kind != EntryKind::RegularFile && !entry.chunks.is_empty() {
            return Err(CoreError::InvalidSnapshotManifest {
                snapshot_id: manifest.snapshot_id.clone(),
                object_key: object_key.clone(),
                path: Some(entry.relative_path.clone()),
                reason: "non-file entry contains chunk references",
            });
        }

        if let (EntryKind::RegularFile, Some(size_bytes)) =
            (&entry.metadata.kind, entry.metadata.size_bytes)
        {
            let chunk_bytes = entry
                .chunks
                .iter()
                .try_fold(0_u64, |total, chunk| total.checked_add(chunk.length))
                .ok_or_else(|| CoreError::InvalidSnapshotManifest {
                    snapshot_id: manifest.snapshot_id.clone(),
                    object_key: object_key.clone(),
                    path: Some(entry.relative_path.clone()),
                    reason: "regular-file chunk lengths overflow",
                })?;
            if chunk_bytes != size_bytes {
                return Err(CoreError::InvalidSnapshotManifest {
                    snapshot_id: manifest.snapshot_id.clone(),
                    object_key: object_key.clone(),
                    path: Some(entry.relative_path.clone()),
                    reason: "regular-file chunk lengths do not match captured size",
                });
            }
        }
    }

    for entry in &manifest.body.entries {
        for ancestor in entry.relative_path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                break;
            }

            if entry_kinds
                .get(ancestor)
                .is_some_and(|kind| kind != &EntryKind::Directory)
            {
                return Err(CoreError::InvalidSnapshotManifest {
                    snapshot_id: manifest.snapshot_id.clone(),
                    object_key: object_key.clone(),
                    path: Some(entry.relative_path.clone()),
                    reason: "entry has a non-directory ancestor",
                });
            }
        }
    }

    Ok(())
}

fn validate_manifest_entry_path(
    manifest: &SnapshotManifest,
    object_key: &ObjectKey,
    path: &Path,
) -> CoreResult<()> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(CoreError::InvalidSnapshotManifest {
                    snapshot_id: manifest.snapshot_id.clone(),
                    object_key: object_key.clone(),
                    path: Some(path.to_path_buf()),
                    reason: "entry path is not a normalized relative path",
                });
            }
        }
    }

    if normalized != path {
        return Err(CoreError::InvalidSnapshotManifest {
            snapshot_id: manifest.snapshot_id.clone(),
            object_key: object_key.clone(),
            path: Some(path.to_path_buf()),
            reason: "entry path is not a normalized relative path",
        });
    }

    Ok(())
}

fn validate_indexed_chunk_reference(
    snapshot_id: &str,
    path: &Path,
    chunk: &ManifestChunkRef,
    indexed: &ChunkIndexEntry,
) -> CoreResult<()> {
    let reason = if indexed.object_key != chunk.object_key {
        Some("object key mismatch")
    } else if indexed.plaintext_length != chunk.length {
        Some("plaintext length mismatch")
    } else if indexed.compression != CompressionAlgorithm::Zstd {
        Some("compression algorithm mismatch")
    } else {
        None
    };

    if let Some(reason) = reason {
        return Err(CoreError::ChunkIndexMismatch {
            snapshot_id: snapshot_id.to_owned(),
            path: path.to_path_buf(),
            chunk_id: chunk.chunk_id.clone(),
            object_key: chunk.object_key.clone(),
            reason,
        });
    }

    Ok(())
}

fn ensure_restore_paths_exist(
    manifest: &SnapshotManifest,
    restore_paths: &[PathBuf],
) -> CoreResult<()> {
    for restore_path in restore_paths {
        if manifest.body.entries.iter().any(|entry| {
            restore_path.as_os_str().is_empty()
                || entry.relative_path == *restore_path
                || entry.relative_path.starts_with(restore_path)
        }) {
            continue;
        }

        return Err(CoreError::SnapshotPathNotFound {
            snapshot_id: manifest.snapshot_id.clone(),
            path: restore_path.clone(),
        });
    }

    Ok(())
}

fn snapshot_entry_from_manifest(entry: &ManifestEntry) -> SnapshotEntry {
    SnapshotEntry {
        relative_path: entry.relative_path.clone(),
        kind: entry.metadata.kind.clone(),
        size_bytes: entry.metadata.size_bytes,
        modified: entry.metadata.modified.clone(),
        metadata_status: metadata_status(&entry.metadata),
    }
}

fn entry_parent(path: &Path) -> PathBuf {
    path.parent().unwrap_or_else(|| Path::new("")).to_path_buf()
}

fn metadata_status(metadata: &EntryMetadata) -> MetadataStatus {
    let mut saw_captured = false;
    let mut saw_unsupported = false;

    for value in [&metadata.modified, &metadata.created] {
        match value {
            MetadataValue::Captured(_) => saw_captured = true,
            MetadataValue::Unsupported => saw_unsupported = true,
            MetadataValue::Denied(_) => return MetadataStatus::Partial,
        }
    }

    if metadata.kind == EntryKind::Symlink {
        match &metadata.symlink_target {
            MetadataValue::Captured(_) => saw_captured = true,
            MetadataValue::Unsupported => saw_unsupported = true,
            MetadataValue::Denied(_) => return MetadataStatus::Partial,
        }
    }

    match (saw_captured, saw_unsupported) {
        (false, true) => MetadataStatus::Unsupported,
        (true, true) => MetadataStatus::Partial,
        _ => MetadataStatus::Complete,
    }
}

fn content_id_for_metadata<T: Serialize>(
    master_key: &MasterKey,
    purpose: KeyPurpose,
    context: &[u8],
    value: &T,
) -> CoreResult<String> {
    let bytes = serde_json::to_vec(value).map_err(|source| CoreError::Serialization { source })?;
    keyed_content_id(master_key, purpose, context, &bytes)
        .map(|id| hex_bytes(&id))
        .map_err(|source| CoreError::Encryption { source })
}

fn check_read_error(error: CoreError, expected_key: ObjectKey) -> CoreError {
    match error {
        CoreError::Storage {
            source: StorageError::ObjectNotFound { .. },
        } => CoreError::RepositoryCheckMissingObject { key: expected_key },
        other => other,
    }
}

fn select_check_chunk_ids(
    chunk_targets: &BTreeMap<String, ChunkCheckTarget>,
    selection: CheckReadDataSelection,
) -> Vec<String> {
    let selected_count = match selection {
        CheckReadDataSelection::Full => chunk_targets.len(),
        CheckReadDataSelection::Subset(subset) => subset.selected_count(chunk_targets.len()),
    };

    chunk_targets.keys().take(selected_count).cloned().collect()
}

fn referenced_object_read_error(error: CoreError, expected_key: ObjectKey) -> CoreError {
    match error {
        CoreError::Storage {
            source: StorageError::ObjectNotFound { .. },
        } => CoreError::RepositoryReferencedObjectMissing { key: expected_key },
        other => other,
    }
}

fn object_key_for_id(group: &str, id: &str) -> CoreResult<ObjectKey> {
    let prefix = id.get(..2).ok_or(CoreError::InvalidBackupPipelineConfig {
        reason: "object id must be at least two characters",
    })?;
    ObjectKey::new(format!("{group}/{prefix}/{id}"))
        .map_err(|source| CoreError::ObjectKey { source })
}

fn object_key_for_commit(snapshot_id: &str) -> CoreResult<ObjectKey> {
    ObjectKey::new(format!("commits/{snapshot_id}"))
        .map_err(|source| CoreError::ObjectKey { source })
}

fn object_key_for_forget_marker(snapshot_id: &str) -> CoreResult<ObjectKey> {
    ObjectKey::new(format!("forgets/{snapshot_id}"))
        .map_err(|source| CoreError::ObjectKey { source })
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExclusionRule {
    pattern: String,
    segments: Vec<String>,
    directory_prefix: bool,
}

impl ExclusionRule {
    pub fn new(pattern: impl Into<String>) -> Self {
        let pattern = pattern.into();
        let directory_prefix = pattern.ends_with('/');
        let normalized = pattern.trim_matches('/').replace('\\', "/");
        let segments = normalized
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();

        Self {
            pattern,
            segments,
            directory_prefix,
        }
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn matches(&self, relative_path: &Path) -> bool {
        let path_segments = path_segments(relative_path);
        if path_segments.is_empty() || self.segments.is_empty() {
            return false;
        }

        if self.directory_prefix && path_segments.len() < self.segments.len() {
            return false;
        }

        if !self.pattern.contains('/') && !self.pattern.contains('\\') {
            return path_segments
                .iter()
                .any(|segment| wildcard_match(&self.segments[0], segment));
        }

        if self.directory_prefix {
            return path_segments_match(&self.segments, &path_segments[..self.segments.len()]);
        }

        path_segments_match(&self.segments, &path_segments)
    }
}

fn read_sorted_children(directory: &Path) -> CoreResult<Vec<PathBuf>> {
    let read_dir = fs::read_dir(directory).map_err(|source| CoreError::DirectoryRead {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut children = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|source| CoreError::DirectoryEntryRead {
            path: directory.to_path_buf(),
            source,
        })?;
        children.push(entry.path());
    }

    children.sort();
    Ok(children)
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

fn path_segments_match(pattern: &[String], path: &[String]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some((pattern_head, pattern_tail)), _) if pattern_head == "**" => {
            path_segments_match(pattern_tail, path)
                || path
                    .split_first()
                    .is_some_and(|(_, path_tail)| path_segments_match(pattern, path_tail))
        }
        (Some(_), None) => false,
        (Some((pattern_head, pattern_tail)), Some((path_head, path_tail))) => {
            wildcard_match(pattern_head, path_head) && path_segments_match(pattern_tail, path_tail)
        }
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let mut remaining = value;
    let mut parts = pattern.split('*').peekable();
    let starts_with_wildcard = pattern.starts_with('*');
    let ends_with_wildcard = pattern.ends_with('*');

    if let Some(first) = parts.next() {
        if !first.is_empty() {
            if !remaining.starts_with(first) {
                return false;
            }
            remaining = &remaining[first.len()..];
        } else if !starts_with_wildcard {
            return false;
        }
    }

    while let Some(part) = parts.next() {
        if part.is_empty() {
            continue;
        }

        match remaining.find(part) {
            Some(index) => {
                remaining = &remaining[index + part.len()..];
                if parts.peek().is_none() && !ends_with_wildcard {
                    return remaining.is_empty();
                }
            }
            None => return false,
        }
    }

    ends_with_wildcard || remaining.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_test_pipeline() -> BackupPipeline {
        BackupPipeline::new(BackupPipelineConfig {
            chunking: ChunkingConfig::new(64, 256, 1024),
            compression_level: DEFAULT_ZSTD_COMPRESSION_LEVEL,
            repository_id: "repo-test-id".to_owned(),
        })
        .expect("pipeline")
    }

    fn varied_bytes(seed: usize, len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| ((index * 31 + seed * 17 + index / 5) % 251) as u8)
            .collect()
    }

    struct ReverseListingStore<'a> {
        inner: &'a fileferry_testkit::FakeObjectStore,
    }

    impl ObjectStore for ReverseListingStore<'_> {
        fn capabilities(&self) -> fileferry_storage::StorageCapabilities {
            self.inner.capabilities()
        }

        fn put_if_absent<'a>(
            &'a self,
            key: &'a ObjectKey,
            bytes: &'a [u8],
        ) -> fileferry_storage::StorageFuture<'a, PutStatus> {
            self.inner.put_if_absent(key, bytes)
        }

        fn get<'a>(&'a self, key: &'a ObjectKey) -> fileferry_storage::StorageFuture<'a, Vec<u8>> {
            self.inner.get(key)
        }

        fn exists<'a>(&'a self, key: &'a ObjectKey) -> fileferry_storage::StorageFuture<'a, bool> {
            self.inner.exists(key)
        }

        fn delete<'a>(&'a self, key: &'a ObjectKey) -> fileferry_storage::StorageFuture<'a, ()> {
            self.inner.delete(key)
        }

        fn list_prefix<'a>(
            &'a self,
            prefix: &'a ObjectKeyPrefix,
        ) -> fileferry_storage::StorageFuture<'a, Vec<ObjectKey>> {
            Box::pin(async move {
                let mut keys = self.inner.list_prefix(prefix).await?;
                keys.reverse();
                Ok(keys)
            })
        }
    }

    async fn replace_committed_manifest_for_tests(
        pipeline: &BackupPipeline,
        store: &fileferry_testkit::FakeObjectStore,
        master_key: &MasterKey,
        result: &SnapshotWriteResult,
        mutate: impl FnOnce(&mut SnapshotManifest),
    ) -> (String, ObjectKey) {
        let repository_context = pipeline.config().repository_id.as_bytes();
        let mut manifest = pipeline
            .read_snapshot_manifest(store, master_key, &result.snapshot_id)
            .await
            .expect("manifest read");
        mutate(&mut manifest);

        let new_snapshot_id = content_id_for_metadata(
            master_key,
            KeyPurpose::SnapshotMetadata,
            repository_context,
            &manifest.body,
        )
        .expect("new snapshot id");
        manifest.snapshot_id = new_snapshot_id.clone();
        let manifest_object =
            object_key_for_id("objects/manifest", &new_snapshot_id).expect("manifest object key");
        let manifest_key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, repository_context)
            .expect("manifest key");
        let manifest_bytes = encrypt_repository_object(
            &manifest_key,
            ObjectKind::SnapshotManifest,
            &manifest_object,
            &serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("encrypted manifest");
        store
            .overwrite_for_tests(manifest_object.clone(), manifest_bytes)
            .await;

        store
            .delete(&result.commit_object)
            .await
            .expect("delete old commit");
        let commit_object = object_key_for_commit(&new_snapshot_id).expect("new commit object key");
        let commit = SnapshotCommit {
            schema_version: 0,
            snapshot_id: new_snapshot_id.clone(),
            manifest_object: manifest_object.as_str().to_owned(),
        };
        store
            .overwrite_for_tests(
                commit_object,
                serde_json::to_vec(&commit).expect("commit json"),
            )
            .await;

        (new_snapshot_id, manifest_object)
    }

    #[tokio::test]
    async fn repository_bootstrap_creates_and_unlocks_master_key() {
        use fileferry_testkit::FakeObjectStore;

        let store = FakeObjectStore::new();
        let passphrase = SecretString::from("correct horse battery staple");
        let created = create_repository(&store, &passphrase, KdfParams::for_tests())
            .await
            .expect("create repository");

        assert!(created.created);
        assert_eq!(created.format_version, REPOSITORY_FORMAT_VERSION_V0);
        assert_eq!(created.key_slots, 1);
        assert_eq!(
            created.repository.repository_id.len(),
            REPOSITORY_ID_BYTES * 2
        );

        let opened = open_repository(&store, &passphrase)
            .await
            .expect("open repository");
        assert_eq!(opened.repository_id, created.repository.repository_id);

        let reopened = create_repository(&store, &passphrase, KdfParams::for_tests())
            .await
            .expect("reopen existing repository");
        assert!(!reopened.created);
        assert_eq!(reopened.repository.repository_id, opened.repository_id);
    }

    #[tokio::test]
    async fn repository_bootstrap_fails_closed_for_wrong_passphrase() {
        use fileferry_testkit::FakeObjectStore;

        let store = FakeObjectStore::new();
        create_repository(
            &store,
            &SecretString::from("correct"),
            KdfParams::for_tests(),
        )
        .await
        .expect("create repository");

        let error = open_repository(&store, &SecretString::from("wrong"))
            .await
            .expect_err("wrong passphrase fails");
        assert!(matches!(error, CoreError::RepositoryUnlock { .. }));
    }

    #[tokio::test]
    async fn repository_bootstrap_reports_unsupported_format_and_features() {
        use fileferry_testkit::FakeObjectStore;

        let passphrase = SecretString::from("correct");
        let version_store = FakeObjectStore::new();
        create_repository(&version_store, &passphrase, KdfParams::for_tests())
            .await
            .expect("create repository");
        let bootstrap_key = bootstrap_object_key().expect("bootstrap key");
        let mut bootstrap: serde_json::Value = serde_json::from_slice(
            &version_store
                .get(&bootstrap_key)
                .await
                .expect("bootstrap bytes"),
        )
        .expect("bootstrap json");
        bootstrap["format_version"] = serde_json::json!(999);
        version_store
            .overwrite_for_tests(
                bootstrap_key.clone(),
                serde_json::to_vec(&bootstrap).expect("unsupported format json"),
            )
            .await;
        let version_error = open_repository(&version_store, &passphrase)
            .await
            .expect_err("unsupported format should fail");
        assert!(matches!(
            version_error,
            CoreError::UnsupportedRepositoryFormat {
                format_version: 999
            }
        ));

        let feature_store = FakeObjectStore::new();
        create_repository(&feature_store, &passphrase, KdfParams::for_tests())
            .await
            .expect("create repository");
        let mut bootstrap: serde_json::Value = serde_json::from_slice(
            &feature_store
                .get(&bootstrap_key)
                .await
                .expect("bootstrap bytes"),
        )
        .expect("bootstrap json");
        bootstrap["features"] = serde_json::json!(["future-feature"]);
        feature_store
            .overwrite_for_tests(
                bootstrap_key,
                serde_json::to_vec(&bootstrap).expect("unsupported features json"),
            )
            .await;
        let feature_error = open_repository(&feature_store, &passphrase)
            .await
            .expect_err("unsupported features should fail");
        assert!(matches!(
            feature_error,
            CoreError::UnsupportedRepositoryFeatures
        ));
    }

    #[tokio::test]
    async fn initialized_repository_supports_committed_snapshot_discovery() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample").expect("write sample");
        let store = FakeObjectStore::new();
        let passphrase = SecretString::from("correct");
        let created = create_repository(&store, &passphrase, KdfParams::for_tests())
            .await
            .expect("create repository");
        let pipeline = BackupPipeline::new(BackupPipelineConfig::new(
            created.repository.repository_id.clone(),
        ))
        .expect("pipeline");
        let written = pipeline
            .write_snapshot(
                &store,
                &created.repository.master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["drill".to_owned()],
                },
            )
            .await
            .expect("write snapshot");
        let opened = open_repository(&store, &passphrase)
            .await
            .expect("open repository");
        let discovered = pipeline
            .read_committed_snapshot_manifests(&store, &opened.master_key)
            .await
            .expect("discover snapshots");

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].snapshot_id, written.snapshot_id);
        assert_eq!(snapshot_summaries(&discovered)[0].tags, vec!["drill"]);
    }

    fn relative_entries(entries: &[SourceEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| {
                if entry.relative_path.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    entry.relative_path.display().to_string()
                }
            })
            .collect()
    }

    #[test]
    fn walks_sources_in_deterministic_relative_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("b")).expect("create b");
        fs::create_dir(temp.path().join("a")).expect("create a");
        fs::write(temp.path().join("b/file.txt"), b"b").expect("write b");
        fs::write(temp.path().join("a/file.txt"), b"a").expect("write a");

        let entries = SourceWalker::default()
            .walk(&[temp.path().to_path_buf()])
            .expect("walk");

        assert_eq!(
            relative_entries(&entries),
            vec![".", "a", "b", "a/file.txt", "b/file.txt"]
        );
    }

    #[test]
    fn excludes_matching_files_and_prunes_matching_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("project/target")).expect("create target");
        fs::create_dir_all(temp.path().join("project/src")).expect("create src");
        fs::write(temp.path().join("project/target/build.log"), b"log").expect("write log");
        fs::write(temp.path().join("project/src/main.rs"), b"fn main() {}").expect("write main");
        fs::write(temp.path().join("project/src/main.tmp"), b"tmp").expect("write tmp");

        let walker = SourceWalker::new(vec![
            ExclusionRule::new("**/target"),
            ExclusionRule::new("*.tmp"),
        ]);
        let entries = walker.walk(&[temp.path().to_path_buf()]).expect("walk");

        assert_eq!(
            relative_entries(&entries),
            vec![".", "project", "project/src", "project/src/main.rs"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn records_symlinks_without_following_directory_targets() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("real")).expect("create real");
        fs::write(temp.path().join("real/nested.txt"), b"nested").expect("write nested");
        symlink("real", temp.path().join("link")).expect("symlink");

        let entries = SourceWalker::default()
            .walk(&[temp.path().to_path_buf()])
            .expect("walk");

        assert_eq!(
            relative_entries(&entries),
            vec![".", "link", "real", "real/nested.txt"]
        );
        let link = entries
            .iter()
            .find(|entry| entry.relative_path == Path::new("link"))
            .expect("link entry");
        assert_eq!(link.metadata.kind, EntryKind::Symlink);
    }

    #[test]
    fn rejects_relative_roots() {
        let error = SourceWalker::default()
            .walk(&[PathBuf::from("relative")])
            .expect_err("relative root");

        assert!(matches!(error, CoreError::SourceRootNotAbsolute { .. }));
    }

    #[test]
    fn wildcard_patterns_match_expected_paths() {
        assert!(ExclusionRule::new("**/.git").matches(Path::new("src/.git")));
        assert!(ExclusionRule::new("*.tmp").matches(Path::new("src/cache.tmp")));
        assert!(ExclusionRule::new("node_modules").matches(Path::new("app/node_modules")));
        assert!(!ExclusionRule::new("*.tmp").matches(Path::new("src/cache.txt")));
    }

    #[test]
    fn chunking_config_validates_fastcdc_bounds_and_order() {
        assert!(ChunkingConfig::new(64, 256, 1024).validate().is_ok());
        assert!(matches!(
            ChunkingConfig::new(63, 256, 1024).validate(),
            Err(CoreError::InvalidChunkingConfig { .. })
        ));
        assert!(matches!(
            ChunkingConfig::new(512, 256, 1024).validate(),
            Err(CoreError::InvalidChunkingConfig { .. })
        ));
        assert!(matches!(
            ChunkingConfig::new(64, 2048, 1024).validate(),
            Err(CoreError::InvalidChunkingConfig { .. })
        ));
    }

    #[test]
    fn content_chunker_returns_deterministic_ranges_covering_input() {
        let config = ChunkingConfig::new(64, 256, 1024);
        let chunker = ContentChunker::new(config).expect("valid chunker");
        let bytes = (0..16_384)
            .map(|index| ((index * 31 + index / 7) % 251) as u8)
            .collect::<Vec<_>>();

        let first = chunker.chunk_bytes(&bytes);
        let second = chunker.chunk_bytes(&bytes);

        assert_eq!(first, second);
        assert!(!first.is_empty());
        assert_eq!(first.first().expect("first chunk").offset, 0);

        let mut cursor = 0_u64;
        for chunk in &first {
            assert_eq!(chunk.offset, cursor);
            assert!(chunk.length > 0);
            assert!(chunk.length <= config.max_size as u64);
            cursor += chunk.length;
        }
        assert_eq!(cursor, bytes.len() as u64);
    }

    #[test]
    fn content_chunker_keeps_small_inputs_as_one_chunk() {
        let chunker =
            ContentChunker::new(ChunkingConfig::new(64, 256, 1024)).expect("valid chunker");

        assert_eq!(chunker.chunk_bytes(&[]), Vec::new());
        assert_eq!(
            chunker.chunk_bytes(b"short"),
            vec![ContentChunk {
                offset: 0,
                length: 5,
                gear_hash: 0,
            }]
        );
    }

    #[test]
    fn backup_pipeline_rejects_empty_repository_context() {
        let error = BackupPipeline::new(BackupPipelineConfig {
            chunking: ChunkingConfig::new(64, 256, 1024),
            compression_level: DEFAULT_ZSTD_COMPRESSION_LEVEL,
            repository_id: String::new(),
        })
        .expect_err("empty repository id");

        assert!(matches!(
            error,
            CoreError::InvalidBackupPipelineConfig { .. }
        ));
    }

    #[tokio::test]
    async fn backup_pipeline_writes_encrypted_chunks_index_and_manifest() {
        use fileferry_storage::ObjectKeyPrefix;
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("one.txt"), b"same content").expect("write one");
        fs::write(temp.path().join("two.txt"), b"same content").expect("write two");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();

        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["laptop".to_owned()],
                },
            )
            .await
            .expect("snapshot write");

        assert_eq!(result.entries, 3);
        assert_eq!(result.entries_scanned, 3);
        assert_eq!(result.files_backed_up, 2);
        assert_eq!(result.directories_backed_up, 1);
        assert_eq!(result.symlinks_backed_up, 0);
        assert_eq!(result.special_entries_seen, 0);
        assert_eq!(result.bytes_scanned, 24);
        assert_eq!(result.chunks_seen, 2);
        assert_eq!(result.chunks, 1);
        assert_eq!(result.chunk_objects_written, 1);
        assert_eq!(result.chunk_objects_reused, 1);
        assert_eq!(result.chunks_written, 1);
        assert_eq!(result.chunks_reused, 1);
        assert_eq!(result.index_ids.len(), 1);
        assert!(result.bytes_uploaded > 0);
        assert_eq!(store.object_count().await, 4);

        let keys = store
            .list_prefix(&ObjectKeyPrefix::root())
            .await
            .expect("list objects");
        let rendered_keys = keys
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered_keys.contains("objects/chunk/"));
        assert!(rendered_keys.contains("objects/index/"));
        assert!(rendered_keys.contains("objects/manifest/"));
        assert!(rendered_keys.contains("commits/"));
        assert!(!rendered_keys.contains("one.txt"));
        assert!(!rendered_keys.contains("two.txt"));

        let manifest_bytes = store.get(&result.manifest_object).await.expect("manifest");
        let rendered_manifest = String::from_utf8_lossy(&manifest_bytes);
        assert!(!rendered_manifest.contains("one.txt"));
        assert!(!rendered_manifest.contains("two.txt"));
        assert!(!rendered_manifest.contains("laptop"));
    }

    #[tokio::test]
    async fn backup_pipeline_reads_back_authenticated_manifest_and_index() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();

        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["verified".to_owned()],
                },
            )
            .await
            .expect("snapshot write");

        let manifest = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect("manifest read");
        let index = pipeline
            .read_chunk_index(&store, &master_key, &manifest.body.index_ids[0])
            .await
            .expect("index read");

        assert_eq!(manifest.snapshot_id, result.snapshot_id);
        assert_eq!(manifest.body.tags, vec!["verified"]);
        assert_eq!(index.index_id, manifest.body.index_ids[0]);
        assert_eq!(index.chunks.len(), result.chunks);
    }

    #[tokio::test]
    async fn check_repository_verifies_commits_metadata_indexes_and_chunks() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("one.txt"), b"same content").expect("write one");
        fs::write(temp.path().join("two.txt"), b"same content").expect("write two");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let checked = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect("check repository");

        assert_eq!(checked.repository_id, pipeline.config().repository_id);
        assert_eq!(checked.metadata_objects_checked, 3);
        assert_eq!(checked.chunk_objects_checked, 1);
        assert!(checked.bytes_read > 0);
        assert_eq!(checked.read_data_mode, CheckReadDataMode::Full);
        assert_eq!(checked.read_data_subset, None);
        assert!(checked.errors.is_empty());
        assert!(checked.warnings.is_empty());
    }

    #[tokio::test]
    async fn check_repository_supports_count_and_percent_subsets() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.bin"), varied_bytes(1, 12_000)).expect("write a");
        fs::write(temp.path().join("b.bin"), varied_bytes(2, 16_000)).expect("write b");
        fs::write(temp.path().join("c.bin"), varied_bytes(3, 20_000)).expect("write c");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let written = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        assert!(written.chunks >= 3);

        let count_subset = CheckReadDataSubset::count(2).expect("count subset");
        let count_checked = pipeline
            .check_repository_with_options(
                &store,
                &master_key,
                CheckRepositoryOptions::subset(count_subset),
            )
            .await
            .expect("count subset check");
        assert_eq!(count_checked.chunk_objects_checked, 2);
        assert_eq!(count_checked.read_data_mode, CheckReadDataMode::Subset);
        assert_eq!(count_checked.read_data_subset, Some("2".to_owned()));

        let percent_subset = CheckReadDataSubset::percent(50).expect("percent subset");
        let percent_checked = pipeline
            .check_repository_with_options(
                &store,
                &master_key,
                CheckRepositoryOptions::subset(percent_subset),
            )
            .await
            .expect("percent subset check");
        assert_eq!(
            percent_checked.chunk_objects_checked,
            percent_subset.selected_count(written.chunks)
        );
        assert_eq!(percent_checked.read_data_mode, CheckReadDataMode::Subset);
        assert_eq!(percent_checked.read_data_subset, Some("50%".to_owned()));
    }

    #[tokio::test]
    async fn check_repository_subset_selection_is_deterministic() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("small.bin"), varied_bytes(4, 4_000)).expect("write small");
        fs::write(temp.path().join("medium.bin"), varied_bytes(5, 14_000)).expect("write medium");
        fs::write(temp.path().join("large.bin"), varied_bytes(6, 28_000)).expect("write large");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let options = CheckRepositoryOptions::subset(
            CheckReadDataSubset::percent(50).expect("percent subset"),
        );
        let normal = pipeline
            .check_repository_with_options(&store, &master_key, options)
            .await
            .expect("normal listing subset check");
        let reverse_store = ReverseListingStore { inner: &store };
        let reversed = pipeline
            .check_repository_with_options(&reverse_store, &master_key, options)
            .await
            .expect("reversed listing subset check");

        assert_eq!(normal.chunk_objects_checked, reversed.chunk_objects_checked);
        assert_eq!(normal.bytes_read, reversed.bytes_read);
        assert_eq!(normal.read_data_mode, reversed.read_data_mode);
        assert_eq!(normal.read_data_subset, reversed.read_data_subset);
    }

    #[tokio::test]
    async fn check_repository_subset_fails_closed_for_selected_chunk_integrity_failure() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.bin"), varied_bytes(7, 12_000)).expect("write a");
        fs::write(temp.path().join("b.bin"), varied_bytes(8, 16_000)).expect("write b");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let chunk_prefix = ObjectKeyPrefix::new("objects/chunk").expect("chunk prefix");
        let mut chunk_keys = store.list_prefix(&chunk_prefix).await.expect("list chunks");
        chunk_keys.sort();
        let selected_key = chunk_keys.into_iter().next().expect("selected chunk");
        let mut selected_bytes = store.get(&selected_key).await.expect("chunk bytes");
        selected_bytes[0] ^= 0x01;
        store
            .overwrite_for_tests(selected_key, selected_bytes)
            .await;

        let error = pipeline
            .check_repository_with_options(
                &store,
                &master_key,
                CheckRepositoryOptions::subset(
                    CheckReadDataSubset::count(1).expect("count subset"),
                ),
            )
            .await
            .expect_err("tampered selected chunk should fail");
        assert!(matches!(
            error,
            CoreError::ObjectDecode { .. } | CoreError::ObjectAuthentication { .. }
        ));
    }

    #[tokio::test]
    async fn check_repository_fails_closed_for_missing_or_tampered_chunks() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let chunk_prefix = ObjectKeyPrefix::new("objects/chunk").expect("chunk prefix");
        let chunk_key = store
            .list_prefix(&chunk_prefix)
            .await
            .expect("list chunks")
            .into_iter()
            .next()
            .expect("chunk key");
        let chunk_bytes = store.get(&chunk_key).await.expect("chunk bytes");

        store.delete(&chunk_key).await.expect("delete chunk");
        let missing = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect_err("missing chunk should fail");
        assert!(matches!(
            missing,
            CoreError::RepositoryCheckMissingObject { .. }
        ));

        let mut tampered = chunk_bytes;
        tampered[0] ^= 0x01;
        store.overwrite_for_tests(chunk_key, tampered).await;
        let corrupted = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect_err("tampered chunk should fail");
        assert!(matches!(
            corrupted,
            CoreError::ObjectDecode { .. } | CoreError::ObjectAuthentication { .. }
        ));
    }

    #[tokio::test]
    async fn check_repository_reports_manifest_index_mismatch_with_entry_context() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let mut manifest = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect("manifest read");
        let file_entry = manifest
            .body
            .entries
            .iter()
            .find(|entry| entry.relative_path == Path::new("sample.txt"))
            .expect("file entry");
        let referenced_chunk = file_entry.chunks[0].clone();

        let repository_context = pipeline.config().repository_id.as_bytes();
        let index_key = master_key
            .derive_subkey(KeyPurpose::Index, repository_context)
            .expect("index key");
        let empty_index_id = content_id_for_metadata(
            &master_key,
            KeyPurpose::Index,
            repository_context,
            &Vec::<ChunkIndexEntry>::new(),
        )
        .expect("empty index id");
        let empty_index = ChunkIndex {
            schema_version: 0,
            index_id: empty_index_id.clone(),
            chunks: Vec::new(),
        };
        let empty_index_object =
            object_key_for_id("objects/index", &empty_index_id).expect("empty index object key");
        let empty_index_bytes = encrypt_repository_object(
            &index_key,
            ObjectKind::Index,
            &empty_index_object,
            &serde_json::to_vec(&empty_index).expect("empty index json"),
        )
        .expect("encrypted empty index");
        store
            .overwrite_for_tests(empty_index_object, empty_index_bytes)
            .await;

        manifest.body.index_ids = vec![empty_index_id];
        let new_snapshot_id = content_id_for_metadata(
            &master_key,
            KeyPurpose::SnapshotMetadata,
            repository_context,
            &manifest.body,
        )
        .expect("new snapshot id");
        manifest.snapshot_id = new_snapshot_id.clone();
        let manifest_object =
            object_key_for_id("objects/manifest", &new_snapshot_id).expect("manifest object key");
        let manifest_key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, repository_context)
            .expect("manifest key");
        let manifest_bytes = encrypt_repository_object(
            &manifest_key,
            ObjectKind::SnapshotManifest,
            &manifest_object,
            &serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("encrypted manifest");
        store
            .overwrite_for_tests(manifest_object.clone(), manifest_bytes)
            .await;

        store
            .delete(&result.commit_object)
            .await
            .expect("delete old commit");
        let commit_object = object_key_for_commit(&new_snapshot_id).expect("new commit object key");
        let commit = SnapshotCommit {
            schema_version: 0,
            snapshot_id: new_snapshot_id.clone(),
            manifest_object: manifest_object.as_str().to_owned(),
        };
        store
            .overwrite_for_tests(
                commit_object,
                serde_json::to_vec(&commit).expect("commit json"),
            )
            .await;

        let error = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect_err("manifest/index mismatch should fail check");

        assert!(matches!(
            error,
            CoreError::MissingChunkIndexEntry {
                snapshot_id,
                path,
                chunk_id,
                object_key,
            } if snapshot_id == new_snapshot_id
                && path == Path::new("sample.txt")
                && chunk_id == referenced_chunk.chunk_id
                && object_key == referenced_chunk.object_key
        ));
    }

    #[tokio::test]
    async fn check_repository_reports_chunk_decompression_with_entry_context() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let manifest = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect("manifest read");
        let file_entry = manifest
            .body
            .entries
            .iter()
            .find(|entry| entry.relative_path == Path::new("sample.txt"))
            .expect("file entry");
        let referenced_chunk = file_entry.chunks[0].clone();
        let chunk_object = ObjectKey::new(referenced_chunk.object_key.clone())
            .expect("referenced chunk object key");
        let chunk_key = master_key
            .derive_subkey(
                KeyPurpose::ChunkData,
                pipeline.config().repository_id.as_bytes(),
            )
            .expect("chunk key");
        let invalid_compressed_chunk = encrypt_repository_object(
            &chunk_key,
            ObjectKind::Chunk,
            &chunk_object,
            b"not a zstd frame",
        )
        .expect("encrypted invalid compressed chunk");
        store
            .overwrite_for_tests(chunk_object, invalid_compressed_chunk)
            .await;

        let error = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect_err("invalid compressed chunk should fail check");

        assert!(matches!(
            error,
            CoreError::Decompression {
                chunk_id,
                snapshot_id: Some(snapshot_id),
                path: Some(path),
                object_key: Some(object_key),
                ..
            } if chunk_id == referenced_chunk.chunk_id
                && snapshot_id == result.snapshot_id
                && path == Path::new("sample.txt")
                && object_key == referenced_chunk.object_key
        ));
    }

    #[tokio::test]
    async fn check_repository_rejects_invalid_manifest_entry_paths_with_context() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("one.txt"), b"one").expect("write one");
        fs::write(temp.path().join("two.txt"), b"two").expect("write two");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let (snapshot_id, manifest_object) = replace_committed_manifest_for_tests(
            &pipeline,
            &store,
            &master_key,
            &result,
            |manifest| {
                let two = manifest
                    .body
                    .entries
                    .iter_mut()
                    .find(|entry| entry.relative_path == Path::new("two.txt"))
                    .expect("second entry");
                two.relative_path = PathBuf::from("one.txt");
            },
        )
        .await;

        let error = pipeline
            .check_repository(&store, &master_key)
            .await
            .expect_err("duplicate manifest path should fail check");

        assert!(matches!(
            error,
            CoreError::InvalidSnapshotManifest {
                snapshot_id: error_snapshot_id,
                object_key,
                path: Some(path),
                reason: "duplicate entry path",
            } if error_snapshot_id == snapshot_id
                && object_key == manifest_object
                && path == Path::new("one.txt")
        ));
    }

    #[tokio::test]
    async fn backup_pipeline_publishes_commit_markers_and_lists_committed_manifests() {
        use fileferry_testkit::FakeObjectStore;

        let first_source = tempfile::tempdir().expect("first tempdir");
        fs::write(first_source.path().join("first.txt"), b"first").expect("write first");
        let second_source = tempfile::tempdir().expect("second tempdir");
        fs::write(second_source.path().join("second.txt"), b"second").expect("write second");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let first = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![first_source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["first".to_owned()],
                },
            )
            .await
            .expect("first snapshot write");
        let second = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![second_source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["second".to_owned()],
                },
            )
            .await
            .expect("second snapshot write");

        assert!(first.commit_object.as_str().starts_with("commits/"));
        assert!(second.commit_object.as_str().starts_with("commits/"));

        let manifests = pipeline
            .read_committed_snapshot_manifests(&store, &master_key)
            .await
            .expect("committed manifests");
        let mut ids = manifests
            .iter()
            .map(|manifest| manifest.snapshot_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        let mut expected = vec![first.snapshot_id, second.snapshot_id];
        expected.sort();

        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn forget_markers_hide_snapshots_without_deleting_repository_objects() {
        use fileferry_testkit::FakeObjectStore;

        let first_source = tempfile::tempdir().expect("first tempdir");
        let second_source = tempfile::tempdir().expect("second tempdir");
        fs::write(first_source.path().join("first.txt"), b"first").expect("write first");
        fs::write(second_source.path().join("second.txt"), b"second").expect("write second");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let first = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![first_source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["first".to_owned()],
                },
            )
            .await
            .expect("first snapshot write");
        let second = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![second_source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: vec!["second".to_owned()],
                },
            )
            .await
            .expect("second snapshot write");
        let commit_prefix = ObjectKeyPrefix::new("commits").expect("commit prefix");
        let manifest_prefix = ObjectKeyPrefix::new("objects/manifest").expect("manifest prefix");
        let commits_before = store
            .list_prefix(&commit_prefix)
            .await
            .expect("commits before");
        let manifests_before = store
            .list_prefix(&manifest_prefix)
            .await
            .expect("manifests before");

        let writes = pipeline
            .write_snapshot_forget_markers(&store, std::slice::from_ref(&first.snapshot_id))
            .await
            .expect("forget markers");

        assert_eq!(writes.markers.len(), 1);
        assert_eq!(writes.markers[0].snapshot_id, first.snapshot_id);
        assert!(writes.markers[0].created);
        assert!(writes.markers[0].marker_object.starts_with("forgets/"));

        let manifests = pipeline
            .read_committed_snapshot_manifests(&store, &master_key)
            .await
            .expect("committed manifests after forget");
        assert_eq!(
            manifests
                .iter()
                .map(|manifest| manifest.snapshot_id.as_str())
                .collect::<Vec<_>>(),
            [second.snapshot_id.as_str()]
        );
        assert_eq!(
            store
                .list_prefix(&commit_prefix)
                .await
                .expect("commits after"),
            commits_before
        );
        assert_eq!(
            store
                .list_prefix(&manifest_prefix)
                .await
                .expect("manifests after"),
            manifests_before
        );
    }

    #[test]
    fn snapshot_selection_supports_id_tag_and_latest() {
        let first = test_manifest("snap-a", 10, &["work"]);
        let second = test_manifest("snap-b", 20, &["home"]);
        let third = test_manifest("snap-c", 30, &["work"]);
        let manifests = vec![first, second, third];

        assert_eq!(
            select_snapshot(&manifests, &SnapshotSelection::Id("snap-b".to_owned()))
                .expect("select id")
                .snapshot_id,
            "snap-b"
        );
        assert_eq!(
            select_snapshot(&manifests, &SnapshotSelection::Tag("work".to_owned()))
                .expect("select tag")
                .snapshot_id,
            "snap-c"
        );
        assert_eq!(
            select_snapshot(&manifests, &SnapshotSelection::Latest)
                .expect("select latest")
                .snapshot_id,
            "snap-c"
        );
        assert!(matches!(
            select_snapshot(&manifests, &SnapshotSelection::Tag("missing".to_owned())),
            Err(CoreError::SnapshotNotFound { .. })
        ));
    }

    #[test]
    fn snapshot_summaries_are_newest_first_and_count_sources_and_entries() {
        let mut first = test_manifest("snap-a", 10, &["work"]);
        first.body.entries = vec![
            test_manifest_entry("", EntryKind::Directory, None),
            test_manifest_entry("docs", EntryKind::Directory, None),
            test_manifest_entry("docs/a.txt", EntryKind::RegularFile, Some(1)),
        ];
        let mut second = test_manifest("snap-b", 20, &["home"]);
        second.body.entries = vec![
            test_manifest_entry("", EntryKind::Directory, None),
            test_manifest_entry("b.txt", EntryKind::RegularFile, Some(1)),
        ];

        let summaries = snapshot_summaries(&[first, second]);

        assert_eq!(
            summaries
                .iter()
                .map(|summary| summary.snapshot_id.as_str())
                .collect::<Vec<_>>(),
            vec!["snap-b", "snap-a"]
        );
        assert_eq!(summaries[0].source_count, 1);
        assert_eq!(summaries[0].entry_count, 2);
        assert_eq!(summaries[0].tags, vec!["home"]);
    }

    #[test]
    fn list_snapshot_entries_returns_immediate_children_or_exact_file() {
        let mut manifest = test_manifest("snap-a", 10, &["work"]);
        manifest.body.entries = vec![
            test_manifest_entry("", EntryKind::Directory, None),
            test_manifest_entry("docs", EntryKind::Directory, None),
            test_manifest_entry("docs/a.txt", EntryKind::RegularFile, Some(1)),
            test_manifest_entry("docs/nested", EntryKind::Directory, None),
            test_manifest_entry("docs/nested/b.txt", EntryKind::RegularFile, Some(1)),
        ];

        let root = list_snapshot_entries(&manifest, "").expect("root listing");
        assert_eq!(
            root.entries
                .iter()
                .map(|entry| entry.relative_path.as_path())
                .collect::<Vec<_>>(),
            vec![Path::new("docs")]
        );

        let docs = list_snapshot_entries(&manifest, "docs").expect("docs listing");
        assert_eq!(
            docs.entries
                .iter()
                .map(|entry| entry.relative_path.as_path())
                .collect::<Vec<_>>(),
            vec![Path::new("docs/a.txt"), Path::new("docs/nested")]
        );

        let file = list_snapshot_entries(&manifest, "docs/a.txt").expect("file listing");
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].relative_path, PathBuf::from("docs/a.txt"));
        assert_eq!(file.entries[0].kind, EntryKind::RegularFile);
    }

    #[test]
    fn list_snapshot_entries_rejects_unsafe_or_missing_paths() {
        let manifest = test_manifest("snap-a", 10, &[]);

        assert!(matches!(
            list_snapshot_entries(&manifest, "../outside"),
            Err(CoreError::InvalidRestoreRequest { .. })
        ));
        assert!(matches!(
            list_snapshot_entries(&manifest, "missing"),
            Err(CoreError::SnapshotPathNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn restore_snapshot_contents_filters_paths_and_reassembles_files() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("docs")).expect("create docs");
        fs::create_dir(temp.path().join("logs")).expect("create logs");
        fs::write(temp.path().join("docs/one.txt"), b"one").expect("write one");
        fs::write(temp.path().join("docs/two.txt"), b"two").expect("write two");
        fs::write(temp.path().join("logs/skip.txt"), b"skip").expect("write skip");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_contents(
                &store,
                &master_key,
                RestoreContentRequest {
                    snapshot_id: result.snapshot_id.clone(),
                    paths: vec![PathBuf::from("docs")],
                },
            )
            .await
            .expect("restore contents");

        assert_eq!(restored.snapshot_id, result.snapshot_id);
        assert_eq!(
            restored
                .files
                .iter()
                .map(|file| (file.relative_path.clone(), file.contents.clone()))
                .collect::<Vec<_>>(),
            vec![
                (PathBuf::from("docs/one.txt"), b"one".to_vec()),
                (PathBuf::from("docs/two.txt"), b"two".to_vec()),
            ]
        );
        assert!(restored.selected_entries >= restored.files.len());
    }

    #[tokio::test]
    async fn restore_snapshot_contents_rejects_missing_requested_paths() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("docs")).expect("create docs");
        fs::write(temp.path().join("docs/one.txt"), b"one").expect("write one");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let error = pipeline
            .restore_snapshot_contents(
                &store,
                &master_key,
                RestoreContentRequest {
                    snapshot_id: result.snapshot_id.clone(),
                    paths: vec![PathBuf::from("docs"), PathBuf::from("missing.txt")],
                },
            )
            .await
            .expect_err("missing restore path should fail");

        assert!(matches!(
            error,
            CoreError::SnapshotPathNotFound { snapshot_id, path }
                if snapshot_id == result.snapshot_id && path == Path::new("missing.txt")
        ));
    }

    #[tokio::test]
    async fn restore_snapshot_contents_rejects_unsafe_restore_paths() {
        use fileferry_testkit::FakeObjectStore;

        let pipeline = small_test_pipeline();
        let error = pipeline
            .restore_snapshot_contents(
                &FakeObjectStore::new(),
                &MasterKey::generate(),
                RestoreContentRequest {
                    snapshot_id: "snapshot".to_owned(),
                    paths: vec![PathBuf::from("../outside")],
                },
            )
            .await
            .expect_err("unsafe restore path");

        assert!(matches!(error, CoreError::InvalidRestoreRequest { .. }));
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_writes_and_verifies_files() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        fs::create_dir(source.path().join("docs")).expect("create docs");
        fs::write(source.path().join("docs/one.txt"), b"one").expect("write one");
        fs::write(source.path().join("docs/two.txt"), b"two").expect("write two");
        fs::write(source.path().join("skip.txt"), b"skip").expect("write skip");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id.clone(),
                    paths: vec![PathBuf::from("docs")],
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect("destination restore");

        assert_eq!(restored.snapshot_id, result.snapshot_id);
        assert_eq!(restored.files.len(), 2);
        assert_eq!(restored.metadata_planned, 3);
        assert_eq!(restored.bytes, 6);
        assert_eq!(restored.verified_files, 2);
        assert!(restored.files.iter().all(|file| file.verified));
        assert_eq!(
            fs::read(destination.path().join("docs/one.txt")).expect("restored one"),
            b"one"
        );
        assert_eq!(
            fs::read(destination.path().join("docs/two.txt")).expect("restored two"),
            b"two"
        );
        assert!(!destination.path().join("skip.txt").exists());
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_applies_file_and_directory_modified_timestamps() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        let docs = source.path().join("docs");
        let file = docs.join("one.txt");
        fs::create_dir(&docs).expect("create docs");
        fs::write(&file, b"one").expect("write one");

        let expected = Timestamp {
            seconds: 1_700_000_000,
            nanoseconds: 0,
        };
        let expected_time = system_time_from_timestamp(expected).expect("expected system time");
        set_restored_modified_timestamp(&file, RestoredMetadataTarget::RegularFile, expected_time)
            .expect("set source file mtime");
        set_restored_modified_timestamp(&docs, RestoredMetadataTarget::Directory, expected_time)
            .expect("set source directory mtime");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: vec![PathBuf::from("docs")],
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect("destination restore");

        assert_eq!(restored.metadata_applied, 2);
        assert_eq!(restored.metadata_planned, 2);
        assert_eq!(restored.metadata_warnings, Vec::new());
        assert_eq!(
            capture_metadata(destination.path().join("docs"))
                .expect("restored directory metadata")
                .modified,
            MetadataValue::Captured(expected)
        );
        assert_eq!(
            capture_metadata(destination.path().join("docs/one.txt"))
                .expect("restored file metadata")
                .modified,
            MetadataValue::Captured(expected)
        );
    }

    #[test]
    fn apply_restored_modified_timestamp_records_warning_when_not_captured() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sample.txt");
        fs::write(&path, b"sample").expect("write sample");
        let mut warnings = Vec::new();

        let applied = apply_restored_modified_timestamp(
            &path,
            Path::new("sample.txt"),
            &MetadataValue::Unsupported,
            RestoredMetadataTarget::RegularFile,
            &mut warnings,
        );

        assert_eq!(applied, 0);
        assert_eq!(
            warnings,
            vec![RestoreMetadataWarning {
                relative_path: PathBuf::from("sample.txt"),
                field: "modified",
                reason: "modified timestamp was not captured".to_owned(),
            }]
        );
    }

    #[test]
    fn plan_restored_modified_timestamp_reports_denied_and_invalid_values() {
        let mut warnings = Vec::new();

        plan_restored_modified_timestamp(
            Path::new("denied.txt"),
            &MetadataValue::Denied("permission denied".to_owned()),
            &mut warnings,
        );
        plan_restored_modified_timestamp(
            Path::new("invalid.txt"),
            &MetadataValue::Captured(Timestamp {
                seconds: 0,
                nanoseconds: 1_000_000_000,
            }),
            &mut warnings,
        );

        assert_eq!(
            warnings,
            vec![
                RestoreMetadataWarning {
                    relative_path: PathBuf::from("denied.txt"),
                    field: "modified",
                    reason: "modified timestamp was denied during backup: permission denied"
                        .to_owned(),
                },
                RestoreMetadataWarning {
                    relative_path: PathBuf::from("invalid.txt"),
                    field: "modified",
                    reason: "modified timestamp is outside the supported system time range"
                        .to_owned(),
                },
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_snapshot_to_destination_writes_directories_and_symlinks() {
        use fileferry_testkit::FakeObjectStore;
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        let restore_root = destination.path().join("restored");
        fs::create_dir_all(source.path().join("empty/nested")).expect("create empty tree");
        fs::write(source.path().join("target.txt"), b"target").expect("write target");
        symlink("target.txt", source.path().join("target.link")).expect("create symlink");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: Vec::new(),
                    destination: restore_root.clone(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect("destination restore");

        assert_eq!(restored.directories.len(), 3);
        assert_eq!(restored.files.len(), 1);
        assert_eq!(restored.symlinks.len(), 1);
        assert!(restore_root.join("empty/nested").is_dir());
        assert_eq!(
            fs::read(restore_root.join("target.txt")).expect("restored target"),
            b"target"
        );
        assert_eq!(
            fs::read_link(restore_root.join("target.link")).expect("restored symlink"),
            PathBuf::from("target.txt")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_snapshot_to_destination_path_scoped_symlink_creates_missing_parent_directory()
    {
        use fileferry_testkit::FakeObjectStore;
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        let restore_root = destination.path().join("restored");
        fs::create_dir_all(source.path().join("links")).expect("create links dir");
        fs::write(source.path().join("target.txt"), b"target").expect("write target");
        symlink("../target.txt", source.path().join("links/target.link"))
            .expect("create nested symlink");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: vec![PathBuf::from("links/target.link")],
                    destination: restore_root.clone(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect("path-scoped symlink restore");

        assert_eq!(restored.selected_entries, 1);
        assert_eq!(restored.directories.len(), 0);
        assert_eq!(restored.files.len(), 0);
        assert_eq!(restored.symlinks.len(), 1);
        assert!(restore_root.join("links").is_dir());
        assert_eq!(
            fs::read_link(restore_root.join("links/target.link")).expect("restored symlink"),
            PathBuf::from("../target.txt")
        );
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_dry_run_reports_without_writes() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        fs::write(source.path().join("sample.txt"), b"sample").expect("write sample");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let restored = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: Vec::new(),
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: true,
                    verify: true,
                },
            )
            .await
            .expect("dry-run restore");

        assert!(restored.dry_run);
        assert_eq!(restored.verified_files, 0);
        assert_eq!(restored.metadata_planned, 2);
        assert_eq!(restored.metadata_applied, 0);
        assert_eq!(restored.metadata_warnings, Vec::new());
        assert_eq!(restored.files.len(), 1);
        assert_eq!(
            restored.files[0].action,
            RestoreDestinationAction::WouldWrite
        );
        assert_eq!(restored.files[0].bytes, 6);
        assert!(!destination.path().join("sample.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_snapshot_to_destination_rejects_existing_symlink_paths() {
        use fileferry_testkit::FakeObjectStore;
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        fs::write(source.path().join("target.txt"), b"target").expect("write target");
        symlink("target.txt", source.path().join("target.link")).expect("create source symlink");
        symlink(outside.path(), destination.path().join("target.link"))
            .expect("create destination symlink");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let error = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: vec![PathBuf::from("target.link")],
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::OverwriteFiles,
                    dry_run: false,
                    verify: false,
                },
            )
            .await
            .expect_err("existing symlink path should block restore");

        assert!(matches!(error, CoreError::RestoreDestinationSymlink { .. }));
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_enforces_overwrite_policy() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        fs::write(source.path().join("sample.txt"), b"new").expect("write sample");
        fs::write(destination.path().join("sample.txt"), b"old").expect("write existing");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let blocked = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id.clone(),
                    paths: Vec::new(),
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: false,
                },
            )
            .await
            .expect_err("existing destination should block restore");
        assert!(matches!(
            blocked,
            CoreError::RestoreDestinationExists { .. }
        ));
        assert_eq!(
            fs::read(destination.path().join("sample.txt")).expect("existing file"),
            b"old"
        );

        let overwritten = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: Vec::new(),
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::OverwriteFiles,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect("overwrite restore");
        assert_eq!(overwritten.files.len(), 1);
        assert_eq!(
            fs::read(destination.path().join("sample.txt")).expect("overwritten file"),
            b"new"
        );
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_preflights_conflicts_before_writes() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        fs::create_dir(source.path().join("early")).expect("create early directory");
        fs::write(source.path().join("conflict.txt"), b"new").expect("write source conflict");
        fs::write(destination.path().join("conflict.txt"), b"old")
            .expect("write destination conflict");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let blocked = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: Vec::new(),
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: false,
                },
            )
            .await
            .expect_err("existing destination should block restore");

        assert!(matches!(
            blocked,
            CoreError::RestoreDestinationExists { .. }
        ));
        assert!(!destination.path().join("early").exists());
        assert_eq!(
            fs::read(destination.path().join("conflict.txt")).expect("existing file"),
            b"old"
        );
    }

    #[tokio::test]
    async fn restore_snapshot_to_destination_rejects_invalid_manifest_topology_before_writes() {
        use fileferry_testkit::FakeObjectStore;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        fs::write(source.path().join("file.txt"), b"file").expect("write file");
        fs::write(source.path().join("child.txt"), b"child").expect("write child");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let (snapshot_id, manifest_object) = replace_committed_manifest_for_tests(
            &pipeline,
            &store,
            &master_key,
            &result,
            |manifest| {
                let child = manifest
                    .body
                    .entries
                    .iter_mut()
                    .find(|entry| entry.relative_path == Path::new("child.txt"))
                    .expect("child entry");
                child.relative_path = PathBuf::from("file.txt/child.txt");
            },
        )
        .await;

        let error = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: snapshot_id.clone(),
                    paths: Vec::new(),
                    destination: destination.path().join("restore"),
                    overwrite: RestoreOverwritePolicy::FailIfExists,
                    dry_run: false,
                    verify: true,
                },
            )
            .await
            .expect_err("invalid manifest topology should block restore");

        assert!(matches!(
            error,
            CoreError::InvalidSnapshotManifest {
                snapshot_id: error_snapshot_id,
                object_key,
                path: Some(path),
                reason: "entry has a non-directory ancestor",
            } if error_snapshot_id == snapshot_id
                && object_key == manifest_object
                && path == Path::new("file.txt/child.txt")
        ));
        assert!(!destination.path().join("restore").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_snapshot_to_destination_rejects_symlinked_destination_ancestors() {
        use fileferry_testkit::FakeObjectStore;
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source tempdir");
        let destination = tempfile::tempdir().expect("destination tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        fs::create_dir(source.path().join("docs")).expect("create docs");
        fs::write(source.path().join("docs/sample.txt"), b"sample").expect("write sample");
        symlink(outside.path(), destination.path().join("docs")).expect("symlink ancestor");

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![source.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let error = pipeline
            .restore_snapshot_to_destination(
                &store,
                &master_key,
                RestoreDestinationRequest {
                    snapshot_id: result.snapshot_id,
                    paths: vec![PathBuf::from("docs")],
                    destination: destination.path().to_path_buf(),
                    overwrite: RestoreOverwritePolicy::OverwriteFiles,
                    dry_run: false,
                    verify: false,
                },
            )
            .await
            .expect_err("symlink ancestor should block restore");

        assert!(matches!(error, CoreError::RestoreDestinationSymlink { .. }));
        assert!(!outside.path().join("sample.txt").exists());
    }

    #[tokio::test]
    async fn repository_object_reads_fail_closed_for_wrong_key_bit_flips_truncation_and_swaps() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let wrong_master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");

        let wrong_key_error = pipeline
            .read_snapshot_manifest(&store, &wrong_master_key, &result.snapshot_id)
            .await
            .expect_err("wrong repository key must fail");
        assert!(matches!(
            wrong_key_error,
            CoreError::ObjectAuthentication {
                key,
                source: CryptoError::Decryption
            } if key.as_str() == result.manifest_object.as_str()
        ));

        let manifest_bytes = store
            .get(&result.manifest_object)
            .await
            .expect("manifest bytes");
        let mut corrupted: StoredEncryptedObject =
            serde_json::from_slice(&manifest_bytes).expect("manifest frame");
        corrupted.ciphertext[0] ^= 0x01;
        store
            .overwrite_for_tests(
                result.manifest_object.clone(),
                serde_json::to_vec(&corrupted).expect("corrupted frame"),
            )
            .await;
        let bit_flip_error = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect_err("bit flip must fail");
        assert!(matches!(
            bit_flip_error,
            CoreError::ObjectAuthentication {
                key,
                source: CryptoError::Decryption
            } if key.as_str() == result.manifest_object.as_str()
        ));

        let mut truncated: StoredEncryptedObject =
            serde_json::from_slice(&manifest_bytes).expect("manifest frame");
        truncated
            .ciphertext
            .truncate(truncated.ciphertext.len() / 2);
        store
            .overwrite_for_tests(
                result.manifest_object.clone(),
                serde_json::to_vec(&truncated).expect("truncated frame"),
            )
            .await;
        let truncated_error = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect_err("truncation must fail");
        assert!(matches!(
            truncated_error,
            CoreError::ObjectAuthentication {
                key,
                source: CryptoError::Decryption
            } if key.as_str() == result.manifest_object.as_str()
        ));

        let index_bytes = store.get(&result.index_object).await.expect("index bytes");
        store
            .overwrite_for_tests(result.manifest_object.clone(), index_bytes)
            .await;
        let swapped_error = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect_err("swapped object must fail");
        assert!(matches!(
            swapped_error,
            CoreError::ObjectAuthentication {
                key,
                source: CryptoError::Decryption
            } if key.as_str() == result.manifest_object.as_str()
        ));
    }

    #[tokio::test]
    async fn repository_metadata_reads_reject_replayed_indexes_and_malformed_metadata() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("sample.txt"), b"sample content").expect("write sample");
        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let manifest = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect("manifest read");
        let index_id = manifest.body.index_ids[0].clone();
        let mut index = pipeline
            .read_chunk_index(&store, &master_key, &index_id)
            .await
            .expect("index read");
        index.chunks.clear();

        let index_key = master_key
            .derive_subkey(
                KeyPurpose::Index,
                pipeline.config().repository_id.as_bytes(),
            )
            .expect("index key");
        let replayed = encrypt_repository_object(
            &index_key,
            ObjectKind::Index,
            &result.index_object,
            &serde_json::to_vec(&index).expect("index json"),
        )
        .expect("replayed encrypted index");
        store
            .overwrite_for_tests(result.index_object.clone(), replayed)
            .await;
        let replayed_error = pipeline
            .read_chunk_index(&store, &master_key, &index_id)
            .await
            .expect_err("replayed index contents must fail identity check");
        assert!(matches!(
            replayed_error,
            CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                object_key,
                ..
            } if object_key == result.index_object
        ));

        let manifest_key = master_key
            .derive_subkey(
                KeyPurpose::SnapshotMetadata,
                pipeline.config().repository_id.as_bytes(),
            )
            .expect("manifest key");
        let malformed = encrypt_repository_object(
            &manifest_key,
            ObjectKind::SnapshotManifest,
            &result.manifest_object,
            br#"{"schema_version":"not-a-number"}"#,
        )
        .expect("malformed encrypted object");
        store
            .overwrite_for_tests(result.manifest_object.clone(), malformed)
            .await;
        let malformed_error = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect_err("malformed metadata must fail");
        assert!(matches!(malformed_error, CoreError::MetadataDecode { .. }));
    }

    #[tokio::test]
    async fn backup_pipeline_covers_sparse_trees_symlinks_exclusions_large_and_many_files() {
        use fileferry_testkit::FakeObjectStore;

        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("empty/nested")).expect("create empty tree");
        fs::create_dir_all(temp.path().join("excluded/cache")).expect("create excluded tree");
        fs::create_dir_all(temp.path().join("many")).expect("create many tree");
        fs::write(temp.path().join("excluded/cache/skip.txt"), b"skip").expect("write skip");
        for index in 0..32 {
            fs::write(
                temp.path().join(format!("many/file-{index:02}.txt")),
                format!("many small file {index}"),
            )
            .expect("write many file");
        }
        let large = (0..16_384)
            .map(|index| ((index * 17 + index / 3) % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(temp.path().join("large.bin"), large).expect("write large file");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("large.bin", temp.path().join("large.link")).expect("symlink");
        }

        let pipeline = small_test_pipeline();
        let store = FakeObjectStore::new();
        let master_key = MasterKey::generate();
        let result = pipeline
            .write_snapshot(
                &store,
                &master_key,
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: vec![ExclusionRule::new("excluded/")],
                    tags: Vec::new(),
                },
            )
            .await
            .expect("snapshot write");
        let manifest = pipeline
            .read_snapshot_manifest(&store, &master_key, &result.snapshot_id)
            .await
            .expect("manifest read");

        assert!(
            manifest
                .body
                .entries
                .iter()
                .any(|entry| entry.relative_path == Path::new("empty/nested"))
        );
        assert!(
            !manifest
                .body
                .entries
                .iter()
                .any(|entry| entry.relative_path.starts_with("excluded"))
        );
        assert_eq!(
            manifest
                .body
                .entries
                .iter()
                .filter(|entry| {
                    entry.relative_path.parent() == Some(Path::new("many"))
                        && entry
                            .relative_path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("file-"))
                })
                .count(),
            32
        );

        let large_entry = manifest
            .body
            .entries
            .iter()
            .find(|entry| entry.relative_path == Path::new("large.bin"))
            .expect("large entry");
        assert!(large_entry.chunks.len() > 1);

        #[cfg(unix)]
        {
            let symlink_entry = manifest
                .body
                .entries
                .iter()
                .find(|entry| entry.relative_path == Path::new("large.link"))
                .expect("symlink entry");
            assert_eq!(symlink_entry.metadata.kind, EntryKind::Symlink);
            assert!(symlink_entry.chunks.is_empty());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn backup_pipeline_reports_permission_denied_file_reads() {
        use fileferry_testkit::FakeObjectStore;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let locked = temp.path().join("locked.txt");
        fs::write(&locked, b"locked").expect("write locked");
        let original_permissions = fs::metadata(&locked).expect("metadata").permissions();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock file");

        let pipeline = small_test_pipeline();
        let result = pipeline
            .write_snapshot(
                &FakeObjectStore::new(),
                &MasterKey::generate(),
                BackupRequest {
                    roots: vec![temp.path().to_path_buf()],
                    exclusion_rules: Vec::new(),
                    tags: Vec::new(),
                },
            )
            .await;
        fs::set_permissions(&locked, original_permissions).expect("restore permissions");

        let error = result.expect_err("unreadable file should fail backup");
        assert!(matches!(error, CoreError::FileRead { .. }));
    }

    fn test_manifest(
        snapshot_id: &str,
        created_at_unix_seconds: u64,
        tags: &[&str],
    ) -> SnapshotManifest {
        SnapshotManifest {
            schema_version: 0,
            snapshot_id: snapshot_id.to_owned(),
            body: SnapshotManifestBody {
                created_at_unix_seconds,
                tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
                entries: Vec::new(),
                index_ids: Vec::new(),
            },
        }
    }

    fn test_manifest_entry(
        relative_path: &str,
        kind: EntryKind,
        size_bytes: Option<u64>,
    ) -> ManifestEntry {
        ManifestEntry {
            root: PathBuf::from("/source"),
            path: PathBuf::from("/source").join(relative_path),
            relative_path: PathBuf::from(relative_path),
            metadata: EntryMetadata {
                kind,
                size_bytes,
                modified: MetadataValue::Captured(Timestamp {
                    seconds: 1,
                    nanoseconds: 0,
                }),
                created: MetadataValue::Captured(Timestamp {
                    seconds: 1,
                    nanoseconds: 0,
                }),
                symlink_target: MetadataValue::Unsupported,
                unix: None,
            },
            chunks: Vec::new(),
        }
    }
}
