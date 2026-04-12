# Kompact Modernisation Plan

Branch: `codex/modernisation-pass`
Base commit: `493f2fedb2d0`

## Working Rules

- Do one stage at a time.
- Keep each stage reviewable and finish it with its own commit.
- If a stage uncovers extra churn that is not required to complete that stage, note it here and defer it.
- Prefer stable-first outcomes where possible.

## Stage 0: Branch And Roadmap

Status: done

Goals:

- Create the working branch from the detached base commit.
- Capture the agreed staged plan in this file.

Planned commit:

- `chore: add modernisation roadmap`

## Stage 1: Trivial Dependency Updates

Status: done

Scope:

- Apply only compatible dependency requirement bumps.
- Refresh the relevant lockfiles.
- Check whether any of these bumps enable immediate code simplifications.

Expected manifest updates:

- `core/Cargo.toml`
  - `arc-swap`: `1.6` -> `1.9`
  - `uuid`: `1.3` -> `1.23`
  - `async-std`: `1.12` -> `1.13`
  - `ipnet`: `2.7` -> `2.12`
  - `once_cell`: `1.17` -> `1.21`
- `experiments/dynamic-benches/Cargo.toml`
  - `uuid`: `1.3` -> `1.23`
  - `twox-hash`: `1.5` -> `1.6`

Questions to answer during the stage:

- Are any of these updates more than under-the-hood improvements?
- Do they enable small local clean-ups with no behavioural risk?

Current expectation:

- Most of these are likely to be under-the-hood improvements only.
- The only likely future simplification is around `once_cell`, but that belongs with the MSRV move in Stage 2 rather than here.

Verification:

- `cargo check --workspace --all-targets`

Outcome:

- Compatible manifest bumps applied as planned.
- `cargo update -w` produced no lockfile changes because the existing lock already resolved to the newest compatible releases.
- No immediate source simplifications were justified by these updates alone.
- Reassess whether `once_cell` is needed at all once the MSRV is raised, since the standard library now covers more of this space directly.

Planned commit:

- `chore(deps): apply compatible dependency updates`

## Stage 2: Edition, MSRV, And CI Tooling

Status: done

Scope:

- Move the workspace crates from edition 2018 to edition 2024.
- Declare `rust-version = "1.94.1"` where appropriate.
- Update CI tooling and remove stale `actions-rs` usage.
- Fix the workflow mismatch where the step says stable but actually installs nightly.
- Double check whether `once_cell` is still needed, or whether standard-library replacements such as `std::sync::LazyLock` are sufficient.
- Ensure `./clippy-extra.sh` passes before committing.

Expected follow-up checks:

- `./clippy-extra.sh`
- Targeted cargo checks or tests as needed after lint fixes

Outcome:

- Workspace crates moved to edition 2024 and now declare `rust-version = "1.94.1"`.
- Workspace resolver moved to version 3.
- `once_cell` was removed from direct workspace usage; the remaining occurrences are transitive lockfile entries only.
- Local `once_cell::sync::Lazy` usages were either replaced with standard-library support or removed where eager initialisation was sufficient.
- CI was modernised away from `actions-rs` and now uses the pinned nightly lane `nightly-2026-04-11` for nightly-only jobs, including `clippy` and `fmt`.
- The main test matrix now covers floating `stable`, floating `nightly`, and fixed `1.94.1`; only `clippy` and `fmt` stay on the pinned nightly lane.
- The stable/nightly feature matrix was made consistent so `type_erasure` only runs on nightly in CI until stage 3 ungates it properly.
- Validation passed with the workspace default toolchain:
  - `cargo check --workspace --all-targets`
  - `./clippy-extra.sh`
  - `cargo fmt --all -- --check`
  - Local development is no longer pinned to the MSRV toolchain; CI carries the compatibility check instead.

Planned commit:

- `chore(rust): move to edition 2024 and align MSRV/CI`

## Stage 3: Ungate Stable APIs

Status: done

Scope:

- Revisit the nightly/stable split and remove gates that are no longer justified.
- Make `type_erasure` available on stable if the current `Box<Self>` receiver approach works cleanly.
- Collapse duplicated nightly/stable implementations where stable now supports the same shape.
- Keep only the genuinely preview-dependent pieces gated.

Known candidates:

- `core/src/utils/erased.rs`
- `core/src/runtime/system.rs`
- `core/src/component/system_handle.rs`
- `core/src/utils/iter_extras.rs`
- `core/src/lib.rs`

Expected remaining nightly-only pieces:

- The `Never = !` alias on stable is still blocked by the unstable `!` type.
- The nightly-only experimental crates and benches that explicitly require preview features.

Outcome:

- `type_erasure` now builds on stable by switching the erased constructor trait to a stable object-safe `self: Box<Self>` receiver.
- The duplicated nightly/stable `IterExtras` implementations were collapsed into a single stable implementation.
- The old `unsized_fn_params` nightly requirement was removed from the crate root.
- CI was updated so `type_erasure` is exercised on floating `stable`, floating `nightly`, and the fixed `1.94.1` lane alongside the other misc feature combinations.
- The only remaining crate-level nightly split in this area is the `Never = !` alias, which still falls back to `Infallible` off nightly.

Planned commit:

- `refactor: ungate stable-compatible APIs`

## Stage 4: Incompatible Dependency Upgrades One By One

Status: next

Strategy:

- Handle incompatible upgrades individually, ideally one dependency or one tightly-coupled dependency pair per commit.
- Verify each step independently.
- Do not bundle high-risk runtime changes together.

Planned sequence:

1. `as_num 0.2 -> 0.3`, or remove it entirely if it is truly unused.
2. `rustc-hash 1.1 -> 2.x`
3. `executors 0.9 -> 0.10`
4. `mio 0.8 -> 1.x`
5. `dialoguer 0.10 -> 0.12`
6. DNS example dependencies:
   - `async-std-resolver 0.19 -> newer line`
   - `trust-dns-proto 0.19 -> newer line`
   - Reassess whether the modern path should be the Hickory-branded crates instead.
7. `rand 0.8 -> 0.10` in examples
8. `rand 0.7 -> 0.10` in benches
9. `criterion 0.3 -> 0.8`
10. `twox-hash 1.6 -> 2.x`
11. `rustc_version 0.2 -> 0.4`

Notes:

- `as_num` turned out to be unused in source and was removed outright rather than upgraded.
- `rustc-hash 1.1 -> 2.x` was a no-code migration; the existing `FxHashMap` / `FxHashSet` usage built unchanged on `2.1.2`.
- `executors 0.9 -> 0.10` was also a no-code migration; the scheduler and pool-constructor APIs used here built unchanged on `0.10.0`.
- `mio 0.8 -> 1.2` also built unchanged; the networking layer usage here did not require any API adaptation.
- `dialoguer 0.10 -> 0.12` was a no-code migration in the examples crate; the existing `Input` prompt usage built unchanged.
- The DNS example moved from `async-std-resolver 0.19` to `0.24.x` while preserving the async-std style of the book example.
- The direct `trust-dns-proto` dependency was removed entirely; the example now uses `async_std_resolver::proto` so the protocol types stay aligned with the resolver’s internal stack.
- The Hickory-branded migration was explicitly deferred here because the current resolver ergonomics are Tokio-centred and would add unnecessary runtime churn to a documentation example.
- `rand 0.8 -> 0.10` in the examples crate required only a small API refresh in the load-balancer example.
- That refresh slightly simplified the example by using `rand::rng()` and `rand::make_rng()` in place of the older `thread_rng()` and `SmallRng::from_entropy()` naming.
- `rand 0.7 -> 0.10` in the benches crate was similarly small and stayed confined to `actor_store`.
- The bench code now uses `rand::rng()` and `random_range(...)` instead of the removed crate-root `thread_rng()` and older `gen_range` call shape.
- `executors` and `mio` are the highest-risk production upgrades.
- The DNS example stack is likely coupled and may want to move together.
- Bench-only dependencies can be handled later if earlier runtime work reveals wider churn.

## Tracking Notes

- The effective floor is now declared explicitly rather than being left implicit in dependency resolution.
- `rustfmt.toml` still carries nightly-only options, so stable `cargo fmt` warns even though it formats successfully.
- CI now reserves pinned nightly only for formatting and linting; the main test matrix keeps floating `stable` and `nightly` alongside the fixed MSRV lane.
- The main remaining crate-level nightly dependency is the `Never` alias using `!`; the rest of the type-erasure and iterator-support split has been unified.

## Stage 5: Unsafe Audit And Semantic Labelling

Status: planned

Scope:

- Audit the current `unsafe` surface after the edition-2024 migration.
- Reassess which functions should remain safe wrappers around localised unsafe operations and which should be marked `unsafe` at the API boundary.
- Document the semantics and invariants at each retained `unsafe` site with concise, explicit comments.

Goals:

- Make the safety contract visible at the right abstraction layer.
- Reduce misleading or overly broad `unsafe` markings where the caller does not actually assume an unsafe contract.
- Ensure the remaining `unsafe` blocks are as small and justified as possible.

Planned commit:

- `audit: review unsafe contracts and labelling`
