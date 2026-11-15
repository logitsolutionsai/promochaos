# Bundle Discount Example (JavaScript)

A JavaScript discount Function for the `cart.lines.discounts.generate.run` target. Lines tagged `bundle` get 5 percent off per extra bundle item once the cart holds 2 or more. The default build has three seeded bugs for promochaos to find.

## Seeded Bugs

| Bug | What it does | Found by |
|---|---|---|
| 1 | Targets every line, gift cards included | `never_discount`, `add_ineligible_line` |
| 2 | Has no cap, so 22 or more bundle items emit a percentage over 100 | `valid_output` |
| 3 | Accepts every entered code instead of only `BUNDLE10` | `codes_accepted` |

Percentages are emitted as JSON numbers (`value: 10`), as in Shopify's own JavaScript template. That is valid output and not one of the bugs.

## Expected Findings

The seeded build gives six findings, and `crates/promochaos-cli/tests/examples.rs` checks for exactly these six:

| Rule | Why |
|---|---|
| `valid_output` | Seeded bug 2: a percentage over 100. |
| `never_discount` | Seeded bug 1: gift cards are discounted. |
| `codes_accepted` | Seeded bug 3: any entered code is accepted. |
| `max_discount_percent` | Follows from bug 2: with no cap, big bundles get more than the 60 percent cap in `promochaos.toml`. |
| `add_ineligible_line` | Follows from bug 1: adding a gift card changes the discount, because the gift card is discounted too. |
| `within_instruction_limit` | Carts of 198 lines or more go over the 11 million instruction limit (11,051,224 instructions at 198 lines, measured 2026-11-15 on Windows 11, Intel Core i7-1280P). The JavaScript runtime costs about 55,000 instructions per line here. The fixed build stays under the limit at every size. |

The fixed build must give none.

## Build

JavaScript Functions are compiled to Wasm by the Shopify CLI (it bundles the source and runs Javy). This folder holds the source only. It also needs the Discount Function API schema, `schema.graphql`, which is not included in the repository, because we found no licence that allows us to redistribute it. The schema fetch needs a Shopify login and the client ID of an app in your own Shopify developer account. From `examples/`, with the Shopify CLI installed:

```text
shopify app function schema --path functions/bundle-discount-js --client-id <your app client ID>
cd functions/bundle-discount-js && npm install && cd ../..
shopify app function build --path functions/bundle-discount-js
```

That writes the buggy build to `dist/function.wasm`. For the fixed build:

1. Copy `src/index.fixed.js` over `src/index.js`.
2. Run the build again and rename `dist/function.wasm` to `dist/function-fixed.wasm`.
3. Put back the original `src/index.js` (one line: `export * from './cart_lines_discounts_generate_run';`) and build once more.

`scripts/build-examples.sh` and `scripts/build-examples.ps1` do these steps when the Shopify CLI is on the PATH.

## Run

```text
promochaos run --settings promochaos.toml
```

For the fixed build, run `promochaos run --settings promochaos.fixed.toml`. That run must report no problems.
