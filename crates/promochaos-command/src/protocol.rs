//! Request and response of `promochaos-command/1`.

use promochaos_core::apply::{Ledger, LineDiscount, OrderDiscount, ShippingDiscount};
use promochaos_core::cart::{BuyerKind, Cart, ConfigState, Merchandise};
use promochaos_core::eval::{OrderInfo, Totals};
use promochaos_core::money;
use serde_json::{Map, Value, json};

pub const PROTOCOL: &str = "promochaos-command/1";

pub fn request(cart: &Cart, op: &str, key: Option<&str>) -> Value {
    let lines: Vec<Value> = cart
        .lines
        .iter()
        .map(|l| {
            let (variant_id, product_id, tags, collections) = match &l.merchandise {
                Merchandise::Variant(v) => (
                    Value::String(v.variant_id.clone()),
                    Value::String(v.product_id.clone()),
                    v.tags.clone(),
                    v.collections.clone(),
                ),
                Merchandise::Custom(_) => (Value::Null, Value::Null, Vec::new(), Vec::new()),
            };
            let mut attrs = Map::new();
            for a in &l.attributes {
                attrs.insert(
                    a.key.clone(),
                    a.value.clone().map(Value::String).unwrap_or(Value::Null),
                );
            }
            json!({
                "id": l.id,
                "variant_id": variant_id,
                "product_id": product_id,
                "title": l.merchandise.title(),
                "quantity": l.quantity,
                "unit_price": l.unit_price,
                "tags": tags,
                "collections": collections,
                "gift_card": l.merchandise.is_gift_card(),
                "requires_shipping": l.merchandise.requires_shipping(),
                "attributes": Value::Object(attrs),
            })
        })
        .collect();
    let buyer = match &cart.buyer {
        None => Value::Null,
        Some(b) => {
            let kind = match b.kind {
                BuyerKind::Consumer => "consumer",
                BuyerKind::B2b => "b2b",
            };
            json!({
            "kind": kind,
            "customer_id": b.customer_id,
            "email": b.email,
            "tags": b.tags,
            "company_id": b.company_id,
            })
        }
    };
    let shipping = cart
        .delivery
        .first()
        .and_then(|g| g.selected_option())
        .map(|o| json!({ "handle": o.handle, "price": o.price }))
        .unwrap_or(Value::Null);
    let config = match &cart.config {
        ConfigState::Missing => Value::Null,
        ConfigState::Value(v) => v.clone(),
    };
    json!({
        "protocol": PROTOCOL,
        "op": op,
        "idempotency_key": key,
        "cart": {
            "id": cart.id,
            "currency": cart.currency,
            "currency_exponent": cart.exponent(),
            "lines": lines,
            "codes": cart.codes,
            "buyer": buyer,
            "shipping": shipping,
        },
        "config": config,
    })
}

pub struct Parsed {
    pub ledger: Ledger,
    pub totals: Option<Totals>,
    pub order: Option<OrderInfo>,
    pub errors: Vec<String>,
}

fn int(v: &Value, key: &str, errors: &mut Vec<String>) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i,
            None => {
                errors.push(format!("{key} is {n}, not a whole number of minor units"));
                0
            }
        },
        Some(other) => {
            errors.push(format!("{key} is {other}, not an integer of minor units"));
            0
        }
        None => {
            errors.push(format!("{key} is missing"));
            0
        }
    }
}

pub fn parse_response(cart: &Cart, v: &Value) -> Parsed {
    let mut errors = Vec::new();
    if !v.is_object() {
        errors.push("the response is not a JSON object".to_string());
    }
    let subtotal = int(v, "subtotal", &mut errors);
    let shipping = int(v, "shipping", &mut errors);
    let tax = int(v, "tax", &mut errors);
    let total = int(v, "total", &mut errors);
    let lines_sum = cart.subtotal();
    if errors.is_empty() && subtotal != lines_sum {
        errors.push(format!(
            "subtotal is {subtotal}, but the cart lines add up to {lines_sum}"
        ));
    }
    let mut ledger = Ledger {
        subtotal: lines_sum,
        ..Ledger::default()
    };
    let mut order_product = 0;
    let mut shipping_discounts = 0;
    match v.get("discounts") {
        Some(Value::Array(items)) => {
            ledger.candidates = items.len();
            for (i, d) in items.iter().enumerate() {
                let amount = int(d, "amount", &mut errors);
                if amount < 0 {
                    errors.push(format!("discounts[{i}].amount is below zero"));
                }
                let title = d
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| d.get("code").and_then(Value::as_str).map(str::to_string));
                let line_ids: Vec<String> = d
                    .get("line_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                for id in &line_ids {
                    if cart.line(id).is_none() {
                        errors.push(format!(
                            "discounts[{i}].line_ids lists {id}, which is not in the cart"
                        ));
                    }
                }
                match d.get("kind").and_then(Value::as_str) {
                    Some("product") if !line_ids.is_empty() => {
                        order_product += amount;
                        let weights: Vec<i64> = line_ids
                            .iter()
                            .map(|id| cart.line(id).map(|l| l.subtotal()).unwrap_or(0))
                            .collect();
                        for (id, part) in line_ids.iter().zip(money::allocate(amount, &weights)) {
                            ledger.product_discounts.push(LineDiscount {
                                line_id: id.clone(),
                                amount: part,
                                percent: None,
                                message: title.clone(),
                            });
                        }
                    }
                    Some("product") | Some("order") => {
                        order_product += amount;
                        let excluded: Vec<String> = if line_ids.is_empty() {
                            Vec::new()
                        } else {
                            cart.lines
                                .iter()
                                .filter(|l| !line_ids.contains(&l.id))
                                .map(|l| l.id.clone())
                                .collect()
                        };
                        ledger.order_discounts.push(OrderDiscount {
                            amount,
                            percent: None,
                            base: lines_sum,
                            excluded_line_ids: excluded,
                            message: title,
                            scope_known: !line_ids.is_empty(),
                        });
                    }
                    Some("shipping") => {
                        shipping_discounts += amount;
                        ledger.shipping_discounts.push(ShippingDiscount {
                            target: "shipping".to_string(),
                            amount,
                            percent: None,
                        });
                    }
                    Some(other) => errors.push(format!(
                        "discounts[{i}].kind is {other:?}, not order, product or shipping"
                    )),
                    None => errors.push(format!("discounts[{i}].kind is missing")),
                }
            }
        }
        Some(_) => errors.push("discounts is not an array".to_string()),
        None => errors.push("discounts is missing".to_string()),
    }
    if let Some(Value::Array(codes)) = v.get("codes_applied") {
        ledger.codes_accepted = codes
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    if let Some(Value::Array(codes)) = v.get("codes_rejected") {
        ledger.codes_rejected = codes
            .iter()
            .filter_map(|c| c.get("code").and_then(Value::as_str).or_else(|| c.as_str()))
            .map(str::to_string)
            .collect();
    }
    let order = match v.get("order") {
        Some(Value::Object(o)) => {
            let id = o
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let coupon_uses = o
                .get("coupon_uses")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0)))
                        .collect()
                })
                .unwrap_or_default();
            Some(OrderInfo { id, coupon_uses })
        }
        _ => None,
    };
    let totals = Some(Totals {
        subtotal,
        discounts_order_product: order_product,
        shipping_reported: shipping,
        shipping_discounts,
        tax,
        total,
    });
    Parsed {
        ledger,
        totals,
        order,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use promochaos_core::cart::{ConfigShape, Line, VariantRef};

    fn cart() -> Cart {
        Cart {
            id: "cart-1".into(),
            currency: "USD".into(),
            rate: "1".into(),
            lines: vec![Line {
                id: "gid://shopify/CartLine/1".into(),
                quantity: 2,
                unit_price: 5500,
                compare_at_price: None,
                merchandise: Merchandise::Variant(VariantRef {
                    variant_id: "gid://shopify/ProductVariant/1".into(),
                    product_id: "gid://shopify/Product/1".into(),
                    title: "Tote".into(),
                    variant_title: None,
                    sku: None,
                    handle: "tote".into(),
                    tags: vec![],
                    collections: vec![],
                    gift_card: false,
                    requires_shipping: true,
                    vendor: None,
                    product_type: None,
                }),
                attributes: vec![],
                selling_plan: None,
                parent_line_id: None,
                existing_discounts: vec![],
            }],
            codes: vec!["WELCOME20".into()],
            buyer: None,
            delivery: vec![],
            attributes: vec![],
            config: ConfigState::Missing,
            config_shape: ConfigShape::Normal,
            discount_classes: vec![],
        }
    }

    #[test]
    fn request_has_the_protocol_fields() {
        let r = request(&cart(), "price", None);
        assert_eq!(r["protocol"], PROTOCOL);
        assert_eq!(r["cart"]["lines"][0]["unit_price"], 5500);
        assert_eq!(r["cart"]["currency_exponent"], 2);
        assert_eq!(r["idempotency_key"], Value::Null);
    }

    #[test]
    fn parses_a_priced_cart() {
        let resp = json!({
            "subtotal": 11000,
            "discounts": [
                { "code": "WELCOME20", "title": "20% off", "kind": "order", "amount": 2200, "line_ids": [] },
                { "code": null, "title": "Free shipping", "kind": "shipping", "amount": 800, "line_ids": [] }
            ],
            "shipping": 800, "tax": 704, "total": 9504,
            "codes_applied": ["WELCOME20"], "codes_rejected": [], "order": null
        });
        let p = parse_response(&cart(), &resp);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.ledger.order_total(), 2200);
        assert_eq!(p.ledger.shipping_total(), 800);
        assert!(!p.ledger.order_discounts[0].scope_known);
        assert_eq!(p.totals.map(|t| t.total), Some(9504));
    }

    #[test]
    fn reports_shape_problems() {
        let p = parse_response(&cart(), &json!({ "subtotal": "110.00" }));
        assert!(p.errors.iter().any(|e| e.contains("subtotal")));
        assert!(p.errors.iter().any(|e| e.contains("discounts is missing")));
    }
}
