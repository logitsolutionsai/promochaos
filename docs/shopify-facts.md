# Facts About Shopify Functions That promochaos Depends On

The code depends on each fact below. Every entry has the date it was checked and its source. Re-check all of them at every Shopify API release (quarterly) and at every function-runner bump, and update this file in the same change as the code.

## Discount Function API

Checked 2026-11-14 against API version 2026-10 unless a line says otherwise. Source: the shopify.dev Discount Function API reference and changelog.

- **Run targets.** `cart.lines.discounts.generate.run` (discount classes ORDER and PRODUCT) and `cart.delivery-options.discounts.generate.run` (SHIPPING). The discount's `discountClasses` decide which targets run.
- **Input.** `cart.lines[]` with `id`, `quantity`, `cost`, `merchandise` (a `ProductVariant` or a `CustomProduct`), `discountAllocations`, `attribute`, `sellingPlanAllocation` and `parentRelationship`; `cart.cost`; `cart.buyerIdentity`; `cart.deliveryGroups`; `discount { discountClasses, metafield }`; `enteredDiscountCodes { code, rejectable }`; `triggeringDiscountCode`; `localization`; `presentmentCurrencyRate`; `shop`.
- **Cart output operations.**
  - `enteredDiscountCodesAccept { codes }`.
  - `enteredDiscountCodesReject { codes, message }`, for automatic discounts only.
  - `orderDiscountsAdd { candidates, selectionStrategy: FIRST | MAXIMUM }`. Targets are `orderSubtotal { excludedCartLineIds }`. Conditions are `orderMinimumSubtotal`, `cartLineMinimumSubtotal` and `cartLineMinimumQuantity`.
  - `productDiscountsAdd { candidates, selectionStrategy: ALL | FIRST | MAXIMUM }`. Targets are `cartLine { id, quantity }`. Values are `percentage { value }` or `fixedAmount { amount, appliesToEachItem }`.
- **Delivery output.** `deliveryDiscountsAdd { candidates, selectionStrategy: ALL }`. Targets are `deliveryGroup { id }` or `deliveryOption { handle }`.
- **Configuration.** The input query reads `discount { metafield(namespace: "$app", key: "...") { jsonValue } }`.
- **Limits.** Binary 256 KiB (262,144 bytes). Linear memory 10 MiB (10,485,760 bytes), stack 512 kB. Logs 1 kB, truncated after that. Up to 200 cart lines: 11 million instructions, 128 kB input, 40 kB output. Above 200 lines these "scale proportionally as the number of cart lines increases"; the page gives no formula or cap. Binary and memory sizes are exact byte counts; the other limits count 1 kB as 1,000 bytes. Re-checked 2026-11-15 at https://shopify.dev/docs/api/functions/latest (Resource limits, API version 2026-10): unchanged.
- **Scaling.** The schema marks `cart.lines` and `cart.deliveryGroups[].cartLines` with `@scaleLimits(rate: 0.005)`. function-runner multiplies the field's length by the rate and clamps the factor to between 1 and 10, so 250 lines give a factor of 1.25. Source: `schema.graphql` in Shopify's extensions-templates (`functions-discount-rs`, api_version 2026-07) and function-runner v9.2.3 `src/scale_limits_analyzer.rs`. Checked 2026-11-14.
- **Old APIs.** The Product, Order and Shipping Discount APIs are deprecated. Shopify's May 2025 changelog says to move before API version 2026-04, yet their reference pages still exist at 2026-10. promochaos v1 supports only the current Discount API and refuses an old-API function with exit 2 and a link to the migration guide.

## Schema Details

Source: `functions-discount-rs/schema.graphql` and `functions-discount-js/schema.graphql` in github.com/Shopify/extensions-templates, branch `main`. Checked 2026-11-14.

- The templates' schema is **api_version 2026-07** (the first line of the file says `# api_version: 2026-07`, and both templates' `shopify.extension.toml` set `api_version = "2026-07"`). A 2026-10 schema was not available in the templates on 2026-11-14. The examples set `api_version = "2026-07"`. The schema file itself is not included in this repository (no stated licence); `shopify app function schema --path <function folder>` writes it for the API version in the extension TOML. That command needs a Shopify login: run without one (Shopify CLI 5.0.1, 2026-11-15), it stops with "To run this command, log in to Shopify."
- `isGiftCard` is on `Product` and on `CustomProduct`. It is **not** on `ProductVariant`; a query reads it as `merchandise { ... on ProductVariant { product { isGiftCard } } }`.
- Argument fields that promochaos computes from the cart model: `Product.hasAnyTag(tags)`, `Product.hasTags(tags)`, `Product.inAnyCollection(ids)`, `Product.inCollections(ids)`, `ProductVariant.inAnyCollection(ids)`, `ProductVariant.inCollections(ids)`, `Customer.hasAnyTag(tags)`, `Customer.hasTags(tags)`, `CartLine.attribute(key)`, `Cart.attribute(key)` and every `metafield(namespace, key)`.
- `Metafield.jsonValue` has the scalar type `JSON`. `MoneyV2.amount`, `Percentage.value`, `FixedAmount.amount` and `presentmentCurrencyRate` have the scalar type `Decimal`.
- `CartOperation`, `DeliveryOperation`, every `...Target`, every `...Value` and `Condition` are `@oneOf` input objects: exactly one field must be set.
- The result types come from the `MutationRoot` fields `cartLinesDiscountsGenerateRun(result: CartLinesDiscountsGenerateRunResult!)` and `cartDeliveryOptionsDiscountsGenerateRun(result: CartDeliveryOptionsDiscountsGenerateRunResult!)`.

## Decimal Values in Output

Source: `functions-discount-js/src/cart_lines_discounts_generate_run.liquid` in extensions-templates. Checked 2026-11-14.

- Shopify's own JavaScript template returns `value: { percentage: { value: 10 } }`, with the `Decimal` as a JSON **number**. So a number is accepted for `Decimal` in output, as well as a decimal string. promochaos accepts both, and checks the range (percentages from 0 to 100, amounts 0 or more).
- The Rust template returns `Decimal(10.0)`, which the `shopify_function` crate writes as the string `"10.0"`.

## Rust Functions

Source: `shopify_function` 2.3.0 on crates.io (`src/lib.rs`) and the `functions-discount-rs` template. Checked 2026-11-14.

- `shopify_function` 2.x refuses to build for `wasm32-wasip1` with a `compile_error!`: "Compiling to wasm32-wasip1 is unsupported, change your target to wasm32-unknown-unknown instead".
- Shopify's Rust template builds with `cargo build --target=wasm32-unknown-unknown --release` and points `[extensions.build] path` at `target/wasm32-unknown-unknown/release/<name>.wasm`.
- Exports are named after the Rust function (`cart_lines_discounts_generate_run`). JavaScript exports are kebab-case (`cart-lines-discounts-generate-run`).
- `shopify_function::run_function_with_input(f, payload: &str)` is available on non-Wasm targets for native unit tests. Output structs generated by `#[typegen]` derive `Debug`, `PartialEq` and `Clone`.
- function-runner refuses a module that imports both the `shopify_function_v2` provider and WASI, with the hint to build for `wasm32-unknown-unknown`.
- Built Wasm API functions go through the Shopify CLI's trampoline step (`shopify app function build`) before they run. promochaos always runs the CLI's built output.

## function-runner v9.2.3

Source: github.com/Shopify/function-runner at tag `v9.2.3`: `Cargo.toml`, `src/lib.rs`, `src/engine.rs`, `src/io.rs`, `src/validated_module.rs`, `src/function_run_result.rs`, `src/main.rs`, `rust-toolchain.toml`, `LICENSE.md`. Checked 2026-11-14.

- **Package.** Name `function-runner`, version 9.2.3, edition 2021, no `[lib]` section (the library is `function_runner`), no Cargo features of its own. Apache-2.0 (Copyright 2022, Shopify Inc.), though `Cargo.toml` has no `license` field. `rust-toolchain.toml` pins Rust 1.95.0.
- **Dependencies that matter to us.** `wasmtime` 48 (with `cranelift`, `winch`, `cache`, `pooling-allocator` and other features), `wasmtime-wasi` 48, `deterministic-wasi-ctx` =4.0.7, `serde_json` with `preserve_order` **and `arbitrary_precision`**. Cargo unifies features, so every crate in a build that links function-runner gets `arbitrary_precision`.
- **API.** `engine::run(params: FunctionRunParams) -> anyhow::Result<FunctionRunResult>`, synchronous.
  - `FunctionRunParams<'a> { function_path: PathBuf, input: BytesContainer, export: &'a str, profile_opts: Option<&'a ProfileOpts>, scale_factor: f64, module: wasmtime::Module, engine: wasmtime::Engine }`.
  - `FunctionRunResult { name, size (KiB), memory_usage (KiB), instructions, logs, input: BytesContainer, output: BytesContainer, profile, scale_factor, success }`.
  - `engine::new_engine()` builds the engine with fuel and epoch interruption on, and Wasmtime's on-disk compilation cache.
  - `engine::uses_msgpack_provider(&Module)` picks the input codec.
  - `BytesContainer::new(BytesContainerType::Input, Codec::Json, raw)` re-serialises JSON input the way Shopify does (escaped `/`, preserved key order).
  - `bluejay_schema_analyzer::BluejaySchemaAnalyzer::analyze_schema_definition(schema, schema_path, query, query_path, &input_json)` returns the scale factor.
- **The caller compiles the function module** and passes it in, so promochaos can compile each `.wasm` once and reuse it.
- **The provider module is compiled on every run.** `io.rs` calls `Module::from_binary` for the embedded Javy or `shopify_function` provider inside `run`. The provider bytes are private to the crate, so promochaos cannot cache that compile without carrying a modified copy of `engine.rs` and `io.rs`. function-runner PR #619 (open) measures 8 to 12 ms per run for this compile.
- **No instruction cap and no timeout.** `run` starts fuel at `u64::MAX` and sets an epoch deadline that nothing advances, so a function in an endless loop runs until the process is stopped. promochaos adds its own per-run timeout by advancing the engine's epoch from a watchdog.
- **Hard-coded limits.** Output 20,000 bytes, input 128,000 bytes, instructions 11,000,000 (`src/function_run_result.rs`), logs 1,000 bytes (`FUNCTION_LOG_LIMIT`). The output limit differs from the 40 kB in Shopify's docs; promochaos checks against the docs and warns above 20,000 bytes, naming both numbers.
- **Panics.** Parts of `run` use `expect` and `unwrap` (provider loading, the function file name, finalising output). promochaos catches a panic from the runner and reports it as an internal error.

## Testing Helpers Fixture Shape

Source: `src/methods/load-fixture.ts` in github.com/Shopify/shopify-function-test-helpers, branch `main`; npm package `@shopify/shopify-function-test-helpers` 1.1.1, MIT. Checked 2026-11-14.

- `loadFixture(filename)` reads JSON of the shape `{ "payload": { "export": "...", "target": "...", "input": { ... }, "output": { ... } } }` and returns `{ export, input, expectedOutput: payload.output, target }`. Other top-level keys are ignored.
- The template test compares `runResult.result.output` with `fixture.expectedOutput` using `toEqual`, so a fixture pins one exact output.
- `runFunction` returns `metadata { instructionCount, memoryUsageKiB, moduleSizeKiB }`, which a test can use for an instruction check.
