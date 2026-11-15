//! The size sweep: 1, 2, 10, 50, 100, 199, 200, 201 and 250 lines. Limits
//! scale above 200 lines, so these find instruction-budget failures.

use super::{GenContext, GenKind, GeneratedCart, Origin};
use crate::catalog::Item;

pub const SWEEP: &[usize] = &[1, 2, 10, 50, 100, 199, 200, 201, 250];

pub fn sizes(max_lines: usize) -> Vec<usize> {
    let mut s: Vec<usize> = SWEEP.iter().copied().filter(|n| *n <= max_lines).collect();
    if max_lines > 250 {
        s.push(max_lines);
    }
    s
}

pub fn generate(ctx: &mut GenContext) -> Vec<GeneratedCart> {
    let items: Vec<Item> = ctx.ordinary_items().to_vec();
    let store = ctx.resolved.store_currency.clone();
    let mut out = Vec::new();
    for n in sizes(ctx.resolved.generate.max_lines) {
        let mut cart = ctx.empty_cart(&store);
        for i in 0..n {
            if let Some(item) = items.get(i % items.len().max(1)) {
                ctx.push_line(&mut cart, item, 1, None);
            }
        }
        ctx.add_delivery(&mut cart);
        let noun = if n == 1 { "line" } else { "lines" };
        out.push(GeneratedCart {
            cart,
            origin: Origin {
                kind: GenKind::Size,
                label: format!("size sweep, {n} {noun}"),
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_respects_max_lines() {
        assert_eq!(sizes(250), SWEEP.to_vec());
        assert_eq!(sizes(100), vec![1, 2, 10, 50, 100]);
        assert_eq!(sizes(400).last(), Some(&400));
    }
}
