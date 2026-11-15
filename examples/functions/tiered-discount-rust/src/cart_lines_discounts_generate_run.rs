//! Tiered order discount with a multi-buy product discount.
//!
//! - Order discount: the tier percentage from the config on the order subtotal,
//!   with gift-card lines excluded. Thresholds are in the shop currency and are
//!   multiplied by `presentmentCurrencyRate`.
//! - Product discount: `multiBuy.percentage` off `multi-buy` tagged lines when the
//!   variant's total quantity across all lines reaches `multiBuy.minQuantity`.
//!
//! The default build has four seeded bugs for promochaos to find. Build with
//! `--features fixed` for the correct version.

#[cfg(feature = "fixed")]
use std::collections::BTreeMap;

use super::schema;
use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

// FIRST COMPILE: check this generated path
type Merchandise = schema::cart_lines_discounts_generate_run::input::cart::lines::Merchandise;

/// Money is compared in thousandths of the currency unit, so three-decimal
/// currencies (KWD) compare exactly too.
const MINOR_SCALE: f64 = 1000.0;

struct Tier {
    threshold: f64,
    percentage: f64,
}

struct MultiBuy {
    min_quantity: i64,
    percentage: f64,
}

#[derive(Default)]
struct Config {
    tiers: Vec<Tier>,
    multi_buy: Option<MultiBuy>,
}

struct LineFacts {
    id: String,
    #[cfg_attr(not(feature = "fixed"), allow(dead_code))]
    variant_id: Option<String>,
    quantity: i64,
    /// Line subtotal in thousandths of the presentment currency unit.
    subtotal: i64,
    gift_card: bool,
    multi_buy: bool,
}

fn to_scaled(amount: f64) -> i64 {
    (amount * MINOR_SCALE).round() as i64
}

#[shopify_function]
fn cart_lines_discounts_generate_run(
    input: schema::cart_lines_discounts_generate_run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    #[cfg(not(feature = "fixed"))]
    let config_value = {
        // Seeded bug 3: the config metafield is unwrapped. A discount saved
        // before its configuration (no metafield) makes the function panic,
        // and Shopify discards the output of a run that traps.
        input.discount().metafield().unwrap().json_value()
    };
    #[cfg(feature = "fixed")]
    let config_value = match input.discount().metafield() {
        Some(metafield) => metafield.json_value(),
        None => return Ok(no_discount()),
    };
    let config = parse_config(config_value);

    let rate = input.presentment_currency_rate().as_f64();

    let mut lines = Vec::new();
    for line in input.cart().lines().iter() {
        let (variant_id, gift_card, multi_buy) = match line.merchandise() {
            Merchandise::ProductVariant(variant) => (
                Some(variant.id().to_string()),
                variant.product().is_gift_card().to_owned(),
                variant.product().multi_buy().to_owned(),
            ),
            Merchandise::CustomProduct(custom) => (None, custom.is_gift_card().to_owned(), false),
            _ => (None, false, false),
        };
        lines.push(LineFacts {
            id: line.id().to_string(),
            variant_id,
            quantity: i64::from(line.quantity().to_owned()),
            subtotal: to_scaled(line.cost().subtotal_amount().amount().as_f64()),
            gift_card,
            multi_buy,
        });
    }

    let classes = input.discount().discount_classes();
    let has_order_class = classes.contains(&schema::DiscountClass::Order);
    let has_product_class = classes.contains(&schema::DiscountClass::Product);

    let mut operations = Vec::new();

    if has_order_class {
        if let Some(percentage) = tier_percentage(&lines, &config.tiers, rate) {
            let excluded: Vec<String> = lines
                .iter()
                .filter(|line| line.gift_card)
                .map(|line| line.id.clone())
                .collect();
            operations.push(order_discount(percentage, excluded));
        }
    }

    if has_product_class {
        if let Some(multi_buy) = &config.multi_buy {
            let targets = multi_buy_lines(&lines, multi_buy);
            if !targets.is_empty() {
                operations.push(product_discount(multi_buy.percentage, targets));
            }
        }
    }

    Ok(schema::CartLinesDiscountsGenerateRunResult { operations })
}

#[cfg(feature = "fixed")]
fn no_discount() -> schema::CartLinesDiscountsGenerateRunResult {
    schema::CartLinesDiscountsGenerateRunResult { operations: vec![] }
}

// ---------------------------------------------------------------------------
// Config

fn parse_config(value: &JsonValue) -> Config {
    let JsonValue::Object(map) = value else {
        return Config::default();
    };

    let mut tiers = Vec::new();
    if let Some(JsonValue::Array(items)) = map.get("tiers") {
        for item in items {
            let JsonValue::Object(tier) = item else {
                continue;
            };
            if let (Some(threshold), Some(percentage)) =
                (number(tier.get("threshold")), number(tier.get("percentage")))
            {
                if threshold >= 0.0 && percentage > 0.0 && percentage <= 100.0 {
                    tiers.push(Tier {
                        threshold,
                        percentage,
                    });
                }
            }
        }
    }

    let multi_buy = match map.get("multiBuy") {
        Some(JsonValue::Object(rule)) => {
            match (number(rule.get("minQuantity")), number(rule.get("percentage"))) {
                (Some(min_quantity), Some(percentage))
                    if min_quantity >= 1.0 && percentage > 0.0 && percentage <= 100.0 =>
                {
                    Some(MultiBuy {
                        min_quantity: min_quantity as i64,
                        percentage,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    };

    Config { tiers, multi_buy }
}

/// Reads a number written either as a JSON number or as a decimal string.
fn number(value: Option<&JsonValue>) -> Option<f64> {
    let parsed = match value {
        Some(JsonValue::Number(number)) => Some(*number),
        Some(JsonValue::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    parsed.filter(|number| number.is_finite())
}

// ---------------------------------------------------------------------------
// Tiers

/// The percentage of the highest tier the eligible subtotal reaches.
fn pick_tier(eligible: i64, tiers: &[Tier], rate: f64) -> Option<f64> {
    let mut best: Option<(i64, f64)> = None;
    for tier in tiers {
        let threshold = to_scaled(tier.threshold * rate);
        let higher = match best {
            Some((best_threshold, _)) => threshold >= best_threshold,
            None => true,
        };
        if reached(eligible, threshold) && higher {
            best = Some((threshold, tier.percentage));
        }
    }
    best.map(|(_, percentage)| percentage)
}

#[cfg(not(feature = "fixed"))]
fn reached(eligible: i64, threshold: i64) -> bool {
    // Seeded bug 1: `>` where the config means "at least", so a cart of
    // exactly the threshold gets no discount.
    eligible > threshold
}

#[cfg(feature = "fixed")]
fn reached(eligible: i64, threshold: i64) -> bool {
    eligible >= threshold
}

/// Bytes of each line id hashed per inner step of the seeded quadratic loop.
/// Tuned with `promochaos budget` (QA, 2026-11-15) so the buggy build goes
/// over 11,000,000 instructions near 200 lines, as the spec's walkthrough
/// describes. The first value (4 full passes over each id) went over at 69
/// lines.
#[cfg(not(feature = "fixed"))]
const SCAN_BYTES: usize = 8;

#[cfg(not(feature = "fixed"))]
fn tier_percentage(lines: &[LineFacts], tiers: &[Tier], rate: f64) -> Option<f64> {
    // Seeded bug 2: for every line, the eligible subtotal is worked out again
    // by scanning all lines, so the work grows with the square of the line
    // count. Big carts go over the instruction limit and Shopify discards the
    // discount.
    let mut percentage = None;
    let mut checksum: u64 = 0;
    for _line in lines {
        let mut eligible: i64 = 0;
        for other in lines {
            for byte in other.id.bytes().take(SCAN_BYTES) {
                checksum = checksum.wrapping_mul(31).wrapping_add(u64::from(byte));
            }
            checksum = std::hint::black_box(checksum);
            if !other.gift_card {
                eligible = eligible.saturating_add(other.subtotal);
            }
        }
        percentage = pick_tier(eligible, tiers, rate);
    }
    std::hint::black_box(checksum);
    percentage
}

#[cfg(feature = "fixed")]
fn tier_percentage(lines: &[LineFacts], tiers: &[Tier], rate: f64) -> Option<f64> {
    let eligible = lines
        .iter()
        .filter(|line| !line.gift_card)
        .fold(0i64, |sum, line| sum.saturating_add(line.subtotal));
    pick_tier(eligible, tiers, rate)
}

// ---------------------------------------------------------------------------
// Multi-buy

#[cfg(not(feature = "fixed"))]
fn multi_buy_lines(lines: &[LineFacts], rule: &MultiBuy) -> Vec<String> {
    // Seeded bug 4: the multi-buy quantity is counted per cart line, not per
    // variant. Shopify puts one variant on two lines when the line attributes
    // or selling plans differ, and those two shirts lose the discount.
    lines
        .iter()
        .filter(|line| line.multi_buy && !line.gift_card && line.quantity >= rule.min_quantity)
        .map(|line| line.id.clone())
        .collect()
}

#[cfg(feature = "fixed")]
fn multi_buy_lines(lines: &[LineFacts], rule: &MultiBuy) -> Vec<String> {
    let mut per_variant: BTreeMap<&str, i64> = BTreeMap::new();
    for line in lines.iter().filter(|line| line.multi_buy && !line.gift_card) {
        if let Some(variant) = &line.variant_id {
            let total = per_variant.entry(variant.as_str()).or_insert(0);
            *total = total.saturating_add(line.quantity);
        }
    }
    lines
        .iter()
        .filter(|line| line.multi_buy && !line.gift_card)
        .filter(|line| {
            line.variant_id
                .as_deref()
                .and_then(|variant| per_variant.get(variant))
                .is_some_and(|total| *total >= rule.min_quantity)
        })
        .map(|line| line.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Output

fn order_discount(percentage: f64, excluded_cart_line_ids: Vec<String>) -> schema::CartOperation {
    schema::CartOperation::OrderDiscountsAdd(schema::OrderDiscountsAddOperation {
        selection_strategy: schema::OrderDiscountSelectionStrategy::First,
        candidates: vec![schema::OrderDiscountCandidate {
            targets: vec![schema::OrderDiscountCandidateTarget::OrderSubtotal(
                schema::OrderSubtotalTarget {
                    excluded_cart_line_ids,
                },
            )],
            message: Some(format!("{percentage}% off your order")),
            value: schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(percentage),
            }),
            conditions: None,
            associated_discount_code: None,
        }],
    })
}

fn product_discount(percentage: f64, line_ids: Vec<String>) -> schema::CartOperation {
    schema::CartOperation::ProductDiscountsAdd(schema::ProductDiscountsAddOperation {
        selection_strategy: schema::ProductDiscountSelectionStrategy::All,
        candidates: vec![schema::ProductDiscountCandidate {
            targets: line_ids
                .into_iter()
                .map(|id| {
                    schema::ProductDiscountCandidateTarget::CartLine(schema::CartLineTarget {
                        id,
                        quantity: None,
                    })
                })
                .collect(),
            message: Some(format!("Multi-buy {percentage}% off")),
            value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(percentage),
            }),
            associated_discount_code: None,
            prerequisites: None,
        }],
    })
}

// ---------------------------------------------------------------------------
// Tests. `cargo test` runs the checks that hold in both builds and ignores the
// ones that document a seeded bug; `cargo test --features fixed` runs them all.

#[cfg(test)]
mod tests {
    use super::*;
    use shopify_function::run_function_with_input;

    const CONFIG: &str = r#"{
        "tiers": [
            { "threshold": "100.00", "percentage": "10" },
            { "threshold": "200.00", "percentage": "20" }
        ],
        "multiBuy": { "minQuantity": 2, "percentage": "10" }
    }"#;

    fn variant_line(id: &str, variant: &str, quantity: i32, amount: &str, gift_card: bool, multi_buy: bool) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "quantity": {quantity},
                "cost": {{ "subtotalAmount": {{ "amount": "{amount}" }} }},
                "merchandise": {{
                    "__typename": "ProductVariant",
                    "id": "{variant}",
                    "product": {{ "isGiftCard": {gift_card}, "multiBuy": {multi_buy} }}
                }}
            }}"#
        )
    }

    fn input(rate: &str, lines: &[String], metafield: Option<&str>) -> String {
        let metafield = match metafield {
            Some(config) => format!(r#"{{ "jsonValue": {config} }}"#),
            None => "null".to_string(),
        };
        format!(
            r#"{{
                "presentmentCurrencyRate": "{rate}",
                "cart": {{ "lines": [{lines}] }},
                "discount": {{
                    "discountClasses": ["ORDER", "PRODUCT"],
                    "metafield": {metafield}
                }}
            }}"#,
            lines = lines.join(",")
        )
    }

    #[allow(irrefutable_let_patterns)]
    fn order_discounts(result: &schema::CartLinesDiscountsGenerateRunResult) -> Vec<(f64, Vec<String>)> {
        let mut found = Vec::new();
        for operation in &result.operations {
            if let schema::CartOperation::OrderDiscountsAdd(add) = operation {
                for candidate in &add.candidates {
                    let percentage = match &candidate.value {
                        schema::OrderDiscountCandidateValue::Percentage(percentage) => percentage.value.0,
                        _ => continue,
                    };
                    let mut excluded = Vec::new();
                    for target in &candidate.targets {
                        if let schema::OrderDiscountCandidateTarget::OrderSubtotal(subtotal) = target {
                            excluded.extend(subtotal.excluded_cart_line_ids.iter().cloned());
                        }
                    }
                    found.push((percentage, excluded));
                }
            }
        }
        found
    }

    #[allow(irrefutable_let_patterns)]
    fn product_targets(result: &schema::CartLinesDiscountsGenerateRunResult) -> Vec<String> {
        let mut found = Vec::new();
        for operation in &result.operations {
            if let schema::CartOperation::ProductDiscountsAdd(add) = operation {
                for candidate in &add.candidates {
                    for target in &candidate.targets {
                        if let schema::ProductDiscountCandidateTarget::CartLine(line) = target {
                            found.push(line.id.clone());
                        }
                    }
                }
            }
        }
        found
    }

    #[test]
    #[cfg_attr(not(feature = "fixed"), ignore = "documents seeded bug 1")]
    fn a_cart_of_exactly_100_gets_the_10_percent_tier() {
        let lines = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2004", 1, "100.00", false, false)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, Some(CONFIG))).unwrap();
        assert_eq!(order_discounts(&result), vec![(10.0, vec![])]);
        assert!(product_targets(&result).is_empty());
    }

    #[test]
    #[cfg_attr(not(feature = "fixed"), ignore = "documents seeded bug 3")]
    fn a_missing_config_gives_no_discount() {
        let lines = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2004", 1, "120.00", false, false)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, None)).unwrap();
        assert!(result.operations.is_empty());
    }

    #[test]
    #[cfg_attr(not(feature = "fixed"), ignore = "documents seeded bug 4")]
    fn one_variant_on_two_lines_keeps_the_multi_buy_discount() {
        let lines = [
            variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2001", 1, "45.00", false, true),
            variant_line("gid://shopify/CartLine/2", "gid://shopify/ProductVariant/2001", 1, "45.00", false, true),
        ];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, Some(CONFIG))).unwrap();
        assert_eq!(
            product_targets(&result),
            vec!["gid://shopify/CartLine/1".to_string(), "gid://shopify/CartLine/2".to_string()]
        );
        assert!(order_discounts(&result).is_empty());
    }

    #[test]
    fn a_250_cart_gets_the_20_percent_tier() {
        let lines = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2007", 1, "250.00", false, false)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, Some(CONFIG))).unwrap();
        assert_eq!(order_discounts(&result), vec![(20.0, vec![])]);
    }

    #[test]
    fn gift_cards_are_excluded_from_the_order_discount() {
        let lines = [
            variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2013", 1, "100.00", true, false),
            variant_line("gid://shopify/CartLine/2", "gid://shopify/ProductVariant/2005", 6, "150.00", false, false),
        ];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, Some(CONFIG))).unwrap();
        assert_eq!(order_discounts(&result), vec![(10.0, vec!["gid://shopify/CartLine/1".to_string()])]);
        assert!(product_targets(&result).is_empty());
    }

    #[test]
    fn thresholds_follow_the_presentment_rate() {
        let over = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2007", 1, "22500.0", false, false)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("150.0", &over, Some(CONFIG))).unwrap();
        assert_eq!(order_discounts(&result), vec![(10.0, vec![])]);

        let under = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2007", 1, "14999.0", false, false)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("150.0", &under, Some(CONFIG))).unwrap();
        assert!(order_discounts(&result).is_empty());
    }

    #[test]
    fn a_multi_buy_line_of_two_gets_the_product_discount() {
        let lines = [variant_line("gid://shopify/CartLine/1", "gid://shopify/ProductVariant/2001", 2, "90.00", false, true)];
        let result = run_function_with_input(cart_lines_discounts_generate_run, &input("1.0", &lines, Some(CONFIG))).unwrap();
        assert_eq!(product_targets(&result), vec!["gid://shopify/CartLine/1".to_string()]);
    }
}
