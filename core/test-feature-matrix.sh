#!/usr/bin/env bash

set -o xtrace

cargo test
cargo test --no-default-features --features ser_id_8
cargo test --no-default-features --features ser_id_16
cargo test --no-default-features --features ser_id_32
