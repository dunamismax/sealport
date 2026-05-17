set shell := ["sh", "-eu", "-c"]

fmt:
    cargo fmt --all --check

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features
    cargo build --workspace

test:
    cargo test --workspace --all-features

build:
    cargo build --workspace
