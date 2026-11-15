//! Merchant rules from `[rules]`: max_discount_percent, never_discount,
//! tiers, min_subtotal and codes_accepted. They read the application
//! model's ledger, never the target's own arithmetic.

use super::{
    CODES_ACCEPTED, Check, MAX_DISCOUNT_PERCENT, MIN_SUBTOTAL, NEVER_DISCOUNT, OutcomeKind,
    RuleContext, TIERS,
};
use crate::apply::Ledger;
use crate::cart::{Cart, ConfigShape};
use crate::money;
use crate::settings::{DiscountKind, SubtotalBasis, Tier};
use rust_decimal::Decimal;

/// Subtotal of the lines the discount may touch.
pub fn eligible_subtotal(ctx: &RuleContext, cart: &Cart) -> i64 {
    let never = &ctx.resolved.rules.never_discount;
    cart.lines
        .iter()
        .filter(|l| never.reason(&l.merchandise).is_none())
        .map(|l| l.subtotal())
        .sum()
}

/// A store-currency amount in this cart's presentment currency.
pub fn to_presentment(ctx: &RuleContext, cart: &Cart, store_minor: i64) -> i64 {
    if cart.currency == ctx.resolved.store_currency {
        return store_minor;
    }
    money::convert(
        store_minor,
        ctx.resolved.store_exponent(),
        cart.rate(),
        cart.exponent(),
    )
}

fn pct(d: Decimal) -> String {
    format!("{}%", d.normalize())
}

pub fn check(ctx: &RuleContext, cart: &Cart, ledger: &Ledger, rules: &[&str]) -> Vec<Check> {
    let mut out = Vec::new();
    for rule in rules {
        let c = match *rule {
            TIERS => tiers(ctx, cart, ledger),
            NEVER_DISCOUNT => never_discount(ctx, cart, ledger),
            MAX_DISCOUNT_PERCENT => max_discount_percent(ctx, cart, ledger),
            MIN_SUBTOTAL => min_subtotal(ctx, cart, ledger),
            CODES_ACCEPTED => codes_accepted(ctx, cart, ledger),
            _ => continue,
        };
        out.push(c);
    }
    out
}

pub fn max_discount_percent(ctx: &RuleContext, cart: &Cart, ledger: &Ledger) -> Check {
    let Some(cap) = ctx.resolved.rules.max_percent() else {
        return Check::not_applicable(MAX_DISCOUNT_PERCENT, "no cap set");
    };
    let eligible = eligible_subtotal(ctx, cart);
    let total = ledger.merchandise_total();
    if eligible <= 0 {
        return Check::not_applicable(MAX_DISCOUNT_PERCENT, "no eligible lines");
    }
    let got = money::percent_ratio(total, eligible);
    // Each discount is rounded to the minor unit on its own, so allow one
    // minor unit per discount above the exact cap.
    let slack = (ledger.product_discounts.len() + ledger.order_discounts.len()) as i64;
    let allowed = money::percent_of(eligible, cap) + slack;
    if total > allowed {
        Check::fail(
            MAX_DISCOUNT_PERCENT,
            OutcomeKind::RuleBroken,
            "over_cap",
            format!(
                "This cart got {} off its eligible subtotal of {} ({}). Your cap is {}.",
                cart.money(total),
                cart.money(eligible),
                pct(got),
                pct(cap)
            ),
        )
        .with_detail("Discount", cart.money(total))
        .with_detail("Eligible subtotal", cart.money(eligible))
        .with_detail("Cap", pct(cap))
    } else {
        Check::pass(MAX_DISCOUNT_PERCENT)
    }
}

pub fn never_discount(ctx: &RuleContext, cart: &Cart, ledger: &Ledger) -> Check {
    let never = &ctx.resolved.rules.never_discount;
    let protected: Vec<(&str, String, String)> = cart
        .lines
        .iter()
        .filter_map(|l| {
            never
                .reason(&l.merchandise)
                .map(|r| (l.id.as_str(), l.merchandise.display_name(), r))
        })
        .collect();
    if protected.is_empty() {
        return Check::not_applicable(NEVER_DISCOUNT, "no gift card or tagged line");
    }
    for d in &ledger.product_discounts {
        if d.amount <= 0 {
            continue;
        }
        if let Some((_, name, reason)) = protected.iter().find(|(id, _, _)| *id == d.line_id) {
            return Check::fail(
                NEVER_DISCOUNT,
                OutcomeKind::RuleBroken,
                format!("product|{reason}"),
                format!(
                    "{name} {reason}, and the function discounted it by {}.",
                    cart.money(d.amount)
                ),
            )
            .with_detail("Line", d.line_id.clone());
        }
    }
    for o in &ledger.order_discounts {
        if o.amount <= 0 || !o.scope_known {
            continue;
        }
        if let Some((id, name, reason)) = protected
            .iter()
            .find(|(id, _, _)| !o.excluded_line_ids.iter().any(|e| e == id))
        {
            return Check::fail(
                NEVER_DISCOUNT,
                OutcomeKind::RuleBroken,
                format!("order|{reason}"),
                format!(
                    "{name} {reason}, and the order discount covers it: its line is not in excludedCartLineIds."
                ),
            )
            .with_detail("Line", id.to_string());
        }
    }
    Check::pass(NEVER_DISCOUNT)
}

/// The tier the eligible subtotal falls in, with thresholds converted into
/// the cart's currency.
fn expected_tier(tiers: &[(Tier, i64)], eligible: i64) -> Option<&(Tier, i64)> {
    tiers.iter().rev().find(|(_, t)| eligible >= *t)
}

pub fn tiers(ctx: &RuleContext, cart: &Cart, ledger: &Ledger) -> Check {
    let tiers = &ctx.resolved.rules.tiers;
    if tiers.is_empty() {
        return Check::not_applicable(TIERS, "no tiers in the config");
    }
    if cart.config_shape != ConfigShape::Normal {
        return Check::not_applicable(TIERS, "config missing, empty or odd");
    }
    let eligible = eligible_subtotal(ctx, cart);
    let converted: Vec<(Tier, i64)> = tiers
        .iter()
        .map(|t| (t.clone(), to_presentment(ctx, cart, t.threshold)))
        .collect();
    let foreign = cart.currency != ctx.resolved.store_currency;
    if foreign && converted.iter().any(|(_, t)| (eligible - t).abs() <= 1) {
        return Check::not_applicable(TIERS, "within one minor unit of a converted threshold");
    }
    let expected = expected_tier(&converted, eligible);
    let got = ledger.order_percent();
    let got_fixed = got.is_none() && ledger.order_total() > 0;
    let at_threshold = converted.iter().any(|(_, t)| *t == eligible);
    let key = if at_threshold {
        "at_threshold"
    } else {
        "between_thresholds"
    };

    let ok = match (expected, got) {
        (None, None) => !got_fixed,
        (Some((tier, _)), Some(g)) => tier.percent_decimal() == g,
        _ => false,
    };
    if ok {
        return Check::pass(TIERS);
    }
    let subtotal = cart.money(eligible);
    let mut check = match (expected, got) {
        (Some((tier, t)), None) if got_fixed => Check::fail(
            TIERS,
            OutcomeKind::RuleBroken,
            key,
            format!(
                "A cart of {subtotal} got a fixed {} off. Your config puts it in the {} tier, which starts at {}.",
                cart.money(ledger.order_total()),
                pct(tier.percent_decimal()),
                cart.money(*t)
            ),
        ),
        (Some((tier, t)), None) if *t == eligible => Check::fail(
            TIERS,
            OutcomeKind::RuleBroken,
            key,
            format!(
                "A cart of exactly {} got no discount. Your config starts the {} tier at {}.",
                cart.money(*t),
                pct(tier.percent_decimal()),
                cart.money(*t)
            ),
        ),
        (Some((tier, t)), None) => Check::fail(
            TIERS,
            OutcomeKind::RuleBroken,
            key,
            format!(
                "A cart of {subtotal} got no discount. Your config puts it in the {} tier, which starts at {}.",
                pct(tier.percent_decimal()),
                cart.money(*t)
            ),
        ),
        (Some((tier, t)), Some(g)) if *t == eligible => Check::fail(
            TIERS,
            OutcomeKind::RuleBroken,
            key,
            format!(
                "A cart of exactly {} got {} off. Your config starts the {} tier at {}.",
                cart.money(*t),
                pct(g),
                pct(tier.percent_decimal()),
                cart.money(*t)
            ),
        ),
        (Some((tier, t)), Some(g)) => Check::fail(
            TIERS,
            OutcomeKind::RuleBroken,
            key,
            format!(
                "A cart of {subtotal} got {} off. Your config puts it in the {} tier, which starts at {}.",
                pct(g),
                pct(tier.percent_decimal()),
                cart.money(*t)
            ),
        ),
        (None, _) => {
            let lowest = converted.first().map(|(_, t)| *t).unwrap_or(0);
            let what = match got {
                Some(g) => format!("{} off", pct(g)),
                None => format!("a fixed {} off", cart.money(ledger.order_total())),
            };
            Check::fail(
                TIERS,
                OutcomeKind::RuleBroken,
                "below_lowest",
                format!(
                    "A cart of {subtotal} got {what}, below your lowest tier at {}.",
                    cart.money(lowest)
                ),
            )
        }
    };
    check.expected_discount = Some(match expected {
        Some((tier, _)) => money::percent_of(eligible, tier.percent_decimal()),
        None => 0,
    });
    if let Some((tier, t)) = expected {
        check = check.with_detail(
            "Tier from config",
            format!("{} at {}", pct(tier.percent_decimal()), cart.money(*t)),
        );
    }
    check.with_detail("Eligible subtotal", subtotal)
}

pub fn min_subtotal(ctx: &RuleContext, cart: &Cart, ledger: &Ledger) -> Check {
    let Some(min) = &ctx.resolved.rules.min_subtotal else {
        return Check::not_applicable(MIN_SUBTOTAL, "no minimum set");
    };
    let eligible = eligible_subtotal(ctx, cart);
    let basis_amount = match min.basis {
        SubtotalBasis::BeforeDiscounts => eligible,
        SubtotalBasis::AfterDiscounts => eligible - ledger.merchandise_total(),
    };
    let min_amount = to_presentment(ctx, cart, min.amount);
    let mut amount = 0;
    let mut kinds: Vec<&str> = Vec::new();
    for k in &min.kinds {
        let a = match k {
            DiscountKind::Order => ledger.order_total(),
            DiscountKind::Product => ledger.product_total(),
            DiscountKind::Shipping => ledger.shipping_total(),
        };
        if a > 0 {
            kinds.push(k.as_str());
        }
        amount += a;
    }
    if basis_amount >= min_amount || amount <= 0 {
        return Check::pass(MIN_SUBTOTAL);
    }
    let after = min.basis == SubtotalBasis::AfterDiscounts;
    let kind_text = kinds.join(" and ");
    let sentence = if after {
        format!(
            "A cart of {} after discounts got a {kind_text} discount of {}. Your minimum is {} after discounts.",
            cart.money(basis_amount),
            cart.money(amount),
            cart.money(min_amount)
        )
    } else {
        format!(
            "A cart of {} got a {kind_text} discount of {}. Your minimum is {}.",
            cart.money(basis_amount),
            cart.money(amount),
            cart.money(min_amount)
        )
    };
    let mut c = Check::fail(
        MIN_SUBTOTAL,
        OutcomeKind::RuleBroken,
        format!("{}|{kind_text}", if after { "after" } else { "before" }),
        sentence,
    )
    .with_detail("Minimum", cart.money(min_amount));
    c.expected_discount = Some(0);
    c
}

pub fn codes_accepted(ctx: &RuleContext, cart: &Cart, ledger: &Ledger) -> Check {
    if cart.codes.is_empty() && ledger.codes_accepted.is_empty() {
        return Check::not_applicable(CODES_ACCEPTED, "no codes entered");
    }
    let allowed = &ctx.resolved.codes;
    for code in &ledger.codes_accepted {
        let configured = allowed.iter().any(|a| a.eq_ignore_ascii_case(code));
        let entered = cart.codes.iter().any(|a| a.eq_ignore_ascii_case(code));
        if !configured {
            return Check::fail(
                CODES_ACCEPTED,
                OutcomeKind::RuleBroken,
                "not_configured",
                format!("The function accepted the code {code}, which is not one of your configured codes."),
            )
            .with_detail("Configured codes", if allowed.is_empty() { "none".to_string() } else { allowed.join(", ") });
        }
        if !entered {
            return Check::fail(
                CODES_ACCEPTED,
                OutcomeKind::RuleBroken,
                "not_entered",
                format!(
                    "The function accepted the code {code}, which was not entered on this cart."
                ),
            );
        }
    }
    Check::pass(CODES_ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::apply_shopify_output;
    use crate::cart::test_support::*;
    use crate::rules::Status;
    use crate::rules::test_support::resolved;
    use serde_json::json;

    fn tier_config() -> serde_json::Value {
        json!({ "tiers": [
            { "threshold": "100.00", "percentage": "10" },
            { "threshold": "200.00", "percentage": "20" }
        ] })
    }

    fn order_pct(p: &str) -> serde_json::Value {
        json!({ "operations": [ { "orderDiscountsAdd": {
            "selectionStrategy": "FIRST",
            "candidates": [ {
                "targets": [ { "orderSubtotal": { "excludedCartLineIds": [] } } ],
                "value": { "percentage": { "value": p } } } ] } } ] })
    }

    #[test]
    fn exact_threshold_with_no_discount_fails_with_the_spec_sentence() {
        let r = resolved(
            "tiers = { at = \"/tiers\", threshold = \"threshold\", percent = \"percentage\" }",
            Some(tier_config()),
        );
        let ctx = RuleContext::new(&r);
        let c = cart(vec![line(
            1,
            variant(1, "Canvas Tote", &[], false),
            1,
            10000,
        )]);
        let l = apply_shopify_output(&c, &json!({ "operations": [] }));
        let check = tiers(&ctx, &c, &l);
        assert_eq!(check.status, Status::Fail);
        assert_eq!(
            check.sentence,
            "A cart of exactly $100.00 got no discount. Your config starts the 10% tier at $100.00."
        );
        assert_eq!(check.expected_discount, Some(1000));
        let ok = apply_shopify_output(&c, &order_pct("10.0"));
        assert_eq!(tiers(&ctx, &c, &ok).status, Status::Pass);
    }

    #[test]
    fn wrong_tier_at_200() {
        let r = resolved(
            "tiers = { at = \"/tiers\", threshold = \"threshold\", percent = \"percentage\" }",
            Some(tier_config()),
        );
        let ctx = RuleContext::new(&r);
        let c = cart(vec![line(1, variant(1, "Tote", &[], false), 2, 10000)]);
        let l = apply_shopify_output(&c, &order_pct("10"));
        let check = tiers(&ctx, &c, &l);
        assert_eq!(check.status, Status::Fail);
        assert_eq!(check.key, "at_threshold");
    }

    #[test]
    fn gift_cards_must_not_be_discounted() {
        let r = resolved("never_discount = { gift_cards = true }", None);
        let ctx = RuleContext::new(&r);
        let c = cart(vec![
            line(1, variant(1, "Tote", &[], false), 1, 5000),
            line(2, variant(2, "Gift Card", &[], true), 1, 2500),
        ]);
        let out = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "ALL",
            "candidates": [ {
                "targets": [ { "cartLine": { "id": "gid://shopify/CartLine/2" } } ],
                "value": { "percentage": { "value": 10 } } } ] } } ] });
        let l = apply_shopify_output(&c, &out);
        assert_eq!(never_discount(&ctx, &c, &l).status, Status::Fail);
        let l = apply_shopify_output(&c, &order_pct("10"));
        assert_eq!(never_discount(&ctx, &c, &l).status, Status::Fail);
    }

    #[test]
    fn cap_and_minimum() {
        let r = resolved(
            "max_discount_percent = \"30\"\nmin_subtotal = \"50.00\"",
            None,
        );
        let ctx = RuleContext::new(&r);
        let c = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 4000)]);
        let l = apply_shopify_output(&c, &order_pct("35"));
        assert_eq!(max_discount_percent(&ctx, &c, &l).status, Status::Fail);
        assert_eq!(min_subtotal(&ctx, &c, &l).status, Status::Fail);
        let l = apply_shopify_output(&c, &json!({ "operations": [] }));
        assert_eq!(min_subtotal(&ctx, &c, &l).status, Status::Pass);
    }

    #[test]
    fn unknown_codes_must_not_be_accepted() {
        let r = resolved("", None);
        let ctx = RuleContext::new(&r);
        let mut c = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 4000)]);
        c.codes = vec!["NOPE".into()];
        let out = json!({ "operations": [
            { "enteredDiscountCodesAccept": { "codes": [ { "code": "NOPE" } ] } } ] });
        let l = apply_shopify_output(&c, &out);
        assert_eq!(codes_accepted(&ctx, &c, &l).status, Status::Fail);
    }
}
