# Targets

promochaos tests one of two kinds of target: a Shopify discount Function, run offline, or a homegrown cart engine behind a small command protocol.

## Shopify Function Target

```toml
[target]
kind = "shopify-function"
extension = "extensions/tiered-discount"    # folder with shopify.extension.toml
run = "cart.lines.discounts.generate.run"   # optional; default: every discount run target
config = "config/tiered.json"               # optional: the discount metafield's jsonValue
discount_classes = ["ORDER", "PRODUCT"]     # default ["ORDER", "PRODUCT", "SHIPPING"]
wasm = "dist/function-fixed.wasm"           # optional override
runner = "in-process"                       # or "subprocess"
function_runner = "function-runner"         # binary for the subprocess runner
timeout_ms = 10000
```

### Discovery

1. promochaos reads `shopify.extension.toml` in the extension folder: `api_version`, each `[[extensions.targeting]]` entry (`target`, `input_query`, `export`) and `[extensions.build] path`.
2. It keeps the discount run targets, `cart.lines.discounts.generate.run` and `cart.delivery-options.discounts.generate.run`, or only the one named in `[target] run`.
3. It reads `schema.graphql` from the extension folder and each target's input query. If the schema is missing, promochaos stops with exit 2 and the fix: `shopify app function schema`, which needs a Shopify login.
4. It finds the built Wasm file. The first of these that is set wins:
   1. `--wasm FILE` on the command line;
   2. `[target] wasm` in `promochaos.toml`, relative to the extension folder;
   3. `[extensions.build] path` in `shopify.extension.toml`;
   4. `dist/function.wasm`.

If the Wasm file is missing, promochaos stops with exit 3 and the fix: `shopify app function build`. That step needs a local app config but no login. For Wasm API functions it also runs the trampoline step, which is why promochaos always runs the CLI's built output rather than a raw `cargo build` result.

### Old Discount APIs

A function whose targets belong to the deprecated Product, Order or Shipping Discount APIs (for example `purchase.product-discount.run`, `purchase.order-discount.run` or `purchase.shipping-discount.run`) is refused with exit 2 and a link to Shopify's migration guide for the Discount API. promochaos v1 supports only the current Discount API.

### Rendering the Input

Each generated cart is rendered as the function's input for each target's input query:
- Only the fields the query selects are included, under the query's aliases.
- Unions (`merchandise`) follow `__typename` and inline fragments.
- Argument fields are computed from the cart model: `hasAnyTag`, `hasTags`, `inAnyCollection`, `inCollections`, `attribute(key:)` and `metafield(namespace:, key:)`.
- The discount's configuration goes in as `discount.metafield.jsonValue`. Edge carts also try a missing config, an empty one and one with unexpected fields.
- Query variables (`query Input($ids: [ID!])`, filled from a metafield at checkout) are not supported yet. A query that declares any is refused with exit 2, rather than rendered with no value.

### Runners

- **In-process (default).** promochaos links function-runner v9.2.3 as a library and calls `engine::run` with a module it compiled once and reuses for every cart, so the instruction count is the same fuel count Shopify uses. JavaScript (Javy) and Wasm API functions run through the providers embedded in function-runner.
- **Subprocess.** `runner = "subprocess"` runs `function-runner --json` for each cart with the same input, schema and query. It starts one process per cart, and exists as a fallback in case the library API changes. The parity test checks that both runners report the same instruction count for the same input.

### Timeouts

function-runner sets no instruction cap and no timeout of its own: a function stuck in a loop would run forever. promochaos stops any run that takes longer than `timeout_ms` (default 10,000 ms) and reports it under `within_instruction_limit`, saying that the count was not measured.

The timer also covers function-runner's own setup, which compiles the JavaScript provider module on every run. In a debug build of promochaos that compile is slow, and a busy machine can push it past `timeout_ms` before the function starts. function-runner then panics ("Failed to instantiate imported instance: wasm trap: interrupt"). promochaos runs such a cart once more, and if setup is interrupted again it stops with exit 3 and says to raise `timeout_ms` or use a release build. It is never reported as a problem in the function.

## Command Target

```toml
[target]
kind = "command"
run = ["./target/debug/homegrown_cart"]
reset = ["./target/debug/homegrown_cart", "--reset"]
timeout_ms = 2000
tax_inclusive = false
shipping_discount_in = "discounts"          # or "shipping"
```

### Protocol `promochaos-command/1`

promochaos starts the `run` argv directly (no shell), writes one JSON request to stdin, closes stdin and reads one JSON response from stdout. The response is capped at 1 MB and the run at `timeout_ms`. The environment variable `PROMOCHAOS_WORK_DIR` points to a work folder inside the results folder; engines keep any state there. A non-zero exit, a timeout, an oversized response or a response that is not valid protocol JSON is a crash. `reset` is run before each stateful sequence.

All money is integer minor units.

**Request:**

```json
{
  "protocol": "promochaos-command/1",
  "op": "price",
  "idempotency_key": null,
  "cart": {
    "id": "cart-0007",
    "currency": "USD",
    "currency_exponent": 2,
    "lines": [
      {
        "id": "line-1",
        "variant_id": "gid://shopify/ProductVariant/2001",
        "product_id": "gid://shopify/Product/1001",
        "title": "Linen Shirt",
        "quantity": 2,
        "unit_price": 4500,
        "tags": ["multi-buy", "shirt"],
        "collections": ["gid://shopify/Collection/10"],
        "gift_card": false,
        "requires_shipping": true,
        "attributes": { "engraving": "AB" }
      }
    ],
    "codes": ["WELCOME20"],
    "buyer": { "kind": "consumer", "customer_id": "gid://shopify/Customer/1", "email": "a@example.com", "tags": [], "company_id": null },
    "shipping": { "handle": "standard", "price": 800 }
  },
  "config": null
}
```

- `op` is `price` or `place_order`. For `place_order`, `idempotency_key` is a string.
- `buyer` and `shipping` may be `null`.

**Response:**

```json
{
  "subtotal": 9000,
  "discounts": [
    { "code": "WELCOME20", "title": "20% off", "kind": "order", "amount": 1800, "line_ids": [] }
  ],
  "shipping": 800,
  "tax": 576,
  "total": 8576,
  "codes_applied": ["WELCOME20"],
  "codes_rejected": [{ "code": "NOPE", "reason": "unknown code" }],
  "order": null
}
```

- `kind` is `order`, `product` or `shipping`.
- For `place_order`, `order` is `{ "id": "order-1", "coupon_uses": { "WELCOME20": 1 } }`, the coupon uses after this order.

### Totals

`totals_reconcile` checks `subtotal - sum(order and product discounts) + shipping charged + tax (unless tax_inclusive) = total`.
- With `shipping_discount_in = "discounts"`, a shipping discount is a `shipping` entry in `discounts`, `shipping` is the full price, and shipping charged is `shipping` minus the shipping discounts.
- With `shipping_discount_in = "shipping"`, the `shipping` field is already net, and no `shipping` entries may appear in `discounts`.

### Stateful Checks

- `idempotent_retry`: reset, then `place_order` twice with the same key and cart. The second response must match the first and the coupon uses must not go up.
- `usage_limit`: reset, then `place_order` the limit plus two times with distinct keys and the same code. The code may be applied to at most the limit's number of orders.

These sequences run one at a time; `price` requests may run in parallel.
