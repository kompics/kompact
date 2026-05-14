# Agent Instructions

- Run repository-wide checks with `./run_checks.sh`.
- Run repository-wide tests with `./run_tests.sh` outside the sandbox, after confirming single tests of affected changes inside the sandbox.

## Rust Rules

- Do not run multiple `cargo` instances in parallel! They anyway lock.
- Inside the sandbox, `cargo test` may run either one explicit test or a broader test selection only when passed `-- --test-threads=1`. Any grouped or repeated multi-threaded `cargo test` run must be executed outside the sandbox.
- Format Rust code according to `rustfmt.toml`.
- Keep Rust changes clippy-clean.
- Avoid immediately executed anonymous functions such as `(|| { ... })()`. Prefer ordinary `Result` or `Option` handling when the body is short; extract a named helper for larger bodies.
- Prefer readable control flow over chained iterator side effects.
- Use Snafu-derived error types (`#[derive(Snafu)]`) for Rust error enums.
- Prefer `context(...)` / `with_context(...)` over manual `map_err(...)` when the target error still wraps the original source. Use `with_context(...)` when building the context captures clones, allocations, or other non-trivial work.
- If the only reason to introduce a new error variant is to differentiate the use-site of an existing variant, prefer adding `location: Location` to the existing variant instead.
- Use `#[snafu(module(...))]` plus module-qualified selector names when otherwise identical selector names would collide. Do not introduce custom selector aliases like `FooBarBazSnafu` just to disambiguate use sites.
- Keep Snafu variant names generic inside one error enum. Do not bake call-site names like `PublishStoreAccess` into the variant when the enum type or selector module already provides that context.
- Reserve manual `map_err(...)` for real error translation cases that `context(...)` cannot express cleanly.
- Do not manually construct Snafu boxed-source variants with `Box::new(source)`; use `result.boxed().context(SelectorSnafu)` or `context(...)`/`with_context(...)` instead.
- When splitting a single-file Rust module into a folder module, move the original module contents to `mod.rs` in the new folder.
- Avoid nesting `?` into expressions. It's easier to read if they only occur at the end of a line. Refactor the expression into a field where needed.
- Document non-public Rust helpers, fields, variants, and local types whenever their role, invariants, lifecycle, or preconditions are non-trivial or non-obvious. Prefer documenting what the item is supposed to do before adding code that explains how it does it.
- Add loop labels when control flow spans non-trivial nested loops or retries.
- Prefer the following top-level grouping within Rust files unless there is a strong local reason not to:
    1. public items (`pub`)
    2. restricted-visibility items (`pub(<qualifier>)`)
    3. macros
    4. private items
    5. exposed test helpers
    6. tests
- Within each group, use this order:
    1. constants
    2. traits
    3. functions
    4. structs/enums, each followed immediately by all associated `impl` blocks
- Imports should remain at the very top of the file/module/function.
