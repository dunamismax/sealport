# Operations

Operational notes for behavior that has been exercised through the `ferry`
binary. Keep this document evidence-led: record only drills that were actually
run, and keep backend scope explicit.

## Local Restore Drill - 2026-05-18

Scope:

- Backend: local filesystem repository in a temporary directory.
- Commands: `ferry init`, `ferry backup`, `ferry restore`.
- Snapshot selection: `--tag drill`.
- Restore scope: `--path sample.txt`.
- Verification: `cmp` compared source and restored file bytes, and restore JSON
  reported `verified_files: 1`.

Result:

- Snapshot id:
  `579d03ab7432a318d18cee37b60ec410d2e6878fec2db51b60cc20e8c70d6bab`
- Files written: `1`.
- Bytes written: `20`.
- Verified files: `1`.
- Byte comparison: passed.

Command shape used:

```sh
root="$(mktemp -d)"
repo="$root/repo"
source="$root/source"
restore="$root/restore"
mkdir "$source"
printf 'restore drill bytes\n' > "$source/sample.txt"

FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" init
FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" --json backup --tag drill "$source"
FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" --json restore --tag drill --path sample.txt "$restore"
cmp "$source/sample.txt" "$restore/sample.txt"
```

This drill does not claim S3-compatible restore coverage, metadata restore,
directory entry restore, or symlink restore.
