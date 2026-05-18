# Platform Metadata

SealPort captures platform metadata so restores can be honest and repeatable
across Windows, macOS, Linux, FreeBSD, and NetBSD. Metadata records are stored
inside encrypted snapshot manifests or encrypted metadata objects; source paths,
file names, ownership, attributes, xattrs, ACLs, and platform-specific details
must not appear in plaintext repository objects.

This document defines the v1 metadata target. Implementation must still prove
behavior with platform-specific tests before any platform is called supported.

## Capture Model

Every filesystem entry records:

- Entry kind: regular file, directory, symlink, and explicitly handled special
  file kinds.
- Portable mode facts: readability/writability/executability where the source
  platform exposes them, plus POSIX mode bits on Unix-like systems.
- Size for regular files and symlink target length where available.
- Modification time and creation/birth time where the source platform exposes
  it.
- Ownership identifiers: numeric UID/GID on Unix-like systems and Windows owner
  SID when available.
- Symlink target as metadata, not followed file content.
- Sparse-file extent information where the source platform and filesystem
  expose it reliably.

Platform extensions are captured as namespaced records:

- `windows`: file attributes, reparse point kind, alternate data stream names
  and contents when enabled, owner SID, DACL/SACL capture status, and long-path
  spelling used for the source entry.
- `macos`: POSIX mode/ownership, file flags, birth time, xattrs, resource fork
  if present, Finder metadata exposed through xattrs, and filename normalization
  observations.
- `linux`: POSIX mode/ownership, file type, timestamps, xattrs, ACL capture
  status, sparse extents, and special-file identifiers when explicitly enabled.
- `freebsd`: POSIX mode/ownership, file flags, timestamps, xattrs where
  available, ACL capture status, sparse extents, and special-file identifiers
  when explicitly enabled.
- `netbsd`: POSIX mode/ownership, file flags where available, timestamps, xattrs
  where available, ACL capture status, sparse extents, and special-file
  identifiers when explicitly enabled.

Metadata capture must distinguish three states:

- Captured: the value was read and stored.
- Unsupported: the source platform or filesystem does not expose the field.
- Denied: the field exists but permissions prevented capture.

Denied and unsupported metadata are not fatal to backup by default, but they
must be surfaced as warnings in human output and as structured JSON/JSONL
events. A strict mode may promote those warnings to failure.

## Restore Behavior

Restore applies content first, then portable metadata, then platform-specific
metadata that the destination can represent.

When metadata cannot be represented on the destination platform, SealPort must:

- Restore file content and directory structure whenever doing so is safe.
- Skip the unrepresentable metadata field without silently pretending success.
- Emit a human warning on stderr.
- Emit a machine-readable item-level warning that includes the snapshot entry
  id, metadata namespace, metadata field, source platform, destination platform,
  and reason.
- Return partial-success exit code `10` when the restore otherwise succeeds but
  one or more requested metadata fields could not be applied.

When metadata application is denied by destination permissions, SealPort must
report a permission failure for that field. Strict restore mode may fail the
whole restore; default restore records partial success when file content was
restored correctly.

Restore must not invent destination metadata to mimic unsupported source
metadata. If exact metadata restoration is impossible, the report needs to say
which field was skipped and why.

## Safety Rules

- Symlinks are restored as symlinks by default and must not be followed during
  restore writes.
- Special files require explicit opt-in before creation.
- Ownership, ACLs, file flags, Windows attributes, xattrs, resource forks, and
  alternate data streams are restored only after path destination checks pass.
- Case collisions and reserved names must be detected before writes begin.
- Timestamp restoration happens after content writes and fsync where practical.
- Any metadata parser failure after decryption is a repository integrity error,
  not a best-effort warning.

## Reference Points

The v1 implementation should verify API choices against current primary
documentation before coding each platform path:

- Microsoft Win32 file attribute constants and related file information APIs.
- Apple filesystem metadata, URL resource values, and file manager attribute
  documentation.
- Linux `stat`, `statx`, xattr, ACL, and sparse-file interfaces.
- FreeBSD `stat`, flags, xattr/extattr, ACL, and sparse-file interfaces.
- NetBSD `stat`, flags, extended attribute, ACL, and sparse-file interfaces.
