//! Metamorphic rules: a twin cart whose discount relates to the original's
//! in a known way. The tolerance is one minor unit per line in the twin, for
//! rounding.

use super::{Check, OutcomeKind};
use crate::cart::Cart;
use crate::eval::Evaluation;
use crate::settings::Relation;

/// The whole discount of one run: order, product and shipping.
pub fn total_discount(eval: &Evaluation) -> Option<i64> {
    eval.ledger
        .as_ref()
        .map(|l| l.merchandise_total() + l.shipping_total())
}

/// What the twin changed, for sentences: the line that was split, added or
/// raised.
#[derive(Debug, Clone, Default)]
pub struct TwinNote {
    pub title: String,
    pub quantity: i64,
}

pub fn check(
    relation: Relation,
    original: &Cart,
    original_eval: &Evaluation,
    twin: &Cart,
    twin_eval: &Evaluation,
    note: &TwinNote,
) -> Check {
    let rule = relation.id();
    if original_eval.crash.is_some() || twin_eval.crash.is_some() {
        return Check::not_applicable(rule, "trapped first");
    }
    let (Some(a), Some(b)) = (total_discount(original_eval), total_discount(twin_eval)) else {
        return Check::not_applicable(rule, "produced no output");
    };
    let tolerance = twin.lines.len() as i64;
    let ok = match relation {
        Relation::SplitLine | Relation::ReorderLines | Relation::AddIneligibleLine => {
            (a - b).abs() <= tolerance
        }
        Relation::MoreSpendNeverLess => b + tolerance >= a,
    };
    if ok {
        return Check::pass(rule);
    }
    let lost = a > 0 && b == 0;
    // One key per relation: losing the discount and changing it are the same
    // defect (the spec's fingerprint has no extra key for a broken rule), so
    // they must collapse into one finding, not two.
    let key = "twin_differs";
    let from_to = format!("from {} to {}", original.money(a), twin.money(b));
    let sentence = match relation {
        Relation::SplitLine => {
            if lost {
                format!(
                    "Splitting the {} line (quantity {}) into two lines removed the discount.",
                    note.title, note.quantity
                )
            } else {
                format!(
                    "Splitting the {} line (quantity {}) into two lines changed the discount {from_to}.",
                    note.title, note.quantity
                )
            }
        }
        Relation::ReorderLines => {
            format!("Putting the same lines in another order changed the discount {from_to}.")
        }
        Relation::AddIneligibleLine => format!(
            "Adding {}, which the discount must ignore, changed the discount {from_to}.",
            note.title
        ),
        Relation::MoreSpendNeverLess => format!(
            "Adding one more {} lowered the discount {from_to}.",
            note.title
        ),
    };
    Check::fail(rule, OutcomeKind::RuleBroken, key, sentence)
        .with_detail(
            "Original",
            format!("{}. Discount {}", original.summary(), original.money(a)),
        )
        .with_detail(
            "Twin",
            format!("{}. Discount {}", twin.summary(), twin.money(b)),
        )
}

/// A one-line guess at the cause, shown under the twin.
pub fn likely_cause(relation: Relation) -> &'static str {
    match relation {
        Relation::SplitLine => {
            "Quantity is counted per cart line, not per variant. Shopify puts one variant on two lines when the line attributes or selling plans differ."
        }
        Relation::ReorderLines => {
            "The function depends on line order, for example by taking the first matching line."
        }
        Relation::AddIneligibleLine => {
            "Lines the discount must ignore still count toward a threshold or receive part of the discount."
        }
        Relation::MoreSpendNeverLess => {
            "A threshold or cap drops the discount when the cart grows. Some promotions do this on purpose; this rule is opt-in."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::Ledger;
    use crate::cart::test_support::*;
    use crate::rules::Status;

    fn eval_with(amount: i64) -> Evaluation {
        let mut e = Evaluation::crashed(serde_json::json!({}), String::new(), String::new());
        e.crash = None;
        e.ledger = Some(Ledger {
            product_discounts: vec![crate::apply::LineDiscount {
                line_id: "x".into(),
                amount,
                percent: None,
                message: None,
            }],
            ..Ledger::default()
        });
        e
    }

    #[test]
    fn split_line_losing_the_discount_fails() {
        let orig = cart(vec![line(
            1,
            variant(1, "Linen Shirt", &[], false),
            2,
            4500,
        )]);
        let twin = cart(vec![
            line(1, variant(1, "Linen Shirt", &[], false), 1, 4500),
            line(2, variant(1, "Linen Shirt", &[], false), 1, 4500),
        ]);
        let note = TwinNote {
            title: "Linen Shirt".into(),
            quantity: 2,
        };
        let c = check(
            Relation::SplitLine,
            &orig,
            &eval_with(900),
            &twin,
            &eval_with(0),
            &note,
        );
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.key, "twin_differs");
        assert_eq!(
            c.sentence,
            "Splitting the Linen Shirt line (quantity 2) into two lines removed the discount."
        );
        let c = check(
            Relation::SplitLine,
            &orig,
            &eval_with(900),
            &twin,
            &eval_with(901),
            &note,
        );
        assert_eq!(c.status, Status::Pass);
    }

    /// Regression (QA 2026-11-15): one split_line defect was reported as two
    /// findings, "removed the discount" and "changed the discount", because
    /// the two sentences had different fingerprint keys.
    #[test]
    fn lost_and_changed_twins_share_one_fingerprint() {
        let orig = cart(vec![line(
            1,
            variant(1, "Linen Shirt", &[], false),
            3,
            4500,
        )]);
        let twin = cart(vec![
            line(1, variant(1, "Linen Shirt", &[], false), 1, 4500),
            line(2, variant(1, "Linen Shirt", &[], false), 2, 4500),
        ]);
        let note = TwinNote {
            title: "Linen Shirt".into(),
            quantity: 3,
        };
        let lost = check(
            Relation::SplitLine,
            &orig,
            &eval_with(1350),
            &twin,
            &eval_with(0),
            &note,
        );
        let changed = check(
            Relation::SplitLine,
            &orig,
            &eval_with(1350),
            &twin,
            &eval_with(900),
            &note,
        );
        assert_eq!(lost.status, Status::Fail);
        assert_eq!(changed.status, Status::Fail);
        assert!(lost.sentence.contains("removed the discount"));
        assert!(changed.sentence.contains("from $13.50 to $9.00"));
        assert_eq!(lost.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn more_spend_never_less_allows_more() {
        let orig = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 4500)]);
        let twin = cart(vec![line(1, variant(1, "Tote", &[], false), 2, 4500)]);
        let note = TwinNote {
            title: "Tote".into(),
            quantity: 1,
        };
        let c = check(
            Relation::MoreSpendNeverLess,
            &orig,
            &eval_with(100),
            &twin,
            &eval_with(500),
            &note,
        );
        assert_eq!(c.status, Status::Pass);
        let c = check(
            Relation::MoreSpendNeverLess,
            &orig,
            &eval_with(500),
            &twin,
            &eval_with(100),
            &note,
        );
        assert_eq!(c.status, Status::Fail);
    }
}
