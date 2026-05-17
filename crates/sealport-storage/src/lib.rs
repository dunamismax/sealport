//! Local and object storage abstractions and backend implementations.

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage behavior is not implemented yet")]
    NotImplemented,
}
