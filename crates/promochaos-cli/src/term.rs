//! Terminal output, following the "Terminal Output" board:
//! one line per stage with the time at the right edge; problems last, one
//! block each; the verdict just above the file paths. Symbols carry the
//! meaning (✗ failed, ▲ warning, ✓ passed); colour only adds to them.
//! NO_COLOR and `--color never` are honoured.

use promochaos_core::money::group_int;
use promochaos_core::results::RunResults;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static COLOR: AtomicBool = AtomicBool::new(false);
static QUIET: AtomicBool = AtomicBool::new(false);
/// 0 = human output on stdout, 1 = `--json` (logs on stderr).
static MODE: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

pub fn setup(color: ColorChoice, quiet: bool, json: bool) {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let tty = if json {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    };
    let on = match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => tty && !no_color,
    };
    COLOR.store(on, Ordering::SeqCst);
    QUIET.store(quiet, Ordering::SeqCst);
    MODE.store(u8::from(json), Ordering::SeqCst);
}

pub fn json_mode() -> bool {
    MODE.load(Ordering::SeqCst) == 1
}

fn paint(code: &str, s: &str) -> String {
    if COLOR.load(Ordering::SeqCst) {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn muted(s: &str) -> String {
    paint("38;2;139;151;168", s)
}
pub fn fail(s: &str) -> String {
    paint("38;2;249;112;102", s)
}
pub fn fail_bold(s: &str) -> String {
    paint("1;38;2;249;112;102", s)
}
pub fn caution(s: &str) -> String {
    paint("1;38;2;253;176;34", s)
}
pub fn blue(s: &str) -> String {
    paint("38;2;91;155;255", s)
}
pub fn bold(s: &str) -> String {
    paint("1", s)
}

/// Human output goes to stdout, or to stderr in `--json` mode.
pub fn say(line: &str) {
    if QUIET.load(Ordering::SeqCst) {
        return;
    }
    if json_mode() {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// Errors always print, to stderr.
pub fn error(line: &str) {
    eprintln!("{line}");
}

const WIDTH: usize = 82;

/// `generate  500 carts + 180 twins · ...                         0.04 s`
pub fn stage(label: &str, text: &str, secs: Option<f64>) {
    let head = format!("{label:<10}");
    let time = secs.map(|s| format!("{s:.2} s")).unwrap_or_default();
    let used = head.chars().count() + text.chars().count();
    let pad = if time.is_empty() {
        0
    } else {
        WIDTH.saturating_sub(used + time.chars().count()).max(2)
    };
    say(&format!(
        "{}{text}{}{}",
        muted(&head),
        " ".repeat(pad),
        muted(&time)
    ));
}

/// For the engine's progress callback.
pub fn progress(stage_name: &str, text: &str, secs: Option<f64>) {
    stage(stage_name, text, secs);
}

pub fn blank() {
    say("");
}

pub fn header(tool_line: &str, command_line: &str) {
    say(&format!("{}{command_line}", muted("$ ")));
    say(&muted(tool_line));
    blank();
}

fn unit(n: usize, unit: &str) -> String {
    let singular = match unit {
        "carts" => "cart",
        "pairs" => "pair",
        "sequences" => "sequence",
        other => other,
    };
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {unit}")
    }
}

/// Problem blocks, the verdict and the file paths.
pub fn findings_and_verdict(r: &RunResults, results_dir: &str) {
    if !r.findings.is_empty() {
        blank();
    }
    let rule_w = r
        .findings
        .iter()
        .map(|f| f.rule.chars().count())
        .max()
        .unwrap_or(0)
        .max(20);
    // Wide enough for the longest count ("4 sequences"), so the shrink
    // summary never runs into it.
    let count_w = r
        .findings
        .iter()
        .map(|f| unit(f.count, &f.count_unit).chars().count())
        .max()
        .unwrap_or(0)
        .max(9);
    for f in &r.findings {
        let warn = f.severity == "warning";
        let head = format!(
            "{} {:<3} {:<rule_w$}",
            if warn { "▲" } else { "✗" },
            f.label,
            f.rule
        );
        let head = if warn {
            caution(&head)
        } else {
            fail_bold(&head)
        };
        say(&format!(
            "{head}   {:<count_w$}  {}",
            unit(f.count, &f.count_unit),
            f.shrunk_summary
        ));
        say(&format!("      {}", f.sentence));
        say(&blue(&format!("      {}", f.replay)));
    }
    blank();
    let failures = r.failures();
    let carts = group_int(r.counts.carts as u64);
    let discarded = r
        .findings
        .iter()
        .filter(|f| f.severity == "failure" && (f.kind == "crashed" || f.kind == "over_limit"))
        .count();
    let mut verdict = if failures == 0 {
        format!("No problems in {carts} carts.")
    } else if failures == 1 {
        format!("1 problem in {carts} carts.")
    } else {
        format!("{failures} problems in {carts} carts.")
    };
    if r.target.kind != "command" && discarded > 0 {
        let who = match (discarded, failures) {
            (1, 1) => "It makes".to_string(),
            (1, _) => "One makes".to_string(),
            (d, f) if d == f && d == 2 => "Both make".to_string(),
            (d, f) if d == f => "All of them make".to_string(),
            (2, _) => "Two make".to_string(),
            (3, _) => "Three make".to_string(),
            (d, _) => format!("{d} make"),
        };
        verdict.push_str(&format!(" {who} Shopify discard the discount."));
    }
    let warnings = r.warnings();
    if warnings > 0 {
        verdict.push_str(&format!(
            " {} {}.",
            warnings,
            if warnings == 1 { "warning" } else { "warnings" }
        ));
    }
    say(&bold(&verdict));
    if r.findings.iter().any(|f| f.test_path.is_some()) {
        say(&format!(
            "{}{results_dir}/findings/F-*/test.rs",
            muted(&format!("{:<10}", "tests"))
        ));
    } else if r.findings.iter().any(|f| f.fixture_path.is_some()) {
        say(&format!(
            "{}{results_dir}/findings/F-*/fixture.json",
            muted(&format!("{:<10}", "tests"))
        ));
    }
    say(&format!(
        "{}{results_dir}/report.html   {}{results_dir}/junit.xml   {}",
        muted(&format!("{:<10}", "report")),
        muted("junit "),
        muted(&format!("exit {}", r.exit_code))
    ));
}
