# Changelog

All notable changes to promochaos are recorded here. The format follows Keep a Changelog, and versions follow Semantic Versioning.

## [0.1.0] - Unreleased

### Added

- `promochaos run`: seeded cart generation (boundaries, size sweep, edge shapes, pairwise coverage, random), checks, shrinking and reports.
- Shopify Function target for discount Functions on the current Discount API (`cart.lines.discounts.generate.run` and `cart.delivery-options.discounts.generate.run`), run in-process through function-runner 9.2.3, with a subprocess fallback.
- Command target for homegrown cart engines (protocol `promochaos-command/1`).
- Rules: limits, output validity, determinism, merchant rules (`max_discount_percent`, `never_discount`, `tiers`, `min_subtotal`, `codes_accepted`), metamorphic rules (`split_line`, `reorder_lines`, `add_ineligible_line`, `more_spend_never_less`), and command-target rules (`totals_reconcile`, `idempotent_retry`, `usage_limit`).
- Outputs: `report.html` (one offline file), `results.json`, `junit.xml`, and per finding a shrunk cart, raw output, ledger, a JSON fixture for the JS test helpers and a Rust `#[test]`.
- `promochaos replay`, `promochaos report`, `promochaos budget`, `promochaos init` and `promochaos check-settings`.
- Examples with seeded bugs: a Rust tiered discount, a JavaScript bundle discount and a homegrown cart engine.
