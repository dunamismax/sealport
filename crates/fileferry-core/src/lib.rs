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
    collections::{BTreeMap, VecDeque},
    fs, io,
    path::Component,
    path::{Path, PathBuf},
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
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
        #[source]
        source: io::Error,
    },

    #[error("chunk {chunk_id} has an invalid length")]
    InvalidChunkLength { chunk_id: String },

    #[error("chunk {chunk_id} is missing from the loaded indexes")]
    MissingChunkIndexEntry { chunk_id: String },

    #[error("restored chunk identity mismatch: expected {expected}, found {actual}")]
    ChunkIdentityMismatch { expected: String, actual: String },

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

    #[error("repository object framing could not be decoded")]
    ObjectDecode {
        #[source]
        source: serde_json::Error,
    },

    #[error("repository metadata could not be decoded")]
    MetadataDecode {
        #[source]
        source: serde_json::Error,
    },

    #[error("{kind} metadata identity mismatch: expected {expected}, found {actual}")]
    MetadataIdentityMismatch {
        kind: &'static str,
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

    #[error("repository bootstrap is invalid: {reason}")]
    InvalidRepositoryBootstrap { reason: &'static str },

    #[error("repository could not be unlocked")]
    RepositoryUnlock {
        #[source]
        source: CryptoError,
    },

    #[error("snapshot selection {selection} did not match any loaded snapshot")]
    SnapshotNotFound { selection: String },

    #[error("snapshot path {path} was not found in snapshot {snapshot_id}")]
    SnapshotPathNotFound { snapshot_id: String, path: PathBuf },

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
    let bytes = store
        .get(&key)
        .await
        .map_err(|source| CoreError::Storage { source })?;
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
            return Err(CoreError::InvalidRepositoryBootstrap {
                reason: "repository format version is not supported",
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
            return Err(CoreError::InvalidRepositoryBootstrap {
                reason: "repository uses unsupported features",
            });
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
                expected: snapshot_id.to_owned(),
                actual: manifest.snapshot_id,
            });
        }
        if actual != snapshot_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "snapshot manifest",
                expected: snapshot_id.to_owned(),
                actual,
            });
        }

        Ok(manifest)
    }

    pub async fn read_committed_snapshot_manifests(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
    ) -> CoreResult<Vec<SnapshotManifest>> {
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
                    .await?,
            );
        }
        manifests.sort_by(compare_snapshot_manifests);

        Ok(manifests)
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
                expected: index_id.to_owned(),
                actual: index.index_id,
            });
        }
        if actual != index_id {
            return Err(CoreError::MetadataIdentityMismatch {
                kind: "chunk index",
                expected: index_id.to_owned(),
                actual,
            });
        }

        Ok(index)
    }

    pub async fn restore_snapshot_contents(
        &self,
        store: &dyn ObjectStore,
        master_key: &MasterKey,
        request: RestoreContentRequest,
    ) -> CoreResult<RestoreContentResult> {
        let restore_paths = normalize_restore_paths(&request.paths)?;
        let manifest = self
            .read_snapshot_manifest(store, master_key, &request.snapshot_id)
            .await?;
        let scoped_entries = scoped_manifest_entries(&manifest, &restore_paths);
        let selected_entries = scoped_entries.len();
        let chunk_index = self
            .load_chunk_index_entries(store, master_key, &manifest)
            .await?;
        let repository_context = self.config.repository_id.as_bytes();
        let chunk_key = master_key
            .derive_subkey(KeyPurpose::ChunkData, repository_context)
            .map_err(|source| CoreError::Encryption { source })?;
        let mut files = Vec::new();

        for entry in scoped_entries {
            if entry.metadata.kind != EntryKind::RegularFile {
                continue;
            }

            let mut contents = Vec::new();
            for chunk in &entry.chunks {
                let indexed = chunk_index.get(&chunk.chunk_id).ok_or_else(|| {
                    CoreError::MissingChunkIndexEntry {
                        chunk_id: chunk.chunk_id.clone(),
                    }
                })?;
                if indexed.object_key != chunk.object_key
                    || indexed.plaintext_length != chunk.length
                    || indexed.compression != CompressionAlgorithm::Zstd
                {
                    return Err(CoreError::InvalidChunkLength {
                        chunk_id: chunk.chunk_id.clone(),
                    });
                }

                let object_key = ObjectKey::new(chunk.object_key.clone())
                    .map_err(|source| CoreError::ObjectKey { source })?;
                let encrypted = store
                    .get(&object_key)
                    .await
                    .map_err(|source| CoreError::Storage { source })?;
                let compressed = decrypt_repository_object(
                    &chunk_key,
                    ObjectKind::Chunk,
                    &object_key,
                    &encrypted,
                )?;
                let expected_len = usize::try_from(indexed.plaintext_length).map_err(|_| {
                    CoreError::InvalidChunkLength {
                        chunk_id: chunk.chunk_id.clone(),
                    }
                })?;
                let plaintext =
                    zstd::bulk::decompress(&compressed, expected_len).map_err(|source| {
                        CoreError::Decompression {
                            chunk_id: chunk.chunk_id.clone(),
                            source,
                        }
                    })?;
                if plaintext.len() != expected_len {
                    return Err(CoreError::InvalidChunkLength {
                        chunk_id: chunk.chunk_id.clone(),
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
                    });
                }

                contents.extend_from_slice(&plaintext);
            }

            files.push(RestoredFile {
                relative_path: entry.relative_path.clone(),
                contents,
            });
        }

        Ok(RestoreContentResult {
            snapshot_id: manifest.snapshot_id,
            selected_entries,
            files,
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
        let mut planned_files = Vec::with_capacity(contents.files.len());

        for file in contents.files {
            let destination_path =
                safe_destination_path(&request.destination, &file.relative_path)?;
            ensure_restore_destination_safe(
                &request.destination,
                &destination_path,
                request.overwrite,
            )?;
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
                action: if request.dry_run {
                    RestoreDestinationAction::WouldWrite
                } else {
                    RestoreDestinationAction::Written
                },
                verified: request.verify && !request.dry_run,
            });
        }

        let bytes = planned_files.iter().map(|file| file.bytes).sum();
        Ok(RestoreDestinationResult {
            snapshot_id: contents.snapshot_id,
            selected_entries: contents.selected_entries,
            files: planned_files,
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
            let index = self.read_chunk_index(store, master_key, index_id).await?;
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
    pub files: Vec<RestoredFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredFile {
    pub relative_path: PathBuf,
    pub contents: Vec<u8>,
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
    pub files: Vec<RestoreDestinationFile>,
    pub bytes: u64,
    pub dry_run: bool,
    pub verified_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDestinationFile {
    pub relative_path: PathBuf,
    pub destination_path: PathBuf,
    pub bytes: u64,
    pub action: RestoreDestinationAction,
    pub verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreDestinationAction {
    WouldWrite,
    Written,
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
    let encrypted = store
        .get(object_key)
        .await
        .map_err(|source| CoreError::Storage { source })?;
    let plaintext = decrypt_repository_object(key, kind, object_key, &encrypted)?;
    serde_json::from_slice(&plaintext).map_err(|source| CoreError::MetadataDecode { source })
}

fn decrypt_repository_object(
    key: &fileferry_crypto::Subkey,
    kind: ObjectKind,
    object_key: &ObjectKey,
    bytes: &[u8],
) -> CoreResult<Vec<u8>> {
    let stored: StoredEncryptedObject =
        serde_json::from_slice(bytes).map_err(|source| CoreError::ObjectDecode { source })?;
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

    decrypt_object(key, &context, &object).map_err(|source| CoreError::Encryption { source })
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
        assert_eq!(restored.files.len(), 1);
        assert_eq!(
            restored.files[0].action,
            RestoreDestinationAction::WouldWrite
        );
        assert_eq!(restored.files[0].bytes, 6);
        assert!(!destination.path().join("sample.txt").exists());
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
            CoreError::Encryption {
                source: CryptoError::Decryption
            }
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
            CoreError::Encryption {
                source: CryptoError::Decryption
            }
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
            CoreError::Encryption {
                source: CryptoError::Decryption
            }
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
            CoreError::Encryption {
                source: CryptoError::Decryption
            }
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
                ..
            }
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
