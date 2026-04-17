# Issue 170 Follow-up Plan

This file tracks the remaining work after the initial `kompact-net` extraction PR.
It exists to keep the follow-up coherent while we make the default `kompact` story
local-only and reshape examples, docs, and CI around that contract.

## Goals

G1. Make plain `kompact` default to the local-only runtime surface.

G2. Keep distributed path-based APIs available as an explicit opt-in via
`features = ["distributed"]`.

G3. Keep the provided transport ergonomic through `kompact-net`, which enables
the required `kompact` features itself.

G4. Showcase the split through crate boundaries in `docs/examples` rather than
through feature flags inside a single examples crate.

G5. Resolve the still-open review comments before or while reshaping the public
contract, so the follow-up does not build on callback/type plumbing that we
already know is unsatisfactory.

## Breaking Changes To Call Out

B6. `kompact` becomes local-only by default.

B7. Distributed APIs in `kompact` require `features = ["distributed"]`.

B8. `serde_support`, `implicit_routes`, and the `ser_id_*` feature family are no
longer part of the plain default feature set.

B9. Users of the provided transport should depend on `kompact-net`, not on plain
`kompact` defaults plus ad-hoc distributed/network setup.

B10. Existing downstream users that rely on `ActorPath`, registration, routing,
`receive_network(...)`, or serialised distributed messaging through default
`kompact` will need to opt into the appropriate features explicitly.

## Work Plan

### Phase 1: Resolve Open Review Threads

W11. Revisit the still-open PR comments in the callback/system-handle area and
settle on the simplest acceptable API shape before further feature churn.

W12. Revert the `ComponentContext::system()` return-shape to the simpler direct
handle form that was preferred in review, instead of the current trait-alias
workaround.

W13. Simplify the dispatcher-definition callback plumbing as far as the trait
object boundaries allow.

W14. If a plain `impl FnOnce(&mut dyn Any)` is not usable at the `SystemComponents`
boundary, document the concrete reason in code and in the PR thread instead of
continuing to elaborate the type signatures.

W15. Resolve or reply to the remaining PR threads only once the final shape is
actually in place.

### Phase 2: Flip The `kompact` Default Contract

W16. Update [core/Cargo.toml](../core/Cargo.toml) so `default` contains only the
local runtime defaults we still want to bless.

W17. Remove `distributed`, `implicit_routes`, `serde_support`, and all `ser_id_*`
features from the default feature set.

W18. Ensure `implicit_routes` cannot be meaningfully enabled without
`distributed`.

W19. Audit the codebase for assumptions that distributed or serialisation support
is present under plain default `kompact`.

W20. Keep `kompact-net` depending on `kompact` with the full explicit feature set
it needs, so network users still get the right behaviour without extra manifest
work.

### Phase 3: Update Workspace Consumers

W21. Audit all workspace crates and tests that currently rely on the old defaults.

W22. Update benches and helper crates to enable `distributed`, `serde_support`,
and the appropriate `ser_id_*` feature explicitly when they use distributed or
serialised messaging.

W23. Update CI workflows and any helper bash scripts that assume the previous
default feature contract.

W24. Re-run the feature-matrix checks locally once those manifests and scripts
have been updated enough to reflect the new defaults.

### Phase 4: Split `docs/examples`

W25. Replace the single `docs/examples` crate with two workspace members:

- `docs/examples/local`
- `docs/examples/net`

W26. The local examples crate should depend on plain default `kompact`, because
it is meant to showcase the intended default user experience.

W27. The net examples crate should depend on `kompact-net`, which in turn pulls
in the distributed and serialisation features it requires.

W28. Move these examples into `docs/examples/local`:

- `helloworld`
- `actor_helloworld`
- `counters`
- `counters_pool`
- `counters_channel_pool`
- `workers`
- `workers_sender`
- `dns_resolver`
- `logging`
- `logging_custom`
- `buncher_regular`
- `buncher_adaptive`
- `buncher_config`
- `unstable_counter`
- `dynamic_components`

W29. Move these examples into `docs/examples/net`:

- `bootstrapping`
- `serialisation`
- `leader_election`
- `load_balancer`

W30. Remove leftover `receive_network(...)` boilerplate from the local examples.

W31. Only introduce a shared examples support crate if repeated code actually
makes it necessary.

### Phase 5: Update Documentation

W32. Update mdBook snippet paths to reflect the new examples crate layout.

W33. Update the prose so it clearly says:

- plain `kompact` defaults to local-only
- distributed mode is opt-in via `features = ["distributed"]`
- the provided transport lives in `kompact-net`

W34. Make the examples documentation reflect the new two-crate story explicitly,
so users can tell from the manifest alone which mode each example is teaching.

W35. Update the PR description to call out all breaking changes plainly.

### Phase 6: Validation And Benchmark Gate

W36. Run the relevant `cargo check`, `cargo test`, and doctest passes for the new
default contract after the feature and examples reshaping is in place.

W37. Re-run focused benchmarks only after checking in with the user first.

W38. Do not run the benchmark comparison under battery/VPN conditions unless the
user explicitly asks for it anyway.

## Validation Checklist

V39. `cargo check -p kompact`

V40. `cargo check -p kompact --features distributed`

V41. `cargo check -p kompact-net`

V42. Relevant `cargo test` targets for `kompact`, `kompact-net`, and updated
workspace consumers

V43. Checks for the new `docs/examples/local` and `docs/examples/net` crates

V44. CI/helper script sanity checks after their feature assumptions are updated

V45. Focused benchmark rerun only after explicit user check-in

## Notes

N46. The remaining benchmark rerun is intentionally not part of the immediate
work stream. The implementation should stop and report before that stage.

N47. The main risk is not implementation complexity, but breaking the default
contract in ways that are not clearly surfaced in docs, examples, CI, and the
PR description.
