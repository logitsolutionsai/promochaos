//! Seeded cart generation (spec section 7). Generators run in order until
//! `carts` is reached: boundaries, size sweep, edge shapes, pairwise, then
//! seeded random carts. The same seed gives the same carts on every
//! platform (ChaCha8).

pub mod boundaries;
pub mod edges;
pub mod metamorphic;
pub mod pairwise;
pub mod sizes;

use crate::cart::{
    Attribute, Buyer, BuyerKind, Cart, ConfigShape, ConfigState, DeliveryGroup, DeliveryOption,
    Line,
};
use crate::catalog::Item;
use crate::money;
use crate::settings::Resolved;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

/// Which generator made a cart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenKind {
    Boundary,
    Size,
    Edge,
    Pairwise,
    Random,
}

impl GenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GenKind::Boundary => "boundaries",
            GenKind::Size => "sizes",
            GenKind::Edge => "edges",
            GenKind::Pairwise => "pairwise",
            GenKind::Random => "random",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: GenKind,
    /// What the generator was aiming at: `threshold $100.00, exact`.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedCart {
    pub cart: Cart,
    pub origin: Origin,
}

/// A small deterministic random source.
pub struct Rng(ChaCha8Rng);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(ChaCha8Rng::seed_from_u64(seed))
    }

    /// A number in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.0.next_u64() % n
    }

    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// True with probability `percent` out of 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub fn range(&mut self, lo: i64, hi_inclusive: i64) -> i64 {
        if hi_inclusive <= lo {
            return lo;
        }
        lo + self.below((hi_inclusive - lo + 1) as u64) as i64
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.index(items.len()))
        }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.index(i + 1);
            items.swap(i, j);
        }
    }
}

/// Shared state for the generators.
pub struct GenContext<'a> {
    pub resolved: &'a Resolved,
    pub items: Vec<Item>,
    /// Items the discount may touch (not gift cards, no never_discount tag).
    pub eligible: Vec<Item>,
    /// Items the discount must ignore.
    pub ineligible: Vec<Item>,
    pub rng: Rng,
    next_cart: usize,
}

impl<'a> GenContext<'a> {
    pub fn new(resolved: &'a Resolved) -> GenContext<'a> {
        let items = resolved.catalog.items();
        let never = &resolved.rules.never_discount;
        let (ineligible, eligible): (Vec<Item>, Vec<Item>) = items
            .iter()
            .cloned()
            .partition(|i| i.is_gift_card() || never.reason(&i.merchandise).is_some());
        GenContext {
            resolved,
            items,
            eligible,
            ineligible,
            rng: Rng::new(resolved.generate.seed),
            next_cart: 0,
        }
    }

    pub fn next_cart_id(&mut self) -> String {
        self.next_cart += 1;
        format!("cart-{:04}", self.next_cart)
    }

    /// The configured config as a cart config.
    pub fn normal_config(&self) -> (ConfigState, ConfigShape) {
        match &self.resolved.config {
            Some(v) => (ConfigState::Value(v.clone()), ConfigShape::Normal),
            None => (ConfigState::Missing, ConfigShape::Normal),
        }
    }

    /// Items to build ordinary lines from: eligible ones, or anything.
    pub fn ordinary_items(&self) -> &[Item] {
        if self.eligible.is_empty() {
            &self.items
        } else {
            &self.eligible
        }
    }

    /// An empty cart in `currency` with the normal config.
    pub fn empty_cart(&mut self, currency: &str) -> Cart {
        let (config, config_shape) = self.normal_config();
        let rate = self.resolved.generate.rate(currency);
        Cart {
            id: self.next_cart_id(),
            currency: currency.to_string(),
            rate: if currency == self.resolved.store_currency {
                "1".to_string()
            } else {
                rate.normalize().to_string()
            },
            lines: Vec::new(),
            codes: Vec::new(),
            buyer: None,
            delivery: Vec::new(),
            attributes: Vec::new(),
            config,
            config_shape,
            discount_classes: self.resolved.discount_classes.clone(),
        }
    }

    /// The store price of an item in the cart's currency.
    pub fn price_in(&self, cart: &Cart, store_price: i64) -> i64 {
        if cart.currency == self.resolved.store_currency {
            store_price
        } else {
            money::convert(
                store_price,
                self.resolved.store_exponent(),
                cart.rate(),
                cart.exponent(),
            )
        }
    }

    /// Appends a line of `item`, priced in the cart's currency unless
    /// `unit_price` overrides it (already in the cart's currency).
    pub fn push_line(&self, cart: &mut Cart, item: &Item, quantity: i64, unit_price: Option<i64>) {
        let id = cart.next_line_id();
        let price = unit_price.unwrap_or_else(|| self.price_in(cart, item.price));
        let compare_at = item.compare_at_price.map(|p| self.price_in(cart, p));
        // Shopify keeps one line per variant and attributes, so a repeated
        // variant gets an attribute that tells the lines apart.
        let repeat = item
            .merchandise
            .variant_id()
            .map(|v| {
                cart.lines
                    .iter()
                    .filter(|l| l.merchandise.variant_id() == Some(v))
                    .count()
            })
            .unwrap_or(0);
        let attributes = if repeat > 0 {
            vec![Attribute {
                key: "_promochaos_line".to_string(),
                value: Some((repeat + 1).to_string()),
            }]
        } else {
            Vec::new()
        };
        cart.lines.push(Line {
            id,
            quantity: quantity.max(1),
            unit_price: price.max(0),
            compare_at_price: compare_at,
            merchandise: item.merchandise.clone(),
            attributes,
            selling_plan: None,
            parent_line_id: None,
            existing_discounts: Vec::new(),
        });
    }

    /// Adds one standard shipping group covering every line that ships.
    pub fn add_delivery(&self, cart: &mut Cart) {
        let line_ids: Vec<String> = cart
            .lines
            .iter()
            .filter(|l| l.merchandise.requires_shipping())
            .map(|l| l.id.clone())
            .collect();
        cart.delivery.clear();
        if line_ids.is_empty() {
            return;
        }
        let standard = self.price_in(cart, 800);
        let express = self.price_in(cart, 2000);
        cart.delivery.push(DeliveryGroup {
            id: "gid://shopify/CartDeliveryGroup/1".to_string(),
            line_ids,
            options: vec![
                DeliveryOption {
                    handle: "standard".to_string(),
                    title: "Standard".to_string(),
                    price: standard,
                    method: "SHIPPING".to_string(),
                },
                DeliveryOption {
                    handle: "express".to_string(),
                    title: "Express".to_string(),
                    price: express,
                    method: "SHIPPING".to_string(),
                },
            ],
            selected: Some("standard".to_string()),
        });
    }

    pub fn consumer(&mut self) -> Buyer {
        let n = self.rng.range(0, 12);
        Buyer {
            kind: BuyerKind::Consumer,
            customer_id: Some(format!("gid://shopify/Customer/{}", 500 + n)),
            email: Some(format!("customer{n}@example.com")),
            tags: if n % 3 == 0 {
                vec!["vip".to_string()]
            } else {
                Vec::new()
            },
            number_of_orders: n,
            amount_spent: n * 4200,
            company_id: None,
            company_location_id: None,
        }
    }

    pub fn b2b(&mut self) -> Buyer {
        let n = self.rng.range(0, 4);
        Buyer {
            kind: BuyerKind::B2b,
            customer_id: Some(format!("gid://shopify/Customer/{}", 900 + n)),
            email: Some(format!("buyer{n}@company.example")),
            tags: vec!["wholesale".to_string()],
            number_of_orders: 3 + n,
            amount_spent: 150_000,
            company_id: Some(format!("gid://shopify/Company/{}", 70 + n)),
            company_location_id: Some(format!("gid://shopify/CompanyLocation/{}", 80 + n)),
        }
    }

    pub fn unknown_code(&self) -> String {
        "PROMOCHAOS-UNKNOWN".to_string()
    }

    /// A random ordinary cart of `lines` lines.
    pub fn random_cart(&mut self, lines: usize, currency: &str) -> Cart {
        let mut cart = self.empty_cart(currency);
        let items: Vec<Item> = self.ordinary_items().to_vec();
        for _ in 0..lines.max(1) {
            if let Some(item) = self.rng.pick(&items).cloned() {
                let qty = if self.rng.chance(70) {
                    1
                } else {
                    self.rng.range(2, 5)
                };
                self.push_line(&mut cart, &item, qty, None);
            }
        }
        self.add_delivery(&mut cart);
        cart
    }
}

/// Every generated cart, in generator order, plus how many each made.
pub struct Generated {
    pub carts: Vec<GeneratedCart>,
}

impl Generated {
    pub fn count(&self, kind: GenKind) -> usize {
        self.carts.iter().filter(|c| c.origin.kind == kind).count()
    }
}

fn push_all(out: &mut Vec<GeneratedCart>, more: Vec<GeneratedCart>, limit: usize) {
    for c in more {
        if out.len() >= limit {
            return;
        }
        out.push(c);
    }
}

/// Runs the generators in order until `carts` is reached.
pub fn generate(resolved: &Resolved) -> Generated {
    let limit = resolved.generate.carts;
    let mut ctx = GenContext::new(resolved);
    let mut out: Vec<GeneratedCart> = Vec::new();
    let b = boundaries::generate(&mut ctx);
    push_all(&mut out, b, limit);
    let s = sizes::generate(&mut ctx);
    push_all(&mut out, s, limit);
    let e = edges::generate(&mut ctx);
    push_all(&mut out, e, limit);
    let p = pairwise::generate(&mut ctx);
    push_all(&mut out, p, limit);
    while out.len() < limit {
        let cart = random(&mut ctx);
        out.push(cart);
    }
    Generated { carts: out }
}

/// Only the size sweep, for `promochaos budget`.
pub fn generate_sizes(resolved: &Resolved) -> Generated {
    let mut ctx = GenContext::new(resolved);
    Generated {
        carts: sizes::generate(&mut ctx),
    }
}

/// One seeded random cart.
fn random(ctx: &mut GenContext) -> GeneratedCart {
    let max_lines = ctx.resolved.generate.max_lines.max(1);
    let roll = ctx.rng.below(100);
    let lines = if roll < 70 {
        ctx.rng.range(1, 5)
    } else if roll < 95 {
        ctx.rng.range(6, 30)
    } else {
        ctx.rng.range(31, max_lines as i64)
    } as usize;
    let lines = lines.min(max_lines);
    let store = ctx.resolved.store_currency.clone();
    let currencies = ctx.resolved.generate.currencies.clone();
    let currency = if ctx.rng.chance(70) {
        store
    } else {
        ctx.rng.pick(&currencies).cloned().unwrap_or(store)
    };
    let mut cart = ctx.random_cart(lines, &currency);
    if !ctx.ineligible.is_empty() && ctx.rng.chance(15) {
        let ineligible = ctx.ineligible.clone();
        if let Some(item) = ctx.rng.pick(&ineligible).cloned() {
            ctx.push_line(&mut cart, &item, 1, None);
        }
    }
    let codes = ctx.resolved.codes.clone();
    match ctx.rng.below(10) {
        0..=5 => {}
        6..=7 => {
            if let Some(c) = ctx.rng.pick(&codes) {
                cart.codes.push(c.clone());
            }
        }
        8 => cart.codes.push(ctx.unknown_code()),
        _ => {
            cart.codes.extend(codes.iter().cloned());
            cart.codes.push(ctx.unknown_code());
        }
    }
    cart.buyer = match ctx.rng.below(10) {
        0..=3 => None,
        4..=8 => Some(ctx.consumer()),
        _ => Some(ctx.b2b()),
    };
    if ctx.rng.chance(10) {
        cart.config = ConfigState::Missing;
        cart.config_shape = ConfigShape::Missing;
    }
    ctx.add_delivery(&mut cart);
    GeneratedCart {
        cart,
        origin: Origin {
            kind: GenKind::Random,
            label: format!("random, {lines} lines"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::test_support::resolved;

    #[test]
    fn the_same_seed_gives_the_same_carts() {
        let r = resolved("", Some(serde_json::json!({ "tiers": [] })));
        let a = serde_json::to_string(&generate(&r).carts).unwrap_or_default();
        let b = serde_json::to_string(&generate(&r).carts).unwrap_or_default();
        assert!(!a.is_empty());
        assert_eq!(a, b);
    }

    #[test]
    fn generates_exactly_the_requested_count() {
        let mut r = resolved("", None);
        r.generate.carts = 37;
        assert_eq!(generate(&r).carts.len(), 37);
    }

    #[test]
    fn rng_is_stable() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let xs: Vec<u64> = (0..5).map(|_| a.below(1000)).collect();
        let ys: Vec<u64> = (0..5).map(|_| b.below(1000)).collect();
        assert_eq!(xs, ys);
    }
}
