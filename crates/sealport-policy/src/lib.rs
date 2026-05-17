//! Retention, forget, prune, and lifecycle policy logic.

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy behavior is not implemented yet")]
    NotImplemented,
}
