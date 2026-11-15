# Tiered Discount Example (Rust)

A discount Function for the `cart.lines.discounts.generate.run` target, written with `shopify_function` 2.x. It gives 10 percent off orders from 100.00 and 20 percent from 200.00 (gift cards excluded, thresholds converted with `presentmentCurrencyRate`), and 10 percent off `multi-buy` products when a variant reaches a quantity of 2.

The default build has four seeded bugs for promochaos to find. The `fixed` feature turns all of them off.

## Seeded Bugs

| Bug | What it does | Found by |
|---|---|---|
| 1 | Compares with `>` instead of `>=` at tier thresholds, so a cart of exactly 100.00 gets no discount | boundary carts, `tiers` |
| 2 | Works out the eligible subtotal again for every line, so the work grows with the square of the line count | size sweep, `within_instruction_limit` |
| 3 | Calls `unwrap()` on the config metafield, so a discount with no configuration panics | edge carts, `no_trap` |
| 4 | Counts the multi-buy quantity per cart line instead of per variant | `split_line` |

`SCAN_BYTES` in `src/cart_lines_discounts_generate_run.rs` sets how much extra work bug 2 does per step. It is tuned so the buggy build goes over 11,000,000 instructions near 200 lines: measured on 2026-11-15, the first cart over the limit has 178 lines, and a 200-line cart uses 13,516,413 instructions. Run `promochaos budget --settings promochaos.toml` to see the curve.

## Build

The Function needs the Discount Function API schema, `schema.graphql`, in this folder. It is not included in the repository, because we found no licence that allows us to redistribute it. Fetch it with the Shopify CLI, which needs a Shopify login and the client ID of an app in your own Shopify developer account. From `examples/`:

```text
shopify app function schema --path functions/tiered-discount-rust --client-id <your app client ID>
```

The `#[typegen("schema.graphql")]` macro reads the file at compile time, so `cargo build` and `cargo test` fail without it, and the build scripts stop with the fetch command.

The Function builds for `wasm32-unknown-unknown` (the `shopify_function` 2.x crate refuses `wasm32-wasip1`):

```text
rustup target add wasm32-unknown-unknown
../../../scripts/build-examples.sh        # or scripts/build-examples.ps1 on Windows
```

The script writes `dist/function.wasm` (buggy) and `dist/function-fixed.wasm` (fixed). With the Shopify CLI installed it builds through `shopify app function build`, which also runs the trampoline step that Wasm API functions need. Without it, the script runs `cargo build` and then `shopify-function-trampoline` when that tool is on the PATH.

## Test

```text
cargo test                     # checks that hold in both builds; the seeded bugs are ignored
cargo test --features fixed    # every check, all must pass
promochaos run --settings promochaos.toml          # finds the four bugs
promochaos run --settings promochaos.fixed.toml    # must report no problems
```
