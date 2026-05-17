//! Key derivation, envelope encryption, authenticated objects, and secret types.

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("crypto behavior is not implemented yet")]
    NotImplemented,
}
