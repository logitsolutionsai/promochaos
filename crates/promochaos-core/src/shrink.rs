//! Shrinking (spec section 9). Steps, in order, each kept only if the
//! fingerprint still matches:
//! 1. Remove lines (ddmin).
//! 2. Lower quantities toward 1.
//! 3. Move prices toward the nearest declared boundary or a round number.
//! 4. Remove codes, attributes and buyer identity.
//! 5. Simplify the config, only when the failure is about config.
//!
//! The budget is a number of target runs. Instruction-limit failures shrink
//! by line count instead, to the smallest count that still goes over, and
//! keep the instruction count at each size tried.

use crate::cart::{Cart, ConfigShape, ConfigState};
use serde::{Deserialize, Serialize};

/// One simplification attempt; returns whether it changed the cart.
type CartEdit = Box<dyn Fn(&mut Cart) -> bool>;

/// The oracle's answer for one candidate cart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub reproduces: bool,
    pub instructions: Option<u64>,
    pub limit: Option<u64>,
}

impl Probe {
    pub fn yes() -> Probe {
        Probe {
            reproduces: true,
            instructions: None,
            limit: None,
        }
    }
    pub fn no() -> Probe {
        Probe {
            reproduces: false,
            instructions: None,
            limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub lines: usize,
    pub instructions: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shrunk {
    pub cart: Cart,
    pub runs: usize,
    pub flaky: bool,
    pub original_lines: usize,
    /// Instruction-limit failures: the count at each line count tried.
    pub curve: Vec<CurvePoint>,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// Declared boundaries in the cart's currency (thresholds).
    pub boundaries: Vec<i64>,
    /// Shrink by line count (instruction-limit failures).
    pub by_line_count: bool,
}

struct Oracle<F: FnMut(&Cart) -> Probe> {
    f: F,
    runs: usize,
    budget: usize,
    curve: Vec<CurvePoint>,
}

impl<F: FnMut(&Cart) -> Probe> Oracle<F> {
    fn left(&self) -> bool {
        self.runs < self.budget
    }

    fn test(&mut self, c: &Cart) -> bool {
        if !self.left() {
            return false;
        }
        self.runs += 1;
        let p = (self.f)(c);
        if let (Some(i), Some(l)) = (p.instructions, p.limit) {
            self.curve.push(CurvePoint {
                lines: c.lines.len(),
                instructions: i,
                limit: l,
            });
        }
        p.reproduces
    }
}

fn with_lines(cart: &Cart, keep: &[usize]) -> Cart {
    let mut c = cart.clone();
    c.lines = keep
        .iter()
        .filter_map(|i| cart.lines.get(*i).cloned())
        .collect();
    c.repair_references();
    c
}

/// Delta debugging over lines: try removing chunks, keep any removal that
/// still fails, refine the chunk size.
fn ddmin<F: FnMut(&Cart) -> Probe>(cart: Cart, o: &mut Oracle<F>) -> Cart {
    let mut current = cart;
    let mut n = 2_usize;
    while current.lines.len() >= 2 && o.left() {
        let len = current.lines.len();
        let chunk = len.div_ceil(n);
        let mut reduced = false;
        let mut start = 0;
        while start < len {
            let end = (start + chunk).min(len);
            let keep: Vec<usize> = (0..len).filter(|i| *i < start || *i >= end).collect();
            if !keep.is_empty() {
                let candidate = with_lines(&current, &keep);
                if o.test(&candidate) {
                    current = candidate;
                    n = (n - 1).max(2);
                    reduced = true;
                    break;
                }
            }
            start = end;
        }
        if !reduced {
            if n >= len {
                break;
            }
            n = (n * 2).min(len);
        }
    }
    current
}

fn lower_quantities<F: FnMut(&Cart) -> Probe>(cart: Cart, o: &mut Oracle<F>) -> Cart {
    let mut current = cart;
    for i in 0..current.lines.len() {
        let q = current.lines.get(i).map(|l| l.quantity).unwrap_or(1);
        if q <= 1 {
            continue;
        }
        // Try 1 first, then halve toward the current quantity.
        let mut lo = 1;
        let mut hi = q;
        while lo < hi && o.left() {
            let mid = lo + (hi - lo) / 2;
            let mut candidate = current.clone();
            if let Some(l) = candidate.lines.get_mut(i) {
                l.quantity = mid;
            }
            if o.test(&candidate) {
                current = candidate;
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
    }
    current
}

fn round_candidates(price: i64, boundaries: &[i64], quantity: i64) -> Vec<i64> {
    let mut c = Vec::new();
    // A boundary reached by this line alone.
    for b in boundaries {
        if quantity > 0 && b % quantity == 0 {
            c.push(b / quantity);
        }
    }
    for unit in [100_000_i64, 10_000, 1_000, 100] {
        // Only units no larger than the price: rounding $45.00 "up" to the
        // next 100,000 minor units made shrunk carts of $1,000.00 shirts.
        let r = (price / unit) * unit;
        if r <= 0 {
            continue;
        }
        c.push(r);
        let up = r + unit;
        if (up - price).abs() < unit {
            c.push(up);
        }
    }
    c.retain(|p| *p != price && *p > 0);
    c.dedup();
    c
}

fn move_prices<F: FnMut(&Cart) -> Probe>(cart: Cart, hints: &Hints, o: &mut Oracle<F>) -> Cart {
    let mut current = cart;
    for i in 0..current.lines.len() {
        let Some(line) = current.lines.get(i) else {
            continue;
        };
        let candidates = round_candidates(line.unit_price, &hints.boundaries, line.quantity);
        for p in candidates {
            if !o.left() {
                return current;
            }
            let mut candidate = current.clone();
            if let Some(l) = candidate.lines.get_mut(i) {
                l.unit_price = p;
            }
            if o.test(&candidate) {
                current = candidate;
                break;
            }
        }
    }
    current
}

fn simplify_extras<F: FnMut(&Cart) -> Probe>(cart: Cart, o: &mut Oracle<F>) -> Cart {
    let mut current = cart;
    let tries: Vec<CartEdit> = vec![
        Box::new(|c: &mut Cart| {
            let had = !c.codes.is_empty();
            c.codes.clear();
            had
        }),
        Box::new(|c: &mut Cart| {
            let mut had = false;
            for l in &mut c.lines {
                had |= !l.attributes.is_empty() || !l.existing_discounts.is_empty();
                l.attributes.clear();
                l.existing_discounts.clear();
            }
            had |= !c.attributes.is_empty();
            c.attributes.clear();
            had
        }),
        Box::new(|c: &mut Cart| c.buyer.take().is_some()),
        Box::new(|c: &mut Cart| {
            let mut had = false;
            for l in &mut c.lines {
                had |= l.selling_plan.take().is_some();
                had |= l.parent_line_id.take().is_some();
            }
            had
        }),
    ];
    for t in &tries {
        if !o.left() {
            break;
        }
        let mut candidate = current.clone();
        if t(&mut candidate) && o.test(&candidate) {
            current = candidate;
        }
    }
    // One code at a time, when removing them all lost the failure.
    if current.codes.len() > 1 {
        let mut i = 0;
        while i < current.codes.len() && o.left() {
            let mut candidate = current.clone();
            candidate.codes.remove(i);
            if o.test(&candidate) {
                current = candidate;
            } else {
                i += 1;
            }
        }
    }
    current
}

fn simplify_config<F: FnMut(&Cart) -> Probe>(cart: Cart, o: &mut Oracle<F>) -> Cart {
    let mut current = cart;
    let keys: Vec<String> = match &current.config {
        ConfigState::Value(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        _ => return current,
    };
    for k in keys {
        if !o.left() {
            break;
        }
        let mut candidate = current.clone();
        if let ConfigState::Value(serde_json::Value::Object(m)) = &mut candidate.config {
            m.remove(&k);
        }
        if o.test(&candidate) {
            current = candidate;
        }
    }
    current
}

/// Binary search on the line count (keeping a prefix of the lines) for the
/// smallest cart that still goes over.
fn by_line_count<F: FnMut(&Cart) -> Probe>(cart: Cart, o: &mut Oracle<F>) -> Cart {
    let mut best = cart.clone();
    let mut lo = 1_usize;
    let mut hi = cart.lines.len();
    while lo < hi && o.left() {
        let mid = lo + (hi - lo) / 2;
        let keep: Vec<usize> = (0..mid).collect();
        let candidate = with_lines(&cart, &keep);
        if o.test(&candidate) {
            best = candidate;
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    best
}

/// Shrinks `cart` while `oracle` says the failure still reproduces. The
/// oracle is called first on the original: if that does not reproduce, the
/// failure is `flaky` and the cart is returned as it was.
pub fn shrink<F>(cart: &Cart, budget: usize, hints: &Hints, oracle: F) -> Shrunk
where
    F: FnMut(&Cart) -> Probe,
{
    let original_lines = cart.lines.len();
    let mut o = Oracle {
        f: oracle,
        runs: 0,
        budget: budget.max(1),
        curve: Vec::new(),
    };
    if !o.test(cart) {
        let Oracle { runs, curve, .. } = o;
        return Shrunk {
            cart: cart.clone(),
            runs,
            flaky: true,
            original_lines,
            curve,
            steps: vec!["did not reproduce".to_string()],
        };
    }
    let mut steps = Vec::new();
    let mut current = cart.clone();
    if hints.by_line_count {
        current = by_line_count(current, &mut o);
        steps.push(format!(
            "line count {original_lines} to {}",
            current.lines.len()
        ));
    } else {
        let before = current.lines.len();
        current = ddmin(current, &mut o);
        steps.push(format!("lines {before} to {}", current.lines.len()));
        current = lower_quantities(current, &mut o);
        current = move_prices(current, hints, &mut o);
        current = simplify_extras(current, &mut o);
        if current.config_shape != ConfigShape::Normal {
            current = simplify_config(current, &mut o);
        }
    }
    let Oracle {
        runs, mut curve, ..
    } = o;
    curve.sort_by_key(|p| p.lines);
    curve.dedup_by_key(|p| p.lines);
    Shrunk {
        cart: current,
        runs,
        flaky: false,
        original_lines,
        curve,
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;

    fn big_cart(n: u32) -> Cart {
        cart(
            (1..=n)
                .map(|i| {
                    line(
                        i,
                        variant(i, &format!("Item {i}"), &[], i == 7),
                        1 + i64::from(i % 3),
                        1234,
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn ddmin_finds_the_one_line_that_matters() {
        let c = big_cart(20);
        let s = shrink(&c, 200, &Hints::default(), |c: &Cart| {
            if c.lines.iter().any(|l| l.merchandise.is_gift_card()) {
                Probe::yes()
            } else {
                Probe::no()
            }
        });
        assert!(!s.flaky);
        assert_eq!(s.cart.lines.len(), 1);
        assert!(s.cart.lines[0].merchandise.is_gift_card());
        assert_eq!(s.cart.lines[0].quantity, 1);
        assert!(s.runs <= 200);
    }

    #[test]
    fn line_count_search_reports_the_curve() {
        let c = big_cart(250);
        // "Over the limit" from 178 lines.
        let s = shrink(
            &c,
            200,
            &Hints {
                by_line_count: true,
                ..Hints::default()
            },
            |c: &Cart| {
                let n = c.lines.len() as u64;
                Probe {
                    reproduces: n >= 178,
                    instructions: Some(n * n * 350),
                    limit: Some(11_000_000),
                }
            },
        );
        assert_eq!(s.cart.lines.len(), 178);
        assert!(s.curve.iter().any(|p| p.lines == 178));
        assert!(s.curve.windows(2).all(|w| w[0].lines < w[1].lines));
    }

    #[test]
    fn a_failure_that_does_not_reproduce_is_flaky() {
        let c = big_cart(3);
        let s = shrink(&c, 10, &Hints::default(), |_c: &Cart| Probe::no());
        assert!(s.flaky);
        assert_eq!(s.runs, 1);
    }

    #[test]
    fn the_budget_is_respected() {
        let c = big_cart(100);
        let s = shrink(&c, 7, &Hints::default(), |_c: &Cart| Probe::yes());
        assert!(s.runs <= 7);
    }

    #[test]
    fn prices_move_to_round_numbers() {
        assert!(round_candidates(4537, &[], 1).contains(&4500));
        assert!(round_candidates(4537, &[10000], 2).contains(&5000));
    }

    /// Regression (QA 2026-11-15): a $45.00 line was "rounded" to $1,000.00
    /// because the next unit up was tried for units larger than the price.
    #[test]
    fn round_candidates_stay_near_the_price() {
        let c = round_candidates(4500, &[], 3);
        assert_eq!(c, vec![4000, 5000]);
        assert!(c.iter().all(|p| *p <= 2 * 4500));
        assert!(round_candidates(50, &[], 1).is_empty());
    }
}
