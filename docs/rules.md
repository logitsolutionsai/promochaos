# Rules

Every check promochaos makes is a named rule with a fixed sentence template. Rules come from the fixed set below; you switch them on and give them values in `promochaos.toml`. There is no expression language and no `eval`.

Each rule gives one of four outcomes for each cart:

| Outcome | Meaning |
|---|---|
| Passed | The rule was checked and held. |
| Failed | The rule was checked and broken. The cart becomes a finding and is shrunk. |
| Warning | Allowed, but likely to cost you later. Reported, never shrunk, does not change the exit code. |
| Not applicable | The rule could not be checked for this cart, with the reason. Never counted as passed. |

## Built-In Rules for Shopify Functions

| Rule | Passes when | Not applicable when |
|---|---|---|
| `no_trap` | The run succeeds: no trap, no panic, no non-zero exit, no timeout. Shopify discards the output of a failed run. | Never. |
| `within_instruction_limit` | Instructions are at most 11,000,000 times the scale factor. | The run trapped first. |
| `within_output_limit` | Output is at most 40,000 bytes times the scale factor. A **warning** above 20,000 bytes, the limit hard-coded in function-runner 9.2.3, naming both numbers. | The run trapped or produced no output. |
| `within_memory_limit` | Linear memory is at most 10 MiB. | Never. |
| `logs_fit` | Logs are at most 1,000 bytes. Otherwise a **warning**, because Shopify truncates the logs rather than failing the run. | Never. |
| `valid_output` | The output matches the result type in `schema.graphql`: required fields present, no unknown fields, enum values valid, exactly one field set on each `@oneOf` input, `Decimal` given as a decimal string or a JSON number. Every target id (cart line, delivery group, delivery option handle) exists in the input. Percentages run from 0 to 100, and amounts are 0 or more. | The run trapped or produced no output. |
| `deterministic` | The same input run twice gives byte-identical output. Checked on a sample (`deterministic_sample`, default 50). | The cart is not in the sample. |

The input size is also checked against 128,000 bytes times the scale factor. A cart over it is reported as a problem with the generated cart, not with the function.

**Scale factor.** Above 200 cart lines, Shopify scales the limits by `lines x 0.005`, clamped to between 1 and 10 (the `@scaleLimits(rate: 0.005)` directive on `cart.lines`). promochaos takes the factor from function-runner's own analyser on the rendered input, so it matches the runner exactly.

## Merchant Rules

Set in `[rules]`. All amounts are decimal strings in the store currency.

| Rule | Passes when | Not applicable when |
|---|---|---|
| `max_discount_percent` | This run's discount, worked out with the application model below, is at most the cap, as a percent of the eligible subtotal. Shipping discounts are not counted. | The run produced no output, or the eligible subtotal is zero. |
| `never_discount` | No product candidate targets a gift card or a line with a listed tag, and no order discount covers them unless they are listed in `excludedCartLineIds`. | The cart has no such line. |
| `tiers` | The order discount percentage matches the tier the eligible subtotal falls in, read from the config with the JSON Pointer in `at`. Below the lowest tier, no order discount is expected. | The cart's config is missing or has no tiers, the run produced no output, or the eligible subtotal is within one minor unit of a converted threshold (see below). |
| `min_subtotal` | No discount is given below the minimum. | No minimum is set. |
| `codes_accepted` | Every code in `enteredDiscountCodesAccept` is one of `[store] codes`. | The cart has no entered codes. |

**Eligible subtotal.** The cart subtotal minus the lines that `never_discount` protects (gift cards and listed tags).

**`min_subtotal` forms.** A string applies to every discount kind and compares the subtotal before discounts:

```toml
min_subtotal = "50.00"
```

A table chooses the kinds and the basis:

```toml
min_subtotal = { amount = "100.00", kinds = ["shipping"], basis = "after_discounts" }
```

`kinds` is any of `order`, `product` and `shipping` (default: all three). `basis` is `before_discounts` (default) or `after_discounts`, the subtotal after order and product discounts. The table form is how promochaos finds a free-shipping threshold that is checked before discounts are taken off.

**Tiers and presentment currencies.** Tier thresholds are in the store currency. For a cart in another presentment currency, promochaos multiplies each threshold by the cart's `presentmentCurrencyRate` and rounds it to the presentment currency's minor unit (using the ISO 4217 exponent: JPY 0, USD 2, KWD 3). If the eligible subtotal is within one minor unit of a converted threshold, the rule is not applicable for that cart, because the function's own rounding decides the result there. Boundary carts at the exact threshold are generated in the store currency, where no conversion applies.

**Sentence templates** (examples):
- `tiers`, a missed boundary: "A cart of exactly {threshold} got no discount. Your config starts the {percent} tier at {threshold}."
- `tiers`, the wrong tier: "A cart of {subtotal} got {got} off. Your config puts it in the {expected} tier, which starts at {threshold}."
- `max_discount_percent`: "This cart got {discount} off its eligible subtotal of {subtotal} ({percent}). Your cap is {cap}." Each discount is rounded to the minor unit on its own, so the rule allows one minor unit per discount above the exact cap.
- `never_discount`: "{line} is a gift card, and the function discounted it by {amount}." or "{line} is tagged {tag}, and the order discount covers it: its line is not in excludedCartLineIds."
- `codes_accepted`: "The function accepted the code {code}, which is not one of your configured codes."

## Metamorphic Rules

For a sample of carts (`[generate] twins`, default 60 pairs per relation), promochaos builds a twin cart whose expected relation to the original is known, runs both, and compares this function's total discount as worked out by the application model.

| Rule | Twin | Passes when |
|---|---|---|
| `split_line` | One line of quantity n becomes two lines that add up to n. The second line gets a distinct line attribute, which is when Shopify puts one variant on two lines. | The twin's discount equals the original's. |
| `reorder_lines` | The same lines in another order. | The twin's discount equals the original's. |
| `add_ineligible_line` | A line the discount must ignore is added (a gift card or a line with a `never_discount` tag). | The twin's discount equals the original's. |
| `more_spend_never_less` | One eligible line's quantity is raised. Opt-in, because some promotions legitimately drop at a boundary. | The twin's discount is at least the original's. |

**Tolerance.** Discounts may differ by up to one minor unit per line in the twin, because a percentage of two smaller lines can round differently from the same percentage of one line.

`split_line` is not applicable when no line has a quantity of 2 or more. `add_ineligible_line` is not applicable when `never_discount` is not set.

## Command-Target Rules

For homegrown engines behind the command protocol (see `targets.md`):

| Rule | Passes when |
|---|---|
| `totals_reconcile` | `subtotal - order and product discounts + shipping charged + tax = total`. With `tax_inclusive = true`, tax is already inside the line prices and is not added again. A shipping discount is counted exactly once, in the place `shipping_discount_in` says. |
| `max_discount_percent`, `never_discount`, `min_subtotal` | As above, using the discounts the engine reports. |
| `idempotent_retry` | A `place_order` retried with the same idempotency key returns the first result (same order id, same totals) and does not use the coupon a second time. |
| `usage_limit` | A coupon is never applied to more orders than its limit in `[rules] usage_limit`. promochaos resets the engine, places the limit plus two orders with distinct keys and the same code, and counts the orders where the code was applied. |

`no_trap` also applies to command targets: a non-zero exit, a timeout, output over 1 MB or a response that is not valid protocol JSON is a crash.

## The Single-Function Application Model

To check percent caps, tiers and metamorphic relations, promochaos works out what **this one function's** candidates add up to. This is promochaos's model of one function, not Shopify's checkout, and every ledger in the report is labelled that way.

1. **Product candidates.** For each `productDiscountsAdd` operation: `ALL` applies every candidate, `FIRST` applies the first, and `MAXIMUM` applies the candidate worth the most.
2. **Order candidates.** For each `orderDiscountsAdd` operation, the candidates whose conditions hold (`orderMinimumSubtotal`, `cartLineMinimumSubtotal`, `cartLineMinimumQuantity`), then `FIRST` or `MAXIMUM`. The value is taken on the subtotal minus the lines in `excludedCartLineIds`.
3. **Fixed amounts** with `appliesToEachItem` are multiplied by the targeted quantity.
4. **Percentages** are rounded half up to the currency's minor unit.
5. **No line goes below zero.** A discount larger than what is left on a line is cut to what is left.
6. **Delivery candidates** (`deliveryDiscountsAdd`, `ALL`) apply to the targeted delivery groups or options. They count as shipping discounts and are left out of `max_discount_percent`.
7. **Codes.** Accepted and rejected codes are listed, not priced.

How this function's discount combines with other discounts is decided by Shopify from each discount's combination settings. promochaos does not simulate it, and every report says so.

## Fingerprints and Shrinking

A finding's fingerprint is the triple `(rule id, outcome kind, detail)`, where the detail is the first line of the trap message for a crash and the limit name for a limit failure. For a broken rule the detail is one fixed word per rule, with three exceptions that separate different defects: `tiers` (at a threshold, between thresholds, or below the lowest tier), `never_discount` (product or order discount, and the reason) and `min_subtotal` (basis and discount kinds). Metamorphic rules and `idempotent_retry` use one detail each, so one defect is one finding whether the twin lost its discount or only changed it, and whether or not the retried order carried a coupon. Carts with the same fingerprint collapse into one finding with a cart count.

A Shopify Function run that is still going when `timeout_ms` stops it is reported as `within_instruction_limit` (detail `instructions|timeout`), not as a trap: Shopify stops a function at the instruction limit. function-runner reports no instruction count for an interrupted run, so the finding says the count was not measured.

Shrinking keeps a step only if the fingerprint still matches. The steps, in order:

1. Remove lines (ddmin).
2. Lower quantities toward 1.
3. Move prices toward the nearest declared boundary or a round number.
4. Remove codes, attributes and the buyer identity.
5. Simplify the config (remove unused keys), only when the failure is about config.

The budget is 200 runs per failure (`--shrink-budget`). A failure that does not reproduce on its first re-run is marked `flaky` and is not shrunk. Instruction-limit failures shrink by line count instead: promochaos searches for the smallest line count that still goes over, and reports the instruction count at each size it tried.
