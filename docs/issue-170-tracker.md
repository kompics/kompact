# Issue #170 Tracker

This file tracks the agreed migration for issue [#170](https://github.com/kompics/kompact/issues/170): compile-time separation of local-only Kompact usage, distributed dispatching, and the provided socket-based networking backend.

## Scope

### S01 Goals

- S01.1 Support three build modes:
  - `kompact` local+typed only
  - `kompact` with `distributed`
  - `kompact-net` on top of `kompact/distributed`
- S01.2 Preserve the existing distributed terminology:
  - keep `NetworkActor`
  - keep `NetMessage`
  - keep `LocalDispatcher`
- S01.3 Avoid measurable performance regressions on the existing hot paths.
- S01.4 Keep mode 2 useful without over-engineering `LocalDispatcher`.
- S01.5 Stop after the second benchmark run and review the results before changing docs/examples.

### S02 Non-goals

- S02.1 Do not rename the distributed APIs to path-specific names in this change.
- S02.2 Do not refactor `NetworkDispatcher` to share internals with `LocalDispatcher`.
- S02.3 Do not attempt to design a generic backend-status abstraction up front.
- S02.4 Do not polish all examples/docs until after benchmark comparison and signoff.

## Target Architecture

### A03 Modes

- A03.1 Mode 1: `kompact` local+typed only
  - No `ActorPath`-style distributed surface.
  - No `receive_network` requirement on `Actor`.
  - No `NetworkStatusPort` surface.
- A03.2 Mode 2: `kompact` with `distributed`
  - Enables distributed dispatching concepts such as `ActorPath`, `NetworkActor`, `NetMessage`, registration, aliasing, and routing.
  - Uses `LocalDispatcher` as the provided local distributed dispatcher.
  - Must support alternative dispatcher/backend implementations besides `kompact-net`.
- A03.3 Mode 3: `kompact-net`
  - Provides the current socket-backed transport implementation.
  - Owns `NetworkDispatcher`, `NetworkConfig`, `NetworkStatusPort`, `NetworkStatus`, `NetworkStatusRequest`, and transport-specific helpers.

### A04 Trait compartmentalisation

- A04.1 Split the current system-handle surface into a base local handle and distributed-only extension traits.
- A04.2 Split dispatcher-facing transport-specific APIs away from the core dispatcher traits.
- A04.3 Remove transport-specific stubs from core traits where possible instead of leaving `unimplemented!()` placeholders in local-only mode.

### A05 `LocalDispatcher` boundary

- A05.1 `LocalDispatcher` will remain a separate implementation from `NetworkDispatcher`.
- A05.2 `LocalDispatcher` should gain only the minimum functionality needed to make mode 2 usable:
  - unique registration
  - alias registration/update
  - local path resolution and delivery
  - deadletter fallback
  - only the routing support required to keep the distributed API coherent
- A05.3 `LocalDispatcher` should not try to reproduce transport concerns such as retries, connection management, or network status reporting.

### A06 `NetworkStatusPort` decision

- A06.1 Treat `NetworkStatusPort` and related types as transport-specific for this migration.
- A06.2 Move them with `kompact-net` rather than keeping them in `kompact/distributed`.
- A06.3 Revisit a shared status abstraction only if a second backend produces concrete overlap.

## Work Plan

### W07 Step 1: tracker

- W07.1 Write this tracker file before any architectural code changes.
- W07.2 Keep it updated with benchmark baselines, key design decisions, and completion status.

### W08 Step 2: baseline benchmarks

- W08.1 Re-enable the existing benchmark targets in `experiments/dynamic-benches/Cargo.toml`.
- W08.2 Make only the minimum benchmark-specific fixes needed to get them running on the current code.
- W08.3 Run the relevant benchmarks and record the baseline here before feature work starts.
- W08.4 Relevant existing benchmark sources:
  - `experiments/dynamic-benches/src/actorrefs.rs`
  - `experiments/dynamic-benches/src/actor_store/mod.rs`
  - `experiments/dynamic-benches/src/network_latency.rs`
  - `experiments/dynamic-benches/src/pingperf.rs`
- W08.5 Keep `hashes` out of the default suite.
  - Use a dedicated feature gate instead of commenting the target in and out by hand.
  - The default baseline suite should remain focused on the migration-relevant benches.

### W09 Step 3: trait split and feature gates

- W09.1 Introduce the feature structure for local-only vs distributed.
- W09.2 Split `Actor` and related runtime traits so local-only builds do not carry the distributed requirements.
- W09.3 Split system-handle and dispatcher-facing traits so distributed-only capabilities are only present where enabled.
- W09.4 Keep naming and user-facing semantics stable where agreed.

### W10 Step 4: move networking to `kompact-net`

- W10.1 Create a new `kompact-net` crate in the workspace.
- W10.2 Move the provided transport implementation there.
- W10.3 Move transport-specific public types and helpers there.
- W10.4 Keep mode 2 open to alternative backends via the distributed traits in `kompact`.

### W11 Step 5: minimum `LocalDispatcher`

- W11.1 Add the minimum distributed functionality required by mode 2.
- W11.2 Keep this intentionally smaller and simpler than `NetworkDispatcher`.
- W11.3 Avoid speculative parity work that is not required by tests, examples, or core API coherence.

### W12 Step 6: post-change benchmarks and review stop

- W12.1 Re-run the same benchmark set used for the baseline.
- W12.2 Compare results against the recorded baseline.
- W12.3 Stop and review the results before updating docs/examples.

### W13 Step 7: docs and examples

- W13.1 Update user-facing docs for the three-way split.
- W13.2 Update examples to fit the correct mode:
  - local-only examples stay on `kompact`
  - distributed local examples use `kompact` with `distributed`
  - socket/network examples move to `kompact-net`
- W13.3 Update any feature-specific instructions in the book and supporting text.

## Validation

### V14 Benchmark matrix

- V14.1 Mode 1 focus:
  - typed `ActorRef` and `Recipient` operations
- V14.2 Mode 2 focus:
  - `ActorPath` operations
  - actor-store and routing-related lookup behaviour
- V14.3 Mode 3 focus:
  - network latency
  - ping/performance transport benches

### V15 Acceptance criteria

- V15.1 Local-only builds compile without the distributed/path-addressing surface.
- V15.2 Distributed builds compile without `kompact-net`.
- V15.3 `kompact-net` builds on top of the distributed mode and preserves existing networking behaviour.
- V15.4 Benchmarks show no unacceptable regressions on existing hot paths.
- V15.5 The final docs/examples match the new split.

## Benchmark Record

### R16 Baseline before architectural changes

- R16.1 Status: collected
- R16.2 Environment:
  - run outside the sandbox
  - `hashes` excluded from the default suite via `bench-hashes`
- R16.3 Benchmark-local fixes applied before baseline collection:
  - re-enabled bench targets in `experiments/dynamic-benches/Cargo.toml`
  - updated `experiments/dynamic-benches/src/pingperf.rs` to the current `ask` API
  - added transient boot retry and short socket-release waits in `experiments/dynamic-benches/src/network_latency.rs`
- R16.4 Commands used:
  - `cargo test -p dynamic-benches --benches --no-run`
  - `cargo bench -p dynamic-benches --bench network_latency`
  - `cargo bench -p dynamic-benches --bench pingperf`
  - `cargo bench -p dynamic-benches` with `hashes` gated off, using the successful `actor_store`, `actorrefs`, and `loop_opts` results from that run
- R16.5 Representative baseline figures to compare against after the refactor:
  - `actorrefs`
    - clone `ActorRef`: `3.27 ns`
    - clone `Recipient`: `19.45 ns`
    - clone `ActorPath`: `0.865 ns`
    - tell `ActorRef`: `36.14 ns`
    - tell `Recipient`: `34.38 ns`
    - tell strong `ActorRef`: `37.02 ns`
    - trigger port: `25.75 ns`
  - `actor_store`
    - insert `SequenceTrie/10000`: `2.191 ms`
    - insert `PathTrie/10000`: `1.323 ms`
    - lookup `SequenceTrie/10000`: `517.36 us`
    - lookup `PathTrie/10000`: `238.76 us`
    - group lookup `SequenceTrie/10000`: `418.17 us`
    - group lookup `PathTrie/10000`: `6.29 ns`
    - cleanup `SequenceTrie/10000`: `2.038 ms`
    - cleanup `PathTrie/10000`: `720.39 us`
  - `network_latency`
    - RTT static: `63.24 us`
    - RTT indexed: `63.65 us`
    - RTT pipeline all static: `5.84 us`
    - RTT pipeline all indexed: `3.86 us`
    - RTT static by threadpool size:
      - `1`: `64.61 us`
      - `2`: `63.94 us`
      - `3`: `63.84 us`
      - `4`: `63.67 us`
      - `5`: `63.57 us`
      - `6`: `63.90 us`
      - `7`: `63.90 us`
    - throughput with pipelining:
      - `1`: `31.54 Kelem/s`
      - `10`: `255.38 Kelem/s`
      - `100`: `406.07 Kelem/s`
      - `1000`: `433.76 Kelem/s`
  - `pingperf`
    - throughput `ports/1`: `57.34 Melem/s`
    - throughput `strong-refs/1`: `49.03 Melem/s`
    - throughput `weak-refs/1`: `46.87 Melem/s`
    - throughput `ask/1`: `2.00 Melem/s`
    - throughput `ports/16`: `290.61 Melem/s`
    - throughput `strong-refs/16`: `165.63 Melem/s`
    - throughput `weak-refs/16`: `192.70 Melem/s`
    - throughput `ask/16`: `2.79 Melem/s`
    - latency `ports/1`: `1.173 ms`
    - latency `strong-refs/1`: `1.254 ms`
    - latency `weak-refs/1`: `1.310 ms`
    - latency `ask/1`: `1.295 ms`
- R16.6 Full raw Criterion outputs remain available locally under `target/criterion/`.

### R17 Post-change comparison

- R17.1 Status: pending
- R17.2 Notes: stop after collecting these results and review before doc/example updates.

## Progress

- P18.1 Tracker file created.
- P18.2 Baseline benchmarks collected.
- P18.3 Feature split started.
  - `distributed` feature added to `kompact` as the switch for the distributed surface.
  - `SystemHandle` split into a base handle and `DistributedSystemHandle`.
  - `Actor::receive_network` is now only required when `distributed` is enabled.
  - distributed-facing `KompactSystem`, `Dispatching`, and `ActorPathFactory` APIs are being gated instead of merely hidden from the prelude.
- P18.4 Current validation checkpoint:
  - `cargo check -p kompact` passes
  - `cargo check -p kompact --no-default-features --features 'serde_support ser_id_64 use_local_executor implicit_routes'` passes
  - `cargo test -p dynamic-benches --benches --no-run` passes
- P18.5 `kompact-net` crate added to the workspace.
  - current shape is a thin wrapper over the existing networking surface in `kompact`
  - the benchmark crate now depends on `kompact-net` for its networked benches
- P18.6 `LocalDispatcher` now supports the minimum useful local distributed flow:
  - unique registration
  - alias registration
  - local routing-group delivery
  - deadletter fallback for unresolved local paths
- P18.7 Local distributed coverage added:
  - unique-path delivery test
  - alias delivery test
  - local broadcast-group delivery test
- P18.8 Docs/examples not yet updated.
