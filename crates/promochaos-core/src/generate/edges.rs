//! Edge shapes (spec section 7): gift cards, custom products, existing
//! discount allocations, selling plans, bundle children, one variant on two
//! lines, buyers, zero- and three-decimal currencies, config shapes and code
//! combinations.

use super::{GenContext, GenKind, GeneratedCart, Origin};
use crate::cart::{Cart, ConfigShape, ConfigState, ExistingDiscount, Merchandise, SellingPlan};
use crate::catalog::Item;

fn base(ctx: &mut GenContext, currency: &str) -> Cart {
    let items: Vec<Item> = ctx.ordinary_items().to_vec();
    let mut cart = ctx.empty_cart(currency);
    for i in 0..2 {
        if let Some(item) = items.get((i * 3) % items.len().max(1)) {
            ctx.push_line(&mut cart, item, 1, None);
        }
    }
    cart
}

fn done(ctx: &GenContext, mut cart: Cart, label: &str) -> GeneratedCart {
    ctx.add_delivery(&mut cart);
    GeneratedCart {
        cart,
        origin: Origin {
            kind: GenKind::Edge,
            label: label.to_string(),
        },
    }
}

pub fn generate(ctx: &mut GenContext) -> Vec<GeneratedCart> {
    let mut out = Vec::new();
    let store = ctx.resolved.store_currency.clone();

    // Lines and products
    let gift: Option<Item> = ctx.items.iter().find(|i| i.is_gift_card()).cloned();
    if let Some(g) = &gift {
        let mut c = base(ctx, &store);
        ctx.push_line(&mut c, g, 1, None);
        out.push(done(ctx, c, "gift-card line"));
        let mut c = ctx.empty_cart(&store);
        ctx.push_line(&mut c, g, 2, None);
        out.push(done(ctx, c, "only gift cards"));
    }
    for item in ctx.ineligible.clone() {
        if item.is_gift_card() {
            continue;
        }
        let mut c = base(ctx, &store);
        ctx.push_line(&mut c, &item, 1, None);
        out.push(done(ctx, c, "line with a never-discount tag"));
        break;
    }
    let custom: Option<Item> = ctx
        .items
        .iter()
        .find(|i| matches!(i.merchandise, Merchandise::Custom(_)))
        .cloned();
    if let Some(cp) = &custom {
        let mut c = base(ctx, &store);
        ctx.push_line(&mut c, cp, 1, None);
        out.push(done(ctx, c, "CustomProduct line"));
    }
    {
        let mut c = base(ctx, &store);
        if let Some(l) = c.lines.first_mut() {
            let amount = l.subtotal() / 10;
            l.existing_discounts.push(ExistingDiscount {
                title: "Earlier automatic discount".to_string(),
                amount,
            });
        }
        out.push(done(ctx, c, "line with an existing discount allocation"));
    }
    {
        let mut c = base(ctx, &store);
        if let Some(l) = c.lines.first_mut() {
            l.selling_plan = Some(SellingPlan {
                id: "gid://shopify/SellingPlan/301".to_string(),
                name: "Deliver every month".to_string(),
                recurring_deliveries: true,
            });
        }
        out.push(done(ctx, c, "selling-plan line"));
    }
    {
        let mut c = base(ctx, &store);
        let parent_id = c.lines.first().map(|l| l.id.clone());
        let items: Vec<Item> = ctx.ordinary_items().to_vec();
        if let (Some(pid), Some(item)) = (parent_id, items.last()) {
            ctx.push_line(&mut c, item, 1, None);
            if let Some(child) = c.lines.last_mut() {
                child.parent_line_id = Some(pid);
            }
        }
        out.push(done(ctx, c, "bundle child line (parentRelationship)"));
    }
    {
        let mut c = ctx.empty_cart(&store);
        let items: Vec<Item> = ctx.ordinary_items().to_vec();
        if let Some(item) = items.first() {
            ctx.push_line(&mut c, item, 1, None);
            ctx.push_line(&mut c, item, 1, None);
        }
        out.push(done(ctx, c, "same variant on two lines"));
    }

    // Buyers
    {
        let c = base(ctx, &store);
        out.push(done(ctx, c, "no buyer identity"));
        let mut c = base(ctx, &store);
        c.buyer = Some(ctx.consumer());
        out.push(done(ctx, c, "logged-in customer"));
        let mut c = base(ctx, &store);
        c.buyer = Some(ctx.b2b());
        out.push(done(ctx, c, "B2B purchasingCompany"));
    }

    // Currencies
    let currencies = ctx.resolved.generate.currencies.clone();
    for cur in currencies.iter().filter(|c| **c != store) {
        let c = base(ctx, cur);
        out.push(done(ctx, c, &format!("presentment currency {cur}")));
        let items: Vec<Item> = ctx.ordinary_items().to_vec();
        let mut c = ctx.empty_cart(cur);
        for item in items.iter().take(5) {
            ctx.push_line(&mut c, item, 3, None);
        }
        out.push(done(
            ctx,
            c,
            &format!("presentment currency {cur}, larger cart"),
        ));
    }

    // Config shapes
    {
        let mut c = base(ctx, &store);
        c.config = ConfigState::Missing;
        c.config_shape = ConfigShape::Missing;
        out.push(done(ctx, c, "config missing"));
        let mut c = base(ctx, &store);
        c.config = ConfigState::Value(serde_json::json!({}));
        c.config_shape = ConfigShape::Empty;
        out.push(done(ctx, c, "config empty"));
        if let Some(cfg) = ctx.resolved.config.clone() {
            let mut extra = cfg;
            if let Some(obj) = extra.as_object_mut() {
                obj.insert("promochaosUnexpected".to_string(), serde_json::json!(true));
            }
            let mut c = base(ctx, &store);
            c.config = ConfigState::Value(extra);
            c.config_shape = ConfigShape::Unexpected;
            out.push(done(ctx, c, "config with unexpected fields"));
        }
    }

    // Codes
    let codes = ctx.resolved.codes.clone();
    {
        let c = base(ctx, &store);
        out.push(done(ctx, c, "no codes"));
        if let Some(code) = codes.first() {
            let mut c = base(ctx, &store);
            c.codes = vec![code.clone()];
            out.push(done(ctx, c, "the configured code"));
            let mut c = base(ctx, &store);
            c.codes = vec![code.to_lowercase()];
            out.push(done(ctx, c, "the configured code in lower case"));
        }
        let mut c = base(ctx, &store);
        c.codes = vec![ctx.unknown_code()];
        out.push(done(ctx, c, "an unknown code"));
        let mut c = base(ctx, &store);
        c.codes = codes.clone();
        c.codes.push(ctx.unknown_code());
        c.codes.push("SECOND-UNKNOWN".to_string());
        out.push(done(ctx, c, "several codes"));
    }
    out
}
