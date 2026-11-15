//! Twin carts for the metamorphic rules: for each sampled cart, a twin whose
//! expected relation to the original is known.
//! - `split_line`: one line of quantity n becomes two lines adding up to n
//!   (the second with a different line attribute, as Shopify would make it).
//! - `reorder_lines`: the same lines in another order.
//! - `add_ineligible_line`: add a line the discount must ignore.
//! - `more_spend_never_less`: raise one eligible line's quantity by one.

use super::{GeneratedCart, Rng};
use crate::cart::{Attribute, Cart, Line};
use crate::catalog::Item;
use crate::rules::metamorphic::TwinNote;
use crate::settings::{NeverDiscount, Relation, Resolved};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Twin {
    pub relation: Relation,
    /// Index of the original in the generated carts.
    pub original: usize,
    pub cart: Cart,
    pub note_title: String,
    pub note_quantity: i64,
}

impl Twin {
    pub fn note(&self) -> TwinNote {
        TwinNote {
            title: self.note_title.clone(),
            quantity: self.note_quantity,
        }
    }
}

/// Builds the twin of `cart` for `relation`, or `None` when the relation
/// does not apply to this cart.
pub fn make_twin(
    relation: Relation,
    cart: &Cart,
    ineligible: &[Item],
    never: &NeverDiscount,
    store_currency: &str,
) -> Option<(Cart, String, i64)> {
    let mut twin = cart.clone();
    twin.id = format!("{}-{}", cart.id, relation.id());
    match relation {
        Relation::SplitLine => {
            let (idx, line) = cart
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.quantity >= 2)
                .max_by(|(ia, a), (ib, b)| a.quantity.cmp(&b.quantity).then(ib.cmp(ia)))?;
            let first = line.quantity / 2;
            let second = line.quantity - first;
            let mut new_line: Line = line.clone();
            new_line.id = twin.next_line_id();
            new_line.quantity = second;
            new_line.attributes.push(Attribute {
                key: "_promochaos_split".to_string(),
                value: Some("2".to_string()),
            });
            if let Some(l) = twin.lines.get_mut(idx) {
                l.quantity = first;
            }
            for g in &mut twin.delivery {
                if g.line_ids.contains(&line.id) {
                    g.line_ids.push(new_line.id.clone());
                }
            }
            twin.lines.insert(idx + 1, new_line);
            Some((twin, line.merchandise.title().to_string(), line.quantity))
        }
        Relation::ReorderLines => {
            if cart.lines.len() < 2 {
                return None;
            }
            twin.lines.reverse();
            Some((twin, String::new(), 0))
        }
        Relation::AddIneligibleLine => {
            // Only same-currency carts, so the added line's price is simple.
            if cart.currency != store_currency {
                return None;
            }
            let item = ineligible.first()?;
            let id = twin.next_line_id();
            twin.lines.push(Line {
                id,
                quantity: 1,
                unit_price: item.price,
                compare_at_price: None,
                merchandise: item.merchandise.clone(),
                attributes: Vec::new(),
                selling_plan: None,
                parent_line_id: None,
                existing_discounts: Vec::new(),
            });
            Some((twin, item.merchandise.display_name(), 1))
        }
        Relation::MoreSpendNeverLess => {
            let idx = cart
                .lines
                .iter()
                .position(|l| never.reason(&l.merchandise).is_none())?;
            let title = cart.lines.get(idx)?.merchandise.title().to_string();
            if let Some(l) = twin.lines.get_mut(idx) {
                l.quantity += 1;
            }
            Some((twin, title, 1))
        }
    }
}

/// Samples up to `[generate] twins` carts per relation and builds twins.
/// Carts from the boundary generator come first, the rest are shuffled with
/// the run's seed.
pub fn twins(resolved: &Resolved, carts: &[GeneratedCart]) -> Vec<Twin> {
    let items = resolved.catalog.items();
    let never = &resolved.rules.never_discount;
    let ineligible: Vec<Item> = items
        .into_iter()
        .filter(|i| i.is_gift_card() || never.reason(&i.merchandise).is_some())
        .collect();
    let mut out = Vec::new();
    for (r_index, relation) in resolved.rules.metamorphic.iter().enumerate() {
        if *relation == Relation::AddIneligibleLine && !never.is_set() {
            continue;
        }
        let mut rng = Rng::new(resolved.generate.seed ^ (0x9e37_79b9_7f4a_7c15 + r_index as u64));
        let mut boundary: Vec<usize> = Vec::new();
        let mut rest: Vec<usize> = Vec::new();
        for (i, g) in carts.iter().enumerate() {
            // Twins of big carts cost time and say little.
            if g.cart.lines.len() > 30 {
                continue;
            }
            if g.origin.kind == super::GenKind::Boundary {
                boundary.push(i);
            } else {
                rest.push(i);
            }
        }
        rng.shuffle(&mut rest);
        let mut taken = 0;
        for i in boundary.into_iter().chain(rest) {
            if taken >= resolved.generate.twins {
                break;
            }
            let Some(g) = carts.get(i) else { continue };
            if let Some((cart, title, qty)) = make_twin(
                *relation,
                &g.cart,
                &ineligible,
                never,
                &resolved.store_currency,
            ) {
                out.push(Twin {
                    relation: *relation,
                    original: i,
                    cart,
                    note_title: title,
                    note_quantity: qty,
                });
                taken += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;

    #[test]
    fn split_line_keeps_the_quantity() {
        let c = cart(vec![
            line(1, variant(1, "Linen Shirt", &["multi-buy"], false), 2, 4500),
            line(2, variant(2, "Tote", &[], false), 1, 10000),
        ]);
        let (t, title, qty) = make_twin(
            Relation::SplitLine,
            &c,
            &[],
            &NeverDiscount::default(),
            "USD",
        )
        .unwrap_or_else(|| (cart(vec![]), String::new(), 0));
        assert_eq!(t.lines.len(), 3);
        assert_eq!(t.total_quantity(), c.total_quantity());
        assert_eq!(t.subtotal(), c.subtotal());
        assert_eq!(t.lines[0].quantity, 1);
        assert_eq!(t.lines[1].quantity, 1);
        assert_eq!(t.lines[1].id, "gid://shopify/CartLine/3");
        assert_eq!((title.as_str(), qty), ("Linen Shirt", 2));
    }

    #[test]
    fn reorder_needs_two_lines() {
        let c = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 10000)]);
        assert!(
            make_twin(
                Relation::ReorderLines,
                &c,
                &[],
                &NeverDiscount::default(),
                "USD"
            )
            .is_none()
        );
    }
}
