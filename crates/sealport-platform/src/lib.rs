//! Cross-platform path and filesystem metadata behavior.

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("platform behavior is not implemented yet")]
    NotImplemented,
}
