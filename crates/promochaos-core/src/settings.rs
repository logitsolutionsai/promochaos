//! `promochaos.toml`: raw settings as written, and the resolved form the run
//! uses (defaults applied, paths absolute, decimals parsed, tiers read from
//! the config through their JSON Pointer).

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::hard_limits::{MAX_CARTS, MAX_LINES};
use crate::money;
use crate::util::read_input_file;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    #[default]
    ShopifyFunction,
    Command,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerKind {
    #[default]
    InProcess,
    Subprocess,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShippingIn {
    #[default]
    Discounts,
    Shipping,
}

/// `run` is a run target name for Shopify functions and an argv for commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunSpec {
    One(String),
    Argv(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSettings {
    #[serde(default)]
    pub kind: TargetKind,
    pub extension: Option<String>,
    pub run: Option<RunSpec>,
    pub reset: Option<Vec<String>>,
    pub config: Option<String>,
    pub discount_classes: Option<Vec<String>>,
    pub wasm: Option<String>,
    pub runner: Option<RunnerKind>,
    pub function_runner: Option<String>,
    pub timeout_ms: Option<u64>,
    pub tax_inclusive: Option<bool>,
    pub shipping_discount_in: Option<ShippingIn>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSettings {
    pub currency: Option<String>,
    pub catalog: Option<String>,
    pub codes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeverDiscountSettings {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub gift_cards: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TiersPointer {
    /// JSON Pointer to the array of tiers in the config, e.g. `/tiers`.
    pub at: String,
    /// Key of the threshold in each tier object.
    pub threshold: String,
    /// Key of the percentage in each tier object.
    pub percent: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscountKind {
    Order,
    Product,
    Shipping,
}

impl DiscountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscountKind::Order => "order",
            DiscountKind::Product => "product",
            DiscountKind::Shipping => "shipping",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtotalBasis {
    #[default]
    BeforeDiscounts,
    AfterDiscounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinSubtotalTable {
    pub amount: String,
    pub kinds: Option<Vec<DiscountKind>>,
    pub basis: Option<SubtotalBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MinSubtotalSpec {
    Amount(String),
    Table(MinSubtotalTable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    SplitLine,
    ReorderLines,
    AddIneligibleLine,
    MoreSpendNeverLess,
}

impl Relation {
    pub fn id(self) -> &'static str {
        match self {
            Relation::SplitLine => "split_line",
            Relation::ReorderLines => "reorder_lines",
            Relation::AddIneligibleLine => "add_ineligible_line",
            Relation::MoreSpendNeverLess => "more_spend_never_less",
        }
    }

    pub fn all_default() -> Vec<Relation> {
        vec![
            Relation::SplitLine,
            Relation::ReorderLines,
            Relation::AddIneligibleLine,
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSettings {
    pub max_discount_percent: Option<String>,
    pub never_discount: Option<NeverDiscountSettings>,
    pub tiers: Option<TiersPointer>,
    pub min_subtotal: Option<MinSubtotalSpec>,
    pub metamorphic: Option<Vec<Relation>>,
    pub deterministic_sample: Option<usize>,
    pub usage_limit: Option<BTreeMap<String, u32>>,
    pub idempotent_retry: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateSettings {
    pub seed: Option<u64>,
    pub carts: Option<usize>,
    pub max_lines: Option<usize>,
    pub currencies: Option<Vec<String>>,
    pub twins: Option<usize>,
    pub rates: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSettings {
    pub rust_function: Option<String>,
    pub rust_schema: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub target: TargetSettings,
    #[serde(default)]
    pub store: StoreSettings,
    #[serde(default)]
    pub rules: RuleSettings,
    #[serde(default)]
    pub generate: GenerateSettings,
    #[serde(default)]
    pub export: ExportSettings,
}

impl Settings {
    pub fn parse(text: &str, origin: &str) -> Result<Settings> {
        toml::from_str(text).map_err(|e| {
            let hint =
                if e.message().contains("unknown field") || e.message().contains("missing field") {
                    "check the key names against docs/targets.md and docs/rules.md"
                } else {
                    "check the TOML syntax and value types at that line"
                };
            Error::bad_input_hint(
                format!(
                    "{origin} is not valid: {}",
                    crate::util::toml_error_line(text, &e)
                ),
                hint,
            )
        })
    }

    pub fn starter_toml(kind: TargetKind) -> &'static str {
        match kind {
            TargetKind::ShopifyFunction => STARTER_SHOPIFY,
            TargetKind::Command => STARTER_COMMAND,
        }
    }
}

/// Command-line values that replace settings.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub extension: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub wasm: Option<PathBuf>,
    pub seed: Option<u64>,
    pub carts: Option<usize>,
    pub runner: Option<RunnerKind>,
}

/// One tier from the config, in store-currency minor units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tier {
    pub threshold: i64,
    pub percent: String,
}

impl Tier {
    pub fn percent_decimal(&self) -> Decimal {
        money::parse_decimal(&self.percent).unwrap_or(Decimal::ZERO)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinSubtotal {
    /// Store-currency minor units.
    pub amount: i64,
    pub kinds: Vec<DiscountKind>,
    pub basis: SubtotalBasis,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeverDiscount {
    pub tags: Vec<String>,
    pub gift_cards: bool,
}

impl NeverDiscount {
    pub fn is_set(&self) -> bool {
        self.gift_cards || !self.tags.is_empty()
    }

    /// Why this merchandise must never be discounted, if it must not.
    pub fn reason(&self, m: &crate::cart::Merchandise) -> Option<String> {
        if self.gift_cards && m.is_gift_card() {
            return Some("is a gift card".to_string());
        }
        m.tags()
            .iter()
            .find(|t| self.tags.iter().any(|x| x == *t))
            .map(|t| format!("is tagged {t}"))
    }
}

/// Plain values come before nested tables, so the struct also serializes
/// as TOML (run.toml) with any serializer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_discount_percent: Option<String>,
    pub metamorphic: Vec<Relation>,
    pub deterministic_sample: usize,
    pub idempotent_retry: bool,
    pub never_discount: NeverDiscount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiers_pointer: Option<TiersPointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_subtotal: Option<MinSubtotal>,
    pub usage_limit: BTreeMap<String, u32>,
    pub tiers: Vec<Tier>,
}

impl Rules {
    pub fn max_percent(&self) -> Option<Decimal> {
        self.max_discount_percent
            .as_deref()
            .and_then(|s| money::parse_decimal(s).ok())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopifySpec {
    pub extension_dir: PathBuf,
    pub run_targets: Vec<String>,
    pub wasm: Option<PathBuf>,
    pub runner: RunnerKind,
    pub function_runner: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub run: Vec<String>,
    pub reset: Option<Vec<String>>,
    pub timeout_ms: u64,
    pub tax_inclusive: bool,
    pub shipping_discount_in: ShippingIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedTarget {
    Shopify(ShopifySpec),
    Command(CommandSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generate {
    pub seed: u64,
    pub carts: usize,
    pub max_lines: usize,
    pub currencies: Vec<String>,
    pub twins: usize,
    /// Store currency to presentment currency.
    pub rates: BTreeMap<String, String>,
}

impl Generate {
    pub fn rate(&self, code: &str) -> Decimal {
        self.rates
            .get(code)
            .and_then(|r| money::parse_decimal(r).ok())
            .unwrap_or(Decimal::ONE)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub base_dir: PathBuf,
    pub settings_path: Option<PathBuf>,
    pub target: ResolvedTarget,
    pub store_currency: String,
    pub catalog: Catalog,
    pub catalog_path: Option<PathBuf>,
    pub codes: Vec<String>,
    /// The configured discount config (metafield jsonValue), if any.
    pub config: Option<serde_json::Value>,
    pub config_path: Option<PathBuf>,
    pub discount_classes: Vec<String>,
    pub rules: Rules,
    pub generate: Generate,
    pub export: ExportSettings,
}

fn resolve_path(base: &Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Reads the value at a JSON Pointer.
fn pointer<'a>(v: &'a serde_json::Value, ptr: &str) -> Option<&'a serde_json::Value> {
    if ptr.is_empty() || ptr == "/" {
        return Some(v);
    }
    v.pointer(ptr)
}

/// Reads the tiers out of the config through the pointer. Thresholds can be
/// decimal strings or numbers; percentages too.
pub fn read_tiers(
    config: &serde_json::Value,
    ptr: &TiersPointer,
    store_exponent: u32,
) -> Result<Vec<Tier>> {
    let Some(arr) = pointer(config, &ptr.at).and_then(|v| v.as_array()) else {
        return Err(Error::bad_input(format!(
            "[rules] tiers points at {:?}, but the config has no array there",
            ptr.at
        )));
    };
    let mut tiers = Vec::new();
    for (i, t) in arr.iter().enumerate() {
        let threshold = t
            .get(&ptr.threshold)
            .and_then(money::decimal_from_json)
            .and_then(|d| money::decimal_to_minor_rounded(d, store_exponent))
            .ok_or_else(|| {
                Error::bad_input(format!(
                    "tier {i} in the config has no decimal {:?}",
                    ptr.threshold
                ))
            })?;
        let percent = t
            .get(&ptr.percent)
            .and_then(money::decimal_from_json)
            .ok_or_else(|| {
                Error::bad_input(format!(
                    "tier {i} in the config has no decimal {:?}",
                    ptr.percent
                ))
            })?;
        tiers.push(Tier {
            threshold,
            percent: percent.normalize().to_string(),
        });
    }
    tiers.sort_by_key(|t| t.threshold);
    Ok(tiers)
}

fn parse_percent(s: &str, what: &str) -> Result<Decimal> {
    let d = money::parse_decimal(s).map_err(|e| Error::bad_input(format!("{what}: {e}")))?;
    if d < Decimal::ZERO || d > Decimal::ONE_HUNDRED {
        return Err(Error::bad_input(format!(
            "{what} must be between 0 and 100, got {s}"
        )));
    }
    Ok(d)
}

impl Resolved {
    /// Resolves settings read from `settings_path` (or built-in defaults when
    /// `None`), applying command-line overrides.
    pub fn resolve(
        settings: Settings,
        settings_path: Option<&Path>,
        overrides: &Overrides,
    ) -> Result<Resolved> {
        let base_dir = settings_path
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let base_dir = if base_dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            base_dir
        };

        // Store
        let store_currency = settings
            .store
            .currency
            .clone()
            .unwrap_or_else(|| "USD".to_string());
        let Some(store_exp) = money::exponent(&store_currency) else {
            return Err(Error::bad_input(format!(
                "[store] currency {store_currency:?} is not a known ISO 4217 code"
            )));
        };
        let (catalog, catalog_path) = match &settings.store.catalog {
            Some(c) => {
                let path = resolve_path(&base_dir, c);
                let text = read_input_file(&path)?;
                (
                    Catalog::parse(&text, &path.display().to_string())?,
                    Some(path),
                )
            }
            None => (
                Catalog::parse(Catalog::starter_toml(), "the starter catalog")?,
                None,
            ),
        };
        let codes = settings.store.codes.clone().unwrap_or_default();

        // Config
        let config_path = match (&overrides.config, &settings.target.config) {
            (Some(p), _) => Some(p.clone()),
            (None, Some(c)) => Some(resolve_path(&base_dir, c)),
            (None, None) => None,
        };
        let config = match &config_path {
            Some(p) => {
                let text = read_input_file(p)?;
                Some(
                    serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
                        Error::bad_input(format!("{} is not valid JSON: {e}", p.display()))
                    })?,
                )
            }
            None => None,
        };

        // Target
        let target = match settings.target.kind {
            TargetKind::ShopifyFunction => {
                let extension_dir = match (&overrides.extension, &settings.target.extension) {
                    (Some(p), _) => p.clone(),
                    (None, Some(e)) => resolve_path(&base_dir, e),
                    (None, None) => {
                        return Err(Error::bad_input_hint(
                            "no function to test",
                            "pass the extension folder (promochaos run extensions/my-discount) or set [target] extension",
                        ));
                    }
                };
                let run_targets = match &settings.target.run {
                    None => Vec::new(),
                    Some(RunSpec::One(s)) => vec![s.clone()],
                    Some(RunSpec::Argv(v)) => v.clone(),
                };
                let wasm = match (&overrides.wasm, &settings.target.wasm) {
                    (Some(p), _) => Some(p.clone()),
                    (None, Some(w)) => Some(resolve_path(&extension_dir, w)),
                    (None, None) => None,
                };
                ResolvedTarget::Shopify(ShopifySpec {
                    extension_dir,
                    run_targets,
                    wasm,
                    runner: overrides
                        .runner
                        .or(settings.target.runner)
                        .unwrap_or_default(),
                    function_runner: settings
                        .target
                        .function_runner
                        .clone()
                        .unwrap_or_else(|| "function-runner".to_string()),
                    timeout_ms: settings.target.timeout_ms.unwrap_or(10_000),
                })
            }
            TargetKind::Command => {
                let run = match &settings.target.run {
                    Some(RunSpec::Argv(v)) if !v.is_empty() => v.clone(),
                    Some(RunSpec::One(s)) if !s.is_empty() => vec![s.clone()],
                    _ => {
                        return Err(Error::bad_input_hint(
                            "[target] kind = \"command\" needs run = [\"program\", \"arg\"]",
                            "the program reads a cart on stdin and prints a priced cart; see docs/targets.md",
                        ));
                    }
                };
                ResolvedTarget::Command(CommandSpec {
                    run,
                    reset: settings.target.reset.clone(),
                    timeout_ms: settings.target.timeout_ms.unwrap_or(2_000),
                    tax_inclusive: settings.target.tax_inclusive.unwrap_or(false),
                    shipping_discount_in: settings.target.shipping_discount_in.unwrap_or_default(),
                })
            }
        };

        let discount_classes = settings.target.discount_classes.clone().unwrap_or_else(|| {
            vec![
                "ORDER".to_string(),
                "PRODUCT".to_string(),
                "SHIPPING".to_string(),
            ]
        });
        for c in &discount_classes {
            if !matches!(c.as_str(), "ORDER" | "PRODUCT" | "SHIPPING") {
                return Err(Error::bad_input(format!(
                    "[target] discount_classes: {c:?} is not ORDER, PRODUCT or SHIPPING"
                )));
            }
        }

        // Rules
        let r = &settings.rules;
        if let Some(p) = &r.max_discount_percent {
            parse_percent(p, "[rules] max_discount_percent")?;
        }
        let tiers = match (&r.tiers, &config) {
            (Some(ptr), Some(cfg)) => read_tiers(cfg, ptr, store_exp)?,
            (Some(_), None) => {
                return Err(Error::bad_input_hint(
                    "[rules] tiers reads the config, but there is no config",
                    "set [target] config or pass --config",
                ));
            }
            (None, _) => Vec::new(),
        };
        let min_subtotal = match &r.min_subtotal {
            None => None,
            Some(MinSubtotalSpec::Amount(a)) => Some(MinSubtotal {
                amount: money::parse_minor(a, store_exp)
                    .map_err(|e| Error::bad_input(format!("[rules] min_subtotal: {e}")))?,
                kinds: vec![
                    DiscountKind::Order,
                    DiscountKind::Product,
                    DiscountKind::Shipping,
                ],
                basis: SubtotalBasis::BeforeDiscounts,
            }),
            Some(MinSubtotalSpec::Table(t)) => Some(MinSubtotal {
                amount: money::parse_minor(&t.amount, store_exp)
                    .map_err(|e| Error::bad_input(format!("[rules] min_subtotal: {e}")))?,
                kinds: t.kinds.clone().unwrap_or_else(|| {
                    vec![
                        DiscountKind::Order,
                        DiscountKind::Product,
                        DiscountKind::Shipping,
                    ]
                }),
                basis: t.basis.unwrap_or_default(),
            }),
        };
        let is_command = matches!(target, ResolvedTarget::Command(_));
        let rules = Rules {
            max_discount_percent: r.max_discount_percent.clone(),
            never_discount: r
                .never_discount
                .as_ref()
                .map(|n| NeverDiscount {
                    tags: n.tags.clone(),
                    gift_cards: n.gift_cards,
                })
                .unwrap_or_default(),
            tiers,
            tiers_pointer: r.tiers.clone(),
            min_subtotal,
            metamorphic: r.metamorphic.clone().unwrap_or_else(Relation::all_default),
            deterministic_sample: r.deterministic_sample.unwrap_or(50),
            usage_limit: r.usage_limit.clone().unwrap_or_default(),
            idempotent_retry: r.idempotent_retry.unwrap_or(is_command),
        };

        // Generate
        let g = &settings.generate;
        let carts = overrides.carts.or(g.carts).unwrap_or(500);
        if carts == 0 || carts > MAX_CARTS {
            return Err(Error::bad_input(format!(
                "carts must be between 1 and {MAX_CARTS}, got {carts}"
            )));
        }
        let max_lines = g.max_lines.unwrap_or(250);
        if max_lines == 0 || max_lines > MAX_LINES {
            return Err(Error::bad_input(format!(
                "[generate] max_lines must be between 1 and {MAX_LINES}, got {max_lines}"
            )));
        }
        let currencies = g
            .currencies
            .clone()
            .unwrap_or_else(|| vec![store_currency.clone()]);
        let mut rates = BTreeMap::new();
        for c in &currencies {
            if money::exponent(c).is_none() {
                return Err(Error::bad_input(format!(
                    "[generate] currencies: {c:?} is not a known ISO 4217 code"
                )));
            }
            let rate = match g.rates.as_ref().and_then(|r| r.get(c)) {
                Some(s) => money::parse_decimal(s)
                    .map_err(|e| Error::bad_input(format!("[generate] rates.{c}: {e}")))?,
                None => money::default_rate(&store_currency, c),
            };
            if rate <= Decimal::ZERO {
                return Err(Error::bad_input(format!(
                    "[generate] rates.{c} must be above zero"
                )));
            }
            rates.insert(c.clone(), rate.normalize().to_string());
        }
        let generate = Generate {
            seed: overrides.seed.or(g.seed).unwrap_or(42),
            carts,
            max_lines,
            currencies,
            twins: g.twins.unwrap_or(60),
            rates,
        };

        Ok(Resolved {
            base_dir,
            settings_path: settings_path.map(Path::to_path_buf),
            target,
            store_currency,
            catalog,
            catalog_path,
            codes,
            config,
            config_path,
            discount_classes,
            rules,
            generate,
            export: settings.export.clone(),
        })
    }

    /// Reads and resolves a settings file.
    pub fn load(path: &Path, overrides: &Overrides) -> Result<Resolved> {
        let text = read_input_file(path)?;
        let settings = Settings::parse(&text, &path.display().to_string())?;
        Resolved::resolve(settings, Some(path), overrides)
    }

    pub fn store_exponent(&self) -> u32 {
        money::exponent(&self.store_currency).unwrap_or(2)
    }

    pub fn is_command(&self) -> bool {
        matches!(self.target, ResolvedTarget::Command(_))
    }

    /// The thresholds boundary carts are built around, in store minor units.
    pub fn thresholds(&self) -> Vec<i64> {
        let mut t: Vec<i64> = self.rules.tiers.iter().map(|t| t.threshold).collect();
        if let Some(m) = &self.rules.min_subtotal {
            t.push(m.amount);
        }
        t.sort_unstable();
        t.dedup();
        t
    }
}

const STARTER_SHOPIFY: &str = r#"# promochaos settings. See docs/targets.md and docs/rules.md.

[target]
kind = "shopify-function"
extension = "extensions/my-discount"        # folder with shopify.extension.toml
# run = "cart.lines.discounts.generate.run" # default: every discount run target
config = "config/discount.json"             # the discount metafield's jsonValue
discount_classes = ["ORDER", "PRODUCT"]

[store]
currency = "USD"
catalog = "catalog.toml"
codes = []

[rules]
max_discount_percent = "30"
never_discount = { tags = ["gift-card"], gift_cards = true }
# tiers = { at = "/tiers", threshold = "threshold", percent = "percentage" }
# min_subtotal = "50.00"
metamorphic = ["split_line", "reorder_lines", "add_ineligible_line"]

[generate]
seed = 42
carts = 500
max_lines = 250
currencies = ["USD", "JPY", "KWD"]
"#;

const STARTER_COMMAND: &str = r#"# promochaos settings for a homegrown cart engine. See docs/targets.md.

[target]
kind = "command"
run = ["./target/debug/my_engine"]          # cart JSON on stdin, priced cart JSON on stdout
# reset = ["./target/debug/my_engine", "--reset"]
timeout_ms = 2000
tax_inclusive = false
shipping_discount_in = "discounts"

[store]
currency = "USD"
catalog = "catalog.toml"
codes = ["WELCOME20"]

[rules]
max_discount_percent = "30"
never_discount = { tags = ["gift-card"], gift_cards = true }
# usage_limit = { WELCOME20 = 3 }

[generate]
seed = 42
carts = 300
currencies = ["USD"]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_settings_parse() {
        assert!(Settings::parse(STARTER_SHOPIFY, "starter").is_ok());
        assert!(Settings::parse(STARTER_COMMAND, "starter").is_ok());
    }

    #[test]
    fn unknown_keys_are_bad_input() {
        let e = Settings::parse("[rules]\nmax_discount = \"30\"\n", "t");
        assert!(matches!(e, Err(Error::BadInput { .. })));
    }

    #[test]
    fn min_subtotal_has_two_forms() {
        let a = Settings::parse("[rules]\nmin_subtotal = \"50.00\"\n", "t");
        assert!(matches!(
            a.map(|s| s.rules.min_subtotal),
            Ok(Some(MinSubtotalSpec::Amount(_)))
        ));
        let b = Settings::parse(
            "[rules]\nmin_subtotal = { amount = \"100.00\", kinds = [\"shipping\"], basis = \"after_discounts\" }\n",
            "t",
        );
        assert!(matches!(
            b.map(|s| s.rules.min_subtotal),
            Ok(Some(MinSubtotalSpec::Table(_)))
        ));
    }

    #[test]
    fn reads_tiers_through_the_pointer() {
        let cfg = serde_json::json!({
            "tiers": [
                { "threshold": "200.00", "percentage": "20" },
                { "threshold": 100, "percentage": 10 }
            ]
        });
        let ptr = TiersPointer {
            at: "/tiers".into(),
            threshold: "threshold".into(),
            percent: "percentage".into(),
        };
        let tiers = read_tiers(&cfg, &ptr, 2).unwrap_or_default();
        assert_eq!(
            tiers,
            vec![
                Tier {
                    threshold: 10000,
                    percent: "10".into()
                },
                Tier {
                    threshold: 20000,
                    percent: "20".into()
                },
            ]
        );
    }

    #[test]
    fn command_settings_resolve() {
        let s = Settings::parse(STARTER_COMMAND, "t").unwrap_or_default();
        let r = Resolved::resolve(s, None, &Overrides::default());
        // The starter refers to catalog.toml, which does not exist here.
        assert!(matches!(r, Err(Error::Read { .. })));
    }
}
