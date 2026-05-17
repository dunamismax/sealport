# Dependency Policy

SealPort keeps dependency decisions explicit because backup, storage, and
cryptography code must stay inspectable.

## Workspace Dependencies

First-party crates use dependencies declared in the root `Cargo.toml`
`[workspace.dependencies]` table unless there is a documented reason to pin or
feature-gate a dependency locally.

## Adding Dependencies

Before adding a dependency:

- Prefer small, maintained Rust crates with clear licensing and release history.
- Use current primary documentation for APIs and security-sensitive behavior.
- Keep UI, terminal, and progress dependencies out of library crates.
- Keep cryptography and secret-handling dependencies narrow and deliberate.
- Avoid dependencies that shell out to backup tools, cloud CLIs, OpenSSL, or
  platform backup utilities for core behavior.

Every new runtime dependency should have a clear owner crate and a concrete
reason. Development-only dependencies belong under `dev-dependencies`.

## Updating Dependencies

Dependency updates must pass:

```sh
just check
```

Security-sensitive dependency updates also need focused tests or a written note
describing the manual evidence required before release.
