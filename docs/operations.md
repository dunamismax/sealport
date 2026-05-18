# Operations

Operational notes for behavior that has been exercised through the `ferry`
binary. Keep this document evidence-led: record only drills that were actually
run, and keep backend scope explicit.

## Local Restore And Check Drill - 2026-05-18

Scope:

- Backend: local filesystem repository in a temporary directory.
- Commands: `ferry init`, `ferry backup`, `ferry restore`, `ferry check`.
- Snapshot selection: `--tag drill`.
- Restore scope: full snapshot.
- Restored entry kinds: directory entries, one regular file, and one Unix
  symlink.
- Verification: `cmp` compared source and restored file bytes, and restore JSON
  reported `verified_files: 1`; `test -d` verified the restored empty nested
  directory; `readlink` verified the restored symlink target; `ferry check`
  authenticated the committed manifest, chunk index, and referenced chunk.
  This drill predates modified-timestamp restore coverage and did not verify
  metadata application.

Result:

- Snapshot id:
  `c3e77490485db4e7a37036cf11fe9caf750b156e67061163f0f61bef49364115`
- Entries selected: `5`.
- Directories written: `3`.
- Files written: `1`.
- Symlinks written: `1`.
- Bytes written: `20`.
- Verified files: `1`.
- Check metadata objects: `3`.
- Check chunk objects: `1`.
- Check read data mode: `full`.
- Byte comparison: passed.
- Directory verification: passed.
- Symlink target verification: passed.

Command shape used:

```sh
root="$(mktemp -d)"
repo="$root/repo"
source="$root/source"
restore="$root/restore"
mkdir -p "$source/empty/nested"
printf 'restore drill bytes\n' > "$source/sample.txt"
ln -s sample.txt "$source/sample.link"

FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" init
FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" --json backup --tag drill "$source"
FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" --json restore --tag drill "$restore"
FILEFERRY_PASSWORD='throwaway-passphrase' ferry --repo "$repo" --json check
cmp "$source/sample.txt" "$restore/sample.txt"
test -d "$restore/empty/nested"
test "$(readlink "$restore/sample.link")" = 'sample.txt'
```

This drill does not claim S3-compatible restore coverage, metadata application
coverage, configurable check subset coverage, or symlink restore behavior on
non-Unix platforms. Current code has separate tests for restored regular-file
and directory modified timestamps.
