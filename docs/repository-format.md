# Repository Format

FileFerry's repository format is original to FileFerry. Format v0 is not frozen
and has no compatibility fixtures yet. This document defines the initial shape
needed for implementation work; byte-level fixtures become the compatibility
contract only after the first intentional freeze.

## Format Principles

- Repository object names must not reveal source paths, file names, directory
  structure, tags, hostnames, usernames, or backup shape.
- Object storage is not POSIX. Correctness must not depend on rename,
  directory mutation, or immediate listing consistency.
- Writes should be immutable and retry-safe.
- Repository reads must authenticate encrypted objects before parsing
  plaintext.
- Prune must be two-phase and recoverable.
- Format migrations must be explicit, detectable, and tested.

## Plaintext Bootstrap

Only the following fields may be plaintext in format v0:

- Repository magic: identifies a FileFerry repository.
- Repository format version.
- Repository id: random public identifier used for key context binding.
- Supported algorithm ids for KDF and AEAD.
- Key-slot KDF parameters and salt.
- Key-slot AEAD nonce and encrypted master-key bytes.
- Optional non-sensitive feature flags required before unlock.

Reasons:

- The CLI needs to detect an uninitialized, unsupported, or incompatible
  repository before unlock.
- Passphrase unlock requires KDF parameters and salt before the master key is
  available.
- AEAD decrypt requires the nonce before plaintext exists.
- A random repository id lets keys and authentication contexts be bound to one
  repository without exposing backup contents.

No plaintext field may contain source paths, snapshot ids derived from
plaintext metadata, tags, hostnames, usernames, retention policy details, index
contents, chunk sizes tied to files, or object counts that are not already
visible from object storage listing.

Current implementation status:

- `ferry init` writes a plaintext `bootstrap` JSON object for local
  filesystem repositories.
- The bootstrap object contains `magic`, `format_version`, a random
  32-byte repository id encoded as lowercase hex, key-slot KDF parameters,
  key-slot salt, key-slot AEAD nonce, encrypted master-key bytes, and an empty
  feature list.
- The key-slot fields are plaintext only to the extent required for
  passphrase unlock; the repository master key remains encrypted and
  authenticated.
- `ferry snapshots` and `ferry ls` read the bootstrap, unlock the master key
  from `FILEFERRY_PASSWORD` or `FILEFERRY_PASSWORD_FILE`, then authenticate
  encrypted manifests before returning snapshot metadata.

## Object Name Layout

Object names are storage paths, not trusted metadata. They are opaque placement
keys and must be authenticated by object contents.

Format v0 object names:

```text
bootstrap
objects/chunk/<prefix>/<random-or-content-id>
objects/manifest/<prefix>/<manifest-id>
objects/index/<prefix>/<index-id>
objects/policy/<prefix>/<policy-id>
objects/upload/<writer-id>/<upload-id>
objects/prune/<plan-id>/<mark-id>
commits/<commit-id>
locks/<lease-id>
```

Rules:

- `<prefix>` is derived from the object id, not from source paths.
- Chunk ids and metadata ids must not be raw plaintext file hashes unless that
  hash is keyed or otherwise protected from offline path/content guessing.
- Object ids are authenticated inside encrypted metadata before use.
- Temporary upload names must include writer/upload randomness and no source
  path material.
- Listing object names may reveal approximate repository size; it must not
  reveal backed-up path names or tree shape.

## Authentication Context

Every encrypted object uses AEAD associated data. Format v0 object associated
data is:

```text
"fileferry\0format-v0\0object\0"
|| format_version
|| len(object_kind)
|| object_kind
|| len(object_name)
|| object_name
```

Initial object kinds:

- `chunk`
- `snapshot-manifest`
- `index`
- `policy-config`
- `repository-config`
- `upload-state`
- `prune-mark`

This binds ciphertext to its repository-format version, semantic object kind,
and storage name. Moving a ciphertext to a different name or opening it as a
different kind must fail authentication.

## Snapshot Manifest

Snapshot manifests are encrypted metadata objects. They describe a point-in-time
backup after source walking, chunking, compression, encryption, and index writes
have completed.

Required manifest fields before format freeze:

- Manifest schema version.
- Snapshot id.
- Parent snapshot ids, if used.
- Creation timestamp.
- Source roots represented as encrypted path records.
- Tags and host/profile metadata, encrypted.
- File, directory, symlink, and special-file records.
- Chunk references for regular file content.
- Platform metadata records or references.
- Index ids required to restore the snapshot.
- Backup command summary fields that are safe after encryption.

Manifest records must not store plaintext paths, tags, usernames, hostnames, or
directory shape. Restore code must authenticate and parse the manifest before
presenting any decrypted metadata to the user.

Current format-v0 readers also validate decrypted manifest entry structure
before restore writes or check data reads. Entry paths must be normalized
snapshot-relative paths, duplicate entry paths are rejected, non-file entries
must not contain chunk references, captured regular-file sizes must match the
sum of referenced chunk lengths, and any recorded ancestor entry for a child
must be a directory. Violations are treated as integrity failures.

## Chunk Index

Chunk indexes are encrypted metadata objects that map chunk identities to object
locations and integrity data.

Required index fields before format freeze:

- Index schema version.
- Index id.
- Chunk id or keyed content id.
- Encrypted chunk object name.
- Plain compressed/encrypted length only if justified as operational metadata.
- Compression algorithm id.
- AEAD algorithm id.
- Optional pack membership if pack files are introduced later.

Indexes are sensitive because they reveal deduplication and backup shape. They
remain encrypted and authenticated.

Current implementation status:

- The first core pipeline writes one encrypted index object per snapshot write.
- Chunk identities are keyed BLAKE3 values derived from the repository master
  key context, not raw plaintext hashes.
- Index object names are derived from keyed metadata identities and do not
  include source paths, tags, hostnames, or profile names.
- The index schema is still v0 implementation scaffolding and is not a frozen
  compatibility contract.

## Commit Markers And Upload State

Backups publish a snapshot only after all referenced chunks, indexes, and the
manifest are durably written.

Format v0 commit model:

1. Write chunks and upload-state objects with retry-safe names.
2. Write indexes after their referenced chunks exist.
3. Write the encrypted manifest.
4. Write an immutable commit marker that references the manifest id.

Commit markers are small operational objects. They may contain only random ids,
format version, and encrypted or authenticated references needed to discover a
committed snapshot. They must not contain plaintext tags, paths, source counts,
or human names.

Current implementation status:

- The first backup pipeline writes `commits/<snapshot-id>` only after chunk,
  index, and encrypted manifest objects have been written.
- The commit marker is plaintext JSON containing `schema_version`,
  `snapshot_id`, and `manifest_object`.
- These plaintext fields are allowed because the same keyed snapshot id and
  manifest object key are already visible in repository object names. The marker
  adds no path, tag, source count, hostname, username, policy, or directory
  shape information.
- Commit marker contents are not trusted. Snapshot discovery validates that the
  marker key, snapshot id, and manifest object name agree, then authenticates
  and decrypts the encrypted manifest before returning snapshot metadata.

## Forget Markers

Forget without prune is a state change only. It marks snapshots as no longer
visible to normal snapshot selection without deleting repository objects.

Current implementation status:

- `ferry forget` writes immutable `forgets/<snapshot-id>` marker objects for
  snapshots selected by the retention plan.
- The marker is plaintext JSON containing `schema_version`, `snapshot_id`,
  `manifest_object`, `commit_object`, and `forgotten_at_unix_seconds`.
- The plaintext ids and object keys are allowed because the same keyed snapshot
  id, manifest object key, and commit object key are already visible in
  repository object names. The marker adds no path, tag, source count,
  hostname, username, policy, or directory shape information.
- Marker contents are not trusted. Snapshot discovery validates that the marker
  key, snapshot id, manifest object name, and commit object name agree before a
  marker can hide a committed snapshot.
- Forget markers do not delete chunks, manifests, indexes, or commit markers.
  Storage reclamation requires the separate two-phase prune path.

Upload state records are encrypted. Interrupted uploads can be retried or
abandoned based on upload id and writer id. Correctness must not require
renaming a temporary object into place.

## Lock Or Lease Model

Concurrent backups should be safe when they write disjoint immutable objects and
publish separate commits. Commands that mutate shared repository state, such as
prune and key-slot changes, need a lease.

Format v0 lease rules:

- Leases are best-effort coordination, not the only protection against data
  loss.
- Lease objects use random ids and expiration timestamps.
- A writer must include command kind, writer id, start time, and expiration in
  encrypted or non-sensitive authenticated form.
- A stale lease can be broken only after its expiration and after a repository
  check confirms no required recovery action is pending.
- If backend capabilities cannot support the needed lease semantics, concurrent
  mutation must fail with a stable unsupported-capability error.

## Concurrent Backup Behavior

Concurrent backups may be allowed when:

- They do not share mutable upload ids.
- They write immutable chunks, indexes, manifests, and commit markers.
- Deduplication races are handled by idempotent object writes.
- Commit discovery tolerates out-of-order listing.

Concurrent backup must be rejected when the backend cannot provide the minimum
idempotent write and visibility behavior needed for safe publication.

Backup and prune must not run concurrently unless prune can prove the backup's
committed or in-progress objects are protected from deletion.

## Prune Mark, Sweep, And Recovery

Prune is two-phase:

1. Mark: compute a prune plan and write encrypted prune-mark objects that
   identify candidate objects, retained roots, and plan metadata.
2. Sweep: delete only objects named by a committed prune plan after checking
   that no protected snapshot, in-progress upload, or active lease references
   them.

Recovery rules:

- A mark without a completed sweep is recoverable by rechecking live commits and
  either resuming or abandoning the plan.
- A sweep must be idempotent; missing candidate objects are recorded as already
  gone, not fatal corruption by themselves.
- Prune plans must expire or be explicitly abandoned before a later prune can
  rely on their state.
- Dry-run prune uses the same reachability logic but writes no marks and deletes
  nothing.

## Migrations

Every repository has a detectable format version. Future migrations must:

- Refuse unknown future versions.
- Explain unsupported older versions with a stable error.
- Record whether migration is read-compatible, write-compatible, or requires a
  one-way rewrite.
- Have tests for old fixtures before the migration is advertised.

## Fixture Status

No golden fixtures exist yet. Add fixtures only after the format version is
intentionally frozen, and treat them as compatibility contracts.
