//! Boundary carts: for every declared threshold, carts at the threshold
//! minus one minor unit, exactly at it, and plus one, built from real
//! catalog prices with the remainder carried by one line. Also quantities at
//! a configured minimum quantity minus 1, equal and plus 1.

use super::{GenContext, GenKind, GeneratedCart, Origin};
use crate::cart::Cart;
use crate::catalog::Item;
use serde_json::Value;

/// Lines that add up to exactly `target` store minor units, using real
/// prices: as many of the dearest item that fits, then one line carrying the
/// remainder.
pub fn lines_for_subtotal(items: &[Item], target: i64) -> Vec<(Item, i64, i64)> {
    if target <= 0 || items.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&Item> = items.iter().filter(|i| i.price > 0).collect();
    sorted.sort_by_key(|i| std::cmp::Reverse(i.price));
    let Some(main) = sorted.iter().find(|i| i.price <= target).copied() else {
        // Every item costs more than the target: one line at the target.
        return items
            .first()
            .map(|i| vec![(i.clone(), 1, target)])
            .unwrap_or_default();
    };
    let qty = (target / main.price).min(50);
    let remainder = target - qty * main.price;
    let mut lines = vec![(main.clone(), qty, main.price)];
    if remainder > 0 {
        let carrier = sorted
            .iter()
            .find(|i| i.merchandise != main.merchandise)
            .copied()
            .unwrap_or(main);
        lines.push((carrier.clone(), 1, remainder));
    }
    lines
}

/// Integer "minimum quantity" values in the config, found by key name.
pub fn quantity_thresholds(config: &Value) -> Vec<i64> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<i64>) {
        match v {
            Value::Object(m) => {
                for (k, child) in m {
                    let key = k.to_ascii_lowercase();
                    if key.contains("quantity")
                        && let Some(n) = child.as_i64()
                        && (1..=1000).contains(&n)
                    {
                        out.push(n);
                    }
                    walk(child, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    walk(config, &mut out);
    out.sort_unstable();
    out.dedup();
    out
}

fn build(ctx: &mut GenContext, lines: &[(Item, i64, i64)], label: String) -> GeneratedCart {
    let store = ctx.resolved.store_currency.clone();
    let mut cart: Cart = ctx.empty_cart(&store);
    for (item, qty, price) in lines {
        ctx.push_line(&mut cart, item, *qty, Some(*price));
    }
    ctx.add_delivery(&mut cart);
    GeneratedCart {
        cart,
        origin: Origin {
            kind: GenKind::Boundary,
            label,
        },
    }
}

pub fn generate(ctx: &mut GenContext) -> Vec<GeneratedCart> {
    let mut out = Vec::new();
    let items: Vec<Item> = ctx.ordinary_items().to_vec();
    let store = ctx.resolved.store_currency.clone();
    for threshold in ctx.resolved.thresholds() {
        for (delta, name) in [(-1_i64, "minus one"), (0, "exact"), (1, "plus one")] {
            let target = threshold + delta;
            let lines = lines_for_subtotal(&items, target);
            if lines.is_empty() {
                continue;
            }
            let label = format!(
                "threshold {}, {name}",
                crate::money::format_display(threshold, &store)
            );
            out.push(build(ctx, &lines, label));
        }
    }
    let qty_thresholds = ctx
        .resolved
        .config
        .as_ref()
        .map(quantity_thresholds)
        .unwrap_or_default();
    // Tagged items are the likeliest to have quantity rules.
    let mut tagged: Vec<Item> = items
        .iter()
        .filter(|i| !i.merchandise.tags().is_empty())
        .cloned()
        .collect();
    if tagged.is_empty() {
        tagged = items.clone();
    }
    tagged.truncate(3);
    for q in qty_thresholds {
        for item in &tagged {
            for qty in [q - 1, q, q + 1] {
                if qty < 1 {
                    continue;
                }
                let price = item.price;
                let label = format!(
                    "quantity {qty} of {}, minimum {q}",
                    item.merchandise.title()
                );
                out.push(build(ctx, &[(item.clone(), qty, price)], label));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    #[test]
    fn builds_exact_subtotals_from_real_prices() {
        let items = Catalog::parse(Catalog::starter_toml(), "s")
            .map(|c| c.items())
            .unwrap_or_default();
        let ordinary: Vec<Item> = items.into_iter().filter(|i| !i.is_gift_card()).collect();
        for target in [9999_i64, 10000, 10001, 19999, 20000, 20001, 5000] {
            let lines = lines_for_subtotal(&ordinary, target);
            let sum: i64 = lines.iter().map(|(_, q, p)| q * p).sum();
            assert_eq!(sum, target, "target {target}");
            assert!(lines.len() <= 2);
        }
    }

    #[test]
    fn finds_minimum_quantities_in_the_config() {
        let cfg = serde_json::json!({ "multiBuy": { "minQuantity": 2, "percentage": "10" }, "tiers": [] });
        assert_eq!(quantity_thresholds(&cfg), vec![2]);
    }
}
