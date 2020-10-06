#!/bin/bash

set -e
set -o xtrace

# setup environment
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off -Zpanic_abort_tests -Cpanic=abort"
export RUSTDOCFLAGS="-Cpanic=abort"

# build and test
cargo clean --verbose
cargo build --verbose
cd core
./test-feature-matrix.sh
cd ..

# generate report
grcov ./target/debug/ -s . -t html --llvm --branch --ignore-not-existing -o ./target/debug/coverage/

if [[ "$OSTYPE" == "darwin"* ]]; then
        open target/debug/coverage/index.html;
else
        # don't know how to do this on linux/windows
        echo "open target/debug/coverage/index.html in your browser";
fi
