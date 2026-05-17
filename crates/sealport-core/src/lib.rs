//! Core repository, snapshot, backup, restore, and check orchestration.

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("core behavior is not implemented yet")]
    NotImplemented,
}
