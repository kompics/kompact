#!/usr/bin/env bash
set -euo pipefail

# Run Clippy in feature-compatible groups. Cargo unifies features across all
# selected workspace members, so these steps keep local-only crates, network
# crates, and benchmark feature variants in the same feature graphs CI checks.

set -x

cargo clippy --workspace --exclude dynamic-benches --exclude kompact-examples-local --exclude kompact-examples-net --exclude macro-test --all-targets -- -D warnings
cargo clippy -p dynamic-benches --all-targets -- -D warnings
cargo clippy -p dynamic-benches --all-targets --features bench-distributed -- -D warnings
cargo clippy -p dynamic-benches --all-targets --features bench-network -- -D warnings
cargo clippy -p macro-test --all-targets -- -D warnings
cargo clippy -p kompact-examples-local --all-targets -- -D warnings
cargo clippy -p kompact-examples-net --all-targets -- -D warnings
cargo clippy --manifest-path=core/Cargo.toml --all-targets --no-default-features --features distributed,serde_support,ser_id_8 -- -D warnings
cargo clippy --manifest-path=core/Cargo.toml --all-targets --features thread_pinning -- -D warnings
cargo clippy --manifest-path=core/Cargo.toml --all-targets --features low_latency -- -D warnings
cargo clippy --manifest-path=core/Cargo.toml --all-targets --features type_erasure -- -D warnings
