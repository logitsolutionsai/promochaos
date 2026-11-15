//! promochaos-core: everything that does not depend on a particular target.
//!
//! Money is held in integer minor units (`i64`) and parsed with `rust_decimal`.
//! Floats are never used for money. Generation is seeded and deterministic.

pub mod apply;
pub mod cart;
pub mod catalog;
pub mod engine;
pub mod error;
pub mod eval;
pub mod generate;
pub mod money;
pub mod results;
pub mod rules;
pub mod settings;
pub mod shrink;
pub mod util;

pub use error::{Error, ExitKind, Result};

/// The tool version, shown in every report and in `run.toml`.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Hard limits from the spec (section 14).
pub mod hard_limits {
    /// Most carts in one run.
    pub const MAX_CARTS: usize = 10_000;
    /// Most lines in one cart.
    pub const MAX_LINES: usize = 2_000;
    /// Largest input file promochaos reads (settings, catalog, config, schema, query).
    pub const MAX_INPUT_FILE_BYTES: u64 = 10 * 1024 * 1024;
}
