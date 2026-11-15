//! Exports a shrunk finding as tests:
//! - `fixture.json` in the shape `loadFixture` from
//!   `@shopify/shopify-function-test-helpers` 1.1.1 reads:
//!   `{ "payload": { "export", "target", "input", "output" } }`.
//! - `test.rs`, a Rust `#[test]` using `shopify_function::run_function_with_input`
//!   (shopify_function 2.x), for Rust functions.

use promochaos_core::results::{Expect, FindingRecord};
use serde_json::{Value, json};

/// The fixture for the JS test helpers. `payload.output` is the output the
/// function produced when promochaos found the problem, because the helpers
/// compare outputs exactly and promochaos cannot know the corrected output.
/// The `promochaos` block says what to change.
pub fn fixture(finding: &FindingRecord, export: &str, target: &str) -> Value {
    let note = match &finding.expect {
        Expect::NoTrap => {
            "payload.output is empty because the function trapped. Set it to the output you expect once the function no longer traps."
        }
        Expect::NotExportable { .. } => {
            "payload.output is what the function produced. The problem is not visible in the output; check runResult.metadata in your test (for instructions, instructionCount)."
        }
        _ => {
            "payload.output is what the function produced when promochaos found the problem. Edit it to the output you expect, so this fixture fails until the function is fixed."
        }
    };
    let mut block = json!({
        "finding": finding.id,
        "rule": finding.rule,
        "sentence": finding.sentence,
        "note": note,
        "expect": finding.expect,
        "replay": finding.replay,
    });
    if let Some(p) = finding
        .curve
        .iter()
        .find(|p| p.lines == finding.cart.lines.len())
        && let Some(obj) = block.as_object_mut()
    {
        obj.insert("instructions".to_string(), json!(p.instructions));
        obj.insert("instruction_limit".to_string(), json!(p.limit));
    }
    json!({
        "payload": {
            "export": export,
            "target": target,
            "input": finding.input,
            "output": finding.output.clone().unwrap_or_else(|| json!({})),
        },
        "promochaos": block,
    })
}

/// A raw string literal that cannot be closed early by the content.
fn raw_literal(s: &str) -> String {
    let mut hashes = 1;
    loop {
        let close = format!("\"{}", "#".repeat(hashes));
        if !s.contains(&close) {
            break;
        }
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}\"{s}\"{h}")
}

fn ident(id: &str, rule: &str) -> String {
    let mut out = format!("{}_{}", id.to_lowercase().replace('-', "_"), rule);
    out.retain(|c| c.is_ascii_alphanumeric() || c == '_');
    out
}

/// `cart.lines.discounts.generate.run` to `CartLinesDiscountsGenerateRunResult`.
pub fn result_type_name(target: &str) -> String {
    let mut out = String::new();
    for part in target.split(['.', '-']).filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out.push_str("Result");
    out
}

fn str_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("&[{}]", quoted.join(", "))
}

/// The Rust test for a finding. `function_path` and `schema_path` are the
/// paths inside the function crate, from `[export]` in promochaos.toml.
pub fn rust_test(
    finding: &FindingRecord,
    target: &str,
    function_path: &str,
    schema_path: &str,
) -> String {
    let name = ident(&finding.id, &finding.rule);
    let module = format!("promochaos_{}", finding.id.to_lowercase().replace('-', "_"));
    let input_json = serde_json::to_string_pretty(&finding.input).unwrap_or_default();
    let result_ty = result_type_name(target);
    let cart_lines = target == crate::discover::CART_LINES_RUN;
    let mut body = String::new();
    let mut extra_consts = String::new();
    let mut ignore: Option<String> = None;
    let mut helpers = String::new();

    let run_line = "    let result = run_function_with_input(function_under_test, INPUT)\n        .expect(\"the function returned an error\");\n";
    match (&finding.expect, cart_lines) {
        (Expect::NoTrap, _) => {
            body.push_str("    // Panics (fails) while the function still traps on this input.\n");
            body.push_str("    let _ = run_function_with_input(function_under_test, INPUT)\n        .expect(\"the function returned an error\");\n");
        }
        (Expect::OrderPercent { percent: Some(p) }, true) => {
            body.push_str(run_line);
            body.push_str(&format!(
                "    let percent: f64 = {:?};\n",
                p.parse::<f64>().unwrap_or(0.0)
            ));
            body.push_str(&format!(
                "    let found = result.operations.iter().any(|op| match op {{\n        schema::CartOperation::OrderDiscountsAdd(add) => add.candidates.iter().any(|c| match &c.value {{\n            schema::OrderDiscountCandidateValue::Percentage(p) => (p.value.0 - percent).abs() < 1e-9,\n            _ => false,\n        }}),\n        _ => false,\n    }});\n    assert!(found, \"expected an order discount of {{percent}}% (promochaos {})\");\n",
                finding.id
            ));
        }
        (Expect::OrderPercent { percent: None }, true) => {
            body.push_str(run_line);
            body.push_str(&format!(
                "    let any_order = result.operations.iter().any(|op| matches!(op, schema::CartOperation::OrderDiscountsAdd(add) if !add.candidates.is_empty()));\n    assert!(!any_order, \"expected no order discount below the lowest tier (promochaos {})\");\n",
                finding.id
            ));
        }
        (Expect::NoDiscount, true) => {
            body.push_str(run_line);
            body.push_str(&format!(
                "    let any = result.operations.iter().any(|op| match op {{\n        schema::CartOperation::OrderDiscountsAdd(add) => !add.candidates.is_empty(),\n        schema::CartOperation::ProductDiscountsAdd(add) => !add.candidates.is_empty(),\n        _ => false,\n    }});\n    assert!(!any, \"expected no discount below the minimum subtotal (promochaos {})\");\n",
                finding.id
            ));
        }
        (Expect::LinesNotDiscounted { line_ids }, true) => {
            extra_consts.push_str(&format!(
                "const PROTECTED: &[&str] = {};\n\n",
                str_list(line_ids)
            ));
            body.push_str(run_line);
            body.push_str(&format!(
                "    for op in &result.operations {{\n        match op {{\n            schema::CartOperation::ProductDiscountsAdd(add) => {{\n                for c in &add.candidates {{\n                    for t in &c.targets {{\n                        if let schema::ProductDiscountCandidateTarget::CartLine(line) = t {{\n                            assert!(!PROTECTED.contains(&line.id.as_str()), \"{{}} must not be discounted (promochaos {id})\", line.id);\n                        }}\n                    }}\n                }}\n            }}\n            schema::CartOperation::OrderDiscountsAdd(add) => {{\n                for c in &add.candidates {{\n                    for t in &c.targets {{\n                        if let schema::OrderDiscountCandidateTarget::OrderSubtotal(sub) = t {{\n                            for id in PROTECTED {{\n                                assert!(sub.excluded_cart_line_ids.iter().any(|e| e.as_str() == *id), \"{{id}} must be in excludedCartLineIds (promochaos {id})\");\n                            }}\n                        }}\n                    }}\n                }}\n            }}\n            _ => {{}}\n        }}\n    }}\n",
                id = finding.id
            ));
        }
        (Expect::CodesOnly { allowed }, true) => {
            extra_consts.push_str(&format!(
                "const ALLOWED: &[&str] = {};\n\n",
                str_list(allowed)
            ));
            body.push_str(run_line);
            body.push_str(&format!(
                "    for op in &result.operations {{\n        if let schema::CartOperation::EnteredDiscountCodesAccept(accept) = op {{\n            for c in &accept.codes {{\n                assert!(ALLOWED.iter().any(|a| a.eq_ignore_ascii_case(&c.code)), \"accepted {{}}, which is not configured (promochaos {})\", c.code);\n            }}\n        }}\n    }}\n",
                finding.id
            ));
        }
        (Expect::TwinKeepsDiscount, true) => {
            if let Some(t) = &finding.twin {
                let twin_json = serde_json::to_string_pretty(&t.input).unwrap_or_default();
                extra_consts.push_str(&format!(
                    "const TWIN_INPUT: &str = {};\n\n",
                    raw_literal(&twin_json)
                ));
                helpers.push_str(&format!(
                    "fn has_discount(r: &schema::{result_ty}) -> bool {{\n    r.operations.iter().any(|op| match op {{\n        schema::CartOperation::OrderDiscountsAdd(add) => !add.candidates.is_empty(),\n        schema::CartOperation::ProductDiscountsAdd(add) => !add.candidates.is_empty(),\n        _ => false,\n    }})\n}}\n\n"
                ));
                body.push_str("    let original = run_function_with_input(function_under_test, INPUT)\n        .expect(\"the function returned an error\");\n");
                body.push_str("    let twin = run_function_with_input(function_under_test, TWIN_INPUT)\n        .expect(\"the function returned an error on the twin\");\n");
                body.push_str(&format!(
                    "    if has_discount(&original) {{\n        assert!(has_discount(&twin), \"the twin cart ({}) lost the discount (promochaos {})\");\n    }}\n",
                    t.relation.id(),
                    finding.id
                ));
            } else {
                ignore = Some("the twin cart was not recorded; use promochaos replay".to_string());
            }
        }
        (Expect::NotExportable { reason }, _) => ignore = Some(reason.clone()),
        (_, false) => {
            ignore = Some(format!(
                "typed assertions are written for {} only; use promochaos replay",
                crate::discover::CART_LINES_RUN
            ));
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "//! Exported by promochaos {} from finding {} ({}).\n//!\n",
        promochaos_core::TOOL_VERSION,
        finding.id,
        finding.rule
    ));
    for line in finding.sentence.lines() {
        out.push_str(&format!("//! {line}\n"));
    }
    // The test module must be a child of the module that defines the
    // function: a `#[shopify_function]` fn is private, so a sibling module
    // declared in src/main.rs cannot call it (QA 2026-11-15, E0603).
    let defining_file = defining_file(function_path);
    out.push_str(&format!(
        "//!\n//! To use it: copy this file next to {defining_file} as\n//! {module}.rs and add to {defining_file}:\n//!\n//!     #[cfg(test)]\n//!     #[path = \"{module}.rs\"]\n//!     mod {module};\n//!\n//! Then run `cargo test`. It fails until the problem is fixed.\n//! Replay the original: {}\n\n",
        finding.replay
    ));
    out.push_str("#![allow(unused_imports, irrefutable_let_patterns, clippy::all)]\n\n");
    out.push_str("use shopify_function::run_function_with_input;\n");
    out.push_str(&format!("use {schema_path} as schema;\n"));
    out.push_str(&format!("use {function_path} as function_under_test;\n\n"));
    out.push_str(&format!(
        "const INPUT: &str = {};\n\n",
        raw_literal(&input_json)
    ));
    out.push_str(&extra_consts);
    out.push_str(&helpers);
    out.push_str("#[test]\n");
    if let Some(reason) = &ignore {
        out.push_str(&format!(
            "#[ignore = {:?}]\n",
            format!("promochaos: {reason}")
        ));
    }
    out.push_str(&format!("fn {name}() {{\n"));
    if ignore.is_some() {
        out.push_str("    let _ = run_function_with_input(function_under_test, INPUT);\n");
    } else {
        out.push_str(&body);
    }
    out.push_str("}\n");
    out
}

/// The source file that defines the function, from its Rust path:
/// `crate::cart_lines_discounts_generate_run::run` is
/// `src/cart_lines_discounts_generate_run.rs`; a function at the crate root
/// is in `src/main.rs`.
fn defining_file(function_path: &str) -> String {
    let segments: Vec<&str> = function_path
        .trim_start_matches("crate::")
        .split("::")
        .filter(|s| !s.is_empty())
        .collect();
    match segments.split_last() {
        Some((_, modules)) if !modules.is_empty() => format!("src/{}.rs", modules.join("/")),
        _ => "src/main.rs".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (QA 2026-11-15): the exported test told users to declare
    /// it in src/main.rs, where it cannot call the private function.
    #[test]
    fn exported_tests_go_next_to_the_defining_file() {
        assert_eq!(
            defining_file(
                "crate::cart_lines_discounts_generate_run::cart_lines_discounts_generate_run"
            ),
            "src/cart_lines_discounts_generate_run.rs"
        );
        assert_eq!(defining_file("crate::run"), "src/main.rs");
    }

    #[test]
    fn raw_literals_never_close_early() {
        assert_eq!(raw_literal("{}"), "r#\"{}\"#");
        assert_eq!(raw_literal("a\"#b"), "r##\"a\"#b\"##");
    }

    #[test]
    fn result_type_names() {
        assert_eq!(
            result_type_name("cart.lines.discounts.generate.run"),
            "CartLinesDiscountsGenerateRunResult"
        );
        assert_eq!(
            result_type_name("cart.delivery-options.discounts.generate.run"),
            "CartDeliveryOptionsDiscountsGenerateRunResult"
        );
    }

    #[test]
    fn idents_are_valid() {
        assert_eq!(ident("F-002", "tiers"), "f_002_tiers");
    }
}
