//! `catalog.toml`: the synthetic products carts are built from.

use crate::cart::{CustomRef, Merchandise, VariantRef};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Minor units of the store currency.
    pub price: i64,
    #[serde(default)]
    pub compare_at_price: Option<i64>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default = "yes")]
    pub requires_shipping: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub collections: Vec<String>,
    #[serde(default)]
    pub gift_card: bool,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub product_type: Option<String>,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProduct {
    pub title: String,
    pub price: i64,
    #[serde(default)]
    pub gift_card: bool,
    #[serde(default)]
    pub requires_shipping: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default)]
    pub products: Vec<Product>,
    #[serde(default)]
    pub custom_products: Vec<CustomProduct>,
}

/// One thing a cart line can hold, flattened from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub merchandise: Merchandise,
    /// Minor units of the store currency.
    pub price: i64,
    pub compare_at_price: Option<i64>,
}

impl Item {
    pub fn is_gift_card(&self) -> bool {
        self.merchandise.is_gift_card()
    }
}

fn slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

impl Catalog {
    pub fn parse(text: &str, origin: &str) -> Result<Catalog> {
        let catalog: Catalog = toml::from_str(text).map_err(|e| {
            Error::bad_input(format!(
                "{origin} is not a valid catalog: {}",
                crate::util::toml_error_line(text, &e)
            ))
        })?;
        catalog.validate(origin)?;
        Ok(catalog)
    }

    pub fn validate(&self, origin: &str) -> Result<()> {
        if self.products.is_empty() && self.custom_products.is_empty() {
            return Err(Error::bad_input_hint(
                format!("{origin} has no products"),
                "add [[products]] with at least one [[products.variants]], or run promochaos init",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for p in &self.products {
            if p.variants.is_empty() {
                return Err(Error::bad_input(format!(
                    "{origin}: product {:?} has no variants",
                    p.title
                )));
            }
            for v in &p.variants {
                if v.price < 0 {
                    return Err(Error::bad_input(format!(
                        "{origin}: variant {} has a negative price",
                        v.id
                    )));
                }
                if !seen.insert(v.id.clone()) {
                    return Err(Error::bad_input(format!(
                        "{origin}: variant id {} is used twice",
                        v.id
                    )));
                }
            }
        }
        for c in &self.custom_products {
            if c.price < 0 {
                return Err(Error::bad_input(format!(
                    "{origin}: custom product {:?} has a negative price",
                    c.title
                )));
            }
        }
        Ok(())
    }

    /// Every variant and custom product, in catalog order.
    pub fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for p in &self.products {
            for v in &p.variants {
                items.push(Item {
                    merchandise: Merchandise::Variant(VariantRef {
                        variant_id: v.id.clone(),
                        product_id: p.id.clone(),
                        title: p.title.clone(),
                        variant_title: v.title.clone(),
                        sku: v.sku.clone(),
                        handle: p.handle.clone().unwrap_or_else(|| slug(&p.title)),
                        tags: p.tags.clone(),
                        collections: p.collections.clone(),
                        gift_card: p.gift_card,
                        requires_shipping: v.requires_shipping && !p.gift_card,
                        vendor: p.vendor.clone(),
                        product_type: p.product_type.clone(),
                    }),
                    price: v.price,
                    compare_at_price: v.compare_at_price,
                });
            }
        }
        for c in &self.custom_products {
            items.push(Item {
                merchandise: Merchandise::Custom(CustomRef {
                    title: c.title.clone(),
                    gift_card: c.gift_card,
                    requires_shipping: c.requires_shipping,
                }),
                price: c.price,
                compare_at_price: None,
            });
        }
        items
    }

    /// The starter catalog `promochaos init` writes. Ids are made up.
    pub fn starter_toml() -> &'static str {
        STARTER_CATALOG
    }
}

const STARTER_CATALOG: &str = r#"# promochaos catalog: synthetic products. Ids are made up.
# Prices are minor units of [store] currency in promochaos.toml (USD cents here).

[[products]]
id = "gid://shopify/Product/1001"
title = "Linen Shirt"
tags = ["multi-buy", "shirt"]
collections = ["gid://shopify/Collection/10"]

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2001"
  title = "M / Natural"
  price = 4500

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2002"
  title = "L / Navy"
  price = 4500

[[products]]
id = "gid://shopify/Product/1002"
title = "Canvas Tote"
tags = ["bag"]

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2003"
  title = "Natural"
  price = 10000

[[products]]
id = "gid://shopify/Product/1003"
title = "Enamel Mug"
tags = ["kitchen"]

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2004"
  price = 1000

[[products]]
id = "gid://shopify/Product/1004"
title = "Wool Throw"
tags = ["home"]

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2005"
  price = 12500

[[products]]
id = "gid://shopify/Product/1005"
title = "Gift Card"
tags = ["gift-card"]
gift_card = true

  [[products.variants]]
  id = "gid://shopify/ProductVariant/2006"
  title = "$25"
  price = 2500
  requires_shipping = false

[[custom_products]]
title = "Engraving"
price = 1000
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_catalog_parses() {
        let c = Catalog::parse(Catalog::starter_toml(), "starter");
        assert!(c.is_ok(), "{c:?}");
        let items = c.map(|c| c.items()).unwrap_or_default();
        assert_eq!(items.len(), 7);
        assert!(items.iter().any(Item::is_gift_card));
    }

    #[test]
    fn rejects_duplicate_variant_ids() {
        let text = r#"
[[products]]
id = "p1"
title = "A"
  [[products.variants]]
  id = "v1"
  price = 1
  [[products.variants]]
  id = "v1"
  price = 2
"#;
        assert!(Catalog::parse(text, "t").is_err());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Linen Shirt"), "linen-shirt");
        assert_eq!(slug("  $25 Gift!"), "25-gift");
    }
}
