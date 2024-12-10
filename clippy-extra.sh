#!/usr/bin/env bash

set -o xtrace
set -e

cargo clippy --all-targets -- -D warnings

pushd macros/actor-derive
cargo clippy --all-targets -- -D warnings
popd

pushd macros/component-definition-derive
cargo clippy --all-targets -- -D warnings
popd

pushd feature-tests/protobuf-test
cargo clippy --all-targets -- -D warnings
popd

pushd docs/examples
cargo clippy --all-targets -- -D warnings
popd
