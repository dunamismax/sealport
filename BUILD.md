# BUILD.md

Active build plan for FileFerry and its `ferry` command.

`README.md` explains the product. `AGENTS.md` holds durable repo operating
rules. This file tracks current state, implementation phases, release scope,
and verification.

Treat unchecked boxes as plan. Move stable material into `docs/`, `README.md`,
or runbooks as the implementation matures.

Last reviewed: 2026-05-18.

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
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`; S3-compatible repository
  bootstrap is still not wired into the CLI.
- `ferry backup` opens initialized local repositories, unlocks them with
  `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, creates encrypted,
  compressed, deduplicated snapshots through the core backup pipeline, and
  exposes tested human, JSON, and JSONL-safe output paths.
- `ferry snapshots` and `ferry ls` open initialized local repositories,
  authenticate committed encrypted manifests, and expose tested human, JSON,
  and JSONL-safe output paths.
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
- `fileferry-core` can restore regular-file content to a destination directory
  with destination safety checks, explicit overwrite policy, dry-run reporting,
  and optional byte-for-byte verification.
- `fileferry-core` writes commit markers after encrypted snapshot manifests,
  can discover committed manifests from those markers, and has tested snapshot
  summary and immediate-entry listing primitives for future `snapshots` and
  `ls` commands.
- `fileferry-web` serves the public `fileferry.app` homepage with Axum,
  server-rendered Leptos views, embedded CSS, and a `/healthz` endpoint.
- The initial product brief has been distilled into `README.md`,
  `BUILD.md`, and `AGENTS.md`.
- The product target is an all-Rust, cross-platform, encrypted backup CLI
  named `ferry`.

The repo is still pre-v1 and restore is not wired into the CLI. Describe
backup, restore, repository, storage, crypto, or platform behavior only to the
level backed by code, tests, and platform evidence.

The `fileferry-web` crate is public marketing infrastructure only. It does not
turn FileFerry into a backup server, hosted product, daemon, scheduler, or web
application.

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
- [ ] `ferry init` creates encrypted local and S3-compatible repositories.
- [x] `ferry backup` creates encrypted, compressed, deduplicated snapshots.
- [ ] `ferry restore` restores by snapshot id, tag, path, and `latest`.
- [x] `ferry snapshots` and `ferry ls` have human, JSON, and JSONL-safe
      behavior where appropriate.
- [ ] `ferry check` verifies metadata and configurable data subsets.
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
- [ ] At least one restore drill is documented from a real FileFerry snapshot.

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
- [ ] Add restore drill docs.

### Phase 7 - Listing, Search, And Diff

- [x] Implement `snapshots`.
- [x] Implement `ls`.
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

- [x] Implement retention policy parser.
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
