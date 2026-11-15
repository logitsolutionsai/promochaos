//! Pairwise coverage across line count, gift cards present, buyer type,
//! currency, codes and config shape: every pair of values of any two
//! factors appears in at least one cart. Greedy and deterministic.

use super::{GenContext, GenKind, GeneratedCart, Origin};
use crate::cart::{ConfigShape, ConfigState};
use crate::catalog::Item;

/// For each factor pair `(a, b)`, the value pairs `(va, vb)` still to cover.
type PairTable = Vec<((usize, usize), Vec<(usize, usize)>)>;

/// Builds a covering array. `levels[f]` is the number of values of factor
/// `f`. Rows are vectors of value indexes.
pub fn covering_rows(levels: &[usize]) -> Vec<Vec<usize>> {
    let f = levels.len();
    if f == 0 {
        return Vec::new();
    }
    if f == 1 {
        return (0..levels[0]).map(|v| vec![v]).collect();
    }
    // uncovered[(a, b)] is a set of (va, vb) pairs still to cover.
    let mut uncovered: PairTable = Vec::new();
    for a in 0..f {
        for b in (a + 1)..f {
            let mut pairs = Vec::new();
            for va in 0..levels[a] {
                for vb in 0..levels[b] {
                    pairs.push((va, vb));
                }
            }
            uncovered.push(((a, b), pairs));
        }
    }
    let mut rows = Vec::new();
    let remaining = |u: &PairTable| -> usize { u.iter().map(|(_, p)| p.len()).sum() };
    while remaining(&uncovered) > 0 {
        // Seed the row with the first uncovered pair.
        let mut row: Vec<Option<usize>> = vec![None; f];
        if let Some(((a, b), pairs)) = uncovered.iter().find(|(_, p)| !p.is_empty())
            && let Some((va, vb)) = pairs.first()
        {
            row[*a] = Some(*va);
            row[*b] = Some(*vb);
        }
        // Fill the other factors with the value that covers the most pairs.
        for factor in 0..f {
            if row[factor].is_some() {
                continue;
            }
            let mut best = (0_usize, 0_usize);
            for v in 0..levels[factor] {
                let mut gain = 0;
                for ((a, b), pairs) in &uncovered {
                    if *a == factor {
                        if let Some(vb) = row[*b]
                            && pairs.contains(&(v, vb))
                        {
                            gain += 1;
                        }
                    } else if *b == factor
                        && let Some(va) = row[*a]
                        && pairs.contains(&(va, v))
                    {
                        gain += 1;
                    }
                }
                if gain > best.1 {
                    best = (v, gain);
                }
            }
            row[factor] = Some(best.0);
        }
        let row: Vec<usize> = row.into_iter().map(|v| v.unwrap_or(0)).collect();
        for ((a, b), pairs) in &mut uncovered {
            let pair = (row[*a], row[*b]);
            pairs.retain(|p| *p != pair);
        }
        rows.push(row);
    }
    rows
}

const LINE_COUNTS: &[usize] = &[1, 3, 12];

pub fn generate(ctx: &mut GenContext) -> Vec<GeneratedCart> {
    let currencies = ctx.resolved.generate.currencies.clone();
    let has_gift = !ctx.ineligible.is_empty();
    let has_codes = !ctx.resolved.codes.is_empty();
    let has_config = ctx.resolved.config.is_some();
    let levels = vec![
        LINE_COUNTS.len(),
        if has_gift { 2 } else { 1 },
        3, // buyer: none, consumer, b2b
        currencies.len().max(1),
        if has_codes { 4 } else { 2 }, // none, configured, unknown, several
        if has_config { 3 } else { 1 }, // normal, missing, empty
    ];
    let rows = covering_rows(&levels);
    let items: Vec<Item> = ctx.ordinary_items().to_vec();
    let ineligible = ctx.ineligible.clone();
    let codes = ctx.resolved.codes.clone();
    let store = ctx.resolved.store_currency.clone();
    let mut out = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let lines = LINE_COUNTS.get(row[0]).copied().unwrap_or(1);
        let currency = currencies
            .get(row[3])
            .cloned()
            .unwrap_or_else(|| store.clone());
        let mut cart = ctx.empty_cart(&currency);
        for i in 0..lines {
            if let Some(item) = items.get((i + n) % items.len().max(1)) {
                let qty = 1 + ((i + n) % 3) as i64;
                ctx.push_line(&mut cart, item, qty, None);
            }
        }
        if row[1] == 1
            && let Some(g) = ineligible.first()
        {
            ctx.push_line(&mut cart, g, 1, None);
        }
        cart.buyer = match row[2] {
            1 => Some(ctx.consumer()),
            2 => Some(ctx.b2b()),
            _ => None,
        };
        cart.codes = match (has_codes, row[4]) {
            (true, 1) => codes.iter().take(1).cloned().collect(),
            (true, 2) | (false, 1) => vec![ctx.unknown_code()],
            (true, 3) => {
                let mut c: Vec<String> = codes.clone();
                c.push(ctx.unknown_code());
                c
            }
            _ => Vec::new(),
        };
        match row[5] {
            1 => {
                cart.config = ConfigState::Missing;
                cart.config_shape = ConfigShape::Missing;
            }
            2 => {
                cart.config = ConfigState::Value(serde_json::json!({}));
                cart.config_shape = ConfigShape::Empty;
            }
            _ => {}
        }
        ctx.add_delivery(&mut cart);
        out.push(GeneratedCart {
            cart,
            origin: Origin {
                kind: GenKind::Pairwise,
                label: format!("pairwise row {}", n + 1),
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_pair() {
        let levels = vec![3, 2, 3, 3, 4, 3];
        let rows = covering_rows(&levels);
        for a in 0..levels.len() {
            for b in (a + 1)..levels.len() {
                for va in 0..levels[a] {
                    for vb in 0..levels[b] {
                        assert!(
                            rows.iter().any(|r| r[a] == va && r[b] == vb),
                            "pair ({a}={va}, {b}={vb}) not covered"
                        );
                    }
                }
            }
        }
        // Far fewer rows than the 648 of the full product.
        assert!(rows.len() < 40, "{} rows", rows.len());
    }
}
