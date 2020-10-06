#!/usr/bin/env bash

set -o xtrace
set -e

echo "%%%%%% Testing default features %%%%%%"
cargo clippy -- -D warnings
cargo test -- "$@"
echo "%%%%%% Finished testing default features %%%%%%"

# these also test without the use_local_executor feature
echo "%%%%%% Testing different ser_id sizes %%%%%%"
cargo clippy --no-default-features --features ser_id_8 -- -D warnings
cargo test --no-default-features --features ser_id_8 -- "$@"
cargo test --no-default-features --features ser_id_16 -- "$@"
cargo test --no-default-features --features ser_id_32 -- "$@"
echo "%%%%%% Finished testing different ser_id sizes %%%%%%"

echo "%%%%%% Testing thread pinning %%%%%%"
cargo clippy --features thread_pinning -- -D warnings
cargo test --features thread_pinning -- "$@"
echo "%%%%%% Finished testing thread pinning %%%%%%"

echo "%%%%%% Testing low_latency %%%%%%"
cargo clippy --features low_latency -- -D warnings
cargo test --features low_latency -- "$@"
echo "%%%%%% Finished testing low_latency %%%%%%"
