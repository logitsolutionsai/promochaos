//! End-to-end runs of the three examples through the `promochaos` binary.
//!
//! Each test needs something built outside `cargo test`: the homegrown engine
//! (`cargo build -p homegrown-cart`) or the example wasm files
//! (`scripts/build-examples.sh`). When it is missing, the test prints why and
//! returns without failing, so `cargo test` works on a fresh checkout. Set
//! `PROMOCHAOS_REQUIRE_EXAMPLES=1` in any job that builds the examples to make a missing build fail
//! instead of passing silently.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn exe(p: PathBuf) -> PathBuf {
    if cfg!(windows) {
        p.with_extension("exe")
    } else {
        p
    }
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("promochaos-it-{name}-{}", std::process::id()))
}

fn skip(what: &str, fix: &str) {
    let required = std::env::var("PROMOCHAOS_REQUIRE_EXAMPLES").is_ok_and(|v| v == "1");
    assert!(
        !required,
        "{what} is missing and PROMOCHAOS_REQUIRE_EXAMPLES=1. Build it first: {fix}"
    );
    eprintln!("SKIPPED: {what} is missing. Build it first: {fix}");
}

/// Every finding, failures and warnings, as (rule, severity), in order.
fn findings(results: &Value) -> Vec<(String, String)> {
    results["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["rule"].as_str().unwrap_or_default().to_string(),
                        f["severity"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Exactly one failure per expected rule and nothing else: one seeded bug
/// reported as two findings, or a false alarm, fails the test.
fn expect_exactly(results: &Value, expected: &[&str]) {
    let mut got: Vec<String> = findings(results)
        .into_iter()
        .filter(|(_, sev)| sev == "failure")
        .map(|(rule, _)| rule)
        .collect();
    got.sort();
    let mut want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "findings differ from the seeded bugs");
}

/// Points a copy of a finding folder at other settings (the fixed build), so
/// replay can show the finding passes once the bug is fixed.
fn retarget(finding: &Path, settings: &Path, into: &Path) -> PathBuf {
    let name = finding
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    let dest = into.join(name);
    std::fs::create_dir_all(&dest).expect("create the retargeted finding folder");
    for e in std::fs::read_dir(finding)
        .expect("read the finding folder")
        .flatten()
    {
        if e.path().is_file() {
            std::fs::copy(e.path(), dest.join(e.file_name())).expect("copy a finding file");
        }
    }
    let toml_path = dest.join("finding.toml");
    let text = std::fs::read_to_string(&toml_path).expect("read finding.toml");
    let mut table: toml::Table = toml::from_str(&text).expect("parse finding.toml");
    let abs = std::path::absolute(settings).expect("absolute settings path");
    table.insert(
        "settings".to_string(),
        toml::Value::String(abs.display().to_string()),
    );
    std::fs::write(
        &toml_path,
        toml::to_string(&table).expect("write finding.toml"),
    )
    .expect("write finding.toml");
    dest
}

fn finding_dirs(out: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(out.join("findings"))
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn replay(dir: &Path) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_promochaos"))
        .arg("--quiet")
        .arg("replay")
        .arg(dir)
        .status()
        .expect("the promochaos binary starts")
        .code()
        .unwrap_or(-1)
}

fn run(settings: &Path, out: &Path, extra: &[&str]) -> (i32, Value) {
    let status = Command::new(env!("CARGO_BIN_EXE_promochaos"))
        .arg("--quiet")
        .arg("run")
        .arg("--settings")
        .arg(settings)
        .arg("--out")
        .arg(out)
        .args(extra)
        .status()
        .expect("the promochaos binary starts");
    let code = status.code().unwrap_or(-1);
    let results = std::fs::read_to_string(out.join("results.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    (code, results)
}

fn failed_rules(results: &Value) -> BTreeSet<String> {
    results["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["severity"] == "failure")
                .filter_map(|f| f["rule"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn replay_all(out: &Path) -> Vec<i32> {
    let mut codes = Vec::new();
    let Ok(entries) = std::fs::read_dir(out.join("findings")) else {
        return codes;
    };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let status = Command::new(env!("CARGO_BIN_EXE_promochaos"))
            .arg("--quiet")
            .arg("replay")
            .arg(&d)
            .status()
            .expect("the promochaos binary starts");
        codes.push(status.code().unwrap_or(-1));
    }
    codes
}

fn expect_found(found: &BTreeSet<String>, expected: &[&str]) {
    for rule in expected {
        assert!(
            found.contains(*rule),
            "expected a {rule} finding, found {found:?}"
        );
    }
}

#[test]
fn homegrown_seeded_bugs_are_found_and_the_fixed_build_is_clean() {
    let engine = exe(root().join("target").join("debug").join("homegrown_cart"));
    if !engine.exists() {
        return skip(
            &engine.display().to_string(),
            "cargo build -p homegrown-cart",
        );
    }
    let dir = root().join("examples").join("homegrown-cart");
    let out = out_dir("homegrown");
    let (code, results) = run(&dir.join("promochaos.toml"), &out, &[]);
    assert_eq!(code, 1, "the buggy engine must fail");
    let found = failed_rules(&results);
    expect_found(
        &found,
        &["max_discount_percent", "min_subtotal", "idempotent_retry"],
    );
    // Three seeded bugs, three findings: the retry bug used to be reported
    // twice (with and without a coupon).
    expect_exactly(
        &results,
        &["max_discount_percent", "min_subtotal", "idempotent_retry"],
    );
    validate_results(&out);
    // Every finding still reproduces on replay, and there is one replay per
    // finding (an empty findings folder used to pass this check).
    let codes = replay_all(&out);
    assert_eq!(codes.len(), 3);
    assert!(codes.iter().all(|c| *c == 1), "replay codes {codes:?}");

    let out = out_dir("homegrown-fixed");
    let (code, results) = run(&dir.join("promochaos.fixed.toml"), &out, &[]);
    assert_eq!(
        failed_rules(&results),
        BTreeSet::new(),
        "the fixed engine must be clean"
    );
    assert_eq!(code, 0);
}

#[test]
fn tiered_rust_seeded_bugs_are_found_and_the_fixed_build_is_clean() {
    let dir = root()
        .join("examples")
        .join("functions")
        .join("tiered-discount-rust");
    let wasm = dir.join("dist").join("function.wasm");
    let fixed = dir.join("dist").join("function-fixed.wasm");
    if !wasm.exists() || !fixed.exists() {
        return skip(
            &wasm.display().to_string(),
            "bash scripts/build-examples.sh (or pwsh scripts/build-examples.ps1)",
        );
    }
    let out = out_dir("tiered");
    let (code, results) = run(&dir.join("promochaos.toml"), &out, &[]);
    assert_eq!(code, 1);
    let found = failed_rules(&results);
    expect_found(
        &found,
        &["tiers", "within_instruction_limit", "split_line", "no_trap"],
    );
    // Four seeded bugs, four findings: split_line used to be reported twice.
    expect_exactly(
        &results,
        &["tiers", "within_instruction_limit", "split_line", "no_trap"],
    );
    validate_results(&out);
    let codes = replay_all(&out);
    assert_eq!(codes.len(), 4);
    assert!(codes.iter().all(|c| *c == 1), "replay codes {codes:?}");
    for f in results["findings"].as_array().into_iter().flatten() {
        let id = f["id"].as_str().unwrap_or_default();
        assert!(out.join("findings").join(id).join("fixture.json").exists());
        assert!(out.join("findings").join(id).join("test.rs").exists());
    }
    // The instruction-limit finding reports real, measured counts: the
    // smallest failing cart is over 11,000,000 and the budget is non-empty.
    let budget = results["budget"].as_array().cloned().unwrap_or_default();
    assert!(budget.len() >= 9, "the size sweep has nine sizes");
    assert!(budget.iter().any(|p| {
        p["instructions"].as_u64().unwrap_or(0) > p["limit"].as_u64().unwrap_or(u64::MAX)
    }));
    for p in &budget {
        let lines = p["lines"].as_u64().unwrap_or(0);
        let limit = p["limit"].as_u64().unwrap_or(0);
        let expected = if lines <= 200 {
            11_000_000
        } else {
            lines * 55_000
        };
        assert_eq!(limit, expected, "limit at {lines} lines");
    }
    // Each finding passes when replayed against the fixed build.
    let fixed_settings = dir.join("promochaos.fixed.toml");
    let swapped = out_dir("tiered-replay-fixed");
    for d in finding_dirs(&out) {
        let r = retarget(&d, &fixed_settings, &swapped);
        assert_eq!(
            replay(&r),
            0,
            "{} must pass on the fixed build",
            d.display()
        );
    }

    let out = out_dir("tiered-fixed");
    let (code, results) = run(&dir.join("promochaos.fixed.toml"), &out, &[]);
    assert_eq!(failed_rules(&results), BTreeSet::new());
    assert_eq!(code, 0);
}

#[test]
fn bundle_js_seeded_bugs_are_found_and_the_fixed_build_is_clean() {
    let dir = root()
        .join("examples")
        .join("functions")
        .join("bundle-discount-js");
    let wasm = dir.join("dist").join("function.wasm");
    let fixed = dir.join("dist").join("function-fixed.wasm");
    if !wasm.exists() || !fixed.exists() {
        return skip(
            &wasm.display().to_string(),
            "the Shopify CLI: see examples/functions/bundle-discount-js/README.md",
        );
    }
    let out = out_dir("bundle");
    let (code, results) = run(&dir.join("promochaos.toml"), &out, &[]);
    assert_eq!(code, 1);
    // Six findings, by design (see the example's README): the three seeded bugs (valid_output, never_discount,
    // codes_accepted); two that follow from them (max_discount_percent from
    // the uncapped bundle percentage, add_ineligible_line from discounting
    // gift cards); and within_instruction_limit from 198 lines, which the
    // JavaScript runtime's per-line cost reaches and the fixed build does not.
    expect_exactly(
        &results,
        &[
            "valid_output",
            "never_discount",
            "codes_accepted",
            "max_discount_percent",
            "add_ineligible_line",
            "within_instruction_limit",
        ],
    );
    let limit = results["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["rule"] == "within_instruction_limit")
        .and_then(|f| f["sentence"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        limit.contains("198 or more lines"),
        "the JS build goes over the limit from 198 lines: {limit}"
    );
    validate_results(&out);
    let codes = replay_all(&out);
    assert_eq!(codes.len(), 6);
    assert!(codes.iter().all(|c| *c == 1), "replay codes {codes:?}");

    let out = out_dir("bundle-fixed");
    let (code, results) = run(&dir.join("promochaos.fixed.toml"), &out, &[]);
    assert_eq!(failed_rules(&results), BTreeSet::new());
    assert_eq!(code, 0);
}

#[test]
fn in_process_and_subprocess_runners_count_the_same_instructions() {
    let dir = root()
        .join("examples")
        .join("functions")
        .join("tiered-discount-rust");
    if !dir.join("dist").join("function.wasm").exists() {
        return skip("the tiered example wasm", "bash scripts/build-examples.sh");
    }
    let has_runner = Command::new("function-runner")
        .arg("--version")
        .output()
        .is_ok();
    if !has_runner {
        return skip(
            "function-runner on the PATH",
            "install function-runner 9.2.3 from its GitHub releases",
        );
    }
    let settings = dir.join("promochaos.toml");
    let a = out_dir("parity-in");
    let b = out_dir("parity-sub");
    let (_, ra) = run(&settings, &a, &["--carts", "50"]);
    let (_, rb) = run(&settings, &b, &["--carts", "50", "--runner", "subprocess"]);
    let counts = |r: &Value| -> Vec<Option<u64>> {
        r["carts"]
            .as_array()
            .map(|a| a.iter().map(|c| c["instructions"].as_u64()).collect())
            .unwrap_or_default()
    };
    assert!(!counts(&ra).is_empty());
    assert_eq!(
        counts(&ra),
        counts(&rb),
        "instruction counts differ between runners"
    );
}

/// results.json validates against docs/results-schema.json, which is
/// generated from the result types (promochaos-core, results.rs).
fn validate_results(out: &Path) {
    let schema_text = std::fs::read_to_string(root().join("docs").join("results-schema.json"))
        .expect("read docs/results-schema.json");
    let schema: Value = serde_json::from_str(&schema_text).expect("parse the schema");
    let results: Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("results.json")).expect("read results.json"),
    )
    .expect("parse results.json");
    let validator = jsonschema::validator_for(&schema).expect("a valid JSON Schema");
    let errors: Vec<String> = validator
        .iter_errors(&results)
        .take(5)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{} does not match docs/results-schema.json: {errors:?}",
        out.join("results.json").display()
    );
}

/// Snapshots of what a person sees: the terminal board and report.html for
/// the homegrown example. Timings, the embedded fonts and the input hash are
/// replaced, so the snapshots are the same on every machine.
#[test]
fn homegrown_terminal_output_and_report_match_the_snapshots() {
    let engine = exe(root().join("target").join("debug").join("homegrown_cart"));
    if !engine.exists() {
        return skip(
            &engine.display().to_string(),
            "cargo build -p homegrown-cart",
        );
    }
    let dir = root().join("examples").join("homegrown-cart");
    // A relative --out, so no absolute path reaches the output.
    let rel_out = "../../target/promochaos-snapshot-homegrown";
    let _ = std::fs::remove_dir_all(dir.join(rel_out));
    let output = Command::new(env!("CARGO_BIN_EXE_promochaos"))
        .current_dir(&dir)
        .args(["--color", "never", "run", "--settings", "promochaos.toml"])
        .args(["--out", rel_out])
        .env("NO_COLOR", "1")
        .output()
        .expect("the promochaos binary starts");
    assert_eq!(output.status.code(), Some(1));
    let terminal = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let report = std::fs::read_to_string(dir.join(rel_out).join("report.html"))
        .expect("read report.html")
        .replace("\r\n", "\n");
    insta::with_settings!({
        filters => vec![
            // Timings at the end of a stage line, with the padding before them.
            (r" +\d+(\.\d+)? s\n", "  [time]\n"),
            (r"base64,[A-Za-z0-9+/=]+", "base64,[font]"),
            (r"inputs [0-9a-f]+…[0-9a-f]+", "inputs [hash]"),
        ],
        snapshot_path => "snapshots",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("homegrown_terminal", terminal);
        insta::assert_snapshot!("homegrown_report_html", report);
    });
}
