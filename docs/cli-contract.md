# CLI Contract

FileFerry's command-line interface is intended for scripts first. Human output
may improve while the project is pre-v1, but stdout/stderr separation, exit
code families, and machine-output envelopes are treated as compatibility
surfaces once marked stable.

## Streams

- Stdout is data.
- Stderr is diagnostics, logs, and progress.
- `--json` writes exactly one JSON document to stdout.
- `--jsonl` writes one JSON event per line to stdout.
- Completion scripts are stdout data and are not wrapped in JSON or JSONL.
- Progress UI must never be written to stdout in JSON or JSONL modes.

## Exit Codes

These exit code families are stable for the current CLI foundation:

```text
0   success
1   generic failure or internal serialization/completion failure
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

The current implementation can emit these families for the implemented command
surface: `0`, `1`, `2`, `3`, `4`, `5`, `6`, `7`, `9`, and `10`.

## Global Precedence

Configuration is resolved in this order:

```text
CLI flags > environment variables > selected config profile > root config > defaults
```

Implemented environment variables:

```text
FILEFERRY_CONFIG
FILEFERRY_PROFILE
FILEFERRY_REPOSITORY
FILEFERRY_PASSWORD
FILEFERRY_PASSWORD_FILE
FILEFERRY_LOG
```

When no config path is supplied by `--config` or `FILEFERRY_CONFIG`, FileFerry
looks for `fileferry.toml` and then `.fileferry.toml` in the current working
directory.

## JSON Document Envelope

`--json` emits one document:

```json
{
  "schema_version": 1,
  "command": "version",
  "status": "success",
  "data": {
    "command": "ferry",
    "version": "0.0.0"
  }
}
```

`status` is `success` for completed commands. Future commands may add
command-specific fields under `data` without changing the envelope.
Runtime failures after CLI parsing use `status: "failure"` and the
`command_failed` data schema documented below. Argument parsing errors from
`clap` are still emitted as normal usage diagnostics.

Current schema:

```text
CommandDocument<T>
  schema_version: integer, currently 1
  command: string
  status: "success" | "failure"
  data: T
```

Required v1 command document data schemas:

```text
init
  data.repository_id: string
  data.repository_url: redacted string
  data.format_version: integer
  data.backend: "local" | "s3"
  data.created: boolean
  data.key_slots: integer

backup
  data.snapshot_id: string
  data.repository_id: string
  data.started_at_unix_seconds: integer
  data.completed_at_unix_seconds: integer
  data.sources: array of redacted strings
  data.tags: array of strings
  data.entries_scanned: integer
  data.files_backed_up: integer
  data.directories_backed_up: integer
  data.symlinks_backed_up: integer
  data.special_entries_seen: integer
  data.bytes_scanned: integer
  data.bytes_uploaded: integer
  data.chunks_seen: integer
  data.chunks_written: integer
  data.chunks_reused: integer
  data.index_ids: array of strings
  data.manifest_id: string

restore
  data.snapshot_id: string
  data.destination: redacted string
  data.paths: array of snapshot-relative strings
  data.dry_run: boolean
  data.overwrite: "fail_if_exists" | "overwrite_files"
  data.entries_selected: integer
  data.files_written: integer
  data.directories_written: integer
  data.symlinks_written: integer
  data.metadata_applied: integer
  data.metadata_warnings: array of RestoreMetadataWarning
  data.bytes_written: integer
  data.verified_files: integer

snapshots
  data.snapshots: array of SnapshotSummary

ls
  data.snapshot_id: string
  data.path: snapshot-relative string
  data.entries: array of SnapshotEntry

check
  data.repository_id: string
  data.checked_at_unix_seconds: integer
  data.metadata_objects_checked: integer
  data.chunk_objects_checked: integer
  data.bytes_read: integer
  data.read_data_mode: "metadata_only" | "subset" | "full"
  data.read_data_subset: string | null
  data.errors: array of CheckFinding
  data.warnings: array of CheckFinding

forget
  data.dry_run: boolean
  data.snapshots_matched: integer
  data.snapshots_forgotten: integer
  data.retained_snapshots: integer
  data.removed_snapshot_ids: array of strings
  data.policy_summary: RetentionPolicySummary

prune
  data.dry_run: boolean
  data.plan_id: string | null
  data.objects_candidates: integer
  data.objects_deleted: integer
  data.objects_retained: integer
  data.bytes_candidates: integer
  data.bytes_deleted: integer
  data.bytes_retained: integer
  data.recovery_state: "none" | "mark_written" | "sweep_completed"

key add
  data.key_slot_id: string
  data.key_slots: integer
  data.kdf: KdfSummary

key remove
  data.removed_key_slot_id: string
  data.key_slots: integer

key rotate
  data.added_key_slot_id: string
  data.removed_key_slot_ids: array of strings
  data.key_slots: integer
  data.reencrypted_master_key_only: boolean

key export-recovery
  data.export_id: string
  data.warning_acknowledged: boolean
  data.destination: redacted string | null

version
  data.command: "ferry"
  data.version: semantic-version string from the package version
```

Shared data records:

```text
SnapshotSummary
  snapshot_id: string
  created_at_unix_seconds: integer
  tags: array of strings
  source_count: integer
  entry_count: integer

RestoreMetadataWarning
  path: snapshot-relative string
  field: string
  reason: string

SnapshotEntry
  path: snapshot-relative string
  kind: "regular_file" | "directory" | "symlink" | "other"
  size_bytes: integer | null
  modified: TimestampValue
  metadata_status: "complete" | "partial" | "unsupported"

TimestampValue
  status: "captured" | "unsupported" | "denied"
  seconds: integer, present only when status is "captured"
  nanoseconds: integer, present only when status is "captured"
  denial_reason: string, present only when status is "denied"

RestoreMetadataWarning
  path: snapshot-relative string
  field: string
  reason: string

CheckFinding
  code: stable string
  severity: "warning" | "error"
  object_key: string | null
  snapshot_id: string | null
  path: snapshot-relative string | null
  message: string

RetentionPolicySummary
  keep_daily: integer | null
  keep_weekly: integer | null
  keep_monthly: integer | null
  keep_yearly: integer | null
  keep_tags: array of strings

KdfSummary
  algorithm: "argon2id_v19"
  memory_cost_kib: integer
  time_cost: integer
  parallelism: integer
```

`completion <SHELL>` writes the requested shell script directly to stdout. It
does not support JSON wrapping because the completion script itself is the data.

## JSONL Event Envelope

`--jsonl` emits newline-delimited events:

```json
{"schema_version":1,"event":"command_started","command":"version","status":"started","data":null}
{"schema_version":1,"event":"command_completed","command":"version","status":"success","data":{"command":"ferry","version":"0.0.0"}}
```

Reserved event names:

```text
command_started
progress
warning
command_completed
command_failed
```

Long-running commands must emit at least `command_started` and either
`command_completed` or `command_failed`.

Current schema:

```text
CommandEvent<T>
  schema_version: integer, currently 1
  event: "command_started" | "progress" | "warning" | "command_completed" | "command_failed"
  command: string
  status: "started" | "success" | "failure"
  data: T | null
```

Long-operation JSONL event data schemas:

```text
command_started
  data.request_id: string
  data.repository_url: redacted string | null
  data.profile: string
  data.dry_run: boolean

progress
  data.phase: stable string
  data.message: string
  data.items_done: integer | null
  data.items_total: integer | null
  data.bytes_done: integer | null
  data.bytes_total: integer | null
  data.snapshot_id: string | null
  data.object_key: string | null

warning
  data: command-specific warning data. Restore currently emits
        RestoreMetadataWarning.

command_completed
  data: same command-specific data as the matching JSON document

command_failed
  data.code: stable string
  data.message: string
  data.exit_code: integer
  data.retryable: boolean
  data.path: redacted string | snapshot-relative string | null
  data.object_key: string | null
```

Required long-running commands must emit at least these phase names when the
phase applies:

```text
init: validate_repository, create_bootstrap, write_key_slot, complete
backup: walk_sources, plan_chunks, write_chunks, write_index, write_manifest, write_commit, complete
restore: load_manifest, read_chunks, write_entries, apply_metadata, verify, complete
check: load_commits, verify_metadata, verify_indexes, read_data, complete
forget: load_snapshots, evaluate_policy, write_forget_state, complete
prune: plan, mark, sweep, verify_reachability, complete
key add: load_bootstrap, derive_key, write_key_slot, complete
key remove: load_bootstrap, verify_remaining_unlock, remove_key_slot, complete
key rotate: load_bootstrap, derive_key, write_key_slot, retire_old_slots, complete
key export-recovery: load_bootstrap, create_export, complete
```

Implemented command events:

```text
init command_started
  status: "started"
  data: null

init command_completed
  status: "success"
  data: Init data schema above

backup command_started
  status: "started"
  data: null

backup progress
  status: "started"
  data.phase: "walk_sources" | "plan_chunks" | "write_chunks" | "write_index" | "write_manifest" | "write_commit" | "complete"
  data.message: string
  data.items_done: integer | null
  data.items_total: integer | null
  data.bytes_done: integer | null
  data.bytes_total: integer | null
  data.snapshot_id: string | null
  data.object_key: string | null

backup command_completed
  status: "success"
  data: Backup data schema above

restore command_started
  status: "started"
  data: null

restore progress
  status: "started"
  data.phase: "load_manifest" | "read_chunks" | "write_entries" | "apply_metadata" | "verify" | "complete"
  data.message: string
  data.items_done: integer | null
  data.items_total: integer | null
  data.bytes_done: integer | null
  data.bytes_total: integer | null
  data.snapshot_id: string | null
  data.object_key: string | null

restore command_completed
  status: "success"
  data: Restore data schema above

snapshots command_started
  status: "started"
  data: null

snapshots command_completed
  status: "success"
  data: Snapshots data schema above

ls command_started
  status: "started"
  data: null

ls command_completed
  status: "success"
  data: Ls data schema above

check command_started
  status: "started"
  data: null

check progress
  status: "started"
  data.phase: "load_commits" | "verify_metadata" | "verify_indexes" | "read_data" | "complete"
  data.message: string
  data.items_done: integer | null
  data.items_total: integer | null
  data.bytes_done: integer | null
  data.bytes_total: integer | null
  data.snapshot_id: string | null
  data.object_key: string | null

check command_completed
  status: "success"
  data: Check data schema above

check command_failed
  status: "failure"
  data: command_failed data schema above
  data.finding: CheckFinding, present when the check failure maps to a
    repository integrity finding

version command_started
  status: "started"
  data: null

version command_completed
  status: "success"
  data.command: "ferry"
  data.version: semantic-version string from the package version
```

Future long-running commands must keep human progress off stdout in both JSON
and JSONL modes. Machine progress belongs in JSONL `progress` events.

## Current Commands

`ferry init` creates an encrypted local filesystem repository when `--repo` or
`FILEFERRY_REPOSITORY` points at a local path or `file:///absolute/path`.
It requires `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`. S3-compatible
repository bootstrap is not wired into the CLI yet.

`ferry backup <SOURCE>...` opens an initialized local repository with
`FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, creates an encrypted,
compressed, deduplicated snapshot through the core backup pipeline, and commits
it so `ferry snapshots` and `ferry ls` can discover it. `--tag <TAG>` may be
repeated. JSON output follows the Backup data schema above; JSONL output emits
the implemented progress phases listed above. Source paths are local
filesystem paths; S3-compatible repository bootstrap remains unwired in the
CLI.

`ferry restore <DESTINATION>` opens an initialized local repository with
`FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, selects `latest` by default
or accepts `--snapshot <ID>` / `--tag <TAG>` / `--latest`, and restores
directory entries, regular-file contents, and Unix symlinks under the
destination directory. `--path <PATH>` may be repeated to restore
snapshot-relative paths. The command fails if a destination file already
exists unless `--overwrite` is supplied. Restored symlink destination paths and
symlinked ancestors are rejected if they already exist; symlinks are created
after directory and regular-file writes so restore writes do not traverse
newly restored symlinks. `--dry-run` reports selected entries and planned
writes without creating destination entries. JSON output follows the Restore
data schema above; JSONL output emits the implemented progress phases listed
above. Current metadata application is limited to captured modified timestamps
for restored regular files and directories. Symlink timestamps, ownership,
mode bits, ACLs, xattrs, resource forks, Windows attributes, BSD flags, and
other platform-specific metadata are not restored yet. If a timestamp is
selected for application but cannot be applied, restore reports a
`metadata_warnings` item and exits with partial-success code `10`; JSON and
JSONL modes keep those warnings on stdout.

`ferry check` opens an initialized local repository with `FILEFERRY_PASSWORD`
or `FILEFERRY_PASSWORD_FILE`, authenticates committed snapshot manifests,
authenticates chunk indexes, reads every referenced chunk object, decompresses
chunk payloads, and verifies keyed chunk identities. JSON output follows the
Check data schema above with `read_data_mode: "full"` and
`read_data_subset: null`. Check failures still fail closed. In JSON and JSONL
modes, runtime check failures emit a machine-readable failure envelope with a
stable `code`, `exit_code`, optional `object_key`, and, for repository
integrity failures, a `finding` object shaped like `CheckFinding`.
Configurable subset checks are not implemented yet.

`ferry snapshots` opens an initialized local repository with
`FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, authenticates committed
snapshot manifests, and emits human, JSON, or JSONL-safe snapshot summaries.

`ferry ls` opens an initialized local repository, selects `latest` by default
or accepts `--snapshot <ID>` / `--tag <TAG>`, and lists immediate entries at a
snapshot-relative path. JSON output uses `"."` for the snapshot root path.

`ferry version` supports human, JSON, and JSONL output.

`ferry completion <SHELL>` writes shell completion data for Bash, Elvish,
Fish, PowerShell, and Zsh.
