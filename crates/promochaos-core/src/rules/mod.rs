//! Rules: named checks with template sentences (spec section 8).
//!
//! Users choose rules from this fixed set and give them values. There is no
//! expression language and no `eval`. Sentences come from templates; no
//! model writes text.

pub mod command;
pub mod limits;
pub mod merchant;
pub mod metamorphic;
pub mod output;

use crate::cart::Cart;
use crate::eval::Evaluation;
use crate::settings::{Resolved, ResolvedTarget, ShippingIn};
use crate::util::first_line;
use serde::{Deserialize, Serialize};

pub const NO_TRAP: &str = "no_trap";
pub const WITHIN_INSTRUCTION_LIMIT: &str = "within_instruction_limit";
pub const WITHIN_OUTPUT_LIMIT: &str = "within_output_limit";
pub const WITHIN_MEMORY_LIMIT: &str = "within_memory_limit";
pub const LOGS_FIT: &str = "logs_fit";
pub const VALID_OUTPUT: &str = "valid_output";
pub const DETERMINISTIC: &str = "deterministic";
pub const MAX_DISCOUNT_PERCENT: &str = "max_discount_percent";
pub const NEVER_DISCOUNT: &str = "never_discount";
pub const TIERS: &str = "tiers";
pub const MIN_SUBTOTAL: &str = "min_subtotal";
pub const CODES_ACCEPTED: &str = "codes_accepted";
pub const TOTALS_RECONCILE: &str = "totals_reconcile";
pub const IDEMPOTENT_RETRY: &str = "idempotent_retry";
pub const USAGE_LIMIT: &str = "usage_limit";
pub const SPLIT_LINE: &str = "split_line";
pub const REORDER_LINES: &str = "reorder_lines";
pub const ADD_INELIGIBLE_LINE: &str = "add_ineligible_line";
pub const MORE_SPEND_NEVER_LESS: &str = "more_spend_never_less";

/// Display order of the rules table.
pub const RULE_ORDER: &[&str] = &[
    NO_TRAP,
    WITHIN_INSTRUCTION_LIMIT,
    WITHIN_OUTPUT_LIMIT,
    WITHIN_MEMORY_LIMIT,
    LOGS_FIT,
    VALID_OUTPUT,
    TOTALS_RECONCILE,
    TIERS,
    NEVER_DISCOUNT,
    MAX_DISCOUNT_PERCENT,
    MIN_SUBTOTAL,
    CODES_ACCEPTED,
    SPLIT_LINE,
    REORDER_LINES,
    ADD_INELIGIBLE_LINE,
    MORE_SPEND_NEVER_LESS,
    DETERMINISTIC,
    IDEMPOTENT_RETRY,
    USAGE_LIMIT,
];

/// Where a rule comes from, for the rules table.
pub fn rule_source(rule: &str) -> &'static str {
    match rule {
        NO_TRAP | DETERMINISTIC | TOTALS_RECONCILE => "Built in",
        WITHIN_INSTRUCTION_LIMIT | WITHIN_OUTPUT_LIMIT | WITHIN_MEMORY_LIMIT | LOGS_FIT => {
            "Shopify limits"
        }
        VALID_OUTPUT => "schema.graphql",
        SPLIT_LINE | REORDER_LINES | ADD_INELIGIBLE_LINE | MORE_SPEND_NEVER_LESS => "Twin carts",
        _ => "promochaos.toml",
    }
}

pub fn rule_index(rule: &str) -> usize {
    RULE_ORDER
        .iter()
        .position(|r| *r == rule)
        .unwrap_or(RULE_ORDER.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    NotApplicable,
}

/// The outcome label from the Report Kit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    OverLimit,
    Crashed,
    RuleBroken,
    Warning,
}

impl OutcomeKind {
    pub fn label(self) -> &'static str {
        match self {
            OutcomeKind::OverLimit => "Over limit",
            OutcomeKind::Crashed => "Crashed",
            OutcomeKind::RuleBroken => "Rule broken",
            OutcomeKind::Warning => "Warning",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeKind::OverLimit => "over_limit",
            OutcomeKind::Crashed => "crashed",
            OutcomeKind::RuleBroken => "rule_broken",
            OutcomeKind::Warning => "warning",
        }
    }
}

/// `(rule id, outcome kind, trap message first line or limit name)`.
/// Shrinking keeps it; findings with the same one collapse into one entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Fingerprint {
    pub rule: String,
    pub kind: OutcomeKind,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Detail {
    pub label: String,
    pub value: String,
}

impl Detail {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Detail {
        Detail {
            label: label.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Check {
    pub rule: String,
    pub status: Status,
    pub kind: OutcomeKind,
    pub key: String,
    pub sentence: String,
    pub details: Vec<Detail>,
    /// Why the rule was not checked.
    pub reason: Option<String>,
    /// What the config says the discount should have been, for the ledger.
    pub expected_discount: Option<i64>,
}

impl Check {
    pub fn pass(rule: &str) -> Check {
        Check {
            rule: rule.to_string(),
            status: Status::Pass,
            kind: OutcomeKind::RuleBroken,
            key: String::new(),
            sentence: String::new(),
            details: Vec::new(),
            reason: None,
            expected_discount: None,
        }
    }

    pub fn not_applicable(rule: &str, reason: impl Into<String>) -> Check {
        Check {
            status: Status::NotApplicable,
            reason: Some(reason.into()),
            ..Check::pass(rule)
        }
    }

    pub fn fail(
        rule: &str,
        kind: OutcomeKind,
        key: impl Into<String>,
        sentence: impl Into<String>,
    ) -> Check {
        Check {
            status: Status::Fail,
            kind,
            key: key.into(),
            sentence: sentence.into(),
            ..Check::pass(rule)
        }
    }

    pub fn warn(rule: &str, key: impl Into<String>, sentence: impl Into<String>) -> Check {
        Check {
            status: Status::Warn,
            kind: OutcomeKind::Warning,
            key: key.into(),
            sentence: sentence.into(),
            ..Check::pass(rule)
        }
    }

    pub fn with_detail(mut self, label: impl Into<String>, value: impl Into<String>) -> Check {
        self.details.push(Detail::new(label, value));
        self
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            rule: self.rule.clone(),
            kind: self.kind,
            key: self.key.clone(),
        }
    }

    pub fn is_problem(&self) -> bool {
        matches!(self.status, Status::Fail | Status::Warn)
    }
}

/// What the rules need to know about the run.
#[derive(Debug, Clone)]
pub struct RuleContext<'a> {
    pub resolved: &'a Resolved,
    pub is_command: bool,
    pub tax_inclusive: bool,
    pub shipping_in: ShippingIn,
}

impl<'a> RuleContext<'a> {
    pub fn new(resolved: &'a Resolved) -> RuleContext<'a> {
        let (is_command, tax_inclusive, shipping_in) = match &resolved.target {
            ResolvedTarget::Command(c) => (true, c.tax_inclusive, c.shipping_discount_in),
            ResolvedTarget::Shopify(_) => (false, false, ShippingIn::Discounts),
        };
        RuleContext {
            resolved,
            is_command,
            tax_inclusive,
            shipping_in,
        }
    }

    /// The single-cart rules this run checks, in table order.
    pub fn cart_rules(&self) -> Vec<&'static str> {
        let r = &self.resolved.rules;
        let mut rules = vec![NO_TRAP];
        if !self.is_command {
            rules.extend([
                WITHIN_INSTRUCTION_LIMIT,
                WITHIN_OUTPUT_LIMIT,
                WITHIN_MEMORY_LIMIT,
                LOGS_FIT,
            ]);
        }
        rules.push(VALID_OUTPUT);
        if self.is_command {
            rules.push(TOTALS_RECONCILE);
        }
        if !r.tiers.is_empty() {
            rules.push(TIERS);
        }
        if r.never_discount.is_set() {
            rules.push(NEVER_DISCOUNT);
        }
        if r.max_discount_percent.is_some() {
            rules.push(MAX_DISCOUNT_PERCENT);
        }
        if r.min_subtotal.is_some() {
            rules.push(MIN_SUBTOTAL);
        }
        rules.push(CODES_ACCEPTED);
        rules
    }
}

/// A short, stable form of a crash message for the fingerprint: the first
/// line, with numbers that change between carts (addresses, sizes) kept, as
/// they are usually stable for one bug.
pub fn crash_key(message: &str) -> String {
    first_line(message)
}

/// Runs every single-cart rule on one evaluation.
pub fn check_cart(ctx: &RuleContext, cart: &Cart, eval: &Evaluation) -> Vec<Check> {
    let mut checks = Vec::new();
    let crashed = eval.crash.is_some();
    // A Shopify Function still running at promochaos's wall-clock timeout
    // is an endless or very long loop. Shopify stops it at the instruction
    // limit, so it is an instruction-limit failure, not a trap.
    let timed_out = crashed && eval.timed_out && !ctx.is_command;

    match &eval.crash {
        Some(_) if timed_out => checks.push(Check::not_applicable(
            NO_TRAP,
            "still running at the timeout",
        )),
        Some(msg) => checks.push(no_trap_failure(ctx, cart, msg)),
        None => checks.push(Check::pass(NO_TRAP)),
    }

    let mut discarded = crashed;
    if !ctx.is_command {
        if timed_out {
            checks.push(limits::timed_out(cart, eval));
            for r in [WITHIN_OUTPUT_LIMIT, WITHIN_MEMORY_LIMIT, LOGS_FIT] {
                checks.push(Check::not_applicable(r, "still running at the timeout"));
            }
        } else if crashed {
            for r in [
                WITHIN_INSTRUCTION_LIMIT,
                WITHIN_OUTPUT_LIMIT,
                WITHIN_MEMORY_LIMIT,
                LOGS_FIT,
            ] {
                checks.push(Check::not_applicable(r, "trapped first"));
            }
        } else {
            let limit_checks = limits::check(cart, eval);
            if limit_checks
                .iter()
                .any(|c| c.status == Status::Fail && c.kind == OutcomeKind::OverLimit)
            {
                discarded = true;
            }
            checks.extend(limit_checks);
        }
    }

    let no_output = discarded || eval.output.is_none();
    let na_reason = if crashed {
        "produced no output"
    } else if discarded {
        "output discarded, over a limit"
    } else {
        "produced no output"
    };

    // Output that could not be read at all (not JSON) is a valid_output
    // failure, not "no output".
    if discarded || (eval.output.is_none() && eval.schema_errors.is_empty()) {
        checks.push(Check::not_applicable(VALID_OUTPUT, na_reason));
    } else {
        checks.push(output::check(ctx, cart, eval));
    }

    if ctx.is_command {
        if no_output {
            checks.push(Check::not_applicable(TOTALS_RECONCILE, na_reason));
        } else {
            checks.push(command::totals_reconcile(ctx, cart, eval));
        }
    }

    let merchant_rules: Vec<&'static str> = ctx
        .cart_rules()
        .into_iter()
        .filter(|r| {
            matches!(
                *r,
                TIERS | NEVER_DISCOUNT | MAX_DISCOUNT_PERCENT | MIN_SUBTOTAL | CODES_ACCEPTED
            )
        })
        .collect();
    match (&eval.ledger, no_output) {
        (Some(ledger), false) => checks.extend(merchant::check(ctx, cart, ledger, &merchant_rules)),
        _ => {
            for r in merchant_rules {
                checks.push(Check::not_applicable(r, na_reason));
            }
        }
    }
    checks
}

fn no_trap_failure(ctx: &RuleContext, cart: &Cart, msg: &str) -> Check {
    use crate::cart::ConfigShape;
    let line = first_line(msg);
    let sentence = if ctx.is_command {
        format!("The engine failed on this cart: {line}.")
    } else {
        match cart.config_shape {
            ConfigShape::Missing => "With no configuration on the discount, the function panicked, so Shopify discards its output.".to_string(),
            ConfigShape::Empty => "With an empty configuration on the discount, the function panicked, so Shopify discards its output.".to_string(),
            ConfigShape::Unexpected => "With unexpected fields in the discount configuration, the function panicked, so Shopify discards its output.".to_string(),
            ConfigShape::Normal => "On this cart the function trapped, so Shopify discards its output.".to_string(),
        }
    };
    let mut c = Check::fail(NO_TRAP, OutcomeKind::Crashed, crash_key(msg), sentence)
        .with_detail("Log, first line", line);
    let when = match (ctx.is_command, cart.config_shape) {
        (false, ConfigShape::Missing) => Some(
            "A discount created before its configuration was saved, or a config written by an older app version",
        ),
        (false, ConfigShape::Empty) => Some("A configuration saved with no fields"),
        (false, ConfigShape::Unexpected) => {
            Some("A configuration with fields this version of the function does not expect")
        }
        _ => None,
    };
    if let Some(w) = when {
        c = c.with_detail("When it happens", w);
    }
    c.key = format!("{}|{}", cart.config_shape.as_str(), c.key);
    c
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::settings::{Overrides, Resolved, Settings};

    /// A resolved Shopify-function run with the starter catalog and the given
    /// rules TOML (the `[rules]` body) and config.
    pub fn resolved(rules_toml: &str, config: Option<serde_json::Value>) -> Resolved {
        let text = format!(
            "[target]\nkind = \"shopify-function\"\nextension = \"ext\"\n[store]\ncodes = [\"WELCOME20\"]\n[rules]\n{rules_toml}\n"
        );
        let mut s = Settings::parse(&text, "test").unwrap_or_default();
        // tiers need a config; resolve() reads it from a file, so inject it.
        let tiers = s.rules.tiers.take();
        let mut r = Resolved::resolve(s, None, &Overrides::default())
            .unwrap_or_else(|e| panic!("test settings: {e}"));
        if let (Some(ptr), Some(cfg)) = (tiers, config.as_ref()) {
            r.rules.tiers = crate::settings::read_tiers(cfg, &ptr, 2).unwrap_or_default();
            r.rules.tiers_pointer = Some(ptr);
        }
        r.config = config;
        r
    }
}
