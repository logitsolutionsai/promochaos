//! report.html (one offline file, fonts embedded, no network requests) and
//! junit.xml.

pub mod chart;

use promochaos_core::apply::{LEDGER_LABEL, LEDGER_LABEL_COMMAND};
use promochaos_core::money::group_int;
use promochaos_core::results::{BudgetPoint, FindingRecord, RunResults};
use promochaos_core::util::short_hash;
use serde::Serialize;

mod fonts {
    include!(concat!(env!("OUT_DIR"), "/fonts.rs"));
}

const TEMPLATE: &str = include_str!("../templates/report.html");

#[derive(Debug)]
pub struct ReportError(pub String);

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReportError {}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, k) in idx.iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(B64[*k as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `@font-face` rules with the embedded fonts, or nothing (system fonts).
pub fn font_css() -> String {
    let mut css = String::new();
    if let Some(bytes) = fonts::GEIST {
        css.push_str(&format!(
            "@font-face{{font-family:Geist;src:url(data:font/woff2;base64,{}) format(\"woff2\");font-weight:100 900;font-style:normal;font-display:swap}}\n",
            base64(bytes)
        ));
    }
    if let Some(bytes) = fonts::GEIST_MONO {
        css.push_str(&format!(
            "@font-face{{font-family:\"Geist Mono\";src:url(data:font/woff2;base64,{}) format(\"woff2\");font-weight:100 900;font-style:normal;font-display:swap}}\n",
            base64(bytes)
        ));
    }
    css
}

pub fn fonts_embedded() -> bool {
    fonts::GEIST.is_some() && fonts::GEIST_MONO.is_some()
}

#[derive(Serialize)]
struct Meta {
    label: String,
    value: String,
    mono: bool,
}

#[derive(Serialize)]
struct Ctx<'a> {
    font_css: String,
    version: &'a str,
    run_line: String,
    verdict: String,
    lede: String,
    meta: Vec<Meta>,
    chart_svg: Option<String>,
    chart_summary: String,
    /// Shown instead of the chart when a Shopify Function run measured no
    /// instruction counts.
    chart_note: Option<String>,
    chart_any_over: bool,
    budget_rows: Vec<BudgetRow>,
    findings: Vec<FindingView<'a>>,
    rules: Vec<RuleRow>,
    not_checked: &'a [promochaos_core::results::NotChecked],
    footer_left: String,
    ledger_label: &'a str,
    /// "function output" or, for a command target, "engine response".
    raw_label: &'a str,
    notes: &'a [String],
}

#[derive(Serialize)]
struct BudgetRow {
    lines: usize,
    instructions: String,
    limit: String,
    over: bool,
}

#[derive(Serialize)]
struct RuleRow {
    rule: String,
    from: String,
    checked: usize,
    failed: usize,
    warned: usize,
    why: String,
}

#[derive(Serialize)]
struct FindingView<'a> {
    f: &'a FindingRecord,
    /// `crashed`, `over_limit`, `rule_broken`, `warning`
    css_kind: &'a str,
    count_text: String,
    has_table: bool,
    /// The function's output, pretty-printed, when there was one.
    raw_output: Option<String>,
    /// The twin cart's output, for split, reorder and add findings.
    raw_twin: Option<String>,
    /// The replay command with the results folder shown relative.
    replay: String,
    test_shown: Option<String>,
    test_label: &'a str,
    twin_rows: Vec<Meta>,
}

/// `C:/x`, `C:\x`, `/x` or `\\server\x`.
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || p.starts_with('\\')
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\'))
}

/// An exported file as the board shows it, relative to the results folder
/// (`findings/F-001/test.rs`), so the report holds no absolute paths.
fn shown_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    if p.starts_with("findings/") {
        return p;
    }
    if let Some(i) = p.rfind("/findings/") {
        return p[i + 1..].to_string();
    }
    if is_absolute(&p) {
        return p.rsplit('/').next().unwrap_or_default().to_string();
    }
    p
}

/// The replay command with an absolute results folder replaced by its own
/// name (replay runs from the folder's parent), quoted when it has a space.
fn shown_replay(replay: &str, results_dir: &str) -> String {
    let r = replay.replace('\\', "/");
    let dir = results_dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() || !is_absolute(dir) {
        return r;
    }
    let Some(i) = r.find(dir) else {
        return r;
    };
    let base = dir.rsplit('/').next().unwrap_or(dir);
    let rest = r[i + dir.len()..].trim_end_matches('"');
    let mut path = format!("{base}{rest}");
    if path.contains(' ') {
        path = format!("\"{path}\"");
    }
    let before = r[..i].trim_end_matches('"');
    format!("{before}{path}")
}

/// Every measured point for the chart: the budget sizes plus the
/// instruction-limit findings' curves, one point per line count.
fn chart_points(r: &RunResults) -> Vec<BudgetPoint> {
    let mut pts: Vec<BudgetPoint> = r.budget.clone();
    for f in &r.findings {
        for c in &f.curve {
            if !pts.iter().any(|p| p.lines == c.lines) {
                pts.push(*c);
            }
        }
    }
    pts.sort_by_key(|p| p.lines);
    pts
}

/// "split, reorder and add", from the twin-cart rules that ran.
fn twin_checks(r: &RunResults) -> Option<String> {
    let names: Vec<String> = r
        .rules
        .iter()
        .filter(|s| s.from == "Twin carts")
        .map(|s| match s.rule.as_str() {
            "split_line" => "split".to_string(),
            "reorder_lines" => "reorder".to_string(),
            "add_ineligible_line" => "add".to_string(),
            other => other.replace('_', " "),
        })
        .collect();
    match names.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [init @ .., last] => Some(format!("{} and {last}", init.join(", "))),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

fn verdict(r: &RunResults) -> (String, String) {
    let failures = r.failures();
    let warnings = r.warnings();
    let carts = r.counts.carts;
    let carts_text = group_int(carts as u64);
    if failures == 0 && warnings == 0 {
        return (
            format!("No problems in {carts_text} carts"),
            "Every rule that applied passed. The sections below show what was checked and what was not.".to_string(),
        );
    }
    let mut v = if failures > 0 {
        format!(
            "{} in {carts_text} carts",
            plural(failures, "problem", "problems")
        )
    } else {
        format!("No problems in {carts_text} carts")
    };
    if warnings > 0 {
        v.push_str(&format!(", {}", plural(warnings, "warning", "warnings")));
    }
    let discarded = r
        .findings
        .iter()
        .filter(|f| f.severity == "failure" && (f.kind == "crashed" || f.kind == "over_limit"))
        .count();
    let is_command = r.target.kind == "command";
    let lede = if is_command || discarded == 0 {
        "Each problem below is shrunk to the smallest cart that still shows it.".to_string()
    } else {
        let lead = match discarded {
            1 => "One of them makes".to_string(),
            n if n == failures && n == 2 => "Both of them make".to_string(),
            n if n == failures => format!("All {n} of them make"),
            n => format!("{} of them make", number_word(n)),
        };
        format!(
            "{lead} Shopify discard the function's output, so the customer gets no discount and nobody sees an error. Each problem below is shrunk to the smallest cart that still shows it."
        )
    };
    (v, lede)
}

fn number_word(n: usize) -> String {
    match n {
        2 => "Two".to_string(),
        3 => "Three".to_string(),
        4 => "Four".to_string(),
        5 => "Five".to_string(),
        6 => "Six".to_string(),
        n => n.to_string(),
    }
}

fn seconds(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

fn meta(r: &RunResults) -> Vec<Meta> {
    let mut m = Vec::new();
    let t = &r.target;
    if t.kind == "command" {
        m.push(Meta {
            label: "Engine".into(),
            value: t.label.clone(),
            mono: true,
        });
    } else {
        let wasm = t
            .wasm
            .as_deref()
            .map(|w| {
                std::path::Path::new(w)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| w.to_string())
            })
            .unwrap_or_default();
        let mut v = wasm;
        if let Some(lang) = &t.language {
            v.push_str(&format!(" · {lang}"));
        }
        if let Some(k) = t.wasm_kib {
            v.push_str(&format!(" · {k}\u{a0}KiB"));
        }
        m.push(Meta {
            label: "Function".into(),
            value: v,
            mono: true,
        });
    }
    let config = match (&r.config_label, &r.config_summary) {
        (Some(l), Some(s)) => Some(format!("{l} · {s}")),
        (Some(l), None) => Some(l.clone()),
        (None, Some(s)) => Some(s.clone()),
        (None, None) => None,
    };
    if let Some(c) = config {
        m.push(Meta {
            label: "Config".into(),
            value: c,
            mono: true,
        });
    }
    let mut runner = t.runner.clone().unwrap_or_default();
    if let Some(api) = &r.tool.api_version {
        runner.push_str(&format!(", API {api}"));
    }
    m.push(Meta {
        label: "Runner".into(),
        value: runner,
        mono: false,
    });
    m.push(Meta {
        label: "Seed".into(),
        value: r.seed.to_string(),
        mono: true,
    });
    let carts = if r.counts.twins > 0 {
        let what = twin_checks(r)
            .map(|c| format!("the {c} checks"))
            .unwrap_or_else(|| "the twin checks".to_string());
        format!(
            "{}, plus {} twins for {what}",
            group_int(r.counts.carts as u64),
            group_int(r.counts.twins as u64)
        )
    } else {
        group_int(r.counts.carts as u64)
    };
    m.push(Meta {
        label: "Carts".into(),
        value: carts,
        mono: false,
    });
    // Wall-clock time is kept out of report.html (it is in run.toml), so the
    // same seed gives a byte-identical report. Runs are counted instead.
    let runs = match r.duration_ms {
        Some(ms) => format!(
            "{}, including {} to shrink",
            seconds(ms),
            plural(r.counts.shrink_runs, "run", "runs")
        ),
        None => format!(
            "{}, including {} to shrink",
            plural(r.counts.runs, "run", "runs"),
            group_int(r.counts.shrink_runs as u64)
        ),
    };
    m.push(Meta {
        label: if r.duration_ms.is_some() {
            "Time".into()
        } else {
            "Runs".into()
        },
        value: runs,
        mono: false,
    });
    m
}

fn finding_view<'a>(f: &'a FindingRecord, results_dir: &str) -> FindingView<'a> {
    let count_text = {
        let unit = if f.count == 1 {
            match f.count_unit.as_str() {
                "pairs" => "pair",
                "sequences" => "sequence",
                _ => "cart",
            }
        } else {
            f.count_unit.as_str()
        };
        let count = match f.of_checked {
            Some(of) => format!("{} of {of} {unit}", f.count),
            None => format!("{} {unit}", f.count),
        };
        format!("{count} · {}", f.shrunk_summary)
    };
    let css_kind = match f.kind.as_str() {
        "crashed" => "crashed",
        "over_limit" => "over_limit",
        "warning" => "warning",
        _ => "rule_broken",
    };
    let has_table = (f.kind == "rule_broken" || f.kind == "warning")
        && f.twin.is_none()
        && !f.cart_rows.is_empty();
    let mut twin_rows = Vec::new();
    if let Some(t) = &f.twin {
        for d in &f.details {
            twin_rows.push(Meta {
                label: d.label.clone(),
                value: d.value.clone(),
                mono: false,
            });
        }
        if let Some(c) = &f.likely_cause {
            twin_rows.push(Meta {
                label: "Likely cause".into(),
                value: c.clone(),
                mono: false,
            });
        }
        let _ = t;
    }
    let raw_twin = f
        .twin
        .as_ref()
        .and_then(|t| t.output.as_ref())
        .and_then(|o| serde_json::to_string_pretty(o).ok());
    let (test_shown, test_label) = match (&f.test_path, &f.fixture_path) {
        (Some(t), _) => (Some(shown_path(t)), "Exported test"),
        (None, Some(x)) => (Some(shown_path(x)), "Exported fixture"),
        (None, None) => (None, "Exported test"),
    };
    FindingView {
        f,
        css_kind,
        count_text,
        has_table,
        raw_output: f.output.as_ref().map(|_| f.output_pretty.clone()),
        raw_twin,
        replay: shown_replay(&f.replay, results_dir),
        test_shown,
        test_label,
        twin_rows,
    }
}

pub fn html(r: &RunResults) -> Result<String, ReportError> {
    let (verdict, lede) = verdict(r);
    let run_line = {
        let mut parts = Vec::new();
        if let Some(at) = &r.started_at {
            parts.push(format!("run {at}"));
        }
        parts.push(format!("seed {}", r.seed));
        if !r.target.label.is_empty() {
            parts.push(r.target.label.clone());
        }
        if let Some(t) = &r.target.run_target {
            parts.push(t.clone());
        }
        parts.join(" · ")
    };
    let points = chart_points(r);
    let chart_svg = if points.is_empty() {
        None
    } else {
        Some(chart::svg(&points))
    };
    // A command target reports no instruction counts, so it has no chart.
    let chart_note = if points.is_empty() && r.target.kind != "command" {
        Some("No run reported an instruction count, so there is nothing to chart.".to_string())
    } else {
        None
    };
    let budget_rows = points
        .iter()
        .map(|p| BudgetRow {
            lines: p.lines,
            instructions: group_int(p.instructions),
            limit: group_int(p.limit),
            over: p.over(),
        })
        .collect();
    let rules = r
        .rules
        .iter()
        .map(|s| RuleRow {
            rule: s.rule.clone(),
            from: s.from.clone(),
            checked: s.checked,
            failed: s.failed,
            warned: s.warned,
            why: s.reasons_text(),
        })
        .collect();
    let mut footer = format!("promochaos {}", r.tool.version);
    if let Some(fr) = &r.tool.function_runner {
        footer.push_str(&format!(" · function-runner {fr}"));
    }
    if let Some(api) = &r.tool.api_version {
        footer.push_str(&format!(" · Functions API {api}"));
    }
    footer.push_str(&format!(" · inputs {}", short_hash(&r.inputs_hash)));
    let ctx = Ctx {
        font_css: font_css(),
        version: &r.tool.version,
        run_line,
        verdict,
        lede,
        meta: meta(r),
        chart_summary: chart::summary(&points),
        chart_note,
        chart_any_over: points.iter().any(|p| p.over()),
        chart_svg,
        budget_rows,
        findings: r
            .findings
            .iter()
            .map(|f| finding_view(f, &r.results_dir))
            .collect(),
        rules,
        not_checked: &r.not_checked,
        footer_left: footer,
        ledger_label: if r.target.kind == "command" {
            LEDGER_LABEL_COMMAND
        } else {
            LEDGER_LABEL
        },
        raw_label: if r.target.kind == "command" {
            "engine response"
        } else {
            "function output"
        },
        notes: &r.notes,
    };
    let mut env = minijinja::Environment::new();
    env.add_template("report.html", TEMPLATE)
        .map_err(|e| ReportError(format!("report template: {e}")))?;
    let tmpl = env
        .get_template("report.html")
        .map_err(|e| ReportError(format!("report template: {e}")))?;
    tmpl.render(&ctx)
        .map_err(|e| ReportError(format!("rendering report.html: {e}")))
}

pub fn junit(r: &RunResults) -> Result<String, ReportError> {
    use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestSuite};
    let mut report = Report::new("promochaos");
    let mut rules = TestSuite::new("promochaos rules");
    for s in &r.rules {
        let status = if s.failed > 0 {
            let mut st = TestCaseStatus::non_success(NonSuccessKind::Failure);
            st.set_message(format!("{} of {} carts failed", s.failed, s.checked));
            st
        } else if s.checked == 0 {
            let mut st = TestCaseStatus::skipped();
            st.set_message(s.reasons_text());
            st
        } else {
            TestCaseStatus::success()
        };
        let mut tc = TestCase::new(s.rule.clone(), status);
        tc.set_classname("promochaos.rules");
        rules.add_test_case(tc);
    }
    report.add_test_suite(rules);
    let mut findings = TestSuite::new("promochaos findings");
    for f in &r.findings {
        let status = if f.severity == "failure" {
            let mut st = TestCaseStatus::non_success(NonSuccessKind::Failure);
            st.set_message(f.sentence.clone());
            st.set_description(format!(
                "{}\n{}",
                f.shrunk_summary,
                shown_replay(&f.replay, &r.results_dir)
            ));
            st
        } else {
            TestCaseStatus::success()
        };
        let mut tc = TestCase::new(format!("{} {}", f.id, f.rule), status);
        tc.set_classname("promochaos.findings");
        findings.add_test_case(tc);
    }
    report.add_test_suite(findings);
    report
        .to_string()
        .map_err(|e| ReportError(format!("junit.xml: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn template_parses() {
        let mut env = minijinja::Environment::new();
        assert!(env.add_template("report.html", TEMPLATE).is_ok());
    }

    const DIR: &str = "C:/Users/Some One/shop/promochaos-results";

    fn point(lines: usize, instructions: u64, limit: u64) -> serde_json::Value {
        serde_json::json!({"lines": lines, "instructions": instructions, "limit": limit})
    }

    /// A small run: one instruction-limit finding with absolute paths, as
    /// the CLI writes them when `--out` is absolute.
    fn sample(kind: &str, budget: Vec<serde_json::Value>) -> RunResults {
        let command = kind == "command";
        let finding = serde_json::json!({
            "id": "F-001", "label": "F1", "rule": "within_instruction_limit",
            "kind": "over_limit", "kind_label": "Over limit", "severity": "failure",
            "fingerprint": {"rule": "within_instruction_limit", "kind": "over_limit", "key": "instructions"},
            "sentence": "Carts with 69 or more lines go over the instruction limit, so Shopify discards the discount.",
            "count": 15, "count_unit": "carts", "of_checked": null,
            "shrunk_summary": "shrunk to 69 lines", "flaky": false, "shrink_runs": 7,
            "details": [{"label": "Smallest failing", "value": "69 lines, 11,005,013 instructions"}],
            "cart": {"id": "cart-0023", "currency": "USD", "rate": "1", "lines": [{
                "id": "gid://shopify/CartLine/1", "quantity": 1, "unit_price": 4500, "compare_at_price": null,
                "merchandise": {"variant": {"variant_id": "gid://shopify/ProductVariant/2001",
                    "product_id": "gid://shopify/Product/1001", "title": "Linen Shirt", "variant_title": "M",
                    "sku": "LS-M", "handle": "linen-shirt", "tags": [], "collections": [], "gift_card": false,
                    "requires_shipping": true, "vendor": "Example Mill", "product_type": "Shirts"}},
                "attributes": [], "selling_plan": null, "parent_line_id": null, "existing_discounts": []}],
                "codes": [], "buyer": null, "delivery": [], "attributes": [], "config": {"value": {}},
                "config_shape": "normal", "discount_classes": ["ORDER"]},
            "cart_rows": [{"line": "CartLine/1", "name": "Linen Shirt, M", "quantity": 1, "subtotal": "$45.00"}],
            "cart_rows_more": 0, "input": {}, "output": {"operations": []},
            "output_pretty": "{\n  \"operations\": []\n}", "logs_first_line": null, "ledger": null,
            "expected_discount": null, "twin": null, "likely_cause": null,
            "curve": [point(69, 11_005_013, 11_000_000)],
            "expect": "no_trap",
            "replay": format!("promochaos replay {DIR}/findings/F-001"),
            "test_path": format!("{DIR}/findings/F-001/test.rs"),
            "fixture_path": format!("{DIR}/findings/F-001/fixture.json"),
        });
        let v = serde_json::json!({
            "schema_version": 1,
            "tool": {"name": "promochaos", "version": "0.1.0",
                "function_runner": if command { None } else { Some("9.2.3") },
                "api_version": if command { None } else { Some("2026-07") }},
            "seed": 42,
            "target": {"kind": kind, "label": "extensions/tiered-discount",
                "run_target": "cart.lines.discounts.generate.run", "export": null,
                "wasm": format!("{DIR}/../dist/function.wasm"), "wasm_kib": 90, "language": "Rust",
                "runner": "function-runner 9.2.3, in-process", "api_version": "2026-07",
                "function_runner_version": "9.2.3", "rust_tests": true},
            "config_summary": "10% from $100.00, 20% from $200.00", "config_label": "config/tiered.json",
            "inputs_hash": "feafd6d4e761f42a",
            "counts": {"carts": 500, "twins": 180, "runs": 757, "shrink_runs": 27,
                "determinism_runs": 50, "order_runs": 0, "by_generator": [["random", 500]]},
            "median_instructions": null,
            "budget": budget,
            "rules": [
                {"rule": "within_instruction_limit", "from": "Shopify limits", "checked": 440, "failed": 15,
                 "warned": 0, "not_applicable": 60, "reasons": [["trapped first", 60]]},
                {"rule": "split_line", "from": "Twin carts", "checked": 50, "failed": 0, "warned": 0,
                 "not_applicable": 0, "reasons": []},
                {"rule": "reorder_lines", "from": "Twin carts", "checked": 50, "failed": 0, "warned": 0,
                 "not_applicable": 0, "reasons": []}],
            "findings": [finding],
            "carts": [],
            "not_checked": [{"title": "Your live store", "text": "No store was contacted."}],
            "notes": [], "results_dir": DIR, "exit_code": 1, "inputs": {},
        });
        serde_json::from_value(v).expect("sample results parse")
    }

    fn tiered() -> RunResults {
        sample(
            "shopify-function",
            vec![
                point(1, 69_473, 11_000_000),
                point(100, 22_279_068, 11_000_000),
                point(200, 86_994_613, 11_000_000),
                point(250, 135_289_769, 13_750_000),
            ],
        )
    }

    #[test]
    fn report_has_no_absolute_paths_and_no_wall_clock() {
        let html = render(&tiered());
        assert!(!html.contains("C:/"), "absolute path in report.html");
        assert!(!html.contains("Some One"), "absolute path in report.html");
        assert!(html.contains("promochaos replay promochaos-results/findings/F-001"));
        assert!(html.contains("<code>findings/F-001/test.rs</code>"));
        assert!(html.contains("function.wasm · Rust · 90\u{a0}KiB"));
        assert!(!html.contains(">Time<"), "no wall-clock time row");
        assert!(html.contains("<dt>Runs</dt><dd>757 runs, including 27 to shrink</dd>"));
    }

    #[test]
    fn the_guide_link_is_the_only_external_url() {
        let html = render(&tiered());
        let urls: Vec<&str> = html
            .match_indices("http")
            .map(|(i, _)| {
                let end = html[i..]
                    .find(['"', '\'', ')', ' ', '<'])
                    .map_or(html.len(), |j| i + j);
                &html[i..end]
            })
            .collect();
        assert_eq!(urls, ["https://logitsolutions.ai/guides/ai-in-ecommerce/"]);
        assert!(!html.contains("<link"), "no stylesheet links");
        assert!(!html.contains(" src="), "no external scripts or images");
        assert!(!html.contains("@import"));
        assert_eq!(
            html.matches("url(").count(),
            html.matches("url(data:font/woff2;base64,").count(),
            "every url() is an embedded font"
        );
    }

    #[test]
    fn fonts_are_embedded() {
        assert!(
            fonts_embedded(),
            "assets/fonts/*.woff2 missing at build time"
        );
        let css = font_css();
        assert!(css.contains("font-family:Geist;src:url(data:font/woff2;base64,d09GMg"));
        assert!(css.contains("font-family:\"Geist Mono\";src:url(data:font/woff2;base64,d09GMg"));
        let html = render(&tiered());
        assert!(html.contains("font-variant-numeric:tabular-nums"));
    }

    #[test]
    fn chart_uses_the_measured_points_and_the_finding_curve() {
        let r = tiered();
        let pts = chart_points(&r);
        let lines: Vec<usize> = pts.iter().map(|p| p.lines).collect();
        assert_eq!(lines, [1, 69, 100, 200, 250]);
        let html = render(&r);
        assert!(html.contains("<td>69</td><td class=\"bad\">11,005,013</td><td>11,000,000</td>"));
        assert!(html.contains("<td>250</td><td class=\"bad\">135,289,769</td><td>13,750,000</td>"));
        assert!(html.contains("first cart over the limit has 69 lines"));
        assert!(html.contains("Over the limit, output discarded by Shopify"));
        assert!(html.contains("500, plus 180 twins for the split and reorder checks"));
    }

    #[test]
    fn clean_run_says_no_cart_reached_the_limit() {
        let mut r = sample(
            "shopify-function",
            vec![
                point(1, 68_869, 11_000_000),
                point(250, 4_868_157, 13_750_000),
            ],
        );
        r.findings.clear();
        let html = render(&r);
        assert!(html.contains("Every cart size measured, up to 250 lines, is within the limit."));
        assert!(!html.contains("first over"));
        assert!(!html.contains("Over the limit, output discarded"));
    }

    #[test]
    fn command_target_has_no_chart() {
        let mut r = sample("command", Vec::new());
        r.findings[0].curve.clear();
        let html = render(&r);
        assert!(!html.contains("Instruction Budget"));
        assert!(!html.contains("<svg"));
        let mut f = sample("shopify-function", Vec::new());
        f.findings[0].curve.clear();
        assert!(html_has(&f, "No run reported an instruction count"));
    }

    /// report.html with minijinja's `&#x2f;` turned back into `/`.
    fn render(r: &RunResults) -> String {
        html(r).expect("renders").replace("&#x2f;", "/")
    }

    fn html_has(r: &RunResults, needle: &str) -> bool {
        render(r).contains(needle)
    }

    #[test]
    fn report_copy_has_no_contractions_or_exclamations() {
        let html = render(&tiered());
        let body =
            &html[html.find("<body>").expect("body")..html.find("<script>").expect("script")];
        let text: String = {
            let mut out = String::new();
            let mut in_tag = false;
            for c in body.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    c if !in_tag => out.push(c),
                    _ => {}
                }
            }
            out
        };
        for bad in ["n't", "'re", "'ll", "'ve", "'m ", "it's", "!"] {
            assert!(!text.contains(bad), "{bad:?} in report copy");
        }
    }

    #[test]
    fn replay_paths_are_shown_relative() {
        assert_eq!(
            shown_replay("promochaos replay C:/a/b/out/findings/F-002", "C:/a/b/out"),
            "promochaos replay out/findings/F-002"
        );
        assert_eq!(
            shown_replay(
                "promochaos replay C:\\a\\my out\\findings\\F-002",
                "C:\\a\\my out"
            ),
            "promochaos replay \"my out/findings/F-002\""
        );
        assert_eq!(
            shown_replay(
                "promochaos replay promochaos-results/findings/F-001",
                "promochaos-results"
            ),
            "promochaos replay promochaos-results/findings/F-001"
        );
        assert_eq!(
            shown_path("C:/x/out/findings/F-001/test.rs"),
            "findings/F-001/test.rs"
        );
        assert_eq!(
            shown_path("promochaos-results/findings/F-001/test.rs"),
            "findings/F-001/test.rs"
        );
        assert_eq!(shown_path("/home/a/elsewhere.rs"), "elsewhere.rs");
    }
}
