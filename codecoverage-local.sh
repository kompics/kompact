#!/bin/bash

set -e
set -o xtrace

# setup environment
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zinstrument-coverage"
export RUSTDOCFLAGS="-Zinstrument-coverage"
export LLVM_PROFILE_FILE="codecov-instrumentation-%p-%m.profraw"

# build and test
cargo clean --verbose
cargo build --verbose
cd core
./test-feature-matrix.sh
./test-feature-matrix.sh --ignored
cd ..

# generate report
grcov core/ --binary-path target/debug/ -s . -t html --llvm --branch --ignore-not-existing --ignore '../**' --ignore '/*' -o target/debug/coverage/

if [[ "$OSTYPE" == "darwin"* ]]; then
        open target/debug/coverage/index.html;
else
        # don't know how to do this on linux/windows
        echo "open target/debug/coverage/index.html in your browser";
fi

# cleanup
rm *.profraw
rm core/*.profraw
rm experiments/datastructures/*.profraw
rm experiments/dynamic-benches/*.profraw
rm feature-tests/protobuf-test/*.profraw
