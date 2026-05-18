//! Cross-platform path and filesystem metadata behavior.

use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("metadata for {path} could not be read")]
    MetadataRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("symlink target for {path} could not be read")]
    SymlinkTargetRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    RegularFile,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataValue<T> {
    Captured(T),
    Unsupported,
    Denied(String),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct EntryMetadata {
    pub kind: EntryKind,
    pub size_bytes: Option<u64>,
    pub modified: MetadataValue<Timestamp>,
    pub created: MetadataValue<Timestamp>,
    pub symlink_target: MetadataValue<PathBuf>,
    pub unix: Option<UnixMetadata>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct UnixMetadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

pub fn capture_metadata(path: impl AsRef<Path>) -> Result<EntryMetadata, PlatformError> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|source| PlatformError::MetadataRead {
        path: path.to_path_buf(),
        source,
    })?;

    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        EntryKind::RegularFile
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    };

    let symlink_target = if kind == EntryKind::Symlink {
        match fs::read_link(path) {
            Ok(target) => MetadataValue::Captured(target),
            Err(source) if source.kind() == io::ErrorKind::PermissionDenied => {
                MetadataValue::Denied(source.to_string())
            }
            Err(source) => {
                return Err(PlatformError::SymlinkTargetRead {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    } else {
        MetadataValue::Unsupported
    };

    Ok(EntryMetadata {
        kind,
        size_bytes: if file_type.is_file() {
            Some(metadata.len())
        } else {
            None
        },
        modified: metadata_value_from_time(metadata.modified()),
        created: metadata_value_from_time(metadata.created()),
        symlink_target,
        unix: unix_metadata(&metadata),
    })
}

fn metadata_value_from_time(result: io::Result<SystemTime>) -> MetadataValue<Timestamp> {
    match result {
        Ok(time) => MetadataValue::Captured(Timestamp::from(time)),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            MetadataValue::Denied(error.to_string())
        }
        Err(_) => MetadataValue::Unsupported,
    }
}

impl From<SystemTime> for Timestamp {
    fn from(value: SystemTime) -> Self {
        match value.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self {
                seconds: duration.as_secs() as i64,
                nanoseconds: duration.subsec_nanos(),
            },
            Err(error) => {
                let duration = error.duration();
                if duration.subsec_nanos() == 0 {
                    Self {
                        seconds: -(duration.as_secs() as i64),
                        nanoseconds: 0,
                    }
                } else {
                    Self {
                        seconds: -(duration.as_secs() as i64) - 1,
                        nanoseconds: 1_000_000_000 - duration.subsec_nanos(),
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
fn unix_metadata(metadata: &fs::Metadata) -> Option<UnixMetadata> {
    use std::os::unix::fs::MetadataExt;

    Some(UnixMetadata {
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

#[cfg(not(unix))]
fn unix_metadata(_metadata: &fs::Metadata) -> Option<UnixMetadata> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_regular_file_metadata_without_reading_contents() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sample.txt");
        fs::write(&path, b"hello").expect("write file");

        let metadata = capture_metadata(&path).expect("metadata");

        assert_eq!(metadata.kind, EntryKind::RegularFile);
        assert_eq!(metadata.size_bytes, Some(5));
        assert!(matches!(
            metadata.modified,
            MetadataValue::Captured(Timestamp { .. })
        ));
        assert!(matches!(
            metadata.symlink_target,
            MetadataValue::Unsupported
        ));
    }

    #[test]
    fn captures_directory_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");

        let metadata = capture_metadata(temp.path()).expect("metadata");

        assert_eq!(metadata.kind, EntryKind::Directory);
        assert_eq!(metadata.size_bytes, None);
    }

    #[cfg(unix)]
    #[test]
    fn captures_symlink_target_without_following_it() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target.txt");
        let link = temp.path().join("link.txt");
        fs::write(&target, b"target").expect("write target");
        symlink("target.txt", &link).expect("symlink");

        let metadata = capture_metadata(&link).expect("metadata");

        assert_eq!(metadata.kind, EntryKind::Symlink);
        assert_eq!(metadata.size_bytes, None);
        assert_eq!(
            metadata.symlink_target,
            MetadataValue::Captured(PathBuf::from("target.txt"))
        );
    }

    #[test]
    fn reports_missing_path_with_path_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing");

        let error = capture_metadata(&missing).expect_err("missing path");

        assert!(error.to_string().contains("missing"));
    }
}
