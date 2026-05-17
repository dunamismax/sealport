//! Shared test helpers, fake stores, corruption fixtures, and platform fixtures.

pub fn temp_repository() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temporary repository directory")
}
