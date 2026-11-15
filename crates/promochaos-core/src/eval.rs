//! What a target returns for one cart, and the trait every target implements.

use crate::apply::Ledger;
use crate::cart::Cart;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Resource use of one Shopify Function run, as function-runner reports it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct RunMetrics {
    /// Wasm fuel used: the same count Shopify uses.
    pub instructions: u64,
    /// Peak linear memory, KiB.
    pub memory_kib: u64,
    /// Module size, KiB.
    pub module_kib: u64,
    /// Limits scale with `cart.lines` above 200 lines (1.0 to 10.0).
    pub scale_factor: f64,
}

/// The totals a command target reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Totals {
    pub subtotal: i64,
    pub discounts_order_product: i64,
    pub shipping_reported: i64,
    pub shipping_discounts: i64,
    pub tax: i64,
    pub total: i64,
}

/// One run of one cart through a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Evaluation {
    /// What was sent: the rendered function input, or the command request.
    pub input: serde_json::Value,
    pub input_bytes: usize,
    /// The parsed output, when there was any.
    pub output: Option<serde_json::Value>,
    /// The output as the target produced it, for the determinism check.
    pub output_text: String,
    pub output_bytes: usize,
    /// First line of the trap, panic, exit status or timeout, when the run
    /// did not succeed.
    pub crash: Option<String>,
    pub logs: String,
    pub metrics: Option<RunMetrics>,
    /// Problems found by the target's own validation (the output schema for
    /// Shopify functions, the response shape for commands).
    pub schema_errors: Vec<String>,
    /// The application model's working, when the output could be read.
    pub ledger: Option<Ledger>,
    /// Command targets only.
    pub totals: Option<Totals>,
    /// Command targets only: the order id and coupon uses for place_order.
    pub order: Option<OrderInfo>,
    /// The run was still going when promochaos's timeout stopped it.
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct OrderInfo {
    pub id: String,
    pub coupon_uses: std::collections::BTreeMap<String, i64>,
}

impl Evaluation {
    pub fn crashed(input: serde_json::Value, message: String, logs: String) -> Evaluation {
        Evaluation {
            input_bytes: serde_json::to_vec(&input).map(|v| v.len()).unwrap_or(0),
            input,
            output: None,
            output_text: String::new(),
            output_bytes: 0,
            crash: Some(message),
            logs,
            metrics: None,
            schema_errors: Vec::new(),
            ledger: None,
            totals: None,
            order: None,
            timed_out: false,
        }
    }

    /// The measured instruction count. None for a run stopped by the
    /// timeout: function-runner does not report a count for it.
    pub fn instructions(&self) -> Option<u64> {
        if self.timed_out {
            return None;
        }
        self.metrics.map(|m| m.instructions)
    }

    /// Metrics with a trustworthy instruction count.
    pub fn measured(&self) -> Option<RunMetrics> {
        if self.timed_out { None } else { self.metrics }
    }
}

/// Facts about a target, for the report header and run.toml.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct TargetInfo {
    /// `shopify-function` or `command`.
    pub kind: String,
    /// Extension folder or command line, as shown to the user.
    pub label: String,
    pub run_target: Option<String>,
    pub export: Option<String>,
    pub wasm: Option<String>,
    pub wasm_kib: Option<u64>,
    /// `Rust`, `JavaScript` or `Wasm`.
    pub language: Option<String>,
    /// `function-runner 9.2.3, in-process`.
    pub runner: Option<String>,
    pub api_version: Option<String>,
    pub function_runner_version: Option<String>,
    /// Whether the exported Rust test applies.
    pub rust_tests: bool,
}

/// Everything promochaos can test. Implementations must be safe to call from
/// several worker threads at once.
pub trait Target: Send + Sync {
    fn info(&self) -> TargetInfo;

    /// Prices one cart (Shopify: runs the function once).
    fn evaluate(&self, cart: &Cart) -> Result<Evaluation>;

    /// Command targets: places an order with an idempotency key. `None` when
    /// the target has no orders.
    fn place_order(&self, _cart: &Cart, _idempotency_key: &str) -> Option<Result<Evaluation>> {
        None
    }

    /// Command targets: runs the reset command before a stateful sequence.
    fn reset(&self) -> Result<()> {
        Ok(())
    }

    /// Whether `place_order` is supported.
    fn supports_orders(&self) -> bool {
        false
    }
}
