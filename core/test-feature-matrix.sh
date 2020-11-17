#!/usr/bin/env bash

set -o xtrace
set -e

LOG_LEVEL="slog/max_level_info"

echo "%%%%%% Testing default features %%%%%%"
cargo clippy -- -D warnings
cargo test --features "$LOG_LEVEL" -- "$@"
echo "%%%%%% Finished testing default features %%%%%%"

# these also test without the use_local_executor feature
echo "%%%%%% Testing different ser_id sizes %%%%%%"
cargo clippy --no-default-features --features ser_id_8 -- -D warnings
cargo test --no-default-features --features ser_id_8,"$LOG_LEVEL" -- "$@"
cargo test --no-default-features --features ser_id_16,"$LOG_LEVEL" -- "$@"
cargo test --no-default-features --features ser_id_32,"$LOG_LEVEL" -- "$@"
echo "%%%%%% Finished testing different ser_id sizes %%%%%%"

echo "%%%%%% Testing thread pinning %%%%%%"
cargo clippy --features thread_pinning -- -D warnings
cargo test --features thread_pinning,"$LOG_LEVEL" -- "$@"
echo "%%%%%% Finished testing thread pinning %%%%%%"

echo "%%%%%% Testing low_latency %%%%%%"
cargo clippy --features low_latency -- -D warnings
cargo test --features low_latency,"$LOG_LEVEL" -- "$@"
echo "%%%%%% Finished testing low_latency %%%%%%"

echo "%%%%%% Testing type_erasure %%%%%%"
cargo clippy --features type_erasure -- -D warnings
cargo test --features type_erasure,"$LOG_LEVEL" -- "$@"
echo "%%%%%% Finished testing type_erasure %%%%%%"
