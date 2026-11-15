//! `results.json`: every cart run and finding. Field order is stable and
//! lists are sorted, and wall-clock facts live in run.toml, so the same seed
//! and inputs give byte-identical files.

use crate::apply::Ledger;
use crate::cart::Cart;
use crate::eval::TargetInfo;
use crate::rules::{Detail, Fingerprint};
use crate::settings::Relation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESULTS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
    pub function_runner: Option<String>,
    pub api_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Counts {
    pub carts: usize,
    pub twins: usize,
    /// Every target run, shrinking included.
    pub runs: usize,
    pub shrink_runs: usize,
    pub determinism_runs: usize,
    pub order_runs: usize,
    /// Carts per generator, in generator order.
    pub by_generator: Vec<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CartRecord {
    pub id: String,
    pub generator: String,
    pub label: String,
    pub lines: usize,
    pub currency: String,
    pub subtotal: i64,
    pub instructions: Option<u64>,
    pub output_bytes: usize,
    pub crashed: bool,
    pub discount: Option<i64>,
    /// Rule ids that failed or warned on this cart, sorted.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct RuleStat {
    pub rule: String,
    pub from: String,
    pub checked: usize,
    pub failed: usize,
    pub warned: usize,
    pub not_applicable: usize,
    /// Why some were not checked, with counts, sorted by count then text.
    pub reasons: Vec<(String, usize)>,
}

impl RuleStat {
    /// "Config missing, empty or odd (6 carts); trapped first (1 cart)".
    /// The reason comes first and the count follows in brackets with its
    /// unit, so a reason never reads as part of a number ("6 this cart's
    /// config has no tiers").
    pub fn reasons_text(&self) -> String {
        let (one, many) = self.unit();
        self.reasons
            .iter()
            .enumerate()
            .map(|(i, (r, n))| {
                let r = if i == 0 { capitalise(r) } else { r.clone() };
                match *n {
                    0 => r,
                    1 => format!("{r} (1 {one})"),
                    n => format!("{r} ({n} {many})"),
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// What one check of this rule counts: a cart, a twin pair or an order
    /// sequence.
    fn unit(&self) -> (&'static str, &'static str) {
        match self.rule.as_str() {
            "split_line" | "reorder_lines" | "add_ineligible_line" | "more_spend_never_less" => {
                ("pair", "pairs")
            }
            "idempotent_retry" | "usage_limit" => ("sequence", "sequences"),
            _ => ("cart", "carts"),
        }
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// What an exported test asserts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// The run must succeed.
    NoTrap,
    /// The order discount must be this percentage, or absent when `None`.
    OrderPercent { percent: Option<String> },
    /// No order or product discount at all.
    NoDiscount,
    /// These lines must get no product discount and must be excluded from
    /// order discounts.
    LinesNotDiscounted { line_ids: Vec<String> },
    /// Only these codes may be accepted.
    CodesOnly { allowed: Vec<String> },
    /// The twin must get a discount whenever the original does.
    TwinKeepsDiscount,
    /// Not expressible as a native test (instruction counts, for example).
    NotExportable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct TwinRecord {
    pub relation: Relation,
    pub cart: Cart,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub discount: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CartRow {
    pub line: String,
    pub name: String,
    pub quantity: i64,
    pub subtotal: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct BudgetPoint {
    pub lines: usize,
    pub instructions: u64,
    pub limit: u64,
}

impl BudgetPoint {
    pub fn over(&self) -> bool {
        self.instructions > self.limit
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct FindingRecord {
    /// `F-001`, the folder name.
    pub id: String,
    /// `F1`, the label in the terminal and report.
    pub label: String,
    pub rule: String,
    /// `over_limit`, `crashed`, `rule_broken` or `warning`.
    pub kind: String,
    pub kind_label: String,
    /// `failure` or `warning`.
    pub severity: String,
    pub fingerprint: Fingerprint,
    pub sentence: String,
    /// Carts (or twin pairs) that showed it.
    pub count: usize,
    pub count_unit: String,
    /// For twin rules: how many pairs were checked.
    pub of_checked: Option<usize>,
    /// `shrunk to 178 lines`, `shrunk from 7 lines to 1`.
    pub shrunk_summary: String,
    pub flaky: bool,
    pub shrink_runs: usize,
    pub details: Vec<Detail>,
    pub cart: Cart,
    pub cart_rows: Vec<CartRow>,
    pub cart_rows_more: usize,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub output_pretty: String,
    pub logs_first_line: Option<String>,
    pub ledger: Option<Ledger>,
    pub expected_discount: Option<String>,
    pub twin: Option<TwinRecord>,
    pub likely_cause: Option<String>,
    pub curve: Vec<BudgetPoint>,
    pub expect: Expect,
    /// Paths relative to the results folder's parent, forward slashes.
    pub replay: String,
    pub test_path: Option<String>,
    pub fixture_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct RunResults {
    pub schema_version: u32,
    pub tool: ToolInfo,
    /// Wall-clock facts. They are kept in run.toml and left out of
    /// results.json and report.html, so the same seed and inputs give
    /// byte-identical files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub seed: u64,
    pub target: TargetInfo,
    /// "10% from $100.00, 20% from $200.00".
    pub config_summary: Option<String>,
    pub config_label: Option<String>,
    pub inputs_hash: String,
    pub counts: Counts,
    pub median_instructions: Option<u64>,
    pub budget: Vec<BudgetPoint>,
    pub rules: Vec<RuleStat>,
    pub findings: Vec<FindingRecord>,
    pub carts: Vec<CartRecord>,
    /// Always present: what promochaos does not simulate.
    pub not_checked: Vec<NotChecked>,
    /// Runner notes, such as the 20,000-byte output mismatch.
    pub notes: Vec<String>,
    pub results_dir: String,
    pub exit_code: i32,
    /// Any extra key/value facts (resolved settings file, catalog).
    pub inputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct NotChecked {
    pub title: String,
    pub text: String,
}

impl RunResults {
    pub fn failures(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == "failure")
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == "warning")
            .count()
    }
}

pub fn not_checked(is_command: bool) -> Vec<NotChecked> {
    if is_command {
        return vec![
            NotChecked {
                title: "Your production checkout".to_string(),
                text: "promochaos priced generated carts through the command you configured. Payment, inventory and the live database were not touched.".to_string(),
            },
            NotChecked {
                title: "Taxes and rates".to_string(),
                text: "Totals are checked to add up. Whether a tax or shipping rate is right for an address is not checked.".to_string(),
            },
            NotChecked {
                title: "Your live store".to_string(),
                text: "Carts come from catalog.toml. No store was contacted and no order was placed.".to_string(),
            },
        ];
    }
    vec![
        NotChecked {
            title: "Other discounts".to_string(),
            text: "Shopify decides how this discount combines with others from each discount's combination settings. That is not simulated.".to_string(),
        },
        NotChecked {
            title: "Checkout totals".to_string(),
            text: "Taxes, shipping rates and rounding at checkout are Shopify's. The ledgers show this function's output only.".to_string(),
        },
        NotChecked {
            title: "Your live store".to_string(),
            text: "Carts come from catalog.toml. No store was contacted and no order was placed.".to_string(),
        },
    ]
}

#[cfg(test)]
mod schema_tests {
    use super::RunResults;
    use std::path::PathBuf;

    fn schema_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("results-schema.json")
    }

    fn generated() -> String {
        let mut schema = schemars::schema_for!(RunResults);
        schema.insert(
            "$id".to_string(),
            "https://github.com/logitsolutionsai/promochaos/blob/main/docs/results-schema.json"
                .into(),
        );
        schema.insert(
            "description".to_string(),
            "results.json written by promochaos run. Generated from crates/promochaos-core/src/results.rs; do not edit by hand.".into(),
        );
        let mut text = serde_json::to_string_pretty(&schema).expect("schema serialises");
        text.push('\n');
        text
    }

    /// docs/results-schema.json is generated from the RunResults type. Run
    /// with PROMOCHAOS_UPDATE_SCHEMA=1 to rewrite it after changing a type.
    #[test]
    fn results_schema_matches_the_results_types() {
        let path = schema_path();
        let want = generated();
        if std::env::var("PROMOCHAOS_UPDATE_SCHEMA").is_ok_and(|v| v == "1") {
            std::fs::write(&path, &want).expect("write docs/results-schema.json");
            return;
        }
        let have = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            have == want,
            "docs/results-schema.json is out of date: run PROMOCHAOS_UPDATE_SCHEMA=1 cargo test -p promochaos-core results_schema"
        );
    }
}
