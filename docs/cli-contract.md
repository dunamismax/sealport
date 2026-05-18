# CLI Contract

SealPort's command-line interface is intended for scripts first. Human output
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

Only `0`, `1`, and `2` are emitted by the current implementation.

## Global Precedence

Configuration is resolved in this order:

```text
CLI flags > environment variables > selected config profile > root config > defaults
```

Implemented environment variables:

```text
SEALPORT_CONFIG
SEALPORT_PROFILE
SEALPORT_REPOSITORY
SEALPORT_LOG
```

When no config path is supplied by `--config` or `SEALPORT_CONFIG`, SealPort
looks for `sealport.toml` and then `.sealport.toml` in the current working
directory.

## JSON Document Envelope

`--json` emits one document:

```json
{
  "schema_version": 1,
  "command": "version",
  "status": "success",
  "data": {
    "command": "sealport",
    "version": "0.0.0"
  }
}
```

`status` is `success` for completed commands. Future commands may add
command-specific fields under `data` without changing the envelope.

Current schema:

```text
CommandDocument<T>
  schema_version: integer, currently 1
  command: string
  status: "success" | "failure"
  data: T
```

Implemented command documents:

```text
version
  data.command: "sealport"
  data.version: semantic-version string from the package version
```

`completion <SHELL>` writes the requested shell script directly to stdout. It
does not support JSON wrapping because the completion script itself is the data.

## JSONL Event Envelope

`--jsonl` emits newline-delimited events:

```json
{"schema_version":1,"event":"command_started","command":"version","status":"started","data":null}
{"schema_version":1,"event":"command_completed","command":"version","status":"success","data":{"command":"sealport","version":"0.0.0"}}
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

Implemented command events:

```text
version command_started
  status: "started"
  data: null

version command_completed
  status: "success"
  data.command: "sealport"
  data.version: semantic-version string from the package version
```

Future long-running commands must keep human progress off stdout in both JSON
and JSONL modes. Machine progress belongs in JSONL `progress` events.

## Current Commands

`sealport version` supports human, JSON, and JSONL output.

`sealport completion <SHELL>` writes shell completion data for Bash, Elvish,
Fish, PowerShell, and Zsh.
