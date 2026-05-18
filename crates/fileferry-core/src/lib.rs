//! Core repository, snapshot, backup, restore, and check orchestration.

use fileferry_platform::{EntryKind, EntryMetadata, PlatformError, capture_metadata};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
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
}
