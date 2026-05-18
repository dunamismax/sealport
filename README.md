# SealPort

Encrypted backups. Same everywhere.

SealPort is a planned all-Rust backup CLI for operators, IT directors,
developers, and teams that need reliable encrypted backups without platform
drama. The command will be `sealport`; the short alias `sp` may be added after
the primary command is stable.

SealPort is starting from an empty implementation. The active build plan lives
in [`BUILD.md`](BUILD.md). This README describes the target product and the
contracts the implementation must satisfy before release.

Future homepage: [sealport.cc](https://sealport.cc/).

## Product Promise

SealPort gives operators a secure, scriptable backup tool that behaves
predictably on every machine they manage.

It is for people who want:

- One binary.
- One config format.
- One scripting contract.
- One encrypted repository format.
- Boring restores.
- First-class Windows, macOS, Linux, and BSD behavior.

It is not a GUI, SaaS dashboard, agent service, FUSE mount, scheduler, server,
mobile app, or compatibility layer for an existing backup repository format.

## Target Command Shape

```sh
sealport init s3://company-backups/laptops
sealport backup ~/Documents --tag laptop --jsonl
sealport snapshots --json
sealport restore latest ~/restore-test
sealport check --read-data-subset 5%
sealport forget --keep-daily 14 --keep-weekly 8 --prune
```

Core command surface under design:

```text
sealport init
sealport backup
sealport restore
sealport snapshots
sealport ls
sealport find
sealport diff
sealport check
sealport forget
sealport prune
sealport key
sealport repo
sealport policy
sealport doctor
sealport completion
sealport version
```

Global flags:

```text
--repo <URL>          Repository URL
--profile <NAME>     Config profile
--config <FILE>      Config file path
--json               Emit one stable JSON document on stdout
--jsonl              Emit stable newline-delimited JSON events on stdout
--quiet              Reduce human output
--log-level <LEVEL>  Set log level
--no-progress        Disable progress UI
```

## Scripting Contract

SealPort is automation-first:

- Stdout is data.
- Stderr is logs, progress, and diagnostics.
- `--json` emits one JSON document.
- `--jsonl` emits one JSON event per line.
- Human progress is optional and never appears in JSON or JSONL output.
- Exit codes are documented and stable before v1.
- Destructive commands support `--dry-run`.
- Long operations can be interrupted safely.

Planned exit code families:

```text
0   success
1   generic failure
2   invalid command, arguments, config, or environment
3   repository not found, uninitialized, locked, or incompatible
4   authentication, password, key, or permission failure
5   storage, network, or filesystem I/O failure
6   integrity, corruption, tampering, or verification failure
7   requested snapshot, path, tag, or policy was not found
8   operation was interrupted after reaching a safe state
9   unsupported platform, filesystem feature, or backend capability
10  partial success; inspect JSON output for item-level failures
```

These numbers are a target contract. Once marked stable for v1, they should
not change without a compatibility plan.

## Security Model

SealPort repositories are encrypted client-side before anything leaves the
machine.

The repository format must protect:

- File contents.
- File names.
- Directory structure.
- Snapshot metadata.
- Indexes.
- Policy/config objects that reveal sensitive backup shape.

The target model is envelope encryption:

- Each repository has a random master key.
- Passphrases or key files unlock that master key.
- Data, metadata, and indexes use derived subkeys.
- Every encrypted object is authenticated.
- Read, check, and restore detect corruption and tampering.

Security-facing commands:

```sh
sealport key add
sealport key remove
sealport key rotate
sealport key export-recovery
sealport repo verify
sealport repo inspect --json
```

Only non-sensitive bootstrap fields, such as format version and key derivation
parameters, may be plaintext. Any plaintext repository field must be justified
in the security design before the format freezes.

## Repository Model

SealPort uses an original repository format. It does not read or write restic,
rustic, Borg, Kopia, or rclone-native repository formats.

Core object groups:

- Encrypted chunks.
- Encrypted snapshot manifests.
- Encrypted indexes.
- Encrypted policy/config object.
- Temporary upload state.
- Prune marks and maintenance metadata.

Design goals:

- Append-friendly writes.
- Safe interruption.
- No required rename operations.
- Safe concurrent backups.
- Two-phase prune.
- Deterministic integrity checks.
- Clear future migrations.

Object storage is not treated like a filesystem. Repository operations must
use immutable objects, idempotent writes, explicit commit markers, retry-safe
upload state, and backend capability checks.

## Architecture

Target Rust workspace:

```text
crates/
  sealport-cli/       clap commands, output formats, config loading
  sealport-core/      snapshots, repository format, backup/restore engine
  sealport-storage/   local and object storage abstraction
  sealport-crypto/    key derivation, encryption, authenticated metadata
  sealport-platform/  filesystem metadata across supported platforms
  sealport-policy/    retention, pruning, lifecycle rules
  sealport-testkit/   fake stores, corruption fixtures, platform helpers
xtask/                release, fixtures, and repo automation when useful
docs/                 durable architecture, security, operations, release docs
```

Expected Rust stack:

- `clap` for command parsing.
- `tokio` for async storage and network work.
- `serde`, `toml`, and `serde_json` for config and machine output.
- `tracing` for logs and instrumentation.
- `miette` or `color-eyre` for human diagnostics.
- `object_store` first for S3-compatible, cloud, and local-style object
  backends.
- Optional OpenDAL adapter later for broader backend support.
- `fastcdc` for content-defined chunking.
- `blake3` for fast content IDs and checksums.
- `zstd` for compression.
- Argon2id for passphrase key derivation.
- `zeroize` and `secrecy` for secret handling.

## Config

Target config example:

```toml
[repository]
url = "s3://company-backups/sealport/laptops"
profile = "default"

[backup]
sources = [
  "~/Documents",
  "~/Projects"
]
exclude = [
  "**/.git",
  "**/node_modules",
  "**/target",
  "**/.DS_Store"
]
tags = ["laptop", "workstation"]

[retention]
keep_daily = 14
keep_weekly = 8
keep_monthly = 12

[storage]
concurrency = 16
timeout = "60s"
retry = 5

[output]
progress = "auto"
log_level = "info"
```

Environment variables:

```text
SEALPORT_REPOSITORY
SEALPORT_PASSWORD
SEALPORT_PASSWORD_FILE
SEALPORT_CONFIG
SEALPORT_PROFILE
SEALPORT_LOG
```

Secrets must be redacted from logs, diagnostics, JSON, crash output, and test
fixtures.

## Storage Backends

V1 target:

- Local filesystem.
- S3-compatible object storage.

Later candidates:

- Azure Blob Storage.
- Google Cloud Storage.
- WebDAV.
- Backblaze B2 native or S3-compatible.
- Optional OpenDAL extra backends.
- Optional rclone bridge.

rclone must not be a core dependency. SealPort's default identity is a
Rust-native backup tool.

## Platform Support

SealPort is cross-platform first, but support is earned by CI and releases.

Target v1 release artifacts:

- Windows x86_64 MSVC.
- Windows ARM64 MSVC.
- macOS x86_64.
- macOS ARM64.
- Linux x86_64 GNU.
- Linux x86_64 musl.
- Linux ARM64 GNU/musl.
- FreeBSD x86_64.
- NetBSD x86_64 where feasible.

OpenBSD is best-effort until build, test, CI, and release support are real.

No platform should be called supported unless CI builds it, tests the relevant
behavior, and release artifacts exist.

## V1 Scope

V1 should include:

- `init`, `backup`, `restore`, `snapshots`, `ls`, `check`, `forget`, and
  `prune`.
- Key management.
- Local backend.
- S3-compatible backend.
- JSON and JSONL output.
- Config profiles.
- Shell completions.
- Signed cross-platform releases with checksums and SBOMs.

V1 should not include:

- GUI or TUI.
- FUSE mount.
- Daemon mode.
- Server mode.
- Every storage provider.
- restic or rustic repository compatibility.
- Mobile apps.
- Built-in scheduling.

## Development

The repo contains the initial Rust workspace, crate boundaries, CLI shell, CI
workflow, planning docs, and first tested crypto primitives for master keys,
passphrase key slots, subkey derivation, and authenticated object envelopes.
The backup engine, frozen repository format, and storage backends are not built
yet.

The normal local gate is:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
```

For docs-only changes:

```sh
git diff --check
```

Durable implementation details should move into `docs/` as they settle,
especially architecture, repository format, security, CLI/JSON contracts,
storage behavior, platform metadata, operations, and release process. Current
design docs include [`docs/security.md`](docs/security.md),
[`docs/repository-format.md`](docs/repository-format.md), and
[`docs/cli-contract.md`](docs/cli-contract.md).

## License

MIT. See [`LICENSE`](LICENSE).
