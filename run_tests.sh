#!/usr/bin/env bash
set -euo pipefail

# Run repository tests in feature-compatible groups. Cargo unifies features
# across all selected workspace members, and the network crates enable
# Kompact's distributed support. Keeping the local-only crates in a separate
# command lets their Actor implementations compile without the distributed
# receive_network requirement.

cargo test --workspace --exclude kompact-net --exclude kompact-examples-local --exclude kompact-examples-net
cargo test -p kompact-examples-local
cargo test -p kompact-net
cargo test -p kompact-examples-net
