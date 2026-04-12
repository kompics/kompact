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
- The only likely future simplification is replacing `once_cell::sync::Lazy` with `std::sync::LazyLock`, but that belongs with the MSRV move in Stage 2 rather than here.

Verification:

- `cargo check --workspace --all-targets`

Outcome:

- Compatible manifest bumps applied as planned.
- `cargo update -w` produced no lockfile changes because the existing lock already resolved to the newest compatible releases.
- No immediate source simplifications were justified by these updates alone.
- The likely `once_cell` to `std::sync::LazyLock` simplification remains better grouped with the MSRV raise in Stage 2.

Planned commit:

- `chore(deps): apply compatible dependency updates`

## Stage 2: Edition, MSRV, And CI Tooling

Status: next

Scope:

- Move the workspace crates from edition 2018 to edition 2024.
- Declare `rust-version = "1.94.1"` where appropriate.
- Update CI tooling and remove stale `actions-rs` usage.
- Fix the workflow mismatch where the step says stable but actually installs nightly.
- Ensure `./clippy-extra.sh` passes before committing.

Expected follow-up checks:

- `./clippy-extra.sh`
- Targeted cargo checks or tests as needed after lint fixes

Planned commit:

- `chore(rust): move to edition 2024 and align MSRV/CI`

## Stage 3: Ungate Stable APIs

Status: planned

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

Planned commit:

- `refactor: ungate stable-compatible APIs`

## Stage 4: Incompatible Dependency Upgrades One By One

Status: planned

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

- `executors` and `mio` are the highest-risk production upgrades.
- The DNS example stack is likely coupled and may want to move together.
- Bench-only dependencies can be handled later if earlier runtime work reveals wider churn.

## Tracking Notes

- The workspace currently builds on nightly locally.
- Local stable `1.93.1` is already below the effective floor because `hierarchical_hash_wheel_timer 1.4.0` requires `rustc 1.94.1`.
- The current CI still tests `type_erasure` under both stable and nightly even though the feature is effectively gated to nightly in code today. Stage 2 or 3 should make that consistent again.
