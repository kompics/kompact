#!/usr/bin/env bash

set -o xtrace
set -e

cargo test
cargo test --no-default-features --features ser_id_8
cargo test --no-default-features --features ser_id_16
cargo test --no-default-features --features ser_id_32
cargo test --features thread_pinning
cargo test --features low_latency
