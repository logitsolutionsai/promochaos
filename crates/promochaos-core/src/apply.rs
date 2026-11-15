//! The single-function application model (spec section 8).
//!
//! This is promochaos's model of what ONE function's candidates add up to,
//! not Shopify's checkout. It ignores other discounts, taxes and shipping
//! rates. The rules are listed in docs/rules.md:
//! - Product candidates: `ALL` applies each one, `FIRST` the first and
//!   `MAXIMUM` the one worth the most.
//! - Order candidates: those whose conditions hold, then `FIRST` or
//!   `MAXIMUM`, on the subtotal minus `excludedCartLineIds` and minus product
//!   discounts already taken on the remaining lines.
//! - Fixed amounts with `appliesToEachItem` are multiplied by quantity. A
//!   fixed amount without it is split across its target lines by subtotal.
//!   No line goes below zero.
//! - Percentages round half away from zero to the minor unit.

use crate::cart::Cart;
use crate::money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const LEDGER_LABEL: &str = "promochaos model of this function's output";
/// The ledger label for a command target, which prices the cart itself:
/// the ledger is promochaos's reading of the engine's priced cart.
pub const LEDGER_LABEL_COMMAND: &str = "promochaos reading of the engine's priced cart";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct LineDiscount {
    pub line_id: String,
    pub amount: i64,
    /// Percentage as a decimal string, when the candidate was a percentage.
    pub percent: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct OrderDiscount {
    pub amount: i64,
    pub percent: Option<String>,
    /// The amount the order discount was taken on.
    pub base: i64,
    pub excluded_line_ids: Vec<String>,
    pub message: Option<String>,
    /// False when the target did not say which lines the order discount
    /// covers (a command engine that reports no line ids). `never_discount`
    /// then cannot check it.
    pub scope_known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ShippingDiscount {
    /// Delivery group id or delivery option handle.
    pub target: String,
    pub amount: i64,
    pub percent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct LedgerRow {
    pub label: String,
    pub value: String,
    /// `normal`, `total` (rule above) or `expected` (failure colour).
    pub style: String,
}

/// The working for one cart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Ledger {
    pub subtotal: i64,
    pub product_discounts: Vec<LineDiscount>,
    pub order_discounts: Vec<OrderDiscount>,
    pub shipping_discounts: Vec<ShippingDiscount>,
    pub codes_accepted: Vec<String>,
    pub codes_rejected: Vec<String>,
    /// Number of candidates the output listed, for the "none" row.
    pub candidates: usize,
    pub rows: Vec<LedgerRow>,
}

impl Ledger {
    pub fn product_total(&self) -> i64 {
        self.product_discounts.iter().map(|d| d.amount).sum()
    }

    pub fn order_total(&self) -> i64 {
        self.order_discounts.iter().map(|d| d.amount).sum()
    }

    pub fn shipping_total(&self) -> i64 {
        self.shipping_discounts.iter().map(|d| d.amount).sum()
    }

    /// Order and product discounts. Shipping is kept apart: it is not part
    /// of the eligible subtotal the percent cap is measured on.
    pub fn merchandise_total(&self) -> i64 {
        self.product_total() + self.order_total()
    }

    /// The highest order-discount percentage, if any order discount is a
    /// percentage.
    pub fn order_percent(&self) -> Option<Decimal> {
        self.order_discounts
            .iter()
            .filter_map(|d| d.percent.as_deref())
            .filter_map(|p| money::parse_decimal(p).ok())
            .max()
    }

    pub fn discounted_line_ids(&self) -> Vec<String> {
        self.product_discounts
            .iter()
            .filter(|d| d.amount > 0)
            .map(|d| d.line_id.clone())
            .collect()
    }

    pub fn push_row(&mut self, label: impl Into<String>, value: impl Into<String>, style: &str) {
        self.rows.push(LedgerRow {
            label: label.into(),
            value: value.into(),
            style: style.to_string(),
        });
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// A candidate's value: percentage or fixed amount.
#[derive(Debug, Clone, PartialEq)]
enum CandidateValue {
    Percent(Decimal),
    Fixed { amount: i64, each_item: bool },
}

fn read_value(v: &Value, exp: u32) -> Option<CandidateValue> {
    let value = v.get("value")?;
    if let Some(p) = value.get("percentage") {
        let d = p.get("value").and_then(money::decimal_from_json)?;
        return Some(CandidateValue::Percent(d));
    }
    if let Some(f) = value.get("fixedAmount") {
        let d = f.get("amount").and_then(money::decimal_from_json)?;
        let amount = money::decimal_to_minor_rounded(d, exp)?;
        let each_item = f
            .get("appliesToEachItem")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return Some(CandidateValue::Fixed { amount, each_item });
    }
    None
}

fn percent_string(v: &CandidateValue) -> Option<String> {
    match v {
        CandidateValue::Percent(d) => Some(d.normalize().to_string()),
        CandidateValue::Fixed { .. } => None,
    }
}

/// What one product candidate would take from each line, given what is left
/// on each line.
fn product_candidate_amounts(
    cart: &Cart,
    candidate: &Value,
    remaining: &BTreeMap<String, i64>,
    exp: u32,
) -> Vec<(String, i64)> {
    let Some(value) = read_value(candidate, exp) else {
        return Vec::new();
    };
    let mut targets: Vec<(String, i64, i64)> = Vec::new(); // id, base, qty
    for t in arr(candidate, "targets") {
        let Some(cl) = t.get("cartLine") else {
            continue;
        };
        let Some(id) = str_field(cl, "id") else {
            continue;
        };
        let Some(line) = cart.line(id) else { continue };
        let qty = cl
            .get("quantity")
            .and_then(Value::as_i64)
            .map(|q| q.clamp(0, line.quantity))
            .unwrap_or(line.quantity);
        targets.push((id.to_string(), line.unit_price.saturating_mul(qty), qty));
    }
    let raw: Vec<i64> = match &value {
        CandidateValue::Percent(p) => targets
            .iter()
            .map(|(_, base, _)| money::percent_of(*base, *p))
            .collect(),
        CandidateValue::Fixed {
            amount,
            each_item: true,
        } => targets
            .iter()
            .map(|(_, _, qty)| amount.saturating_mul(*qty))
            .collect(),
        CandidateValue::Fixed {
            amount,
            each_item: false,
        } => {
            let weights: Vec<i64> = targets.iter().map(|(_, b, _)| *b).collect();
            money::allocate(*amount, &weights)
        }
    };
    targets
        .iter()
        .zip(raw)
        .map(|((id, _, _), amt)| {
            let left = remaining.get(id).copied().unwrap_or(0);
            (id.clone(), amt.clamp(0, left.max(0)))
        })
        .collect()
}

fn conditions_hold(cart: &Cart, candidate: &Value, exp: u32) -> bool {
    for c in arr(candidate, "conditions") {
        if let Some(m) = c.get("orderMinimumSubtotal") {
            let excluded: Vec<&str> = arr(m, "excludedCartLineIds")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let base: i64 = cart
                .lines
                .iter()
                .filter(|l| !excluded.contains(&l.id.as_str()))
                .map(|l| l.subtotal())
                .sum();
            let min = m
                .get("minimumAmount")
                .and_then(money::decimal_from_json)
                .and_then(|d| money::decimal_to_minor_rounded(d, exp))
                .unwrap_or(0);
            if base < min {
                return false;
            }
        }
        if let Some(m) = c.get("cartLineMinimumSubtotal") {
            let ids: Vec<&str> = arr(m, "ids").iter().filter_map(Value::as_str).collect();
            let sum: i64 = cart
                .lines
                .iter()
                .filter(|l| ids.contains(&l.id.as_str()))
                .map(|l| l.subtotal())
                .sum();
            let min = m
                .get("minimumAmount")
                .and_then(money::decimal_from_json)
                .and_then(|d| money::decimal_to_minor_rounded(d, exp))
                .unwrap_or(0);
            if sum < min {
                return false;
            }
        }
        if let Some(m) = c.get("cartLineMinimumQuantity") {
            let ids: Vec<&str> = arr(m, "ids").iter().filter_map(Value::as_str).collect();
            let qty: i64 = cart
                .lines
                .iter()
                .filter(|l| ids.contains(&l.id.as_str()))
                .map(|l| l.quantity)
                .sum();
            let min = m
                .get("minimumQuantity")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if qty < min {
                return false;
            }
        }
    }
    true
}

/// Works out a Shopify discount Function's output for this cart. Handles
/// both run targets: cart lines (`orderDiscountsAdd`, `productDiscountsAdd`,
/// codes) and delivery (`deliveryDiscountsAdd`).
pub fn apply_shopify_output(cart: &Cart, output: &Value) -> Ledger {
    let exp = cart.exponent();
    let mut ledger = Ledger {
        subtotal: cart.subtotal(),
        ..Ledger::default()
    };
    let mut remaining: BTreeMap<String, i64> = cart
        .lines
        .iter()
        .map(|l| (l.id.clone(), l.subtotal()))
        .collect();
    let ops = arr(output, "operations");

    // Product discounts first, then order discounts on what is left.
    for op in ops {
        let Some(add) = op.get("productDiscountsAdd") else {
            continue;
        };
        let candidates = arr(add, "candidates");
        ledger.candidates += candidates.len();
        let strategy = str_field(add, "selectionStrategy").unwrap_or("FIRST");
        let chosen: Vec<&Value> = match strategy {
            "ALL" => candidates.iter().collect(),
            "MAXIMUM" => candidates
                .iter()
                .max_by_key(|c| {
                    product_candidate_amounts(cart, c, &remaining, exp)
                        .iter()
                        .map(|(_, a)| *a)
                        .sum::<i64>()
                })
                .into_iter()
                .collect(),
            _ => candidates.first().into_iter().collect(),
        };
        for c in chosen {
            let value = read_value(c, exp);
            let message = str_field(c, "message").map(str::to_string);
            for (id, amt) in product_candidate_amounts(cart, c, &remaining, exp) {
                if let Some(left) = remaining.get_mut(&id) {
                    *left -= amt;
                }
                ledger.product_discounts.push(LineDiscount {
                    line_id: id,
                    amount: amt,
                    percent: value.as_ref().and_then(percent_string),
                    message: message.clone(),
                });
            }
        }
    }

    for op in ops {
        let Some(add) = op.get("orderDiscountsAdd") else {
            continue;
        };
        let candidates = arr(add, "candidates");
        ledger.candidates += candidates.len();
        let eligible: Vec<&Value> = candidates
            .iter()
            .filter(|c| conditions_hold(cart, c, exp))
            .collect();
        let worth = |c: &Value| -> (i64, i64, Vec<String>, Option<CandidateValue>) {
            let mut excluded: Vec<String> = Vec::new();
            for t in arr(c, "targets") {
                if let Some(os) = t.get("orderSubtotal") {
                    for id in arr(os, "excludedCartLineIds")
                        .iter()
                        .filter_map(Value::as_str)
                    {
                        excluded.push(id.to_string());
                    }
                }
            }
            let base: i64 = remaining
                .iter()
                .filter(|(id, _)| !excluded.contains(id))
                .map(|(_, left)| (*left).max(0))
                .sum();
            let value = read_value(c, exp);
            let amount = match &value {
                Some(CandidateValue::Percent(p)) => money::percent_of(base, *p),
                Some(CandidateValue::Fixed { amount, .. }) => *amount,
                None => 0,
            };
            (amount.clamp(0, base.max(0)), base, excluded, value)
        };
        let strategy = str_field(add, "selectionStrategy").unwrap_or("FIRST");
        let chosen: Option<&Value> = match strategy {
            "MAXIMUM" => eligible.iter().copied().max_by_key(|c| worth(c).0),
            _ => eligible.first().copied(),
        };
        if let Some(c) = chosen {
            let (amount, base, excluded, value) = worth(c);
            // Spread the order discount over the remaining lines so a second
            // order operation cannot take the same money twice.
            let ids: Vec<String> = remaining
                .keys()
                .filter(|id| !excluded.contains(id))
                .cloned()
                .collect();
            let weights: Vec<i64> = ids
                .iter()
                .map(|id| remaining.get(id).copied().unwrap_or(0).max(0))
                .collect();
            for (id, part) in ids.iter().zip(money::allocate(amount, &weights)) {
                if let Some(left) = remaining.get_mut(id) {
                    *left -= part;
                }
            }
            ledger.order_discounts.push(OrderDiscount {
                amount,
                percent: value.as_ref().and_then(percent_string),
                base,
                excluded_line_ids: excluded,
                message: str_field(c, "message").map(str::to_string),
                scope_known: true,
            });
        }
    }

    for op in ops {
        if let Some(acc) = op.get("enteredDiscountCodesAccept") {
            for c in arr(acc, "codes") {
                if let Some(code) = str_field(c, "code") {
                    ledger.codes_accepted.push(code.to_string());
                }
            }
        }
        if let Some(rej) = op.get("enteredDiscountCodesReject") {
            for c in arr(rej, "codes") {
                if let Some(code) = str_field(c, "code") {
                    ledger.codes_rejected.push(code.to_string());
                }
            }
        }
        if let Some(add) = op.get("deliveryDiscountsAdd") {
            let candidates = arr(add, "candidates");
            ledger.candidates += candidates.len();
            for c in candidates {
                let value = read_value(c, exp);
                for t in arr(c, "targets") {
                    let (target, price) = if let Some(g) = t.get("deliveryGroup") {
                        let id = str_field(g, "id").unwrap_or("");
                        let price = cart
                            .delivery
                            .iter()
                            .find(|d| d.id == id)
                            .and_then(|d| d.selected_option())
                            .map(|o| o.price)
                            .unwrap_or(0);
                        (id.to_string(), price)
                    } else if let Some(o) = t.get("deliveryOption") {
                        let handle = str_field(o, "handle").unwrap_or("");
                        let price = cart
                            .delivery
                            .iter()
                            .flat_map(|d| d.options.iter())
                            .find(|op| op.handle == handle)
                            .map(|op| op.price)
                            .unwrap_or(0);
                        (handle.to_string(), price)
                    } else {
                        continue;
                    };
                    let amount = match &value {
                        Some(CandidateValue::Percent(p)) => money::percent_of(price, *p),
                        Some(CandidateValue::Fixed { amount, .. }) => *amount,
                        None => 0,
                    };
                    ledger.shipping_discounts.push(ShippingDiscount {
                        target,
                        amount: amount.clamp(0, price.max(0)),
                        percent: value.as_ref().and_then(percent_string),
                    });
                }
            }
        }
    }

    build_rows(cart, &mut ledger);
    ledger
}

/// Receipt rows for the report.
pub fn build_rows(cart: &Cart, ledger: &mut Ledger) {
    ledger.rows.clear();
    let subtotal = ledger.subtotal;
    ledger.push_row("Subtotal", cart.money(subtotal), "normal");
    let product = ledger.product_total();
    if product > 0 {
        let lines = ledger.discounted_line_ids().len();
        let noun = if lines == 1 { "line" } else { "lines" };
        ledger.push_row(
            format!("Product discounts, {lines} {noun}"),
            cart.money(-product),
            "normal",
        );
    }
    let orders: Vec<(Option<String>, i64, i64, Option<String>)> = ledger
        .order_discounts
        .iter()
        .map(|o| (o.percent.clone(), o.amount, o.base, o.message.clone()))
        .collect();
    for (percent, amount, base, message) in orders {
        // Without a percentage, the target's own message tells stacked
        // discounts apart: two "Order discount on $200.00" rows read as one
        // discount listed twice.
        let message = message
            .map(|m| m.trim().chars().take(60).collect::<String>())
            .filter(|m| !m.is_empty());
        let label = match (percent, message) {
            (Some(p), _) => format!("Order discount, {p}% of {}", cart.money(base)),
            (None, Some(m)) => format!("Order discount on {}, {m}", cart.money(base)),
            (None, None) => format!("Order discount on {}", cart.money(base)),
        };
        ledger.push_row(label, cart.money(-amount), "normal");
    }
    let shipping = ledger.shipping_total();
    if shipping > 0 {
        ledger.push_row("Shipping discount", cart.money(-shipping), "normal");
    }
    if ledger.candidates == 0 {
        ledger.push_row("Candidates returned", "none", "normal");
    }
    ledger.push_row(
        "Discount applied",
        cart.money(ledger.merchandise_total() + shipping),
        "total",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;
    use serde_json::json;

    fn two_line_cart() -> Cart {
        cart(vec![
            line(1, variant(1, "Linen Shirt", &["multi-buy"], false), 2, 4500),
            line(2, variant(2, "Gift Card", &["gift-card"], true), 1, 2500),
        ])
    }

    #[test]
    fn order_percentage_on_subtotal_minus_excluded() {
        let out = json!({ "operations": [ { "orderDiscountsAdd": {
            "selectionStrategy": "FIRST",
            "candidates": [ {
                "targets": [ { "orderSubtotal": { "excludedCartLineIds": ["gid://shopify/CartLine/2"] } } ],
                "value": { "percentage": { "value": "10.0" } }
            } ]
        } } ] });
        let l = apply_shopify_output(&two_line_cart(), &out);
        assert_eq!(l.order_total(), 900);
        assert_eq!(l.order_discounts[0].base, 9000);
        assert_eq!(l.order_percent(), Some(Decimal::from(10)));
    }

    #[test]
    fn product_strategies() {
        let c = two_line_cart();
        let cand = |p: &str, id: u32| {
            json!({
                "targets": [ { "cartLine": { "id": format!("gid://shopify/CartLine/{id}") } } ],
                "value": { "percentage": { "value": p } }
            })
        };
        let all = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "ALL", "candidates": [ cand("10", 1), cand("50", 2) ] } } ] });
        assert_eq!(apply_shopify_output(&c, &all).product_total(), 900 + 1250);
        let first = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "FIRST", "candidates": [ cand("10", 1), cand("50", 2) ] } } ] });
        assert_eq!(apply_shopify_output(&c, &first).product_total(), 900);
        let max = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "MAXIMUM", "candidates": [ cand("10", 1), cand("50", 2) ] } } ] });
        assert_eq!(apply_shopify_output(&c, &max).product_total(), 1250);
    }

    #[test]
    fn fixed_amounts_never_take_a_line_below_zero() {
        let c = two_line_cart();
        let out = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "ALL",
            "candidates": [ {
                "targets": [ { "cartLine": { "id": "gid://shopify/CartLine/1" } } ],
                "value": { "fixedAmount": { "amount": "30.00", "appliesToEachItem": true } }
            } ] } } ] });
        // 30.00 x 2 = 60.00, but the line is only 90.00: 60.00 applies.
        assert_eq!(apply_shopify_output(&c, &out).product_total(), 6000);
        let out = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "ALL",
            "candidates": [ {
                "targets": [ { "cartLine": { "id": "gid://shopify/CartLine/1" } } ],
                "value": { "fixedAmount": { "amount": 100, "appliesToEachItem": true } }
            } ] } } ] });
        assert_eq!(apply_shopify_output(&c, &out).product_total(), 9000);
    }

    #[test]
    fn order_conditions_filter_candidates() {
        let c = two_line_cart();
        let out = json!({ "operations": [ { "orderDiscountsAdd": {
            "selectionStrategy": "MAXIMUM",
            "candidates": [
                { "targets": [ { "orderSubtotal": { "excludedCartLineIds": [] } } ],
                  "value": { "percentage": { "value": 50 } },
                  "conditions": [ { "orderMinimumSubtotal": { "minimumAmount": "500.00", "excludedCartLineIds": [] } } ] },
                { "targets": [ { "orderSubtotal": { "excludedCartLineIds": [] } } ],
                  "value": { "percentage": { "value": 10 } } }
            ] } } ] });
        assert_eq!(apply_shopify_output(&c, &out).order_total(), 1150);
    }

    #[test]
    fn codes_and_empty_output() {
        let c = two_line_cart();
        let out = json!({ "operations": [
            { "enteredDiscountCodesAccept": { "codes": [ { "code": "WELCOME20" } ] } }
        ] });
        let l = apply_shopify_output(&c, &out);
        assert_eq!(l.codes_accepted, vec!["WELCOME20".to_string()]);
        let l = apply_shopify_output(&c, &json!({ "operations": [] }));
        assert_eq!(l.merchandise_total(), 0);
        assert!(
            l.rows
                .iter()
                .any(|r| r.label == "Candidates returned" && r.value == "none")
        );
    }
}
