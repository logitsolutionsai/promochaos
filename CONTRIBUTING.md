# Contributing to promochaos

Thank you for helping. This file explains how to build, test and propose a change.

## Before You Start

- For a bug, open an issue with the promochaos version, your operating system, the command you ran and the output. A `promochaos-results/findings/F-xxx` folder from a run is the most useful attachment.
- For a new rule, generator or target, open an issue first so we can agree on the design. Rules come from a fixed set with values; promochaos has no expression language and no `eval`, and that is on purpose.

## Build

You need the Rust toolchain pinned in `rust-toolchain.toml` (Rust 1.95.0, with the `wasm32-unknown-unknown` target).

```sh
rustup show                      # installs the pinned toolchain and target
cargo build --workspace
cargo test --workspace
```

The example Shopify Functions build separately. They need the Discount Function API schema, `schema.graphql`, which is not in the repository (we found no licence that allows us to redistribute it, and `.gitignore` keeps local copies out of commits). Fetch it with the Shopify CLI, which needs a Shopify login and the client ID of an app in your own Shopify developer account:

```sh
cd examples
shopify app function schema --path functions/tiered-discount-rust --client-id <your app client ID>
shopify app function schema --path functions/bundle-discount-js --client-id <your app client ID>
cd ..
bash scripts/build-examples.sh     # Linux and macOS
pwsh scripts/build-examples.ps1    # Windows
```

The build scripts stop with the fetch command when a schema is missing.

The Rust example builds for `wasm32-unknown-unknown`. The JavaScript example needs the Shopify CLI, which builds it with Javy. Tests that need a built `.wasm` file skip with a message when the file is missing.

## Checks a Pull Request Must Pass

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

CI runs these on Linux, macOS and Windows, then runs promochaos against the homegrown example: the seeded-bug build must report problems (exit 1) and the fixed build must report none (exit 0). CI has no Shopify login, so it cannot fetch `schema.graphql`; it skips building the example Functions, and their tests skip with a message. Run those locally before a change to the Shopify target (README, Running the Shopify Examples).

A few files are generated, and a test or CI job fails when they are out of date:

- `docs/results-schema.json`, from the result types: `PROMOCHAOS_UPDATE_SCHEMA=1 cargo test -p promochaos-core results_schema`.
- The snapshots in `crates/promochaos-cli/tests/snapshots/`: review changes with `cargo insta review`, or accept them with `INSTA_UPDATE=always cargo test -p promochaos-cli --test examples`.
- `THIRD-PARTY-LICENSES.html`, after any change to `Cargo.lock`: the command at the top of `about.toml` (needs cargo-about 0.9.3). `about.toml` accepts only the licences `deny.toml` allows; keep the two lists the same.

## Code Rules

- Money is held in integer minor units. Decimal strings are parsed with `rust_decimal`. Never use floats for money.
- Output is deterministic: the same seed gives the same carts, and every list in the output is sorted.
- No `unwrap` or `expect` in library code outside tests.
- Errors map to the exit codes: 0 clean, 1 problems found, 2 bad input or config, 3 target unavailable, 4 internal error.
- Sentences in reports come from rule templates. No model writes text.

## Shopify API Changes

Shopify changes the Function APIs every quarter. Facts the code depends on live in `docs/shopify-facts.md` with the date each was checked. If you change code because of an API change, update that file in the same pull request.

## Licence

By contributing, you agree that your contribution is licensed under the Apache License, Version 2.0, as described in `LICENSE`.
