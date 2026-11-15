//! Renders a cart as a function's input JSON, following the input query:
//! only the fields the query selects, under the query's aliases, in query
//! order. Argument fields (`hasAnyTag(tags:)`, `inAnyCollection(ids:)`,
//! `attribute(key:)`, `metafield(namespace:, key:)`) are computed from the
//! cart model. Fields promochaos does not model get a value that fits their
//! schema type (null when nullable).

use crate::schema::named_type;
use apollo_compiler::ast::Value as GqlValue;
use apollo_compiler::executable::{Field, Selection, SelectionSet};
use apollo_compiler::schema::{ExtendedType, Type};
use apollo_compiler::{ExecutableDocument, Schema};
use promochaos_core::cart::{
    Buyer, BuyerKind, Cart, ConfigState, DeliveryGroup, DeliveryOption, ExistingDiscount, Line,
    Merchandise, SellingPlan,
};
use promochaos_core::money;
use serde_json::{Map, Value, json};

/// What a GraphQL object in the input is backed by.
#[derive(Clone)]
enum Src<'a> {
    Root,
    Cart,
    CartCost,
    Line(&'a Line),
    LineCost(&'a Line),
    Money(i64),
    Merch(&'a Line),
    Product(&'a Line),
    Attribute(&'a str, Option<&'a str>),
    Buyer(&'a Buyer),
    Customer(&'a Buyer),
    PurchasingCompany(&'a Buyer),
    Company(&'a Buyer),
    CompanyLocation(&'a Buyer),
    DeliveryGroup(&'a DeliveryGroup),
    DeliveryOption(&'a DeliveryOption),
    DiscountAllocation(&'a ExistingDiscount),
    SellingPlanAllocation(&'a SellingPlan),
    SellingPlan(&'a SellingPlan),
    ParentRelationship(&'a Line),
    Discount,
    Metafield(&'a Value),
    EnteredCode(&'a str),
    Localization,
    Country,
    Language,
    Market,
    HasTag(String, bool),
    Membership(String, bool),
    Null,
}

enum R<'a> {
    /// A scalar or enum value, already in JSON form.
    Scalar(Value),
    Object(Src<'a>),
    List(Vec<Src<'a>>),
    Null,
    /// Not modelled: a value that fits the schema type.
    Default,
}

pub struct Renderer<'a> {
    pub schema: &'a Schema,
    pub doc: &'a ExecutableDocument,
    pub cart: &'a Cart,
    /// Codes the discount is configured with, for `triggeringDiscountCode`.
    pub configured_codes: &'a [String],
}

fn is_non_null(ty: &Type) -> bool {
    matches!(ty, Type::NonNullNamed(_) | Type::NonNullList(_))
}

fn is_list(ty: &Type) -> bool {
    matches!(ty, Type::List(_) | Type::NonNullList(_))
}

fn country_for(currency: &str) -> &'static str {
    match currency {
        "JPY" => "JP",
        "KWD" => "KW",
        "BHD" => "BH",
        "EUR" => "DE",
        "GBP" => "GB",
        "CAD" => "CA",
        "AUD" => "AU",
        "KRW" => "KR",
        "INR" => "IN",
        "MXN" => "MX",
        _ => "US",
    }
}

/// Shopify sends decimals with at least one decimal place: `"1.0"`.
fn decimal_string(s: &str) -> String {
    if s.contains('.') {
        s.to_string()
    } else {
        format!("{s}.0")
    }
}

fn gql_strings(v: &GqlValue) -> Vec<String> {
    match v {
        GqlValue::String(s) => vec![s.clone()],
        GqlValue::Enum(n) => vec![n.as_str().to_string()],
        GqlValue::List(items) => items.iter().flat_map(|i| gql_strings(i)).collect(),
        _ => Vec::new(),
    }
}

impl<'a> Renderer<'a> {
    /// The rendered input for the document's (only) operation.
    pub fn render(&self) -> Value {
        let Ok(op) = self.doc.operations.get(None) else {
            return Value::Object(Map::new());
        };
        let root = op.selection_set.ty.as_str().to_string();
        self.render_object(&root, &Src::Root, &[&op.selection_set])
    }

    fn arg_strings(&self, field: &Field, name: &str) -> Option<Vec<String>> {
        if let Some(a) = field.arguments.iter().find(|a| a.name.as_str() == name) {
            return Some(gql_strings(&a.value));
        }
        field
            .definition
            .arguments
            .iter()
            .find(|a| a.name.as_str() == name)
            .and_then(|a| a.default_value.as_ref())
            .map(|v| gql_strings(v))
    }

    fn arg_string(&self, field: &Field, name: &str) -> Option<String> {
        self.arg_strings(field, name)
            .and_then(|v| v.into_iter().next())
    }

    fn applies(&self, condition: &str, concrete: &str) -> bool {
        if condition == concrete {
            return true;
        }
        match self.schema.types.get(condition) {
            Some(ExtendedType::Union(u)) => u.members.iter().any(|m| m.name.as_str() == concrete),
            Some(ExtendedType::Interface(_)) => self.schema.get_object(concrete).is_some_and(|o| {
                o.implements_interfaces
                    .iter()
                    .any(|i| i.name.as_str() == condition)
            }),
            _ => false,
        }
    }

    fn collect_into(
        &self,
        concrete: &str,
        set: &'a SelectionSet,
        out: &mut Vec<(String, Vec<&'a Field>)>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        for sel in &set.selections {
            match sel {
                Selection::Field(f) => {
                    let key = f.response_key().as_str().to_string();
                    let field: &'a Field = f;
                    match out.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, fields)) => fields.push(field),
                        None => out.push((key, vec![field])),
                    }
                }
                Selection::InlineFragment(inline) => {
                    let ok = match &inline.type_condition {
                        None => true,
                        Some(tc) => self.applies(tc.as_str(), concrete),
                    };
                    if ok {
                        self.collect_into(concrete, &inline.selection_set, out, depth + 1);
                    }
                }
                Selection::FragmentSpread(spread) => {
                    if let Some(frag) = self.doc.fragments.get(&spread.fragment_name)
                        && self.applies(frag.type_condition().as_str(), concrete)
                    {
                        self.collect_into(concrete, &frag.selection_set, out, depth + 1);
                    }
                }
            }
        }
    }

    fn render_object(&self, ty: &str, src: &Src<'a>, sets: &[&'a SelectionSet]) -> Value {
        let mut fields: Vec<(String, Vec<&'a Field>)> = Vec::new();
        for set in sets.iter().copied() {
            self.collect_into(ty, set, &mut fields, 0);
        }
        let mut map = Map::new();
        for (key, group) in fields {
            let Some(field) = group.first().copied() else {
                continue;
            };
            let value = if field.name.as_str() == "__typename" {
                Value::String(ty.to_string())
            } else {
                let sub: Vec<&'a SelectionSet> =
                    group.iter().copied().map(|f| &f.selection_set).collect();
                let r = self.resolve(ty, src, field);
                self.shape(&field.definition.ty, r, &sub)
            };
            map.insert(key, value);
        }
        Value::Object(map)
    }

    /// The concrete object type for a source when the field's type is a
    /// union or interface.
    fn concrete(&self, named: &str, src: &Src<'a>) -> String {
        match self.schema.types.get(named) {
            Some(ExtendedType::Union(u)) => {
                let want = match src {
                    Src::Merch(l) => match l.merchandise {
                        Merchandise::Variant(_) => "ProductVariant",
                        Merchandise::Custom(_) => "CustomProduct",
                    },
                    _ => "",
                };
                if u.members.iter().any(|m| m.name.as_str() == want) {
                    want.to_string()
                } else {
                    u.members
                        .iter()
                        .next()
                        .map(|m| m.name.as_str().to_string())
                        .unwrap_or_else(|| named.to_string())
                }
            }
            Some(ExtendedType::Interface(_)) => self
                .schema
                .types
                .iter()
                .find_map(|(n, t)| match t {
                    ExtendedType::Object(o)
                        if o.implements_interfaces
                            .iter()
                            .any(|i| i.name.as_str() == named) =>
                    {
                        Some(n.as_str().to_string())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| named.to_string()),
            _ => named.to_string(),
        }
    }

    fn shape(&self, ty: &Type, r: R<'a>, sets: &[&'a SelectionSet]) -> Value {
        match r {
            R::Scalar(v) => v,
            R::Null => {
                if is_non_null(ty) {
                    self.default_for(ty, sets)
                } else {
                    Value::Null
                }
            }
            R::Default => {
                if is_non_null(ty) {
                    self.default_for(ty, sets)
                } else {
                    Value::Null
                }
            }
            R::Object(src) => {
                let named = named_type(ty).as_str().to_string();
                let concrete = self.concrete(&named, &src);
                self.render_object(&concrete, &src, sets)
            }
            R::List(items) => {
                let named = named_type(ty).as_str().to_string();
                Value::Array(
                    items
                        .iter()
                        .map(|s| {
                            let concrete = self.concrete(&named, s);
                            self.render_object(&concrete, s, sets)
                        })
                        .collect(),
                )
            }
        }
    }

    /// A value that fits a non-null type the cart model does not cover.
    fn default_for(&self, ty: &Type, sets: &[&'a SelectionSet]) -> Value {
        if is_list(ty) {
            return Value::Array(Vec::new());
        }
        let named = named_type(ty).as_str();
        match self.schema.types.get(named) {
            Some(ExtendedType::Enum(e)) => e
                .values
                .keys()
                .next()
                .map(|k| Value::String(k.as_str().to_string()))
                .unwrap_or(Value::Null),
            Some(ExtendedType::Object(_))
            | Some(ExtendedType::Union(_))
            | Some(ExtendedType::Interface(_)) => {
                let concrete = self.concrete(named, &Src::Null);
                self.render_object(&concrete, &Src::Null, sets)
            }
            _ => match named {
                "Int" => json!(0),
                "Float" => json!(0.0),
                "Boolean" => json!(false),
                "Decimal" => json!("0.0"),
                "Date" => json!("2026-01-01"),
                "DateTime" => json!("2026-01-01T00:00:00Z"),
                "DateTimeWithoutTimezone" => json!("2026-01-01T00:00:00"),
                "TimeWithoutTimezone" => json!("00:00:00"),
                "JSON" => json!({}),
                "Void" => Value::Null,
                _ => json!(""),
            },
        }
    }

    fn money(&self, minor: i64) -> R<'a> {
        R::Object(Src::Money(minor))
    }

    fn resolve(&self, parent: &str, src: &Src<'a>, field: &Field) -> R<'a> {
        let cart: &'a Cart = self.cart;
        let name = field.name.as_str();
        let s = |v: &str| R::Scalar(Value::String(v.to_string()));
        let opt = |v: Option<&str>| match v {
            Some(x) => R::Scalar(Value::String(x.to_string())),
            None => R::Null,
        };
        match *src {
            Src::Root => match name {
                "cart" => R::Object(Src::Cart),
                "discount" => R::Object(Src::Discount),
                "enteredDiscountCodes" => R::List(
                    cart.codes
                        .iter()
                        .map(|c| Src::EnteredCode(c.as_str()))
                        .collect(),
                ),
                "localization" => R::Object(Src::Localization),
                "presentmentCurrencyRate" => R::Scalar(Value::String(decimal_string(&cart.rate))),
                "shop" => R::Default,
                "triggeringDiscountCode" => {
                    let code = cart.codes.iter().find(|c| {
                        self.configured_codes
                            .iter()
                            .any(|k| k.eq_ignore_ascii_case(c))
                    });
                    opt(code.map(String::as_str))
                }
                "fetchResult" => R::Null,
                _ => R::Default,
            },
            Src::Cart => match name {
                "lines" => R::List(cart.lines.iter().map(Src::Line).collect()),
                "cost" => R::Object(Src::CartCost),
                "buyerIdentity" => match &cart.buyer {
                    Some(b) => R::Object(Src::Buyer(b)),
                    None => R::Null,
                },
                "deliveryGroups" => R::List(cart.delivery.iter().map(Src::DeliveryGroup).collect()),
                "deliverableLines" => R::List(
                    cart.lines
                        .iter()
                        .filter(|l| l.merchandise.requires_shipping())
                        .map(Src::Line)
                        .collect(),
                ),
                "attribute" => {
                    let key = self.arg_string(field, "key").unwrap_or_default();
                    match cart.attributes.iter().find(|a| a.key == key) {
                        Some(a) => R::Object(Src::Attribute(&a.key, a.value.as_deref())),
                        None => R::Null,
                    }
                }
                "discountApplications" | "localizedFields" => R::List(Vec::new()),
                "metafield" => R::Null,
                _ => R::Default,
            },
            Src::CartCost => {
                let existing: i64 = cart
                    .lines
                    .iter()
                    .flat_map(|l| l.existing_discounts.iter())
                    .map(|d| d.amount)
                    .sum();
                match name {
                    "subtotalAmount" => self.money(cart.subtotal()),
                    "totalAmount" => self.money(cart.subtotal() - existing),
                    _ => R::Null,
                }
            }
            Src::Line(l) => match name {
                "id" => s(&l.id),
                "quantity" => R::Scalar(json!(l.quantity)),
                "cost" => R::Object(Src::LineCost(l)),
                "merchandise" => R::Object(Src::Merch(l)),
                "attribute" => {
                    let key = self.arg_string(field, "key").unwrap_or_default();
                    match l.attribute(&key) {
                        Some(a) => R::Object(Src::Attribute(&a.key, a.value.as_deref())),
                        None => R::Null,
                    }
                }
                "discountAllocations" => R::List(
                    l.existing_discounts
                        .iter()
                        .map(Src::DiscountAllocation)
                        .collect(),
                ),
                "sellingPlanAllocation" => match &l.selling_plan {
                    Some(p) => R::Object(Src::SellingPlanAllocation(p)),
                    None => R::Null,
                },
                "parentRelationship" => {
                    match l.parent_line_id.as_deref().and_then(|p| cart.line(p)) {
                        Some(parent) => R::Object(Src::ParentRelationship(parent)),
                        None => R::Null,
                    }
                }
                _ => R::Default,
            },
            Src::LineCost(l) => {
                let existing: i64 = l.existing_discounts.iter().map(|d| d.amount).sum();
                match name {
                    "amountPerQuantity" => self.money(l.unit_price),
                    "compareAtAmountPerQuantity" => match l.compare_at_price {
                        Some(p) => self.money(p),
                        None => R::Null,
                    },
                    "subtotalAmount" => self.money(l.subtotal()),
                    "totalAmount" => self.money(l.subtotal() - existing),
                    _ => R::Default,
                }
            }
            Src::Money(minor) => match name {
                "amount" => R::Scalar(Value::String(money::format_amount(minor, cart.exponent()))),
                "currencyCode" => s(&cart.currency),
                _ => R::Default,
            },
            Src::Merch(l) => match &l.merchandise {
                Merchandise::Variant(v) => match name {
                    "id" => s(&v.variant_id),
                    "sku" => opt(v.sku.as_deref()),
                    "title" => opt(v.variant_title.as_deref()),
                    "requiresShipping" => R::Scalar(json!(v.requires_shipping)),
                    "product" => R::Object(Src::Product(l)),
                    "inAnyCollection" => {
                        let ids = self.arg_strings(field, "ids").unwrap_or_default();
                        R::Scalar(json!(ids.iter().any(|i| v.collections.contains(i))))
                    }
                    "inCollections" => {
                        let ids = self.arg_strings(field, "ids").unwrap_or_default();
                        R::List(
                            ids.into_iter()
                                .map(|i| {
                                    let member = v.collections.contains(&i);
                                    Src::Membership(i, member)
                                })
                                .collect(),
                        )
                    }
                    "weightUnit" => s("KILOGRAMS"),
                    "metafield" | "weight" => R::Null,
                    _ => R::Default,
                },
                Merchandise::Custom(c) => match name {
                    "isGiftCard" => R::Scalar(json!(c.gift_card)),
                    "requiresShipping" => R::Scalar(json!(c.requires_shipping)),
                    "title" => s(&c.title),
                    "weightUnit" => s("KILOGRAMS"),
                    "weight" => R::Null,
                    _ => R::Default,
                },
            },
            Src::Product(l) => {
                let Merchandise::Variant(v) = &l.merchandise else {
                    return R::Default;
                };
                match name {
                    "id" => s(&v.product_id),
                    "handle" => s(&v.handle),
                    "title" => s(&v.title),
                    "vendor" => opt(v.vendor.as_deref()),
                    "productType" => opt(v.product_type.as_deref()),
                    "isGiftCard" => R::Scalar(json!(v.gift_card)),
                    "hasAnyTag" => {
                        let tags = self.arg_strings(field, "tags").unwrap_or_default();
                        R::Scalar(json!(tags.iter().any(|t| v.tags.contains(t))))
                    }
                    "hasTags" => {
                        let tags = self.arg_strings(field, "tags").unwrap_or_default();
                        R::List(
                            tags.into_iter()
                                .map(|t| {
                                    let has = v.tags.contains(&t);
                                    Src::HasTag(t, has)
                                })
                                .collect(),
                        )
                    }
                    "inAnyCollection" => {
                        let ids = self.arg_strings(field, "ids").unwrap_or_default();
                        R::Scalar(json!(ids.iter().any(|i| v.collections.contains(i))))
                    }
                    "inCollections" => {
                        let ids = self.arg_strings(field, "ids").unwrap_or_default();
                        R::List(
                            ids.into_iter()
                                .map(|i| {
                                    let member = v.collections.contains(&i);
                                    Src::Membership(i, member)
                                })
                                .collect(),
                        )
                    }
                    "metafield" => R::Null,
                    _ => R::Default,
                }
            }
            Src::Attribute(k, v) => match name {
                "key" => s(k),
                "value" => opt(v),
                _ => R::Default,
            },
            Src::Buyer(b) => match name {
                "customer" => match b.customer_id {
                    Some(_) => R::Object(Src::Customer(b)),
                    None => R::Null,
                },
                "email" => opt(b.email.as_deref()),
                "isAuthenticated" => R::Scalar(json!(b.customer_id.is_some())),
                "phone" | "shopUser" => R::Null,
                "purchasingCompany" => match b.kind {
                    BuyerKind::B2b => R::Object(Src::PurchasingCompany(b)),
                    BuyerKind::Consumer => R::Null,
                },
                _ => R::Default,
            },
            Src::Customer(b) => match name {
                "id" => opt(b.customer_id.as_deref()),
                "email" => opt(b.email.as_deref()),
                "displayName" => s("Test Customer"),
                "firstName" => s("Test"),
                "lastName" => s("Customer"),
                "numberOfOrders" => R::Scalar(json!(b.number_of_orders)),
                "amountSpent" => self.money(b.amount_spent),
                "hasAnyTag" => {
                    let tags = self.arg_strings(field, "tags").unwrap_or_default();
                    R::Scalar(json!(tags.iter().any(|t| b.tags.contains(t))))
                }
                "hasTags" => {
                    let tags = self.arg_strings(field, "tags").unwrap_or_default();
                    R::List(
                        tags.into_iter()
                            .map(|t| {
                                let has = b.tags.contains(&t);
                                Src::HasTag(t, has)
                            })
                            .collect(),
                    )
                }
                "metafield" => R::Null,
                _ => R::Default,
            },
            Src::PurchasingCompany(b) => match name {
                "company" => R::Object(Src::Company(b)),
                "location" => R::Object(Src::CompanyLocation(b)),
                "contact" => R::Null,
                _ => R::Default,
            },
            Src::Company(b) => match name {
                "id" => opt(b.company_id.as_deref()),
                "name" => s("Example Wholesale Ltd"),
                "createdAt" | "updatedAt" => s("2026-01-01T00:00:00Z"),
                "externalId" | "metafield" => R::Null,
                _ => R::Default,
            },
            Src::CompanyLocation(b) => match name {
                "id" => opt(b.company_location_id.as_deref()),
                "name" => s("Head office"),
                "createdAt" | "updatedAt" => s("2026-01-01T00:00:00Z"),
                "metafield" | "externalId" => R::Null,
                _ => R::Default,
            },
            Src::DeliveryGroup(g) => match name {
                "id" => s(&g.id),
                "cartLines" => R::List(
                    g.line_ids
                        .iter()
                        .filter_map(|id| cart.line(id))
                        .map(Src::Line)
                        .collect(),
                ),
                "deliveryOptions" => R::List(g.options.iter().map(Src::DeliveryOption).collect()),
                "selectedDeliveryOption" => match g.selected_option() {
                    Some(o) => R::Object(Src::DeliveryOption(o)),
                    None => R::Null,
                },
                "groupType" => s("ONE_TIME_PURCHASE"),
                "discountAllocations" => R::List(Vec::new()),
                "deliveryAddress" => R::Null,
                _ => R::Default,
            },
            Src::DeliveryOption(o) => match name {
                "handle" => s(&o.handle),
                "title" => s(&o.title),
                "code" => s(&o.handle),
                "cost" => self.money(o.price),
                "deliveryMethodType" => s(&o.method),
                "description" => R::Null,
                _ => R::Default,
            },
            Src::DiscountAllocation(d) => match name {
                "discountedAmount" => self.money(d.amount),
                _ => R::Default,
            },
            Src::SellingPlanAllocation(p) => match name {
                "sellingPlan" => R::Object(Src::SellingPlan(p)),
                "priceAdjustments" => R::List(Vec::new()),
                _ => R::Default,
            },
            Src::SellingPlan(p) => match name {
                "id" => s(&p.id),
                "name" => s(&p.name),
                "recurringDeliveries" => R::Scalar(json!(p.recurring_deliveries)),
                "description" | "metafield" => R::Null,
                _ => R::Default,
            },
            Src::ParentRelationship(parent) => match name {
                "parent" => R::Object(Src::Line(parent)),
                _ => R::Default,
            },
            Src::Discount => match name {
                "discountClasses" => R::Scalar(json!(cart.discount_classes)),
                "metafield" => match &cart.config {
                    ConfigState::Missing => R::Null,
                    ConfigState::Value(v) => R::Object(Src::Metafield(v)),
                },
                _ => R::Default,
            },
            Src::Metafield(v) => match name {
                "jsonValue" => R::Scalar(v.clone()),
                "type" => s("json"),
                "value" => R::Scalar(Value::String(serde_json::to_string(v).unwrap_or_default())),
                _ => R::Default,
            },
            Src::EnteredCode(c) => match name {
                "code" => s(c),
                "rejectable" => R::Scalar(json!(true)),
                _ => R::Default,
            },
            Src::Localization => match name {
                "country" => R::Object(Src::Country),
                "language" => R::Object(Src::Language),
                "market" => R::Object(Src::Market),
                _ => R::Default,
            },
            Src::Country => match name {
                "isoCode" => s(country_for(&cart.currency)),
                _ => R::Default,
            },
            Src::Language => match name {
                "isoCode" => s("EN"),
                _ => R::Default,
            },
            Src::Market => match name {
                "id" => s("gid://shopify/Market/1"),
                "handle" => s("primary"),
                "regions" => R::List(Vec::new()),
                "metafield" => R::Null,
                _ => R::Default,
            },
            Src::HasTag(ref t, has) => match name {
                "tag" => s(t),
                "hasTag" => R::Scalar(json!(has)),
                _ => R::Default,
            },
            Src::Membership(ref id, member) => match name {
                "collectionId" => s(id),
                "isMember" => R::Scalar(json!(member)),
                _ => R::Default,
            },
            Src::Null => {
                let _ = parent;
                R::Default
            }
        }
    }
}

/// The scale factor for this input: for each field with
/// `@scaleLimits(rate:)` that the query selects, `rate` times the list's
/// length, the largest of them, clamped to 1 to 10.
pub fn scale_factor(doc: &ExecutableDocument, input: &Value) -> f64 {
    fn walk(
        set: &SelectionSet,
        doc: &ExecutableDocument,
        values: &[&Value],
        best: &mut f64,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        for sel in &set.selections {
            match sel {
                Selection::Field(f) => {
                    let key = f.response_key().as_str();
                    let children: Vec<&Value> = values.iter().filter_map(|v| v.get(key)).collect();
                    if let Some(d) = f.definition.directives.get("scaleLimits") {
                        let rate = d
                            .arguments
                            .iter()
                            .find(|a| a.name.as_str() == "rate")
                            .and_then(|a| match &*a.value {
                                GqlValue::Float(x) => x.as_str().parse::<f64>().ok(),
                                GqlValue::Int(x) => x.as_str().parse::<f64>().ok(),
                                _ => None,
                            })
                            .unwrap_or(0.0);
                        for c in &children {
                            let len = match c {
                                Value::Array(a) => a.len(),
                                Value::String(s) => s.len(),
                                _ => 1,
                            };
                            let factor = rate * len as f64;
                            if factor > *best {
                                *best = factor;
                            }
                        }
                    }
                    let flat: Vec<&Value> = children
                        .iter()
                        .flat_map(|c| match c {
                            Value::Array(a) => a.iter().collect::<Vec<_>>(),
                            other => vec![*other],
                        })
                        .collect();
                    walk(&f.selection_set, doc, &flat, best, depth + 1);
                }
                Selection::InlineFragment(i) => {
                    walk(&i.selection_set, doc, values, best, depth + 1)
                }
                Selection::FragmentSpread(s) => {
                    if let Some(frag) = doc.fragments.get(&s.fragment_name) {
                        walk(&frag.selection_set, doc, values, best, depth + 1);
                    }
                }
            }
        }
    }
    let mut best = 1.0_f64;
    if let Ok(op) = doc.operations.get(None) {
        walk(&op.selection_set, doc, &[input], &mut best, 0);
    }
    best.clamp(1.0, 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_compiler::validation::Valid;
    use promochaos_core::cart::{Attribute, ConfigShape, VariantRef};

    const SCHEMA: &str = r#"
schema { query: Input }
directive @scaleLimits(rate: Float!) on FIELD_DEFINITION
scalar Decimal
scalar JSON
type Input { cart: Cart! discount: Discount! presentmentCurrencyRate: Decimal! }
type Cart { lines: [CartLine!]! @scaleLimits(rate: 0.005) }
type CartLine {
  id: ID!
  quantity: Int!
  cost: CartLineCost!
  merchandise: Merchandise!
  attribute(key: String): Attribute
}
type Attribute { key: String! value: String }
type CartLineCost { subtotalAmount: MoneyV2! }
type MoneyV2 { amount: Decimal! currencyCode: String! }
union Merchandise = CustomProduct | ProductVariant
type CustomProduct { isGiftCard: Boolean! title: String! }
type ProductVariant { id: ID! product: Product! }
type Product { isGiftCard: Boolean! hasAnyTag(tags: [String!]! = []): Boolean! vendor: String }
type Discount { discountClasses: [String!]! metafield(namespace: String, key: String!): Metafield }
type Metafield { jsonValue: JSON! }
"#;

    const QUERY: &str = r#"
query Input {
  presentmentCurrencyRate
  cart {
    lines {
      id
      qty: quantity
      cost { subtotalAmount { amount } }
      engraving: attribute(key: "engraving") { value }
      merchandise {
        __typename
        ... on ProductVariant { id product { isGiftCard multiBuy: hasAnyTag(tags: ["multi-buy"]) } }
        ... on CustomProduct { title }
      }
    }
  }
  discount { metafield(namespace: "$app", key: "function-configuration") { jsonValue } }
}
"#;

    fn cart() -> Cart {
        let variant = Merchandise::Variant(VariantRef {
            variant_id: "gid://shopify/ProductVariant/2001".into(),
            product_id: "gid://shopify/Product/1001".into(),
            title: "Linen Shirt".into(),
            variant_title: None,
            sku: None,
            handle: "linen-shirt".into(),
            tags: vec!["multi-buy".into()],
            collections: vec![],
            gift_card: false,
            requires_shipping: true,
            vendor: Some("Mill".into()),
            product_type: None,
        });
        Cart {
            id: "cart-1".into(),
            currency: "USD".into(),
            rate: "1".into(),
            lines: vec![Line {
                id: "gid://shopify/CartLine/1".into(),
                quantity: 2,
                unit_price: 4500,
                compare_at_price: None,
                merchandise: variant,
                attributes: vec![Attribute {
                    key: "engraving".into(),
                    value: Some("AB".into()),
                }],
                selling_plan: None,
                parent_line_id: None,
                existing_discounts: vec![],
            }],
            codes: vec![],
            buyer: None,
            delivery: vec![],
            attributes: vec![],
            config: ConfigState::Missing,
            config_shape: ConfigShape::Missing,
            discount_classes: vec!["ORDER".into()],
        }
    }

    fn parse() -> Option<(Valid<Schema>, Valid<ExecutableDocument>)> {
        let schema =
            crate::schema::parse_schema(SCHEMA, std::path::Path::new("schema.graphql")).ok()?;
        let doc = ExecutableDocument::parse_and_validate(&schema, QUERY, "query.graphql").ok()?;
        Some((schema, doc))
    }

    #[test]
    fn renders_only_queried_fields_with_aliases() {
        let Some((schema, doc)) = parse() else {
            panic!("test schema or query did not parse");
        };
        let c = cart();
        let r = Renderer {
            schema: &schema,
            doc: &doc,
            cart: &c,
            configured_codes: &[],
        };
        let v = r.render();
        let expected = json!({
            "presentmentCurrencyRate": "1.0",
            "cart": { "lines": [ {
                "id": "gid://shopify/CartLine/1",
                "qty": 2,
                "cost": { "subtotalAmount": { "amount": "90.00" } },
                "engraving": { "value": "AB" },
                "merchandise": {
                    "__typename": "ProductVariant",
                    "id": "gid://shopify/ProductVariant/2001",
                    "product": { "isGiftCard": false, "multiBuy": true }
                }
            } ] },
            "discount": { "metafield": null }
        });
        assert_eq!(v, expected);
        // Keys follow the query order.
        let keys: Vec<&String> = v
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        assert_eq!(keys, vec!["presentmentCurrencyRate", "cart", "discount"]);
        assert!(v.pointer("/cart/lines/0/quantity").is_none());
        assert!(
            v.pointer("/cart/lines/0/merchandise/product/vendor")
                .is_none()
        );
    }

    #[test]
    fn scale_factor_follows_line_count() {
        let Some((_schema, doc)) = parse() else {
            panic!("test schema or query did not parse");
        };
        let lines: Vec<Value> = (0..250).map(|_| json!({})).collect();
        let input = json!({ "cart": { "lines": lines } });
        assert!((scale_factor(&doc, &input) - 1.25).abs() < 1e-9);
        let input = json!({ "cart": { "lines": [ {} ] } });
        assert!((scale_factor(&doc, &input) - 1.0).abs() < 1e-9);
    }
}
