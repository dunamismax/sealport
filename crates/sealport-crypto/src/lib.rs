//! Key derivation, envelope encryption, authenticated objects, and secret types.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload, rand_core::RngCore},
};
use hkdf::Hkdf;
use secrecy::{ExposeSecret, SecretBox, SecretString};
use sha2::Sha256;
use zeroize::Zeroize;

pub const MASTER_KEY_LEN: usize = 32;
pub const SUBKEY_LEN: usize = 32;
pub const KDF_SALT_LEN: usize = 16;
pub const XCHACHA20_POLY1305_NONCE_LEN: usize = 24;
pub const FORMAT_VERSION_V0: u16 = 0;

const KEY_SLOT_AAD_PREFIX: &[u8] = b"sealport\0format-v0\0key-slot-wrap\0";
const OBJECT_AAD_PREFIX: &[u8] = b"sealport\0format-v0\0object\0";
const HKDF_SALT: &[u8] = b"sealport\0format-v0\0hkdf\0";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid KDF parameters: {0}")]
    InvalidKdfParams(&'static str),

    #[error("key derivation failed")]
    KeyDerivation,

    #[error("encryption failed")]
    Encryption,

    #[error("decryption failed")]
    Decryption,

    #[error("decrypted key material has an invalid length")]
    InvalidKeyMaterial,

    #[error("object authentication context is invalid: {0}")]
    InvalidObjectContext(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KdfAlgorithm {
    Argon2idV19,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub algorithm: KdfAlgorithm,
    pub memory_cost_kib: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl KdfParams {
    pub const fn v0_default() -> Self {
        Self {
            algorithm: KdfAlgorithm::Argon2idV19,
            memory_cost_kib: 64 * 1024,
            time_cost: 3,
            parallelism: 4,
        }
    }

    pub const fn v0_high_memory() -> Self {
        Self {
            algorithm: KdfAlgorithm::Argon2idV19,
            memory_cost_kib: 2 * 1024 * 1024,
            time_cost: 1,
            parallelism: 4,
        }
    }

    pub const fn for_tests() -> Self {
        Self {
            algorithm: KdfAlgorithm::Argon2idV19,
            memory_cost_kib: 32,
            time_cost: 3,
            parallelism: 4,
        }
    }

    fn validate(self) -> Result<(), CryptoError> {
        match self.algorithm {
            KdfAlgorithm::Argon2idV19 => {}
        }
        if self.parallelism == 0 {
            return Err(CryptoError::InvalidKdfParams(
                "parallelism must be greater than zero",
            ));
        }
        if self.time_cost == 0 {
            return Err(CryptoError::InvalidKdfParams(
                "time cost must be greater than zero",
            ));
        }
        if self.memory_cost_kib < 8 * self.parallelism {
            return Err(CryptoError::InvalidKdfParams(
                "memory cost must be at least 8 KiB per lane",
            ));
        }

        Ok(())
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::v0_default()
    }
}

pub struct MasterKey(SecretBox<[u8; MASTER_KEY_LEN]>);

impl MasterKey {
    pub fn generate() -> Self {
        Self(SecretBox::init_with_mut(
            |secret: &mut [u8; MASTER_KEY_LEN]| {
                OsRng.fill_bytes(secret);
            },
        ))
    }

    fn from_secret_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != MASTER_KEY_LEN {
            return Err(CryptoError::InvalidKeyMaterial);
        }

        Ok(Self(SecretBox::init_with_mut(
            |secret: &mut [u8; MASTER_KEY_LEN]| {
                secret.copy_from_slice(bytes);
            },
        )))
    }

    fn expose(&self) -> &[u8; MASTER_KEY_LEN] {
        self.0.expose_secret()
    }

    pub fn derive_subkey(
        &self,
        purpose: KeyPurpose,
        context: &[u8],
    ) -> Result<Subkey, CryptoError> {
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), self.expose());
        let mut info = Vec::with_capacity(64 + context.len());
        info.extend_from_slice(b"sealport\0subkey\0");
        info.extend_from_slice(purpose.label());
        write_len_prefixed(&mut info, context);

        let mut output = [0_u8; SUBKEY_LEN];
        hkdf.expand(&info, &mut output)
            .map_err(|_| CryptoError::KeyDerivation)?;
        let subkey = Subkey::from_secret_array(&output);
        output.zeroize();
        Ok(subkey)
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MasterKey([REDACTED])")
    }
}

pub struct Subkey(SecretBox<[u8; SUBKEY_LEN]>);

impl Subkey {
    fn from_secret_array(bytes: &[u8; SUBKEY_LEN]) -> Self {
        Self(SecretBox::init_with_mut(|secret: &mut [u8; SUBKEY_LEN]| {
            secret.copy_from_slice(bytes);
        }))
    }

    fn expose(&self) -> &[u8; SUBKEY_LEN] {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for Subkey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Subkey([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPurpose {
    ChunkData,
    SnapshotMetadata,
    Index,
    PolicyConfig,
    UploadState,
    PruneMark,
}

impl KeyPurpose {
    fn label(self) -> &'static [u8] {
        match self {
            Self::ChunkData => b"chunk-data",
            Self::SnapshotMetadata => b"snapshot-metadata",
            Self::Index => b"index",
            Self::PolicyConfig => b"policy-config",
            Self::UploadState => b"upload-state",
            Self::PruneMark => b"prune-mark",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySlot {
    pub kdf: KdfParams,
    pub salt: [u8; KDF_SALT_LEN],
    pub nonce: [u8; XCHACHA20_POLY1305_NONCE_LEN],
    pub wrapped_master_key: Vec<u8>,
}

pub fn create_master_key(
    passphrase: &SecretString,
    kdf: KdfParams,
) -> Result<(MasterKey, KeySlot), CryptoError> {
    let master_key = MasterKey::generate();
    let key_slot = create_key_slot(&master_key, passphrase, kdf)?;
    Ok((master_key, key_slot))
}

pub fn create_key_slot(
    master_key: &MasterKey,
    passphrase: &SecretString,
    kdf: KdfParams,
) -> Result<KeySlot, CryptoError> {
    kdf.validate()?;

    let mut salt = [0_u8; KDF_SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let wrapping_key = derive_passphrase_key(passphrase, &salt, kdf)?;
    let nonce = random_nonce();
    let aad = key_slot_aad(kdf, &salt);
    let wrapped_master_key = encrypt_with_key(&wrapping_key, &nonce, master_key.expose(), &aad)?;

    Ok(KeySlot {
        kdf,
        salt,
        nonce,
        wrapped_master_key,
    })
}

pub fn unlock_master_key(
    passphrase: &SecretString,
    key_slot: &KeySlot,
) -> Result<MasterKey, CryptoError> {
    key_slot.kdf.validate()?;

    let wrapping_key = derive_passphrase_key(passphrase, &key_slot.salt, key_slot.kdf)?;
    let aad = key_slot_aad(key_slot.kdf, &key_slot.salt);
    let mut plaintext = decrypt_with_key(
        &wrapping_key,
        &key_slot.nonce,
        &key_slot.wrapped_master_key,
        &aad,
    )?;
    let master_key = MasterKey::from_secret_slice(&plaintext);
    plaintext.zeroize();

    master_key
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AeadAlgorithm {
    XChaCha20Poly1305,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Chunk,
    SnapshotManifest,
    Index,
    PolicyConfig,
    RepositoryConfig,
    UploadState,
    PruneMark,
}

impl ObjectKind {
    fn label(self) -> &'static [u8] {
        match self {
            Self::Chunk => b"chunk",
            Self::SnapshotManifest => b"snapshot-manifest",
            Self::Index => b"index",
            Self::PolicyConfig => b"policy-config",
            Self::RepositoryConfig => b"repository-config",
            Self::UploadState => b"upload-state",
            Self::PruneMark => b"prune-mark",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectContext {
    pub format_version: u16,
    pub kind: ObjectKind,
    pub object_name: String,
}

impl ObjectContext {
    pub fn new(kind: ObjectKind, object_name: impl Into<String>) -> Result<Self, CryptoError> {
        Self::for_format(FORMAT_VERSION_V0, kind, object_name)
    }

    pub fn for_format(
        format_version: u16,
        kind: ObjectKind,
        object_name: impl Into<String>,
    ) -> Result<Self, CryptoError> {
        let object_name = object_name.into();
        if object_name.is_empty() {
            return Err(CryptoError::InvalidObjectContext(
                "object name must not be empty",
            ));
        }
        if object_name.as_bytes().contains(&0) {
            return Err(CryptoError::InvalidObjectContext(
                "object name must not contain NUL",
            ));
        }

        Ok(Self {
            format_version,
            kind,
            object_name,
        })
    }

    fn aad(&self) -> Vec<u8> {
        let mut aad = Vec::with_capacity(64 + self.object_name.len());
        aad.extend_from_slice(OBJECT_AAD_PREFIX);
        aad.extend_from_slice(&self.format_version.to_le_bytes());
        write_len_prefixed(&mut aad, self.kind.label());
        write_len_prefixed(&mut aad, self.object_name.as_bytes());
        aad
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedObject {
    pub algorithm: AeadAlgorithm,
    pub nonce: [u8; XCHACHA20_POLY1305_NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

pub fn encrypt_object(
    key: &Subkey,
    context: &ObjectContext,
    plaintext: &[u8],
) -> Result<EncryptedObject, CryptoError> {
    let nonce = random_nonce();
    let aad = context.aad();
    let ciphertext = encrypt_with_key(key, &nonce, plaintext, &aad)?;

    Ok(EncryptedObject {
        algorithm: AeadAlgorithm::XChaCha20Poly1305,
        nonce,
        ciphertext,
    })
}

pub fn decrypt_object(
    key: &Subkey,
    context: &ObjectContext,
    object: &EncryptedObject,
) -> Result<Vec<u8>, CryptoError> {
    match object.algorithm {
        AeadAlgorithm::XChaCha20Poly1305 => {
            decrypt_with_key(key, &object.nonce, &object.ciphertext, &context.aad())
        }
    }
}

fn derive_passphrase_key(
    passphrase: &SecretString,
    salt: &[u8; KDF_SALT_LEN],
    kdf: KdfParams,
) -> Result<Subkey, CryptoError> {
    let params = Params::new(
        kdf.memory_cost_kib,
        kdf.time_cost,
        kdf.parallelism,
        Some(SUBKEY_LEN),
    )
    .map_err(|_| CryptoError::InvalidKdfParams("Argon2 rejected the supplied parameters"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0_u8; SUBKEY_LEN];

    argon2
        .hash_password_into(passphrase.expose_secret().as_bytes(), salt, &mut output)
        .map_err(|_| CryptoError::KeyDerivation)?;
    let subkey = Subkey::from_secret_array(&output);
    output.zeroize();
    Ok(subkey)
}

fn encrypt_with_key(
    key: &Subkey,
    nonce: &[u8; XCHACHA20_POLY1305_NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| CryptoError::InvalidKeyMaterial)?;
    cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encryption)
}

fn decrypt_with_key(
    key: &Subkey,
    nonce: &[u8; XCHACHA20_POLY1305_NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| CryptoError::InvalidKeyMaterial)?;
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

fn random_nonce() -> [u8; XCHACHA20_POLY1305_NONCE_LEN] {
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut output = [0_u8; XCHACHA20_POLY1305_NONCE_LEN];
    output.copy_from_slice(&nonce);
    output
}

fn key_slot_aad(kdf: KdfParams, salt: &[u8; KDF_SALT_LEN]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(64);
    aad.extend_from_slice(KEY_SLOT_AAD_PREFIX);
    aad.push(match kdf.algorithm {
        KdfAlgorithm::Argon2idV19 => 1,
    });
    aad.extend_from_slice(&kdf.memory_cost_kib.to_le_bytes());
    aad.extend_from_slice(&kdf.time_cost.to_le_bytes());
    aad.extend_from_slice(&kdf.parallelism.to_le_bytes());
    write_len_prefixed(&mut aad, salt);
    aad
}

fn write_len_prefixed(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_le_bytes());
    output.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passphrase() -> SecretString {
        SecretString::from("correct horse battery staple")
    }

    #[test]
    fn default_kdf_params_match_v0_security_design() {
        assert_eq!(
            KdfParams::default(),
            KdfParams {
                algorithm: KdfAlgorithm::Argon2idV19,
                memory_cost_kib: 64 * 1024,
                time_cost: 3,
                parallelism: 4,
            }
        );
    }

    #[test]
    fn creates_and_unlocks_master_key() {
        let (master_key, key_slot) =
            create_master_key(&passphrase(), KdfParams::for_tests()).expect("created master key");

        let unlocked =
            unlock_master_key(&passphrase(), &key_slot).expect("unlocked master key from key slot");

        assert_eq!(unlocked.expose(), master_key.expose());
    }

    #[test]
    fn wrong_passphrase_fails_closed() {
        let (_master_key, key_slot) =
            create_master_key(&passphrase(), KdfParams::for_tests()).expect("created master key");
        let wrong = SecretString::from("wrong password");

        let error = unlock_master_key(&wrong, &key_slot).expect_err("wrong passphrase fails");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn key_slot_tampering_fails_closed() {
        let (_master_key, mut key_slot) =
            create_master_key(&passphrase(), KdfParams::for_tests()).expect("created master key");
        key_slot.wrapped_master_key[0] ^= 0x80;

        let error =
            unlock_master_key(&passphrase(), &key_slot).expect_err("tampered key slot fails");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn encrypted_object_round_trips_with_authenticated_context() {
        let master_key = MasterKey::generate();
        let key = master_key
            .derive_subkey(KeyPurpose::ChunkData, b"repo-id")
            .expect("derived subkey");
        let context = ObjectContext::new(ObjectKind::Chunk, "chunks/00/example").expect("context");
        let encrypted =
            encrypt_object(&key, &context, b"plaintext chunk bytes").expect("encrypted object");

        let decrypted = decrypt_object(&key, &context, &encrypted).expect("decrypted object");

        assert_eq!(decrypted, b"plaintext chunk bytes");
    }

    #[test]
    fn wrong_subkey_fails_to_decrypt_object() {
        let first = MasterKey::generate();
        let second = MasterKey::generate();
        let first_key = first
            .derive_subkey(KeyPurpose::ChunkData, b"repo-id")
            .expect("derived first subkey");
        let second_key = second
            .derive_subkey(KeyPurpose::ChunkData, b"repo-id")
            .expect("derived second subkey");
        let context = ObjectContext::new(ObjectKind::Chunk, "chunks/00/example").expect("context");
        let encrypted = encrypt_object(&first_key, &context, b"plaintext").expect("encrypted");

        let error = decrypt_object(&second_key, &context, &encrypted)
            .expect_err("wrong subkey must not decrypt");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn corrupted_ciphertext_fails_to_decrypt_object() {
        let master_key = MasterKey::generate();
        let key = master_key
            .derive_subkey(KeyPurpose::Index, b"repo-id")
            .expect("derived subkey");
        let context = ObjectContext::new(ObjectKind::Index, "indexes/example").expect("context");
        let mut encrypted = encrypt_object(&key, &context, b"index bytes").expect("encrypted");
        encrypted.ciphertext[0] ^= 0x40;

        let error =
            decrypt_object(&key, &context, &encrypted).expect_err("corruption must be detected");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn truncated_ciphertext_fails_to_decrypt_object() {
        let master_key = MasterKey::generate();
        let key = master_key
            .derive_subkey(KeyPurpose::SnapshotMetadata, b"repo-id")
            .expect("derived subkey");
        let context =
            ObjectContext::new(ObjectKind::SnapshotManifest, "snapshots/example").expect("context");
        let mut encrypted = encrypt_object(&key, &context, b"manifest bytes").expect("encrypted");
        encrypted.ciphertext.truncate(4);

        let error =
            decrypt_object(&key, &context, &encrypted).expect_err("truncation must be detected");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn wrong_object_context_fails_to_decrypt_object() {
        let master_key = MasterKey::generate();
        let key = master_key
            .derive_subkey(KeyPurpose::ChunkData, b"repo-id")
            .expect("derived subkey");
        let context = ObjectContext::new(ObjectKind::Chunk, "chunks/00/example").expect("context");
        let wrong_context =
            ObjectContext::new(ObjectKind::Index, "chunks/00/example").expect("context");
        let encrypted = encrypt_object(&key, &context, b"plaintext").expect("encrypted");

        let error = decrypt_object(&key, &wrong_context, &encrypted)
            .expect_err("wrong authenticated context must not decrypt");

        assert!(matches!(error, CryptoError::Decryption));
    }

    #[test]
    fn master_key_debug_is_redacted() {
        let rendered = format!("{:?}", MasterKey::generate());

        assert_eq!(rendered, "MasterKey([REDACTED])");
    }
}
