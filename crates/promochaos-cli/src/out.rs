//! Writes the results folder: run.toml, results.json, junit.xml,
//! report.html and one folder per finding with its inputs, outputs, ledger
//! and exported tests.

use promochaos_core::error::{Error, Result};
use promochaos_core::results::{FindingRecord, RunResults};
use promochaos_core::settings::Resolved;
use promochaos_core::util::write_file;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct RunToml<'a> {
    tool: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_runner: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_version: Option<&'a str>,
    seed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<String>,
    store_currency: &'a str,
    codes: &'a [String],
    // Tables last, for TOML.
    target: TargetToml<'a>,
    generate: &'a promochaos_core::settings::Generate,
    rules: &'a promochaos_core::settings::Rules,
}

#[derive(Serialize)]
struct TargetToml<'a> {
    kind: &'a str,
    label: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_target: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    export: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wasm: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner: Option<&'a str>,
}

#[derive(Serialize)]
struct FindingToml<'a> {
    id: &'a str,
    rule: &'a str,
    kind: &'a str,
    key: &'a str,
    severity: &'a str,
    sentence: &'a str,
    count: usize,
    shrunk: &'a str,
    flaky: bool,
    replay: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_target: Option<&'a str>,
    seed: u64,
}

fn pretty<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(v)? + "\n")
}

fn toml_text<T: Serialize>(v: &T, what: &str) -> Result<String> {
    toml::to_string_pretty(v).map_err(|e| Error::internal(format!("could not write {what}: {e}")))
}

/// Writes exports and fills `test_path` and `fixture_path` on each finding.
pub fn write_findings(
    dir: &Path,
    display_dir: &str,
    resolved: &Resolved,
    results: &mut RunResults,
) -> Result<()> {
    let settings_abs = resolved
        .settings_path
        .as_ref()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()));
    let info = results.target.clone();
    for f in &mut results.findings {
        let fdir = dir.join("findings").join(&f.id);
        // Relative to the finding folder, so a results folder can be moved
        // or shared with its repo and no absolute path leaks into it.
        let settings_path = settings_abs.as_ref().map(|abs| {
            let fabs = std::path::absolute(&fdir).unwrap_or_else(|_| fdir.clone());
            match promochaos_core::util::relative_to(abs, &fabs) {
                Some(rel) => promochaos_core::util::portable_path(&rel),
                None => abs.display().to_string(),
            }
        });
        write_file(&fdir.join("input.json"), pretty(&f.input)?)?;
        write_file(&fdir.join("output.json"), pretty(&f.output)?)?;
        write_file(&fdir.join("ledger.json"), pretty(&f.ledger)?)?;
        write_file(&fdir.join("cart.json"), pretty(&f.cart)?)?;
        if let Some(t) = &f.twin {
            write_file(&fdir.join("twin.json"), pretty(&t.cart)?)?;
            write_file(&fdir.join("twin-input.json"), pretty(&t.input)?)?;
            write_file(&fdir.join("twin-output.json"), pretty(&t.output)?)?;
        }
        if info.kind == "shopify-function" {
            let export = info.export.clone().unwrap_or_default();
            let target = info.run_target.clone().unwrap_or_default();
            let fixture = promochaos_shopify::export::fixture(f, &export, &target);
            write_file(&fdir.join("fixture.json"), pretty(&fixture)?)?;
            f.fixture_path = Some(format!("{display_dir}/findings/{}/fixture.json", f.id));
            if info.rust_tests {
                let function_path = resolved
                    .export
                    .rust_function
                    .clone()
                    .unwrap_or_else(|| format!("crate::{export}::{export}"));
                let schema_path = resolved
                    .export
                    .rust_schema
                    .clone()
                    .unwrap_or_else(|| "crate::schema".to_string());
                let test =
                    promochaos_shopify::export::rust_test(f, &target, &function_path, &schema_path);
                write_file(&fdir.join("test.rs"), test)?;
                f.test_path = Some(format!("{display_dir}/findings/{}/test.rs", f.id));
            }
        }
        let ft = FindingToml {
            id: &f.id,
            rule: &f.rule,
            kind: &f.kind,
            key: &f.fingerprint.key,
            severity: &f.severity,
            sentence: &f.sentence,
            count: f.count,
            shrunk: &f.shrunk_summary,
            flaky: f.flaky,
            replay: &f.replay,
            settings: settings_path,
            run_target: info.run_target.as_deref(),
            seed: results.seed,
        };
        write_file(&fdir.join("finding.toml"), toml_text(&ft, "finding.toml")?)?;
        write_file(&fdir.join("finding.json"), pretty(&*f)?)?;
    }
    Ok(())
}

pub fn write_all(dir: &Path, resolved: &Resolved, results: &RunResults) -> Result<()> {
    let rt = RunToml {
        tool: "promochaos",
        version: &results.tool.version,
        function_runner: results.tool.function_runner.as_deref(),
        api_version: results.tool.api_version.as_deref(),
        seed: results.seed,
        started_at: results.started_at.as_deref(),
        duration_ms: results.duration_ms,
        settings: resolved
            .settings_path
            .as_ref()
            .map(|p| promochaos_core::util::portable_path(p)),
        target: TargetToml {
            kind: &results.target.kind,
            label: &results.target.label,
            run_target: results.target.run_target.as_deref(),
            export: results.target.export.as_deref(),
            wasm: results.target.wasm.as_deref(),
            runner: results.target.runner.as_deref(),
        },
        generate: &resolved.generate,
        rules: &resolved.rules,
        store_currency: &resolved.store_currency,
        codes: &resolved.codes,
    };
    write_file(&dir.join("run.toml"), toml_text(&rt, "run.toml")?)?;
    // Wall-clock facts stay in run.toml only, so the same seed gives
    // byte-identical results.json and report.html.
    let results = &without_clock(results);
    write_file(&dir.join("results.json"), pretty(results)?)?;
    let junit = promochaos_report::junit(results).map_err(|e| Error::internal(e.to_string()))?;
    write_file(&dir.join("junit.xml"), junit)?;
    let html = promochaos_report::html(results).map_err(|e| Error::internal(e.to_string()))?;
    write_file(&dir.join("report.html"), html)?;
    Ok(())
}

/// The results without wall-clock facts (start time, duration).
pub fn without_clock(results: &RunResults) -> RunResults {
    let mut r = results.clone();
    r.started_at = None;
    r.duration_ms = None;
    r
}

/// Reads a finding folder written by `write_findings`.
pub fn read_finding(dir: &Path) -> Result<(FindingRecord, Option<String>)> {
    let path = dir.join("finding.json");
    let text = promochaos_core::util::read_input_file(&path)?;
    let finding: FindingRecord = serde_json::from_str(&text).map_err(|e| {
        Error::bad_input(format!(
            "{} is not a promochaos finding: {e}",
            path.display()
        ))
    })?;
    let toml_path = dir.join("finding.toml");
    let settings = std::fs::read_to_string(&toml_path)
        .ok()
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|t| {
            t.get("settings")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    Ok((finding, settings))
}
