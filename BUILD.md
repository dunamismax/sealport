# BUILD.md

Active build plan for FileFerry and its `ferry` command.

`README.md` explains the product. `AGENTS.md` holds durable repo operating
rules. This file tracks current state, implementation phases, release scope,
and verification.

Treat unchecked boxes as plan. Move stable material into `docs/`, `README.md`,
or runbooks as the implementation matures.

Last reviewed: 2026-05-19.

---

## Current Baseline

- Repository exists with MIT license.
- `origin` fetches from and pushes to
  `https://github.com/dunamismax/fileferry.git`.
- Rust workspace exists with the target crate boundaries, `fileferry-cli`
  binary, `fileferry-web` homepage binary, `just` verification recipes, and
  GitHub Actions CI.
- `ferry version` supports human, JSON, and JSONL output.
- `ferry completion <SHELL>` generates shell completion scripts.
- `ferry init` creates encrypted local filesystem repositories from
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, and encrypted
  S3-compatible repository bootstraps from `s3://bucket[/prefix]` URLs plus
  explicit S3 endpoint, region, and credential environment variables.
- `ferry backup` opens initialized local repositories, unlocks them with
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, creates encrypted,
  compressed, deduplicated snapshots through the core backup pipeline, and
  exposes tested human, JSON, and JSONL-safe output paths.
- `ferry snapshots` and `ferry ls` open initialized local repositories,
  authenticate committed encrypted manifests, and expose tested human, JSON,
  and JSONL-safe output paths.
- `ferry restore` opens initialized local repositories, unlocks with
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, selects snapshots by
  latest, snapshot id, or tag, restores all or path-scoped directory entries,
  regular-file contents, Unix symlinks, and captured modified timestamps for
  restored regular files and directories through the core restore pipeline,
  enforces destination fail-if-exists safety unless `--overwrite` is supplied
  for regular files, preflights destination safety for selected directories,
  regular files, and symlinks before destination writes, rejects requested
  snapshot-relative restore paths that do not match manifest entries before
  destination writes, creates missing parent directories for path-scoped
  symlink restores after destination safety preflight, supports dry-run
  reporting including planned modified timestamp metadata and timestamp
  planning warnings, returns partial-success exit code `10` when metadata
  warnings are produced, and exposes tested human, JSON, and JSONL-safe output
  paths. Authenticated manifests with invalid entry topology are rejected as
  integrity failures before restore destination writes.
- `ferry check` opens initialized local repositories, unlocks with
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, authenticates committed
  manifests and chunk indexes, reads/decompresses every referenced chunk, and
  verifies keyed chunk identities. It also accepts
  `--read-data-subset <N|PERCENT>` for deterministic count-based or
  percentage-based referenced-chunk subsets after committed metadata has been
  authenticated and validated. Runtime check failures in JSON and JSONL modes
  emit stable machine-readable failure envelopes with object keys, including
  encrypted-object authentication failures, and `CheckFinding`-shaped
  integrity details where the failing core error carries enough context.
  Missing objects referenced by committed repository metadata are reported as
  integrity failures instead of uninitialized-repository failures.
  Manifest/index chunk-reference mismatches and chunk decompression failures
  now retain snapshot-relative path, snapshot id, and object-key context where
  committed metadata provides it. Invalid decrypted manifest entry paths,
  duplicate entry paths, non-file chunk references, regular-file
  size/chunk-length mismatches, and non-directory ancestors are reported as
  integrity failures with snapshot id, object key, and path context where
  available. Metadata identity mismatches retain the repository object key in
  CLI machine-readable failure output.
- `ferry forget` opens initialized local repositories, unlocks with
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, authenticates currently
  visible committed encrypted manifests, evaluates `fileferry-policy` keep
  rules, supports dry-run, and writes immutable snapshot forget markers only
  when not in dry-run. It does not delete chunks, manifests, indexes, or
  commit objects; storage reclamation remains unimplemented until prune lands.
  JSON and JSONL output report candidate snapshots, kept snapshots, forgotten
  snapshots, item-level reasons, dry-run status, marker objects written, and
  explicit `object_deletion: false`.
- CLI config discovery, profiles, environment precedence, redacted
  diagnostics, and machine-output envelopes exist for the current command
  surface.
- Format v0 security and repository-format design docs exist.
- `fileferry-crypto` has initial tested primitives for master-key creation,
  passphrase key-slot unlock, HKDF subkeys, and XChaCha20-Poly1305 object
  envelopes.
- `fileferry-storage` has a tested object-store trait, capability model,
  validated object keys, local filesystem backend, S3-compatible backend, and
  reusable retry/timeout/backoff/concurrency policy wrapper.
- `fileferry-policy` has a tested parser for count-based and tag-based
  retention keep rules.
- `docs/platform-metadata.md` defines the v1 metadata capture target and
  restore reporting behavior for unrepresentable metadata.
- `fileferry-platform` has initial tested portable metadata capture for entry
  kind, regular-file size, timestamps where exposed by `std`, symlink targets,
  and Unix mode/ownership where available.
- `fileferry-testkit` has a tested in-memory fake object store for future
  repository and pipeline tests.
- `fileferry-core` has a tested deterministic source walker with wildcard
  exclusion rules, symlink-aware metadata capture, and validated FastCDC
  content-defined chunk planning.
- `fileferry-core` has an initial tested backup pipeline that compresses
  planned chunks with zstd, encrypts chunk/index/manifest objects, writes them
  through the object-store trait, deduplicates same-content chunks by keyed
  chunk identity, and creates an encrypted snapshot manifest.
- `fileferry-core` can read back encrypted snapshot manifests and chunk indexes
  with authenticated object contexts and decrypted metadata identity checks.
- `fileferry-core` has initial tested restore primitives for manifest
  timestamps, snapshot selection by id/tag/latest, and path-scoped regular-file
  content reassembly from encrypted chunks.
- `fileferry-core` can restore directory entries, regular-file content, Unix
  symlinks, and regular-file/directory modified timestamps to a destination
  directory with destination safety checks, explicit overwrite policy, dry-run
  reporting, timestamp metadata planning, and optional byte-for-byte
  verification.
- `fileferry-core` writes commit markers after encrypted snapshot manifests,
  can discover committed manifests from those markers, and has tested snapshot
  summary and immediate-entry listing primitives for future `snapshots` and
  `ls` commands.
- `fileferry-core` writes explicit snapshot forget markers and filters
  forgotten snapshots from normal committed manifest discovery without deleting
  repository objects.
- `fileferry-web` serves the public `fileferry.app` homepage with Axum,
  server-rendered Leptos views, embedded CSS, and a `/healthz` endpoint.
- The initial product brief has been distilled into `README.md`,
  `BUILD.md`, and `AGENTS.md`.
- The product target is an all-Rust, cross-platform, encrypted backup CLI
  named `ferry`.

The repo is still pre-v1. Restore is wired into the CLI for initialized local
repositories, directory entries, regular-file contents, Unix symlinks, and
modified timestamps for restored regular files and directories, but broader
metadata application and S3-compatible backup/restore/check/forget paths are
not complete. `ferry check` supports full referenced-chunk verification and
deterministic count/percentage referenced-chunk subsets for initialized local
repositories. `ferry forget` is marker-only for initialized local repositories;
it hides forgotten snapshots from normal snapshot discovery but does not
delete objects or reclaim storage. Describe backup, restore, check, forget,
repository, storage, crypto, or platform behavior only to the level backed by
code, tests, and platform evidence.

The `fileferry-web` crate is public marketing infrastructure only. It does not
turn FileFerry into a backup server, hosted product, daemon, scheduler, or web
application.

---

## Active Milestones

This section is the current execution queue. Agents should prefer completing
one active milestone end to end over making many small adjacent improvements.
Do not spend another pass on restore/check polish unless it directly supports
one of these milestones or fixes a verified bug.

If a milestone is too large for one work session, split it into explicit
sub-milestones in this section before coding. Each sub-milestone needs its own
definition of done and non-goals. Only check boxes elsewhere in this file when
the completed implementation and verification satisfy the full stated scope.

### Milestone 1 - Configurable Check Subsets

Goal: Implement `ferry check --read-data-subset <N|PERCENT>` end to end for
initialized local repositories.

Definition of done:

- CLI parses, validates, and documents `--read-data-subset`.
- Core supports full checks and deterministic subset checks over referenced
  chunks.
- Subset selection is stable for the same repository state and does not depend
  on object-store listing order.
- JSON and JSONL report `read_data_mode` and `read_data_subset` accurately.
- Invalid subset arguments fail with exit code `2`.
- Corruption, tampering, decompression, identity, and missing-object failures
  still fail with exit code `6`.
- Tests cover full check behavior, count subset, percentage subset, invalid
  subset arguments, deterministic selection, and at least one subset integrity
  failure.
- `docs/cli-contract.md` and this file reflect only the implemented behavior.

Non-goals:

- Repair.
- `doctor`.
- S3-specific check behavior.
- Probabilistic or background checking.
- Changing the repository format.

### Milestone 2 - S3-Compatible Init

Goal: Make `ferry init s3://...` create encrypted S3-compatible repositories
through the existing storage abstraction.

Status: Complete as of 2026-05-19 for encrypted bootstrap creation only.

Definition of done:

- CLI accepts S3-compatible repository URLs for `init`.
- Required endpoint, region, bucket, prefix, credential, and environment
  behavior is documented.
- Secrets and repository URLs are redacted in human, JSON, JSONL, debug, and
  error output.
- Init writes the same encrypted bootstrap model used by local repositories.
- Existing unsupported-format and wrong-password behavior remains unchanged.
- Tests use a fake store, emulator, or gated isolated integration path that
  cannot touch non-test repositories.
- Backblaze B2 development behavior follows
  `docs/backblaze-b2-dev-storage.md` when live credentials are used.
- `README.md`, docs, and this file do not claim S3 backup, restore, check, or
  v1 support unless those paths are implemented and verified.

Non-goals:

- S3 backup or restore unless completed end to end in the same milestone.
- Broad cloud-provider support.
- Multipart upload lifecycle changes.
- Release support claims.

### Milestone 3 - Forget Without Prune

Goal: Implement safe `ferry forget` planning and snapshot forget markers
without deleting repository objects.

Status: Complete as of 2026-05-19 for initialized local repositories.

Definition of done:

- CLI exposes `ferry forget` with documented dry-run behavior.
- Retention selection uses `fileferry-policy` keep rules where implemented.
- Forget writes explicit repository state or markers only when not in dry-run.
- Forget does not delete chunks, manifests, indexes, or commit objects unless
  prune is implemented and verified separately.
- JSON and JSONL report candidate snapshots, kept snapshots, forgotten
  snapshots, dry-run status, and item-level reasons.
- Human output writes diagnostics to stderr and does not put progress in
  stdout data modes.
- No-match behavior and invalid policy arguments have stable exit codes.
- Tests cover tag rules, count rules, dry-run, no-match behavior,
  non-interactive operation, JSON/JSONL envelopes, and repository state after
  forget.
- Docs clearly state that object deletion is not implemented until prune lands.

Non-goals:

- Object deletion.
- Two-phase prune sweep.
- Storage reclamation claims.
- Automatic repair.

### Milestone 4 - Local Backend Interruption And Corruption Evidence

Goal: Turn local backend reliability from partially tested behavior into
documented evidence for v1 planning.

Definition of done:

- Local repository tests cover interrupted or partial writes where practical.
- Tests cover missing objects, stale temporary objects, malformed objects,
  permission errors, and immutable-write conflicts through command or core
  boundaries.
- Failures map to documented exit-code families.
- JSON/JSONL failure envelopes preserve safe object-key/path context where
  available.
- `docs/operations.md` or a dedicated local-backend runbook documents the
  tested evidence without claiming platform-wide support.

Non-goals:

- S3-compatible backend claims.
- Full platform support claims.
- Repair or automatic cleanup beyond implemented behavior.

### Milestone 5 - Key Management First Slice

Goal: Implement the smallest useful key-management command slice without
weakening repository encryption.

Definition of done:

- Choose one command first: `ferry key add`, `ferry key remove`, `ferry key
  rotate`, or `ferry key export-recovery`.
- Document the exact command semantics before implementation if the existing
  security docs are not specific enough.
- CLI supports human, JSON, and JSONL-safe output.
- Every prompt has a non-interactive alternative.
- Tests cover success, wrong password/key, malformed repository state,
  redaction, and exit-code mapping.
- Security docs explain what the command does not rewrite or recover.

Non-goals:

- Completing all key-management commands in one pass unless each is fully
  implemented and tested.
- Rewriting existing encrypted backup data unless explicitly designed,
  documented, and verified.
- Silent weakening or bypassing of KDF/key-slot behavior.

## Current Deprioritized Polish

Do not choose these as primary work unless a test proves a bug or the work is
required by an active milestone:

- More restore edge-case diagnostics around already-covered destination
  preflight behavior.
- More check failure-envelope polish where object-key/path/snapshot context is
  already available.
- Wording-only documentation edits that do not unblock an active milestone.
- Refactors that do not remove a blocker for an active milestone.
- Broad platform, S3, metadata, prune, repair, release, or v1 claims without
  implementation and verification.

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
- **tracing-subscriber** for the CLI/logging boundary once runtime logging
  grows beyond the current command surface.
- **figment** or **config** only if profile, environment, and file layering
  outgrows the current explicit config loader.
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
- **Axum** plus server-rendered **Leptos** for the separate public homepage
  binary.

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
- The repository format is original to FileFerry.
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

- [x] Choose AEAD and key hierarchy, then document rationale.
- [x] Define repository bootstrap plaintext and justify every field.
- [x] Define KDF parameters, migration story, and unlock UX.
- [x] Define recovery export format and user warning text.
- [x] Define key rotation semantics and what rotation does not rewrite.
- [x] Define tamper/corruption error classes and JSON output.
- [x] Add `docs/security.md`.
- [x] Add adversarial tests for wrong password, wrong key, bit flips, truncated
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

- [x] Write `docs/repository-format.md` before committing to object bytes.
- [x] Define object naming without leaking source paths.
- [x] Define object authentication context and domain separation.
- [x] Define snapshot manifest structure.
- [x] Define chunk index structure.
- [x] Define commit markers and upload state.
- [x] Define repository lock or lease model, if any.
- [x] Define concurrent backup behavior.
- [x] Define prune mark, sweep, and recovery behavior.
- [ ] Add golden fixtures after the first format version is intentionally
      frozen.

---

## Target Source Layout

```text
Cargo.toml
rust-toolchain.toml
justfile
crates/
  fileferry-cli/       command parsing, output formats, config loading
  fileferry-core/      snapshots, repository format, backup/restore engine
  fileferry-storage/   local and object storage abstraction
  fileferry-crypto/    key derivation, encryption, authenticated metadata
  fileferry-platform/  filesystem metadata across Windows/macOS/Linux/BSD
  fileferry-policy/    retention, pruning, lifecycle rules
  fileferry-testkit/   fake stores, corruption tests, fixtures, helpers
  fileferry-web/       Axum + Leptos public homepage for fileferry.app
xtask/
docs/
  architecture.md
  cli-contract.md
  config.md
  homepage-deployment.md
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

Keep command presentation in `fileferry-cli`. Library crates should return typed
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
ferry init
ferry backup
ferry restore
ferry snapshots
ferry ls
ferry check
ferry forget
ferry prune
ferry key
ferry completion
ferry version
```

High-value commands that should land before or shortly after v1 if the core is
stable:

```text
ferry find
ferry diff
ferry copy
ferry repo
ferry policy
ferry doctor
```

CLI work:

- [x] Define stable exit codes in `docs/cli-contract.md`.
- [x] Define JSON document schemas for every command.
- [x] Define JSONL event schemas for long operations.
- [x] Add golden tests for help text, JSON output, JSONL event order, and exit
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

- [x] Define metadata capture for files, directories, symlinks, permissions,
      timestamps, ownership, xattrs, ACLs, resource forks, and Windows
      attributes.
- [x] Decide v1 restore behavior for metadata that cannot be represented on
      the destination platform.
- [ ] Add platform-specific tests for path normalization, reserved names,
      symlinks, hard links if supported, case sensitivity, long paths, and
      permission errors.
- [ ] Add CI for supported platforms before claiming support.
- [ ] Add release artifacts only for platforms that pass the support bar.

---

## V1 Release Definition

FileFerry is ready for v1 only when it is boring to initialize, back up, verify,
restore, automate, and install across the supported platform list.

Minimum v1 bar:

- [x] Rust workspace exists with the target crate boundaries.
- [x] `ferry init` creates encrypted local and S3-compatible repositories.
- [x] `ferry backup` creates encrypted, compressed, deduplicated snapshots.
- [x] `ferry restore` restores by snapshot id, tag, path, and `latest`.
- [x] `ferry snapshots` and `ferry ls` have human, JSON, and JSONL-safe
      behavior where appropriate.
- [x] `ferry check` verifies metadata and configurable data subsets.
- [ ] `ferry forget` and `ferry prune` implement retention and two-phase
      deletion safely.
- [ ] Key add/remove/rotate/export-recovery paths exist and are tested.
- [ ] Local backend passes interruption and corruption tests.
- [ ] S3-compatible backend passes retry, resume, and eventual-weirdness tests.
- [x] Stable config profiles and environment variables exist.
- [x] Shell completions are generated.
- [ ] Exit codes, JSON, and JSONL schemas are documented and tested.
- [ ] Platform metadata behavior is documented and tested on every supported
      platform.
- [ ] Release artifacts include archives, checksums, signatures, SBOM, and
      `cargo-auditable` metadata.
- [ ] Install scripts for Unix shells and PowerShell are tested.
- [x] At least one restore drill is documented from a real FileFerry snapshot.

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

- [x] Add `rust-toolchain.toml`.
- [x] Add Cargo workspace with Rust 2024 edition.
- [x] Add crates: `fileferry-cli`, `fileferry-core`, `fileferry-storage`,
      `fileferry-crypto`, `fileferry-platform`, `fileferry-policy`, and
      `fileferry-testkit`.
- [x] Add workspace dependency policy.
- [x] Add `fileferry-cli` binary with `clap`.
- [x] Add `just fmt`, `just check`, `just test`, and `just build`.
- [x] Add GitHub Actions for formatting, clippy, tests, and build.
- [x] Add basic `ferry version`.

### Phase 2 - CLI, Config, And Output Contract

- [x] Implement config discovery and profiles.
- [x] Implement global flags and environment variable precedence.
- [x] Add typed config validation and redacted diagnostics.
- [x] Revisit `figment` or `config` only if the current explicit loader
      becomes harder to audit than a small dependency-backed layering model.
- [x] Define stable event model for command progress.
- [x] Implement human, JSON, and JSONL output surfaces.
- [x] Add CLI golden tests.
- [x] Add `ferry completion`.

### Phase 3 - Crypto And Format Design

- [x] Write `docs/security.md`.
- [x] Write `docs/repository-format.md`.
- [x] Choose AEAD, KDF parameters, and key hierarchy.
- [x] Implement master key creation and unlock.
- [x] Implement encrypted object envelope.
- [x] Add corruption and wrong-key tests.
- [ ] Freeze repository format version `0` only after fixtures exist.

### Phase 4 - Storage Backends

- [x] Implement local filesystem backend.
- [x] Implement S3-compatible backend through `object_store` or a documented
      lower-level choice.
- [x] Add storage capability model.
- [x] Add retry, timeout, concurrency, and backoff behavior.
- [x] Add fake object store in `fileferry-testkit`.
- [x] Add interruption and idempotency tests.

### Public Homepage - fileferry.app

- [x] Add a separate `fileferry-web` workspace crate so homepage dependencies
      stay out of the backup CLI/runtime crates.
- [x] Build the public homepage with Axum and server-rendered Leptos views.
- [x] Serve static CSS and a reverse-proxy-friendly `/healthz` endpoint.
- [x] Document Ubuntu self-hosting shape for `fileferry.app`.
- [x] Add route and render tests for the homepage.

### Phase 5 - Backup Pipeline

- [x] Implement source walking and exclusion rules.
- [x] Implement platform metadata capture.
- [x] Implement content-defined chunking.
- [x] Implement compression and encryption pipeline.
- [x] Implement chunk/index writes.
- [x] Implement snapshot manifest creation.
- [ ] Add resumable backup state.
- [x] Add tests for sparse trees, symlinks, permissions, large files, many
      small files, and excluded paths.

### Phase 6 - Restore Pipeline

- [x] Implement snapshot selection by id, tag, and `latest`.
- [x] Implement path-scoped restore.
- [x] Implement destination safety checks.
- [ ] Implement metadata restore per platform.
- [x] Add overwrite policy and dry-run reporting.
- [x] Add restore verification.
- [x] Add restore drill docs.

### Phase 7 - Listing, Search, And Diff

- [x] Implement `snapshots`.
- [x] Implement `ls`.
- [ ] Implement `find`.
- [ ] Implement `diff`.
- [ ] Keep output stable and machine-readable.
- [ ] Add tests for encrypted metadata lookup without leaking plaintext in the
      repository.

### Phase 8 - Check, Repair Guidance, And Doctor

- [x] Implement repository metadata check.
- [x] Implement configurable data subset checks.
- [x] Implement full read-data check.
- [x] Add deterministic corruption reports.
- [ ] Add `doctor` for environment, config, backend, and permission issues.
- [ ] Document repair guidance without promising unsafe automatic repair.

### Phase 9 - Retention And Prune

- [x] Implement retention policy parser.
- [x] Implement `forget`.
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

- [x] Add cargo-dist or documented release equivalent.
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
- [ ] Decide whether `tracing-subscriber` should own the final CLI logging
      boundary before v1.
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

- 2026-05-19 - Completed Milestone 3 forget without prune for initialized
  local repositories. `ferry forget` now accepts retention keep flags
  (`--keep-last`, hourly/daily/weekly/monthly/yearly counts, and repeatable
  `--keep-tag`), supports `--dry-run`, evaluates selection through
  `fileferry-policy`, and writes immutable `forgets/<snapshot-id>` markers
  only when not in dry-run. Normal snapshot discovery ignores marked snapshots,
  but forget does not delete chunks, manifests, indexes, or commit objects and
  reports `object_deletion: false` in machine output. JSON and JSONL output
  include candidate, kept, and forgotten snapshot items with item-level
  reasons, dry-run status, marker counts, and stable no-match/invalid-policy
  exit behavior. Documented marker-only forget in `docs/cli-contract.md`,
  `docs/repository-format.md`, and README without claiming prune or
  S3-compatible forget support. Verified with `cargo test -p
  fileferry-policy`, `cargo test -p fileferry-core
  forget_markers_hide_snapshots_without_deleting_repository_objects`, `cargo
  test -p fileferry-cli forget_`, `cargo test -p fileferry-cli`, `cargo test
  -p fileferry-core`, `just fmt`, `just check`, `just test`, `just build`,
  and `git diff --check`.
- 2026-05-19 - Completed Milestone 2 S3-compatible init. `ferry init` now
  accepts `s3://bucket[/prefix]` repository URLs for encrypted repository
  bootstrap creation through `S3Store` wrapped in the common storage policy.
  S3 init requires explicit `FILEFERRY_S3_ENDPOINT`, `FILEFERRY_S3_REGION`,
  `FILEFERRY_S3_ACCESS_KEY_ID`, and `FILEFERRY_S3_SECRET_ACCESS_KEY`
  environment variables; credentials in repository URLs, query strings, and
  fragments are rejected. S3 repository URLs are redacted as
  `s3://<redacted>` in human, JSON, JSONL, and error output, and S3 config
  debug output keeps credentials redacted. Added unit coverage for S3 URL and
  environment parsing, CLI coverage for missing S3 environment and redaction,
  and a gated live CLI init test under `FILEFERRY_S3_INIT_INTEGRATION=1` that
  can only initialize a unique prefix below `FILEFERRY_S3_TEST_PREFIX`.
  Documented the CLI S3 init environment contract in `docs/cli-contract.md`,
  S3 storage notes, Backblaze B2 development notes, and README status without
  claiming S3 backup, restore, snapshots, ls, check, or v1 storage support.
  Verified with `cargo test -p fileferry-cli`, `just fmt`, `just check`,
  `just test`, `just build`, and `git diff --check`.
- 2026-05-19 - Completed Milestone 1 configurable check subsets for
  initialized local repositories. `ferry check` now accepts
  `--read-data-subset <N|PERCENT>`, validates counts and percentages as usage
  errors, authenticates all committed metadata before data reads, and selects
  deterministic referenced-chunk subsets from sorted chunk identities so the
  selected subset does not depend on object-store listing order. JSON and
  JSONL success output now report `read_data_mode: "subset"` and the requested
  `read_data_subset` for subset checks while full checks keep
  `read_data_mode: "full"` and `read_data_subset: null`; subset integrity
  failures still map to exit code `6`. Documented the implemented local check
  subset behavior in `docs/cli-contract.md` and updated README/BUILD status
  without adding S3, repair, doctor, or background-check claims. Verified with
  `cargo test -p fileferry-core check_repository_`, `cargo test -p
  fileferry-cli check_read_data_subset`, `cargo test -p fileferry-core -p
  fileferry-cli`, `just fmt`, `just check`, `just test`, `just build`, and
  `git diff --check`.
- 2026-05-19 - Reworked `BUILD.md` as a sharper execution queue for future
  agents without changing implementation status or checking off feature work.
  Added Active Milestones with definitions of done and non-goals for
  configurable check subsets, S3-compatible init, forget without prune, local
  backend interruption/corruption evidence, and the first key-management
  command slice. Added a Current Deprioritized Polish section so future passes
  prefer milestone completion over repeated small restore/check polish. Verified
  with `git diff --check`.
- 2026-05-19 - Tightened path-scoped Unix symlink restore and check identity
  diagnostics without expanding metadata or platform claims. Path-scoped
  symlink restores now create missing destination parent directories after
  destination safety preflight, matching regular-file parent handling while
  still rejecting existing symlink paths and symlinked ancestors.
  `fileferry-core` metadata identity mismatches now carry the repository object
  key, and `fileferry-cli` includes that key in JSON/JSONL check failure
  envelopes and `CheckFinding` details. Documented the path-scoped symlink
  behavior in `docs/cli-contract.md`. Verified initially with `cargo test -p
  fileferry-core path_scoped_symlink`, `cargo test -p fileferry-cli
  restore_path_scoped_symlink_creates_missing_parent_directory`, `cargo test -p
  fileferry-core repository_metadata_reads_reject_replayed_indexes_and_malformed_metadata`,
  and `cargo test -p fileferry-cli
  check_failure_finding_preserves_metadata_identity_object_key`; then with
  `cargo test -p fileferry-core -p fileferry-cli`, `just fmt`, `just check`,
  `just test`, `just build`, and `git diff --check`.
- 2026-05-19 - Tightened authenticated manifest validation before restore and
  check work without broadening repository format claims. `fileferry-core` now
  rejects decrypted manifests with invalid snapshot-relative entry paths,
  duplicate entry paths, non-file chunk references, regular-file
  size/chunk-length mismatches, or child entries whose recorded ancestor is not
  a directory. Restore rejects those manifests before destination writes, and
  `ferry check` reports them as integrity failures with snapshot id, manifest
  object key, and path context where available. `fileferry-cli` maps the new
  `snapshot_manifest_invalid` failure to exit code `6` and includes the
  context in check JSON/JSONL finding envelopes. Documented the behavior in
  `docs/cli-contract.md` and `docs/repository-format.md`. Verified initially
  with `cargo test -p fileferry-core invalid_manifest` and `cargo test -p
  fileferry-cli check_failure_finding_preserves`, then with `cargo test -p
  fileferry-core`, `cargo test -p fileferry-cli`, and the full `just fmt`,
  `just check`, `just test`, `just build`, and `git diff --check` gate.
- 2026-05-18 - Tightened restore destination safety without broadening restore
  scope. `fileferry-core` now preflights destination safety for all selected
  directories, regular files, and symlinks before any non-dry-run destination
  writes, so a later fail-if-exists conflict does not leave earlier selected
  entries behind. Added core and CLI regression coverage proving an existing
  destination file returns exit code `2` in JSON mode, keeps stderr empty, and
  leaves earlier selected directories unwritten. Documented the narrower
  restore guarantee in `docs/cli-contract.md`. Verified initially with
  targeted `cargo test -p fileferry-core
  restore_snapshot_to_destination_preflights_conflicts_before_writes` and
  `cargo test -p fileferry-cli
  restore_json_failure_preflights_destination_conflicts_before_writes`, then
  with `cargo test -p fileferry-core`, `cargo test -p fileferry-cli`, and the
  full `just fmt`, `just check`, `just test`, `just build`, and
  `git diff --check` gate.
- 2026-05-18 - Tightened `ferry check` integrity diagnostics for committed
  chunk-reference failures without adding repair or subset-check behavior.
  `fileferry-core` now carries snapshot id, snapshot-relative path, and
  object-key context through manifest/index chunk-reference mismatches and
  referenced chunk decompression failures when committed metadata provides
  that context. `fileferry-cli` maps those cases to integrity exit code `6`
  and includes the context in JSON/JSONL failure envelopes and `CheckFinding`
  details. Documented the narrower machine-output behavior in
  `docs/cli-contract.md`. Verified initially with targeted `cargo test -p
  fileferry-core ...` and `cargo test -p fileferry-cli ...` commands, then
  with `cargo test -p fileferry-core`, `cargo test -p fileferry-cli`, and the
  full `just fmt`, `just check`, `just test`, `just build`, and
  `git diff --check` gate.
- 2026-05-18 - Tightened restore and repository-incompatibility failure
  behavior without broadening command scope. `fileferry-core` now rejects any
  requested restore `--path` that matches no manifest entry before destination
  writes, and `fileferry-cli` reports that as `snapshot_path_not_found` with
  exit code `7` in machine output. Restore JSONL coverage now proves missing
  referenced chunks fail with an integrity envelope before destination writes.
  Unsupported repository format versions and declared repository features now
  have explicit core error classes and CLI failure codes mapped to the
  incompatible-repository exit family `3`, while malformed bootstrap JSON
  remains an integrity failure. Documented the behavior in
  `docs/cli-contract.md`. Verified initially with targeted `cargo test -p
  fileferry-core ...` and `cargo test -p fileferry-cli ...` commands, then
  with the full `just fmt`, `just check`, `just test`, `just build`, and
  `git diff --check` gate.
- 2026-05-18 - Tightened local repository open/check diagnostics without
  broadening command scope. `fileferry-core` now reports missing objects
  referenced by committed repository metadata as integrity failures outside
  `check` as well, and encrypted repository-object authentication failures now
  retain the object key for JSON/JSONL diagnostics and check findings.
  `fileferry-cli` maps those cases to stable machine-readable failure codes
  while preserving stdout/stderr separation and existing exit-code families.
  Added CLI integration coverage for uninitialized repositories, unsupported
  redacted S3 URLs, wrong passwords, corrupted bootstrap JSON, missing
  referenced manifests, malformed commit markers, and tampered encrypted
  metadata. Verified initially with `cargo test -p fileferry-core` and
  `cargo test -p fileferry-cli`.
- 2026-05-18 - Improved restore dry-run metadata reporting without broadening
  platform metadata claims. `fileferry-core` now reports `metadata_planned`
  for restored regular-file and directory modified timestamp fields and runs
  the same denied/unsupported/invalid timestamp planning checks during
  dry-run, while still applying only captured modified timestamps for regular
  files and directories during real restores. `fileferry-cli` includes
  `metadata_planned` in restore JSON/JSONL output, preserves warning behavior
  on stdout for machine modes, and documents the field in
  `docs/cli-contract.md` and `docs/platform-metadata.md`. Refreshed the local
  operations drill in `docs/operations.md` with observed modified-timestamp
  verification for one regular file and one nested directory. Verified
  initially with `cargo test -p fileferry-core` and `cargo test -p
  fileferry-cli`.
- 2026-05-18 - Added the first narrow restore metadata application slice for
  initialized local repositories. `fileferry-core` now carries captured
  modified timestamps through restore planning and applies them to restored
  regular files and directories after content writes; symlink timestamps,
  ownership, mode bits, ACLs, xattrs, resource forks, Windows attributes, BSD
  flags, and other platform-specific metadata remain unimplemented. Restore
  results now report `metadata_applied` from core, and metadata warning output
  uses partial-success exit code `10` while preserving JSON/JSONL stdout and
  stderr separation. Added core tests for successful file/directory timestamp
  application and warning generation, CLI integration coverage for restored
  file mtimes, and CLI unit coverage for partial-success warning output.
- 2026-05-18 - Improved `ferry check` corruption diagnostics and
  machine-readable failure behavior without adding repair or subset-check
  claims. Authenticated-object decode and metadata decode errors now retain the
  repository object key, and check reads map missing manifest/index objects
  referenced by repository metadata to integrity exit code `6` instead of a
  repository-not-found class. Runtime failures in `--json` and `--jsonl` modes
  now emit stable failure envelopes on stdout with `code`, `exit_code`,
  `retryable`, optional `path`, optional `object_key`, and `finding` details
  for check integrity failures; human mode still writes diagnostics to stderr.
  Added CLI integration tests for missing-chunk JSON failure and tampered-chunk
  JSONL failure with empty stderr, plus targeted core/CLI checks.
- 2026-05-18 - Expanded local restore and added the first `ferry check`
  implementation. Restore now writes explicit directory entries, regular-file
  contents, and Unix symlinks from initialized local repositories; it keeps
  destination containment checks, rejects symlinked destination ancestors and
  pre-existing symlink paths, creates symlinks after directory/file writes,
  reports `directories_written`, `symlinks_written`, `metadata_applied`, and
  `metadata_warnings` honestly, and still leaves metadata application
  unclaimed. Added core and CLI integration tests for directory/symlink
  restore success and symlink destination safety. `ferry check` now opens
  initialized local repositories, authenticates commit markers, encrypted
  manifests, encrypted indexes, and every referenced chunk, decompresses chunk
  payloads, verifies keyed chunk identities, and emits human, JSON, and JSONL
  output with `read_data_mode: "full"` and no configurable subset support.
  Added tests for wrong passwords, uninitialized repositories, missing chunks,
  tampered chunks, and JSON/JSONL success output. Ran a local
  `init -> backup -> restore -> check` drill against a temporary repository
  with an empty directory tree, a regular file, and a Unix symlink; verified
  file bytes with `cmp`, directory existence with `test -d`, symlink target
  with `readlink`, and check counts in JSON. Verified initially with targeted
  `cargo test -p fileferry-core ...` and `cargo test -p fileferry-cli ...`.
- 2026-05-18 - Wired `ferry restore` into `fileferry-cli` for initialized
  local repositories. The command unlocks through `FILEFERRY_PASSWORD` or
  `FILEFERRY_PASSWORD_FILE`, selects latest by default or via `--latest`,
  `--snapshot`, or `--tag`, accepts repeated snapshot-relative `--path`
  filters, restores regular-file contents through the existing core restore
  pipeline, verifies written bytes, supports `--dry-run`, and enforces
  fail-if-exists destination safety unless `--overwrite` is supplied. Added
  CLI integration tests for real `init -> backup -> restore` file-byte
  recovery, JSONL restore phases, wrong-password failure, and destination
  safety/overwrite behavior. Added `docs/operations.md` with a local restore
  drill performed against a temporary real FileFerry snapshot and byte-checked
  with `cmp`; no S3, metadata, directory-entry, or symlink restore coverage is
  claimed. Verified initially with `cargo test -p fileferry-cli`.
- 2026-05-18 - Wired `ferry backup` into `fileferry-cli` for initialized local
  repositories. The command accepts one or more local source paths plus repeated
  `--tag`, unlocks through `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`,
  runs the existing core backup pipeline, commits the snapshot, and emits
  human, JSON, and JSONL-safe output with backup summary fields and progress
  phases. Extended core snapshot write results with scanned entry counts, entry
  kind counts, scanned/uploaded byte counts, chunk seen/written/reused counts,
  and index ids. Added CLI integration tests that run `ferry init`, `ferry
  backup`, `ferry snapshots`, and `ferry ls` end to end against a real local
  repository, plus JSONL and wrong-password coverage. Verified initially with
  `cargo test -p fileferry-core -p fileferry-cli`; full gate passed with
  `just fmt`, `just check`, `just test`, `just build`, and `git diff --check`.
- 2026-05-18 - Wired the first end-user repository commands into
  `fileferry-cli`: `ferry init` now creates an encrypted local filesystem
  repository bootstrap from `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`,
  and `ferry snapshots` / `ferry ls` open initialized local repositories,
  authenticate committed encrypted manifests, and emit human, JSON, and
  JSONL-safe output. Added core bootstrap/open tests and CLI integration tests
  that initialize a real local repository, write a committed snapshot through
  the core pipeline, then list snapshots and entries through the `ferry`
  binary. S3-compatible repository bootstrap remains unchecked. Verified with
  `cargo test -p fileferry-core -p fileferry-cli`, `just fmt`, `just check`,
  `just test`, `just build`, and `git diff --check`.
- 2026-05-18 - Added committed snapshot discovery groundwork in
  `fileferry-core`: snapshot writes now publish a commit marker after chunk,
  index, and encrypted manifest objects; committed manifests can be discovered
  from commit markers; and tested snapshot summary plus immediate-entry listing
  helpers now support future `snapshots` and `ls` commands. Documented the
  plaintext commit marker fields in `docs/repository-format.md`. Added
  `docs/release.md` as the documented release equivalent until dedicated
  release tooling lands. Verified with `cargo test -p fileferry-core`.
- 2026-05-18 - Expanded `docs/cli-contract.md` with required v1 JSON document
  data schemas for `init`, `backup`, `restore`, `snapshots`, `ls`, `check`,
  `forget`, `prune`, `key` subcommands, and `version`, plus JSONL event data
  schemas and required long-operation phase names. Verified with `git diff
  --check` and `just check`.
- 2026-05-18 - Added destination restore primitives in `fileferry-core`:
  regular-file content can now be restored under an absolute destination root
  with path containment checks, symlinked-destination rejection, explicit
  fail-if-exists or overwrite behavior, dry-run reporting, and optional
  byte-for-byte verification. Verified with `cargo test -p fileferry-core`.
- 2026-05-18 - Added the first restore pipeline slice in `fileferry-core`:
  snapshot manifests now carry creation timestamps, loaded manifests can be
  selected by id, newest matching tag, or latest overall, and restore content
  reads path-scoped regular files back from encrypted chunks with chunk identity
  checks. Verified with `cargo test -p fileferry-core`.
- 2026-05-18 - Added authenticated repository-object read helpers for snapshot
  manifests and chunk indexes, including identity checks for decrypted metadata.
  Expanded adversarial coverage for wrong repository keys, bit flips,
  truncation, swapped objects, replayed indexes, and malformed metadata. Added
  backup-pipeline tests for sparse directory trees, symlinks, unreadable files,
  large files, many small files, and excluded paths. Verified with `cargo test
  -p fileferry-core -p fileferry-testkit`.
- 2026-05-18 - Added the first core backup pipeline slice: source entries are
  chunked, zstd-compressed, encrypted through the existing authenticated object
  envelope, written as immutable chunk objects, indexed in an encrypted chunk
  index, and recorded in an encrypted snapshot manifest. Chunk object names use
  keyed content identities so duplicate chunk content is represented once
  without leaking source paths in object names. Verified with `cargo test -p
  fileferry-core`.
- 2026-05-18 - Added validated FastCDC content-defined chunk planning in
  `fileferry-core`, including default v0 chunk-size targets, bounds checking
  against the FastCDC implementation, deterministic chunk-range tests, and
  small-input behavior. Marked the existing tested platform metadata capture
  Phase 5 item complete. Verified with `cargo test -p fileferry-core`.
- 2026-05-18 - Added initial backup-source walking in `fileferry-core` with
  deterministic traversal, absolute-root validation, wildcard exclusion rules
  including `**`, directory pruning, and symlink-aware metadata capture via
  `fileferry-platform`. Added initial portable metadata capture for entry kind,
  file size, timestamps, symlink targets, and Unix mode/ownership where
  available. Revisited the explicit config loader and kept it instead of adding
  `figment` or `config` because the current precedence model remains small and
  auditable. Verified with `cargo test -p fileferry-platform` and `cargo test
  -p fileferry-core`; full gate recorded with this change.
- 2026-05-18 - Added `PolicyObjectStore` and `StoragePolicy` so storage
  operations can be bounded by retry count, per-operation timeout,
  exponential backoff, and max concurrency. Added tests for policy validation,
  retryable failures, permanent conflict handling, timeouts, backoff capping,
  and concurrency limiting. Verified with `cargo test -p fileferry-storage`.
- 2026-05-18 - Added the `fileferry-web` public homepage crate for
  `fileferry.app`: Axum server, Leptos SSR marketing page, embedded CSS,
  `/healthz`, Ubuntu deployment notes, and route/render tests. Verified with
  `cargo test -p fileferry-web`; full gate recorded with this change.
- 2026-05-18 - Added the first `fileferry-policy` retention policy parser for
  count-based keep rules and repeated tag keep rules, documented current CLI
  JSON/JSONL schemas and data-mode progress behavior, and added
  `docs/platform-metadata.md` for v1 metadata capture and cross-platform
  restore reporting decisions. Verified initially with `cargo test -p
  fileferry-policy -p fileferry-cli`; full gate recorded with this change.
- 2026-05-18 - Completed the first Phase 3 slice: documented format v0
  security choices and repository-format structure, selected Argon2id,
  HKDF-SHA-256, and XChaCha20-Poly1305, implemented tested master-key
  creation/unlock, passphrase key slots, subkey derivation, and authenticated
  object envelopes in `fileferry-crypto`. Verified with `cargo test -p
  fileferry-crypto`.
- 2026-05-18 - Completed the first Phase 4 storage slice: added validated
  object keys, the object-store trait, storage capability reporting, a local
  filesystem backend with idempotent immutable writes, leftover temp-object
  listing protection, and an in-memory fake object store in
  `fileferry-testkit`. Added `docs/storage.md`. Verified with `cargo test -p
  fileferry-storage -p fileferry-testkit`.
- 2026-05-18 - Added the first real S3-compatible storage groundwork: an
  `object_store`-backed `S3Store`, HTTPS-only explicit S3 config, redacted
  credential handling, configurable conditional create support,
  prefix-scoped live integration test gate, Backblaze B2 development-bucket
  docs, and `.env` ignore rules. Verified with `just check`; the real
  Backblaze round-trip is gated on local S3 environment variables.
- 2026-05-18 - Completed the Phase 2 CLI foundation: config discovery,
  profiles, CLI/env/config precedence, typed config validation, redacted
  diagnostics, JSON and JSONL envelopes, event names, shell completions, and
  CLI golden tests. Added `docs/cli-contract.md`. Verified with `just check`.
- 2026-05-17 - Bootstrapped the Rust workspace with the planned crate
  boundaries, workspace dependency policy, `fileferry-cli` binary, basic
  `ferry version`, `just` recipes, and GitHub Actions CI. Verified with
  `just check`, individual `just fmt`/`just test`/`just build` recipes,
  direct `ferry version` smoke checks, and workflow YAML parsing.
- 2026-05-17 - Created the initial FileFerry planning docs:
  `README.md`, `BUILD.md`, and `AGENTS.md`.
