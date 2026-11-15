//! Command-target rules: totals_reconcile, idempotent_retry, usage_limit.

use super::{Check, IDEMPOTENT_RETRY, OutcomeKind, RuleContext, TOTALS_RECONCILE, USAGE_LIMIT};
use crate::cart::Cart;
use crate::eval::Evaluation;
use crate::settings::ShippingIn;

/// Subtotal minus discounts, plus shipping, plus tax (unless tax is already
/// inside the prices), equals the total. A shipping discount counts once,
/// where `shipping_discount_in` says.
pub fn totals_reconcile(ctx: &RuleContext, cart: &Cart, eval: &Evaluation) -> Check {
    let Some(t) = &eval.totals else {
        return Check::not_applicable(TOTALS_RECONCILE, "no totals in the response");
    };
    if ctx.shipping_in == ShippingIn::Shipping && t.shipping_discounts > 0 {
        return Check::fail(
            TOTALS_RECONCILE,
            OutcomeKind::RuleBroken,
            "shipping_counted_twice",
            format!(
                "The engine reported a shipping discount of {} in discounts, but shipping_discount_in = \"shipping\" says the shipping field is already net, so it would count twice.",
                cart.money(t.shipping_discounts)
            ),
        );
    }
    let shipping_charged = match ctx.shipping_in {
        ShippingIn::Discounts => t.shipping_reported - t.shipping_discounts,
        ShippingIn::Shipping => t.shipping_reported,
    };
    let tax = if ctx.tax_inclusive { 0 } else { t.tax };
    let expected = t.subtotal - t.discounts_order_product + shipping_charged + tax;
    if expected == t.total {
        return Check::pass(TOTALS_RECONCILE);
    }
    let tax_part = if ctx.tax_inclusive {
        "tax already in the prices".to_string()
    } else {
        format!("plus tax {}", cart.money(t.tax))
    };
    Check::fail(
        TOTALS_RECONCILE,
        OutcomeKind::RuleBroken,
        "mismatch",
        format!(
            "The engine's total is {}, but subtotal {} minus discounts {} plus shipping {} {tax_part} is {}.",
            cart.money(t.total),
            cart.money(t.subtotal),
            cart.money(t.discounts_order_product),
            cart.money(shipping_charged),
            cart.money(expected)
        ),
    )
}

/// A `place_order` retried with the same idempotency key returns the first
/// result and does not use the coupon twice.
pub fn idempotent_retry(cart: &Cart, first: &Evaluation, second: &Evaluation) -> Check {
    if first.crash.is_some() || second.crash.is_some() {
        return Check::not_applicable(IDEMPOTENT_RETRY, "trapped first");
    }
    let (Some(a), Some(b)) = (&first.order, &second.order) else {
        return Check::not_applicable(IDEMPOTENT_RETRY, "the engine returned no order");
    };
    if a == b && first.output_text == second.output_text {
        return Check::pass(IDEMPOTENT_RETRY);
    }
    let code = cart.codes.first().cloned().unwrap_or_default();
    let uses_a = a.coupon_uses.get(&code).copied().unwrap_or(0);
    let uses_b = b.coupon_uses.get(&code).copied().unwrap_or(0);
    let reused = !code.is_empty() && uses_b > uses_a;
    let second_order = a.id != b.id;
    let sentence = match (reused, second_order) {
        (true, true) => format!(
            "Retrying an order with the same idempotency key created a second order ({} after {}) and used the coupon {code} again: {uses_b} uses after the retry, {uses_a} expected.",
            b.id, a.id
        ),
        (true, false) => format!(
            "Retrying an order with the same idempotency key used the coupon {code} again: {uses_b} uses after the retry, {uses_a} expected."
        ),
        (false, true) => format!(
            "Retrying an order with the same idempotency key created a second order ({} after {}).",
            b.id, a.id
        ),
        (false, false) => {
            "Retrying an order with the same idempotency key returned a different result."
                .to_string()
        }
    };
    // One key for the rule: a retry that is not idempotent is one defect,
    // whether or not the cart carried a coupon. Separate keys reported the
    // homegrown example's single retry bug as two findings.
    let key = "retry_not_idempotent";
    Check::fail(IDEMPOTENT_RETRY, OutcomeKind::RuleBroken, key, sentence)
        .with_detail("First order", a.id.clone())
        .with_detail("Retry", b.id.clone())
}

/// A coupon is never used more times than its limit. `applied` is how many
/// of the orders placed in one sequence had the code applied.
pub fn usage_limit(cart: &Cart, code: &str, limit: u32, applied: u32, orders: u32) -> Check {
    if applied <= limit {
        return Check::pass(USAGE_LIMIT);
    }
    let _ = cart;
    Check::fail(
        USAGE_LIMIT,
        OutcomeKind::RuleBroken,
        format!("over|{code}"),
        format!(
            "The coupon {code} was applied to {applied} of {orders} orders. Its limit is {limit}."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;
    use crate::eval::{OrderInfo, Totals};
    use crate::rules::Status;
    use crate::rules::test_support::resolved;

    fn eval_totals(t: Totals) -> Evaluation {
        let mut e = Evaluation::crashed(serde_json::json!({}), String::new(), String::new());
        e.crash = None;
        e.totals = Some(t);
        e
    }

    #[test]
    fn reconciles_with_shipping_in_discounts() {
        let mut r = resolved("", None);
        r.target = crate::settings::ResolvedTarget::Command(crate::settings::CommandSpec {
            run: vec!["x".into()],
            reset: None,
            timeout_ms: 1000,
            tax_inclusive: false,
            shipping_discount_in: ShippingIn::Discounts,
        });
        let ctx = RuleContext::new(&r);
        let c = cart(vec![]);
        let ok = Totals {
            subtotal: 11000,
            discounts_order_product: 2200,
            shipping_reported: 800,
            shipping_discounts: 800,
            tax: 704,
            total: 9504,
        };
        assert_eq!(
            totals_reconcile(&ctx, &c, &eval_totals(ok.clone())).status,
            Status::Pass
        );
        let bad = Totals { total: 10304, ..ok };
        assert_eq!(
            totals_reconcile(&ctx, &c, &eval_totals(bad)).status,
            Status::Fail
        );
    }

    #[test]
    fn retry_that_reuses_the_coupon_fails() {
        let mut c = cart(vec![]);
        c.codes = vec!["WELCOME20".into()];
        let order = |id: &str, uses: i64| {
            let mut e = eval_totals(Totals {
                subtotal: 0,
                discounts_order_product: 0,
                shipping_reported: 0,
                shipping_discounts: 0,
                tax: 0,
                total: 0,
            });
            e.order = Some(OrderInfo {
                id: id.into(),
                coupon_uses: [("WELCOME20".to_string(), uses)].into_iter().collect(),
            });
            e
        };
        let check = idempotent_retry(&c, &order("order-1", 1), &order("order-2", 2));
        assert_eq!(check.status, Status::Fail);
        assert_eq!(check.key, "retry_not_idempotent");
        assert_eq!(
            idempotent_retry(&c, &order("order-1", 1), &order("order-1", 1)).status,
            Status::Pass
        );
    }

    /// Regression (QA 2026-11-15): the homegrown example's one retry bug was
    /// reported twice, once for carts with a coupon ("used the coupon
    /// again") and once for carts without ("created a second order").
    #[test]
    fn retries_with_and_without_a_coupon_are_one_finding() {
        let order = |id: &str, code: &str, uses: i64| {
            let mut e = eval_totals(Totals {
                subtotal: 0,
                discounts_order_product: 0,
                shipping_reported: 0,
                shipping_discounts: 0,
                tax: 0,
                total: 0,
            });
            e.order = Some(OrderInfo {
                id: id.into(),
                coupon_uses: if code.is_empty() {
                    Default::default()
                } else {
                    [(code.to_string(), uses)].into_iter().collect()
                },
            });
            e
        };
        let mut with_code = cart(vec![]);
        with_code.codes = vec!["WELCOME20".into()];
        let a = idempotent_retry(
            &with_code,
            &order("order-1", "WELCOME20", 1),
            &order("order-2", "WELCOME20", 2),
        );
        let b = idempotent_retry(
            &cart(vec![]),
            &order("order-1", "", 0),
            &order("order-2", "", 0),
        );
        assert_eq!(a.status, Status::Fail);
        assert_eq!(b.status, Status::Fail);
        assert!(a.sentence.contains("second order") && a.sentence.contains("coupon WELCOME20"));
        assert!(b.sentence.contains("second order"));
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn usage_over_the_limit_fails() {
        let c = cart(vec![]);
        assert_eq!(usage_limit(&c, "WELCOME20", 3, 3, 5).status, Status::Pass);
        assert_eq!(usage_limit(&c, "WELCOME20", 3, 4, 5).status, Status::Fail);
    }
}
