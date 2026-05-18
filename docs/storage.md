# Storage

SealPort storage is object-oriented. Backends store immutable byte objects by
validated repository object keys; higher layers decide what those bytes mean.

This document describes the current storage contract. It is not the complete
v1 storage design yet, and it does not claim that backup or restore are
implemented.

## Object Keys

Object keys are repository-internal names such as `chunks/aa/blob` or
`indexes/current`.

Valid keys:

- Are relative.
- Use `/` as the only separator.
- Do not contain empty, `.`, or `..` segments.
- Do not contain platform path separators such as `\`.
- Use only ASCII letters, digits, `.`, `_`, `-`, and `=` in each segment.

The key validator prevents local backend path traversal and keeps backend
behavior independent from operating-system path syntax. It does not make a
repository object name non-sensitive by itself; repository-format code must
still avoid deriving object names from source paths or backup shape.

## Store Contract

The `ObjectStore` trait currently exposes:

- `capabilities`
- `put_if_absent`
- `get`
- `exists`
- `delete`
- `list_prefix`

`put_if_absent` is the default write primitive for immutable repository
objects. A backend must return `Created` for a new object and `AlreadyPresent`
when the same key already contains identical bytes. If a key exists with
different bytes, the backend must return `ObjectAlreadyExists`.

Deletes are idempotent for the implemented local and fake stores. Deleting a
missing object succeeds.

## Capability Model

`StorageCapabilities` records backend behavior that repository code must not
guess:

- Backend kind.
- Conditional create support.
- Atomic visibility.
- Strong read-after-write behavior.
- Delete behavior.
- Prefix listing support.

The model intentionally separates capability reporting from command output.
CLI code can later map capability failures into stable diagnostics and exit
codes.

## Local Filesystem Backend

`LocalStore` maps validated object keys under a configured root directory.

Writes use this flow:

1. Create parent directories for the final object path.
2. Write bytes to a unique file under `.sealport-tmp/`.
3. Sync the temporary file.
4. Publish by hard-linking the temporary file to the final object path.
5. Remove the temporary file.

If the final object already exists, the local backend removes the temporary
file and compares existing bytes. Identical bytes make the operation
idempotent; different bytes fail as an immutable write conflict.

Leftover `.sealport-tmp/` files are ignored by prefix listing so interrupted
writes do not appear as repository objects.

## Fake Store

`sealport-testkit` provides `FakeObjectStore`, an in-memory implementation of
the same object-store contract. It is for repository, corruption, and pipeline
tests that need deterministic storage behavior without touching a real backend.

The fake store enforces the same immutable write rule as the local backend:
same bytes are idempotent, different bytes are rejected.

## Not Implemented Yet

S3-compatible storage is still pending. Before it is marked complete it needs
capability checks, retry policy, timeout policy, upload idempotency, stale or
surprising listings, missing-object behavior, partial upload behavior, and
permission-error tests against an isolated test bucket or emulator.
