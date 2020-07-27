#!/usr/bin/env bash

set -o xtrace
set -e

cargo test
cargo test -- --ignored
# these also test without the use_local_executor feature
cargo test --no-default-features --features ser_id_8
cargo test --no-default-features --features ser_id_8 -- --ignored
cargo test --no-default-features --features ser_id_16
cargo test --no-default-features --features ser_id_16 -- --ignored
cargo test --no-default-features --features ser_id_32
cargo test --no-default-features --features ser_id_32 -- --ignored
cargo test --features thread_pinning
cargo test --features thread_pinning -- --ignored
cargo test --features low_latency
cargo test --features low_latency -- --ignored
