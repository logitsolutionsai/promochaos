//! The cart model: what promochaos generates, shrinks and renders.
//!
//! Every price is in integer minor units of the cart's (presentment)
//! currency. Product facts are copied onto each line so the shrinker can
//! change a line without looking anything up.

use crate::money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Attribute {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct VariantRef {
    pub variant_id: String,
    pub product_id: String,
    pub title: String,
    pub variant_title: Option<String>,
    pub sku: Option<String>,
    pub handle: String,
    pub tags: Vec<String>,
    pub collections: Vec<String>,
    pub gift_card: bool,
    pub requires_shipping: bool,
    pub vendor: Option<String>,
    pub product_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CustomRef {
    pub title: String,
    pub gift_card: bool,
    pub requires_shipping: bool,
}

/// `ProductVariant` or `CustomProduct`, as in the Discount API's
/// `Merchandise` union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
// Carts are small and most lines are variants, so boxing the big variant
// would add an allocation per line for no real saving.
#[allow(clippy::large_enum_variant)]
pub enum Merchandise {
    Variant(VariantRef),
    Custom(CustomRef),
}

impl Merchandise {
    pub fn title(&self) -> &str {
        match self {
            Merchandise::Variant(v) => &v.title,
            Merchandise::Custom(c) => &c.title,
        }
    }

    pub fn is_gift_card(&self) -> bool {
        match self {
            Merchandise::Variant(v) => v.gift_card,
            Merchandise::Custom(c) => c.gift_card,
        }
    }

    pub fn tags(&self) -> &[String] {
        match self {
            Merchandise::Variant(v) => &v.tags,
            Merchandise::Custom(_) => &[],
        }
    }

    pub fn variant_id(&self) -> Option<&str> {
        match self {
            Merchandise::Variant(v) => Some(&v.variant_id),
            Merchandise::Custom(_) => None,
        }
    }

    pub fn requires_shipping(&self) -> bool {
        match self {
            Merchandise::Variant(v) => v.requires_shipping,
            Merchandise::Custom(c) => c.requires_shipping,
        }
    }

    /// The display name used in report tables: "Linen Shirt, M / Natural".
    pub fn display_name(&self) -> String {
        match self {
            Merchandise::Variant(v) => match &v.variant_title {
                Some(t) if !t.is_empty() => format!("{}, {}", v.title, t),
                _ => v.title.clone(),
            },
            Merchandise::Custom(c) => format!("{} (custom)", c.title),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct SellingPlan {
    pub id: String,
    pub name: String,
    pub recurring_deliveries: bool,
}

/// A discount already on the line before this function runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ExistingDiscount {
    pub title: String,
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Line {
    /// `gid://shopify/CartLine/1`
    pub id: String,
    pub quantity: i64,
    /// Per item, in minor units of the cart currency.
    pub unit_price: i64,
    pub compare_at_price: Option<i64>,
    pub merchandise: Merchandise,
    pub attributes: Vec<Attribute>,
    pub selling_plan: Option<SellingPlan>,
    /// Bundle child: the id of the parent line.
    pub parent_line_id: Option<String>,
    pub existing_discounts: Vec<ExistingDiscount>,
}

impl Line {
    pub fn subtotal(&self) -> i64 {
        self.unit_price.saturating_mul(self.quantity)
    }

    pub fn attribute(&self, key: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.key == key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BuyerKind {
    Consumer,
    B2b,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Buyer {
    pub kind: BuyerKind,
    pub customer_id: Option<String>,
    pub email: Option<String>,
    pub tags: Vec<String>,
    pub number_of_orders: i64,
    pub amount_spent: i64,
    pub company_id: Option<String>,
    pub company_location_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DeliveryOption {
    pub handle: String,
    pub title: String,
    pub price: i64,
    /// `SHIPPING`, `PICK_UP`, `LOCAL`, ...
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DeliveryGroup {
    pub id: String,
    pub line_ids: Vec<String>,
    pub options: Vec<DeliveryOption>,
    pub selected: Option<String>,
}

impl DeliveryGroup {
    pub fn selected_option(&self) -> Option<&DeliveryOption> {
        match &self.selected {
            Some(h) => self.options.iter().find(|o| &o.handle == h),
            None => self.options.first(),
        }
    }
}

/// The discount's configuration (`discount.metafield.jsonValue`) for one cart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConfigState {
    /// No metafield on the discount: `metafield` is `null`.
    Missing,
    /// A metafield whose jsonValue is this.
    Value(serde_json::Value),
}

/// How a cart's config differs from the configured one. Used by the
/// shrinker ("simplify the config only when the failure is about config")
/// and by sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConfigShape {
    Normal,
    Missing,
    Empty,
    Unexpected,
}

impl ConfigShape {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigShape::Normal => "normal",
            ConfigShape::Missing => "missing",
            ConfigShape::Empty => "empty",
            ConfigShape::Unexpected => "unexpected fields",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Cart {
    pub id: String,
    /// Presentment currency code.
    pub currency: String,
    /// Store currency to presentment currency, as a decimal string.
    pub rate: String,
    pub lines: Vec<Line>,
    /// Entered discount codes.
    pub codes: Vec<String>,
    pub buyer: Option<Buyer>,
    pub delivery: Vec<DeliveryGroup>,
    pub attributes: Vec<Attribute>,
    pub config: ConfigState,
    pub config_shape: ConfigShape,
    pub discount_classes: Vec<String>,
}

impl Cart {
    pub fn exponent(&self) -> u32 {
        money::exponent(&self.currency).unwrap_or(2)
    }

    pub fn rate(&self) -> Decimal {
        Decimal::from_str(&self.rate).unwrap_or(Decimal::ONE)
    }

    pub fn subtotal(&self) -> i64 {
        self.lines.iter().map(Line::subtotal).sum()
    }

    pub fn total_quantity(&self) -> i64 {
        self.lines.iter().map(|l| l.quantity).sum()
    }

    pub fn line(&self, id: &str) -> Option<&Line> {
        self.lines.iter().find(|l| l.id == id)
    }

    pub fn money(&self, minor: i64) -> String {
        money::format_display(minor, &self.currency)
    }

    /// The next free `gid://shopify/CartLine/N` id.
    pub fn next_line_id(&self) -> String {
        let max = self
            .lines
            .iter()
            .filter_map(|l| l.id.rsplit('/').next().and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        format!("gid://shopify/CartLine/{}", max + 1)
    }

    /// Keeps line references consistent after lines were removed: bundle
    /// parents that are gone, delivery groups that lost their lines.
    pub fn repair_references(&mut self) {
        let ids: Vec<String> = self.lines.iter().map(|l| l.id.clone()).collect();
        for line in &mut self.lines {
            if let Some(parent) = &line.parent_line_id
                && !ids.contains(parent)
            {
                line.parent_line_id = None;
            }
        }
        for group in &mut self.delivery {
            group.line_ids.retain(|id| ids.contains(id));
        }
        self.delivery.retain(|g| !g.line_ids.is_empty());
    }

    /// Short description for terminal and report: "3 lines, $120.00, USD".
    pub fn summary(&self) -> String {
        let n = self.lines.len();
        let noun = if n == 1 { "line" } else { "lines" };
        format!("{n} {noun}, {}", self.money(self.subtotal()))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn variant(id: u32, title: &str, tags: &[&str], gift_card: bool) -> Merchandise {
        Merchandise::Variant(VariantRef {
            variant_id: format!("gid://shopify/ProductVariant/{id}"),
            product_id: format!("gid://shopify/Product/{id}"),
            title: title.to_string(),
            variant_title: None,
            sku: None,
            handle: title.to_lowercase().replace(' ', "-"),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            collections: vec![],
            gift_card,
            requires_shipping: !gift_card,
            vendor: None,
            product_type: None,
        })
    }

    pub fn line(n: u32, merchandise: Merchandise, quantity: i64, unit_price: i64) -> Line {
        Line {
            id: format!("gid://shopify/CartLine/{n}"),
            quantity,
            unit_price,
            compare_at_price: None,
            merchandise,
            attributes: vec![],
            selling_plan: None,
            parent_line_id: None,
            existing_discounts: vec![],
        }
    }

    pub fn cart(lines: Vec<Line>) -> Cart {
        Cart {
            id: "cart-test".to_string(),
            currency: "USD".to_string(),
            rate: "1".to_string(),
            lines,
            codes: vec![],
            buyer: None,
            delivery: vec![],
            attributes: vec![],
            config: ConfigState::Value(serde_json::json!({})),
            config_shape: ConfigShape::Normal,
            discount_classes: vec!["ORDER".into(), "PRODUCT".into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn subtotals_and_ids() {
        let c = cart(vec![
            line(1, variant(1, "Linen Shirt", &[], false), 2, 4500),
            line(7, variant(2, "Gift Card", &[], true), 1, 2500),
        ]);
        assert_eq!(c.subtotal(), 11500);
        assert_eq!(c.next_line_id(), "gid://shopify/CartLine/8");
        assert_eq!(c.summary(), "2 lines, $115.00");
    }

    #[test]
    fn repairs_references_after_removal() {
        let mut parent = line(1, variant(1, "Kit", &[], false), 1, 1000);
        parent.id = "gid://shopify/CartLine/1".into();
        let mut child = line(2, variant(2, "Kit part", &[], false), 1, 0);
        child.parent_line_id = Some("gid://shopify/CartLine/1".into());
        let mut c = cart(vec![child]);
        c.delivery.push(DeliveryGroup {
            id: "gid://shopify/CartDeliveryGroup/1".into(),
            line_ids: vec!["gid://shopify/CartLine/1".into()],
            options: vec![],
            selected: None,
        });
        c.repair_references();
        assert_eq!(c.lines[0].parent_line_id, None);
        assert!(c.delivery.is_empty());
        let _ = parent;
    }

    #[test]
    fn cart_json_round_trips() {
        let c = cart(vec![line(1, variant(1, "Tote", &["bag"], false), 1, 10000)]);
        let s = serde_json::to_string(&c).unwrap_or_default();
        let back: Cart = serde_json::from_str(&s).unwrap_or_else(|_| cart(vec![]));
        assert_eq!(back, c);
    }
}
