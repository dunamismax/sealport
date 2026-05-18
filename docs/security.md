# Security Design

SealPort is pre-v1 and the repository format is not frozen. This document
records the security decisions that format v0 implementation work must follow.
Changing these decisions requires updating this document, repository-format
fixtures when they exist, and the focused crypto tests.

## Goals

- Encrypt file contents, file names, directory structure, snapshot metadata,
  indexes, and sensitive repository policy/config objects before storage.
- Authenticate every encrypted object.
- Fail closed for wrong passphrases, wrong keys, tampering, truncation, swapped
  objects, replayed metadata, and malformed metadata.
- Keep plaintext repository bootstrap metadata limited, justified, and
  insufficient to reveal backup shape.
- Keep logs, diagnostics, human output, JSON, JSONL, tests, and debug output
  free of passphrases, raw key material, cloud credentials, signed URLs, bearer
  tokens, and full environment dumps.

## Current References

These references were checked before the v0 choices below:

- RFC 9106 for Argon2id. It specifies Argon2 version 0x13 and recommends
  Argon2id, including 64 MiB/t=3 for memory-constrained environments and
  2 GiB/t=1 for high-memory defaults.
- RFC 8439 for ChaCha20-Poly1305 AEAD.
- libsodium's XChaCha20-Poly1305 guidance for extended-nonce AEAD use and
  nonce uniqueness across related messages.
- RFC 5869 for HKDF extract-and-expand key derivation.

## AEAD

Format v0 uses XChaCha20-Poly1305 for object encryption.

Reasons:

- It is an authenticated encryption mode, so ciphertext modification,
  truncation, wrong-key reads, and wrong authenticated data fail during decrypt.
- The 192-bit nonce gives a wide random nonce space for immutable repository
  objects and avoids relying on a mutable central counter.
- It is available through maintained RustCrypto crates without shelling out to
  OpenSSL or platform tools.

Rules:

- Generate a fresh random nonce for every encryption under a given subkey.
- Store the nonce next to the ciphertext. The nonce is not secret.
- Treat any AEAD open failure as authentication failure. Do not return partial
  plaintext.
- Never reuse a nonce with the same subkey.
- Do not add unauthenticated compression, framing, or metadata around
  ciphertext that affects restore behavior.

## Key Hierarchy

Each repository has a random 256-bit master key. User passphrases and future
key-file unlock methods decrypt that master key; they are not used directly for
repository objects.

Format v0 derives subkeys from the master key with HKDF-SHA-256:

```text
HKDF salt = "sealport\0format-v0\0hkdf\0"
info      = "sealport\0subkey\0" || purpose || len(context) || context
output    = 32 bytes
```

Initial subkey purposes:

- `chunk-data`
- `snapshot-metadata`
- `index`
- `policy-config`
- `upload-state`
- `prune-mark`

The `context` must bind a subkey to the repository identity or a narrower
operation context. Domain labels must not be reused for incompatible object
types.

## Passphrase KDF

Format v0 uses Argon2id version 0x13 for passphrase unlock.

Default parameters:

```text
memory_cost = 65536 KiB
time_cost   = 3
parallelism = 4
salt        = 16 random bytes per key slot
output      = 32 bytes
```

These defaults follow RFC 9106's memory-constrained recommendation. A
high-memory profile may use 2 GiB memory, time cost 1, and parallelism 4 after
unlock latency has been measured on target hardware.

The KDF parameters are plaintext bootstrap metadata because unlock requires
them before the master key is available. They are authenticated as associated
data when decrypting the wrapped master key.

KDF migration is per key slot. Existing key slots remain readable with the
parameters stored in the slot. Raising defaults creates new slots with the new
parameters and can retire older slots after the user proves another unlock path
works. Repository object encryption does not change when only KDF parameters
change because object keys derive from the unchanged repository master key.

## Key Slots And Unlock

A key slot contains:

- KDF algorithm and parameters.
- KDF salt.
- AEAD nonce.
- XChaCha20-Poly1305 ciphertext containing the repository master key.

The key-slot AEAD associated data is:

```text
"sealport\0format-v0\0key-slot-wrap\0"
|| kdf_algorithm
|| memory_cost
|| time_cost
|| parallelism
|| len(salt)
|| salt
```

Unlock flow:

1. Read the plaintext key-slot metadata.
2. Derive the wrapping key with Argon2id.
3. Decrypt the wrapped master key with the key-slot associated data.
4. Fail closed if derivation fails, authentication fails, or the plaintext is
   not exactly 32 bytes.

## Recovery Export

Recovery export is not implemented yet. Format v0 design target:

- Export only an encrypted recovery package.
- Require an explicit command and a strong warning that possession of the
  export plus its recovery secret can unlock the repository.
- Include repository identity, format version, KDF parameters, and encrypted
  master-key material.
- Never print raw master keys to terminal output, JSON, JSONL, logs, or debug
  output.
- Require a file output path by default so accidental terminal capture is less
  likely.

Warning text must state that the export should be stored separately from the
repository and protected like a backup key.

## Key Rotation

Key rotation has two different meanings:

- Unlock rotation changes key slots, passphrases, key files, or recovery
  packages that wrap the same repository master key.
- Repository rekey creates a new master key and rewrites or re-encrypts all
  repository objects.

Format v0 `key rotate` must mean unlock rotation unless a future command name
explicitly chooses full repository rekey. Unlock rotation does not rewrite old
chunks, manifests, indexes, or policy objects. The command must say that
clearly in human and machine output.

## Tamper And Corruption Errors

Library crates should keep these classes distinguishable enough for stable CLI
exit-code mapping:

- `wrong_password`: KDF completed but key-slot authentication failed.
- `wrong_key`: object authentication failed with the supplied repository key.
- `corrupt_object`: ciphertext/tag/nonce/framing is malformed or truncated.
- `context_mismatch`: authenticated object context does not match the object
  being opened.
- `unsupported_format`: bootstrap version or algorithm is unknown.
- `malformed_metadata`: decrypted metadata is not valid for its schema.
- `replayed_metadata`: authenticated metadata is valid but older than the
  required repository state.

The CLI maps authentication failures to exit code 4 and integrity/tamper
failures to exit code 6 when it has enough context to distinguish them. When it
cannot distinguish wrong credentials from tampering without leaking information,
it should use the more conservative authentication/integrity message and keep
machine fields structured.

JSON and JSONL failures should include the normal CLI envelope plus a structured
error object:

```json
{
  "code": "corrupt_object",
  "exit_code": 6,
  "message": "repository object failed authentication",
  "object_kind": "index",
  "object_name": "objects/index/ab/example",
  "recoverable": false
}
```

`message` is for display and may change before v1. `code`, `exit_code`,
`object_kind`, `object_name`, and `recoverable` are the planned stable fields.
Sensitive plaintext metadata must not be placed in any error field.

## Implemented Evidence

The `sealport-crypto` crate currently includes focused tests for:

- Master key creation and passphrase unlock.
- Wrong passphrase failure.
- Tampered key-slot failure.
- Object encryption/decryption.
- Wrong subkey failure.
- Bit-flipped ciphertext failure.
- Truncated ciphertext failure.
- Wrong authenticated object context failure.
- Redacted `Debug` output for master keys.

The broader adversarial test matrix still needs replayed indexes, swapped
objects across realistic repository names, malformed decrypted metadata, and
format migration failures once repository objects exist.
