# Release Process

This checklist describes the normal Kompact release flow. Use `<VERSION>` for
the crate version, for example `0.12.0`, and `v<VERSION>` for the Git tag, for
example `v0.12.0`.

## Release Targets

R1. Publish the public crates in dependency order:

```text
kompact-actor-derive
kompact-component-derive
kompact
kompact-net
```

R2. Keep docs example crate versions aligned with the release version when they
are intended to mirror the published release.

## Preparation

R3. Start from the release branch, normally `master`, with all intended changes
merged.

R4. Make sure CI is green before starting the release preparation.

R5. Ensure a crates.io token is configured for `cargo publish`:

```bash
cargo login
```

Paste a crates.io API token with publish rights when prompted. Afterwards, run a
cheap registry sanity check:

```bash
cargo owner --list kompact
```

This should list the crate owners and confirms Cargo can reach the registry
before the publish step.

R6. Review the worktree so the release commit contains only intentional
changes.

## Version Bump

R7. Update the package versions in:

```text
core/Cargo.toml
network/Cargo.toml
macros/actor-derive/Cargo.toml
macros/component-definition-derive/Cargo.toml
docs/examples/local/Cargo.toml
docs/examples/net/Cargo.toml
```

R8. Update internal crate dependency versions:

```text
core/Cargo.toml:
  kompact-actor-derive = { version = "<VERSION>", ... }
  kompact-component-derive = { version = "<VERSION>", ... }

network/Cargo.toml:
  kompact = { version = "<VERSION>", ... }
```

R9. Refresh `Cargo.lock` if Cargo does not update it during the normal checks.

R10. Commit the version bump after validation, for example:

```bash
git commit -am "Bump version to <VERSION>"
```

## Validation

R11. Run the repository checks:

```bash
./run_checks.sh
```

R12. Run the repository tests:

```bash
./run_tests.sh
```

R13. Run publish dry-runs in dependency order:

```bash
cargo publish -p kompact-actor-derive --dry-run
cargo publish -p kompact-component-derive --dry-run
cargo publish -p kompact --dry-run
cargo publish -p kompact-net --dry-run
```

The first two dry-runs should work before anything is published. The `kompact`
dry-run may fail until the new macro crate versions have actually been
published and indexed by crates.io. Likewise, the `kompact-net` dry-run may fail
until the new `kompact` version has actually been published and indexed.

R14. Merge or push the release commit to the release branch and wait for CI to
pass on the exact commit that will be tagged.

## Publishing

R15. Publish crates in dependency order:

```bash
cargo publish -p kompact-actor-derive
cargo publish -p kompact-component-derive
cargo publish -p kompact
cargo publish -p kompact-net
```

R16. Wait for crates.io indexing between dependent publishes if necessary,
especially before publishing `kompact` after the macro crates and `kompact-net`
after `kompact`.

## Tagging

R17. Create an annotated release tag on the released commit:

```bash
git tag -a v<VERSION> -m "Release version <VERSION>"
```

R18. Push the tag:

```bash
git push upstream v<VERSION>
```

Use `upstream` rather than `origin` when working from a personal fork.

## GitHub Release

R19. Create a GitHub release from `v<VERSION>`.

R20. Summarise user-facing changes since the previous release tag.

R21. Write release notes manually. Do not rely on GitHub's generated release
notes as-is. Use the commit and PR history since the previous release tag to
produce a short, structured summary with:

```text
Highlights
Breaking Changes
Migration Notes
New Features
Fixes
Internal Changes
```

Omit empty sections. Focus on user-facing behaviour and API impact first, then
summarise important implementation or maintenance work. Call out newly published
crates, crate splits, feature changes, MSRV changes, dependency updates that
matter to users, and any known limitations.

## Post-Release Checks

R22. Verify that the crates are visible on crates.io:

```text
https://crates.io/crates/kompact
https://crates.io/crates/kompact-net
https://crates.io/crates/kompact-actor-derive
https://crates.io/crates/kompact-component-derive
```

R23. Verify that docs.rs has built the new release documentation.
