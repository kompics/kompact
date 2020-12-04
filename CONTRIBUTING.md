# Contributing

## Filing an Issue

If you are trying to use Kompact and run into an issue; please file an
issue. When filing an issue, do your best to be as specific as possible. Include
the version of Rust you are using (`rustc --version`) and your operating
system and version. The faster was can reproduce your issue, the faster we
can fix it for you!

## Submitting a PR

Before you submit your pull request, check that you have completed all of the
steps mentioned in the pull request template. Link the issue that your pull
request is responding to, if any, and format your code using [rustfmt][rustfmt].

### Configuring rustfmt

Before submitting code in a PR, make sure that you have formatted the codebase
using [rustfmt][rustfmt]. `rustfmt` is a tool for formatting Rust code, which
helps keep style consistent across the project. If you have not used `rustfmt`
before, it is not too difficult.

If you have not already configured `rustfmt` for the
nightly toolchain, it can be done using the following steps:

**1. Use Nightly Toolchain**

Use the `rustup override` command to make sure that you are using the nightly
toolchain. Run this command in the Kompact root directory you cloned.

```sh
rustup override set nightly
```

**2. Add the rustfmt component**

Install the most recent version of `rustfmt` using this command:

```sh
rustup component add rustfmt-preview --toolchain nightly
```

**3. Running rustfmt**

To run `rustfmt`, use this command:

```sh
cargo +nightly fmt
```

Kompact's configuration for [rustfmt][rustfmt] is always linked to a specific nightly. This may cause your formatting to fail for one of two reasons:
1. Your nightly is too old. In that case, please use the most recent nightly with rustfmt support.
2. Your nightly is newer than the last master commit of the `rustfmt.toml`. In that case feel free to update the version number in that file and include the change in your PR.

[rustfmt]: https://github.com/rust-lang-nursery/rustfmt

### IDE Configuration files
Machine specific configuration files may be generaged by your IDE while working on the project. Please make sure to add these files to either a global or Kompact's local `.gitignore` file, so that they are kept from accidentally being commited to the project.

Some examples of these files are the `.idea` folder created by JetBrains products (WebStorm, IntelliJ, etc) as well as `.vscode` created by Visual Studio Code for workspace specific settings.
