# BUILD.md

Active build plan for SealPort and its `sealport` command.

`README.md` explains the product. `AGENTS.md` holds durable repo operating
rules. This file tracks current state, implementation phases, release scope,
and verification.

Treat unchecked boxes as plan. Move stable material into `docs/`, `README.md`,
or runbooks as the implementation matures.

Last reviewed: 2026-05-17.

---

## Current Baseline

- Repository exists with MIT license.
- `origin` fetches from and pushes to
  `https://github.com/dunamismax/sealport.git`.
- No Rust workspace, source crates, CI, release workflow, or docs directory
  exists yet.
- The initial product brief has been distilled into `README.md`,
  `BUILD.md`, and `AGENTS.md`.
- The product target is an all-Rust, cross-platform, encrypted backup CLI
  named `sealport`.

The repo is pre-implementation. Do not describe any runtime behavior as
working until code, tests, and platform evidence exist.

---

## Stack Direction

- **Rust** for every first-party binary and library.
- **Cargo workspace** with crates under `crates/`.
- **Rust 2024 edition** unless current Rust guidance or MSRV constraints argue
  otherwise before the workspace is created.
- **clap** for CLI parsing and shell completions.
- **tokio** for async storage and network work.
- **serde**, **serde_json**, and **toml** for config and machine-readable
  output.
- **tracing** for logs, spans, progress events, and redaction-aware
  diagnostics.
- **miette** or **color-eyre** for human-facing diagnostics after a short
  spike confirms the best fit.
- **object_store** first for local/S3/object backends. Consider an optional
  OpenDAL adapter only after core storage semantics are proven.
- **fastcdc** for content-defined chunking.
- **blake3** for fast content IDs, checksums, and integrity trees where
  appropriate.
- **zstd** for compression.
- **Argon2id** for passphrase key derivation.
- **zeroize** and **secrecy** for secret handling.
- **xtask** only when build, fixture, compatibility, or release automation
  becomes too large for `just` and Cargo commands.

Use current primary docs before locking crate versions, crypto primitives,
target support, or release tooling.

---

## Product Invariants

- Restore is the product.
- Every command must be scriptable.
- Stdout is data; stderr is logs, progress, and errors.
- `--json` emits one JSON document. `--jsonl` emits an event stream.
- Human output can change. Machine output is a compatibility surface.
- Client-side encryption is mandatory for repository contents and metadata.
- No plaintext file names, directory structure, indexes, snapshot metadata, or
  sensitive policy/config shape in repositories.
- The repository format is original to SealPort.
- No restic, rustic, Borg, Kopia, or rclone repository compatibility in v1.
- Local filesystem and S3-compatible object storage are the v1 storage
  targets.
- Object storage is not a filesystem. Avoid required renames and mutable
  directory assumptions.
- Every destructive command supports `--dry-run`.
- Every long operation can be interrupted safely.
- Platform support is real only when CI, tests, and release artifacts exist.
- Cross-platform correctness beats clever single-platform fast paths.

---

## Security Model

Required v1 security properties:

- Random repository master key.
- Passphrase and key-file unlock paths for that master key.
- Envelope encryption with derived subkeys for data, metadata, and indexes.
- Authenticated encryption for every encrypted object.
- Authenticated snapshot manifests and indexes.
- Corruption and tampering detection during read, check, and restore.
- Redaction of passwords, key material, credentials, signed URLs, and secret
  environment values in logs and diagnostics.
- Secret memory handling with `zeroize`/`secrecy` where practical.
- No repository operation that silently weakens encryption for convenience.

Security design work before format freeze:

- [ ] Choose AEAD and key hierarchy, then document rationale.
- [ ] Define repository bootstrap plaintext and justify every field.
- [ ] Define KDF parameters, migration story, and unlock UX.
- [ ] Define recovery export format and user warning text.
- [ ] Define key rotation semantics and what rotation does not rewrite.
- [ ] Define tamper/corruption error classes and JSON output.
- [ ] Add `docs/security.md`.
- [ ] Add adversarial tests for wrong password, wrong key, bit flips, truncated
      objects, swapped objects, replayed indexes, and malformed metadata.

---

## Repository Model

Core objects:

- Encrypted chunks.
- Encrypted snapshot manifests.
- Encrypted indexes.
- Encrypted policy/config object.
- Temporary upload state.
- Prune marks and maintenance metadata.

Repository design goals:

- Append-friendly.
- Interruption-safe.
- Safe concurrent backups.
- No required rename operations.
- No required object listing for correctness where avoidable.
- Two-phase prune.
- Deterministic integrity checks.
- Future format migrations that can be detected and explained.

Repository format work:

- [ ] Write `docs/repository-format.md` before committing to object bytes.
- [ ] Define object naming without leaking source paths.
- [ ] Define object authentication context and domain separation.
- [ ] Define snapshot manifest structure.
- [ ] Define chunk index structure.
- [ ] Define commit markers and upload state.
- [ ] Define repository lock or lease model, if any.
- [ ] Define concurrent backup behavior.
- [ ] Define prune mark, sweep, and recovery behavior.
- [ ] Add golden fixtures after the first format version is intentionally
      frozen.

---

## Target Source Layout

```text
Cargo.toml
rust-toolchain.toml
justfile
crates/
  sealport-cli/       command parsing, output formats, config loading
  sealport-core/      snapshots, repository format, backup/restore engine
  sealport-storage/   local and object storage abstraction
  sealport-crypto/    key derivation, encryption, authenticated metadata
  sealport-platform/  filesystem metadata across Windows/macOS/Linux/BSD
  sealport-policy/    retention, pruning, lifecycle rules
  sealport-testkit/   fake stores, corruption tests, fixtures, helpers
xtask/
docs/
  architecture.md
  cli-contract.md
  config.md
  repository-format.md
  security.md
  storage.md
  platform-metadata.md
  operations.md
  release.md
tests/
  fixtures/
  integration/
```

Keep command presentation in `sealport-cli`. Library crates should return typed
errors and structured events; the CLI decides human text, JSON, JSONL, and exit
codes.

---

## CLI Contract

Global flags:

```text
--repo <URL>
--profile <NAME>
--config <FILE>
--json
--jsonl
--quiet
--log-level <LEVEL>
--no-progress
```

Required v1 commands:

```text
sealport init
sealport backup
sealport restore
sealport snapshots
sealport ls
sealport check
sealport forget
sealport prune
sealport key
sealport completion
sealport version
```

High-value commands that should land before or shortly after v1 if the core is
stable:

```text
sealport find
sealport diff
sealport copy
sealport repo
sealport policy
sealport doctor
```

CLI work:

- [ ] Define stable exit codes in `docs/cli-contract.md`.
- [ ] Define JSON document schemas for every command.
- [ ] Define JSONL event schemas for long operations.
- [ ] Add golden tests for help text, JSON output, JSONL event order, and exit
      codes.
- [ ] Ensure progress bars never appear in stdout data modes.
- [ ] Ensure `--dry-run` exists for destructive commands.
- [ ] Ensure all prompts have non-interactive alternatives.

---

## Platform And Metadata

Target platforms:

- Windows x86_64 MSVC.
- Windows ARM64 MSVC.
- macOS x86_64.
- macOS ARM64.
- Linux x86_64 GNU.
- Linux x86_64 musl.
- Linux ARM64 GNU/musl.
- FreeBSD x86_64.
- NetBSD x86_64 where feasible.
- OpenBSD best-effort until CI and release support are real.

Platform work:

- [ ] Define metadata capture for files, directories, symlinks, permissions,
      timestamps, ownership, xattrs, ACLs, resource forks, and Windows
      attributes.
- [ ] Decide v1 restore behavior for metadata that cannot be represented on
      the destination platform.
- [ ] Add platform-specific tests for path normalization, reserved names,
      symlinks, hard links if supported, case sensitivity, long paths, and
      permission errors.
- [ ] Add CI for supported platforms before claiming support.
- [ ] Add release artifacts only for platforms that pass the support bar.

---

## V1 Release Definition

SealPort is ready for v1 only when it is boring to initialize, back up, verify,
restore, automate, and install across the supported platform list.

Minimum v1 bar:

- [ ] Rust workspace exists with the target crate boundaries.
- [ ] `sealport init` creates encrypted local and S3-compatible repositories.
- [ ] `sealport backup` creates encrypted, compressed, deduplicated snapshots.
- [ ] `sealport restore` restores by snapshot id, tag, path, and `latest`.
- [ ] `sealport snapshots` and `sealport ls` have human, JSON, and JSONL-safe
      behavior where appropriate.
- [ ] `sealport check` verifies metadata and configurable data subsets.
- [ ] `sealport forget` and `sealport prune` implement retention and two-phase
      deletion safely.
- [ ] Key add/remove/rotate/export-recovery paths exist and are tested.
- [ ] Local backend passes interruption and corruption tests.
- [ ] S3-compatible backend passes retry, resume, and eventual-weirdness tests.
- [ ] Stable config profiles and environment variables exist.
- [ ] Shell completions are generated.
- [ ] Exit codes, JSON, and JSONL schemas are documented and tested.
- [ ] Platform metadata behavior is documented and tested on every supported
      platform.
- [ ] Release artifacts include archives, checksums, signatures, SBOM, and
      `cargo-auditable` metadata.
- [ ] Install scripts for Unix shells and PowerShell are tested.
- [ ] At least one restore drill is documented from a real SealPort snapshot.

V1 must not include GUI, TUI, FUSE mount, daemon mode, server mode, magic
scheduling, mobile apps, or compatibility with restic/rustic repositories.

---

## Phases

Ordered intent, not rigid sequence. Each phase should leave the repo in a state
where documented verification passes on a clean checkout.

### Phase 0 - Bootstrap Docs

- [x] Read local sample docs for Stephen's preferred repo documentation shape.
- [x] Write initial `README.md`, `BUILD.md`, and `AGENTS.md`.
- [x] Record the repo as pre-implementation.

### Phase 1 - Workspace Foundation

- [ ] Add `rust-toolchain.toml`.
- [ ] Add Cargo workspace with Rust 2024 edition.
- [ ] Add crates: `sealport-cli`, `sealport-core`, `sealport-storage`,
      `sealport-crypto`, `sealport-platform`, `sealport-policy`, and
      `sealport-testkit`.
- [ ] Add workspace dependency policy.
- [ ] Add `sealport-cli` binary with `clap`.
- [ ] Add `just fmt`, `just check`, `just test`, and `just build`.
- [ ] Add GitHub Actions for formatting, clippy, tests, and build.
- [ ] Add basic `sealport version`.

### Phase 2 - CLI, Config, And Output Contract

- [ ] Implement config discovery and profiles.
- [ ] Implement global flags and environment variable precedence.
- [ ] Add typed config validation and redacted diagnostics.
- [ ] Define stable event model for command progress.
- [ ] Implement human, JSON, and JSONL output surfaces.
- [ ] Add CLI golden tests.
- [ ] Add `sealport completion`.

### Phase 3 - Crypto And Format Design

- [ ] Write `docs/security.md`.
- [ ] Write `docs/repository-format.md`.
- [ ] Choose AEAD, KDF parameters, and key hierarchy.
- [ ] Implement master key creation and unlock.
- [ ] Implement encrypted object envelope.
- [ ] Add corruption and wrong-key tests.
- [ ] Freeze repository format version `0` only after fixtures exist.

### Phase 4 - Storage Backends

- [ ] Implement local filesystem backend.
- [ ] Implement S3-compatible backend through `object_store` or a documented
      lower-level choice.
- [ ] Add storage capability model.
- [ ] Add retry, timeout, concurrency, and backoff behavior.
- [ ] Add fake object store in `sealport-testkit`.
- [ ] Add interruption and idempotency tests.

### Phase 5 - Backup Pipeline

- [ ] Implement source walking and exclusion rules.
- [ ] Implement platform metadata capture.
- [ ] Implement content-defined chunking.
- [ ] Implement compression and encryption pipeline.
- [ ] Implement chunk/index writes.
- [ ] Implement snapshot manifest creation.
- [ ] Add resumable backup state.
- [ ] Add tests for sparse trees, symlinks, permissions, large files, many
      small files, and excluded paths.

### Phase 6 - Restore Pipeline

- [ ] Implement snapshot selection by id, tag, and `latest`.
- [ ] Implement path-scoped restore.
- [ ] Implement destination safety checks.
- [ ] Implement metadata restore per platform.
- [ ] Add overwrite policy and dry-run reporting.
- [ ] Add restore verification.
- [ ] Add restore drill docs.

### Phase 7 - Listing, Search, And Diff

- [ ] Implement `snapshots`.
- [ ] Implement `ls`.
- [ ] Implement `find`.
- [ ] Implement `diff`.
- [ ] Keep output stable and machine-readable.
- [ ] Add tests for encrypted metadata lookup without leaking plaintext in the
      repository.

### Phase 8 - Check, Repair Guidance, And Doctor

- [ ] Implement repository metadata check.
- [ ] Implement configurable data subset checks.
- [ ] Implement full read-data check.
- [ ] Add deterministic corruption reports.
- [ ] Add `doctor` for environment, config, backend, and permission issues.
- [ ] Document repair guidance without promising unsafe automatic repair.

### Phase 9 - Retention And Prune

- [ ] Implement retention policy parser.
- [ ] Implement `forget`.
- [ ] Implement two-phase prune.
- [ ] Add prune marks and recovery behavior.
- [ ] Add dry-run summaries.
- [ ] Add concurrent backup/prune tests.

### Phase 10 - Key Management

- [ ] Implement `key add`.
- [ ] Implement `key remove`.
- [ ] Implement `key rotate`.
- [ ] Implement `key export-recovery`.
- [ ] Document operational recovery procedures.
- [ ] Add tests for multiple unlock methods and removed keys.

### Phase 11 - Release Engineering

- [ ] Add cargo-dist or documented release equivalent.
- [ ] Add Windows `.zip` artifacts.
- [ ] Add Unix `.tar.xz` artifacts.
- [ ] Add checksums and signatures.
- [ ] Add SBOM generation.
- [ ] Add `cargo-auditable` metadata.
- [ ] Add shell install script.
- [ ] Add PowerShell install script.
- [ ] Add release smoke tests.

### Phase 12 - V1 Hardening

- [ ] Run restore drills from local and S3-compatible repositories.
- [ ] Run interruption tests for backup, restore, check, forget, and prune.
- [ ] Run adversarial corruption tests.
- [ ] Run cross-platform metadata tests.
- [ ] Audit logs and diagnostics for secret leakage.
- [ ] Audit JSON/JSONL stability.
- [ ] Update README, docs, completions, and release notes.
- [ ] Tag v1 only after the exact release candidate passes the evidence path.

---

## Verification

Narrowest useful command first, then broaden.

Docs-only work:

```sh
git diff --check
```

Normal Rust workspace gate once the skeleton exists:

```sh
just fmt
just check
just test
just build
```

Expected `just check` shape:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
```

Expected checks as the project matures:

- Crypto and key-management unit tests.
- Repository format golden fixture tests.
- Corruption, tamper, and wrong-key tests.
- Fake object store idempotency and interruption tests.
- Local backend integration tests.
- S3-compatible integration tests against an isolated test bucket or emulator.
- CLI golden tests for help, JSON, JSONL, and exit codes.
- Platform metadata tests on every supported platform.
- Restore drill from real snapshots.
- Release artifact smoke tests.

If a command cannot run, report why and what was verified instead.

---

## External Sources To Re-check

Use current primary sources before implementation work that depends on
external behavior:

- Rust stable release, edition, MSRV, Cargo workspace, and target support.
- Windows MSVC, macOS, Linux GNU/musl, FreeBSD, NetBSD, and OpenBSD target
  status.
- clap, tokio, serde, tracing, miette, color-eyre, object_store, OpenDAL,
  fastcdc, blake3, zstd, secrecy, zeroize, and Argon2id crate guidance.
- Current cryptographic guidance for AEAD, KDF parameters, nonce handling, and
  key rotation.
- S3-compatible storage behavior, multipart upload behavior, retry semantics,
  consistency guarantees, and provider-specific limits.
- Azure Blob, GCS, WebDAV, and Backblaze B2 docs before adding those backends.
- cargo-dist, signing, SBOM, `cargo-auditable`, Homebrew, Scoop, WinGet,
  FreeBSD ports, and pkgsrc release docs.

Trust current primary docs and observed behavior over this file.

---

## Recent Work

- 2026-05-17 - Created the initial SealPort planning docs:
  `README.md`, `BUILD.md`, and `AGENTS.md`.
