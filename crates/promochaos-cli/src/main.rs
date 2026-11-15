//! `promochaos`: generates carts, runs them through a discount Function or a
//! homegrown cart engine, checks rules and shrinks every failing cart.
//!
//! Exit codes: 0 no failures, 1 failures found, 2 invalid input or config,
//! 3 target unavailable, 4 internal error.

mod out;
mod term;

use clap::{Parser, Subcommand, ValueEnum};
use promochaos_command::CommandTarget;
use promochaos_core::engine::{self, EngineOptions};
use promochaos_core::error::{Error, ExitKind, Result};
use promochaos_core::eval::Target;
use promochaos_core::money::group_int;
use promochaos_core::results::RunResults;
use promochaos_core::settings::{
    Overrides, Resolved, ResolvedTarget, RunnerKind, Settings, TargetKind,
};
use promochaos_core::util::write_file;
use promochaos_shopify::ShopifyTarget;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use term::ColorChoice;

#[derive(Parser, Debug)]
#[command(
    name = "promochaos",
    version,
    about = "Finds the carts that break your discount code, and shrinks each one to the smallest cart that still breaks it."
)]
struct Cli {
    /// Print only errors.
    #[arg(long, global = true)]
    quiet: bool,
    /// Machine output (results.json) on stdout; logs on stderr.
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum KindArg {
    ShopifyFunction,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RunnerArg {
    InProcess,
    Subprocess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Html,
    Json,
    Junit,
}

#[derive(clap::Args, Debug, Clone, Default)]
struct RunArgs {
    /// The function extension folder (with shopify.extension.toml).
    extension: Option<PathBuf>,
    /// promochaos.toml (default: ./promochaos.toml when it exists).
    #[arg(long)]
    settings: Option<PathBuf>,
    /// The discount's configuration (the metafield's jsonValue).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Run this .wasm instead of the extension's build output.
    #[arg(long)]
    wasm: Option<PathBuf>,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    carts: Option<usize>,
    /// Runs at once (default: the number of CPUs, at most 16).
    #[arg(long)]
    jobs: Option<usize>,
    /// Target runs allowed to shrink each failure.
    #[arg(long, default_value_t = 200)]
    shrink_budget: usize,
    /// Results folder.
    #[arg(long, default_value = "promochaos-results")]
    out: PathBuf,
    #[arg(long, value_enum)]
    runner: Option<RunnerArg>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write promochaos.toml and a starter catalog.
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, value_enum, default_value_t = KindArg::ShopifyFunction)]
        kind: KindArg,
    },
    /// Validate promochaos.toml, the catalog, the config and the function's queries.
    CheckSettings {
        #[arg(long)]
        settings: Option<PathBuf>,
        extension: Option<PathBuf>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Generate carts, run them, check every rule and shrink each failure.
    Run(RunArgs),
    /// Re-run one shrunk finding. Exit 1 while it still fails, 0 once fixed.
    Replay { finding_dir: PathBuf },
    /// Re-render a results folder.
    Report {
        results_dir: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Html)]
        format: Format,
    },
    /// Run only the size sweep and print instructions per cart size.
    Budget(RunArgs),
}

/// The last panic message, printed as one line by `main` (exit 4).
static PANIC: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn main() -> ExitCode {
    let cli = Cli::parse();
    term::setup(cli.color, cli.quiet, cli.json);
    // A panic is an internal error: exit 4 with one line, not Rust's
    // multi-line panic report and exit 101. Panics that function-runner
    // raises inside a run are caught in the runner and never reach here.
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let at = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if let Ok(mut slot) = PANIC.lock() {
            *slot = Some(format!(
                "{}{at}",
                promochaos_core::util::first_line(&payload)
            ));
        }
    }));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(cli.command)));
    let result = match outcome {
        Ok(r) => r,
        Err(_) => {
            let msg = PANIC
                .lock()
                .ok()
                .and_then(|m| m.clone())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(Error::internal(format!("internal error: {msg}")))
        }
    };
    exit_with(result)
}

fn dispatch(command: Command) -> Result<ExitKind> {
    match command {
        Command::Init { dir, kind } => init(&dir, kind),
        Command::CheckSettings {
            settings,
            extension,
            config,
        } => check_settings(settings, extension, config),
        Command::Run(args) => run(&args, false),
        Command::Budget(args) => run(&args, true),
        Command::Replay { finding_dir } => replay(&finding_dir),
        Command::Report {
            results_dir,
            format,
        } => report(&results_dir, format),
    }
}

fn exit_with(result: Result<ExitKind>) -> ExitCode {
    match result {
        Ok(kind) => ExitCode::from(kind.code() as u8),
        Err(e) => {
            term::error(&format!("{} {e}", term::fail("error:")));
            if let Some(h) = e.hint() {
                term::error(&format!("{} {h}", term::muted("fix:")));
            }
            ExitCode::from(e.exit_kind().code() as u8)
        }
    }
}

fn init(dir: &Path, kind: KindArg) -> Result<ExitKind> {
    let settings = dir.join("promochaos.toml");
    let catalog = dir.join("catalog.toml");
    for p in [&settings, &catalog] {
        if p.exists() {
            return Err(Error::bad_input_hint(
                format!("{} already exists", p.display()),
                "promochaos init never overwrites files; remove it or use another --dir",
            ));
        }
    }
    let k = match kind {
        KindArg::ShopifyFunction => TargetKind::ShopifyFunction,
        KindArg::Command => TargetKind::Command,
    };
    write_file(&settings, Settings::starter_toml(k))?;
    write_file(&catalog, promochaos_core::catalog::Catalog::starter_toml())?;
    term::say(&format!(
        "{} wrote {}",
        term::muted("init"),
        settings.display()
    ));
    term::say(&format!(
        "{} wrote {}",
        term::muted("init"),
        catalog.display()
    ));
    term::say("Edit [target] in promochaos.toml, then run: promochaos run");
    Ok(ExitKind::Clean)
}

/// Finds and resolves the settings for `run`, `budget` and `check-settings`.
fn resolve(settings: Option<&Path>, overrides: &Overrides) -> Result<Resolved> {
    let path = match settings {
        Some(p) => Some(p.to_path_buf()),
        None => {
            let default = PathBuf::from("promochaos.toml");
            if default.exists() {
                Some(default)
            } else {
                None
            }
        }
    };
    match path {
        Some(p) => Resolved::load(&p, overrides),
        None => {
            if overrides.extension.is_none() {
                return Err(Error::bad_input_hint(
                    "no promochaos.toml here and no extension given",
                    "run `promochaos init`, or pass the extension folder: promochaos run extensions/my-discount",
                ));
            }
            Resolved::resolve(Settings::default(), None, overrides)
        }
    }
}

fn overrides_from(args: &RunArgs) -> Overrides {
    Overrides {
        extension: args.extension.clone(),
        config: args.config.clone(),
        wasm: args.wasm.clone(),
        seed: args.seed,
        carts: args.carts,
        runner: args.runner.map(|r| match r {
            RunnerArg::InProcess => RunnerKind::InProcess,
            RunnerArg::Subprocess => RunnerKind::Subprocess,
        }),
    }
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// One target per run target (Shopify) or the one command.
fn open_targets(resolved: &Resolved, jobs: usize, out: &Path) -> Result<Vec<Box<dyn Target>>> {
    match &resolved.target {
        ResolvedTarget::Shopify(_) => Ok(ShopifyTarget::open(resolved, jobs)?
            .into_iter()
            .map(|t| Box::new(t) as Box<dyn Target>)
            .collect()),
        ResolvedTarget::Command(spec) => {
            let work = out.join(".work");
            Ok(vec![Box::new(CommandTarget::new(
                spec,
                &resolved.base_dir,
                &work,
            )?)])
        }
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn display(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn run(args: &RunArgs, budget_only: bool) -> Result<ExitKind> {
    let overrides = overrides_from(args);
    let resolved = resolve(args.settings.as_deref(), &overrides)?;
    let jobs = args.jobs.unwrap_or_else(default_jobs).max(1);
    let targets = open_targets(&resolved, jobs, &args.out)?;
    let many = targets.len() > 1;
    let mut worst = ExitKind::Clean;
    let mut all: Vec<RunResults> = Vec::new();
    for target in &targets {
        let info = target.info();
        let out_dir = if many {
            args.out
                .join(slug(info.run_target.as_deref().unwrap_or("target")))
        } else {
            args.out.clone()
        };
        let shown = display(&out_dir);
        let mut tool_line = format!("promochaos {}", promochaos_core::TOOL_VERSION);
        if let Some(v) = &info.function_runner_version {
            tool_line.push_str(&format!(" · function-runner {v}"));
        }
        if let Some(api) = &info.api_version {
            tool_line.push_str(&format!(" · Functions API {api}"));
        }
        tool_line.push_str(&format!(" · seed {}", resolved.generate.seed));
        let cmd = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        term::header(&tool_line, &format!("promochaos {cmd}"));
        if info.kind == "command" {
            term::stage("engine", &info.label, None);
        } else {
            let wasm = info
                .wasm
                .as_deref()
                .map(|w| promochaos_core::util::display_path(Path::new(w), &resolved.base_dir))
                .unwrap_or_default();
            let mut facts = Vec::new();
            if let Some(l) = &info.language {
                facts.push(l.clone());
            }
            if let Some(k) = info.wasm_kib {
                facts.push(format!("{k} KiB"));
            }
            term::stage(
                "function",
                &format!(
                    "{wasm} ({}) · {}",
                    facts.join(", "),
                    info.run_target.clone().unwrap_or_default()
                ),
                None,
            );
        }
        let opts = EngineOptions {
            jobs,
            shrink_budget: args.shrink_budget,
            results_dir_display: shown.clone(),
            budget_only,
            progress: Some(term::progress),
        };
        let (mut results, _stages) = engine::run(&resolved, target.as_ref(), &opts)?;
        if budget_only {
            print_budget(&results);
            let over = results.budget.iter().any(|p| p.over());
            let kind = if over {
                ExitKind::Failures
            } else {
                ExitKind::Clean
            };
            worst = worst.max(kind);
            all.push(results);
            continue;
        }
        out::write_findings(&out_dir, &shown, &resolved, &mut results)?;
        out::write_all(&out_dir, &resolved, &results)?;
        term::findings_and_verdict(&results, &shown);
        let kind = if results.exit_code == 1 {
            ExitKind::Failures
        } else {
            ExitKind::Clean
        };
        worst = worst.max(kind);
        all.push(results);
    }
    if term::json_mode() {
        let all: Vec<RunResults> = all.iter().map(out::without_clock).collect();
        let text = if all.len() == 1 {
            serde_json::to_string_pretty(&all[0])?
        } else {
            serde_json::to_string_pretty(&all)?
        };
        println!("{text}");
    }
    Ok(worst)
}

fn print_budget(r: &RunResults) {
    term::blank();
    term::say(&term::muted("     lines    instructions           limit"));
    for p in &r.budget {
        let mark = if p.over() {
            term::fail("✗ over")
        } else {
            term::muted("✓")
        };
        term::say(&format!(
            "{:>10}  {:>14}  {:>14}   {mark}",
            p.lines,
            group_int(p.instructions),
            group_int(p.limit)
        ));
    }
    term::blank();
    match r.budget.iter().find(|p| p.over()) {
        Some(p) => term::say(&term::bold(&format!(
            "Carts of {} or more lines go over the instruction limit, so Shopify discards the discount.",
            p.lines
        ))),
        None if r.budget.is_empty() => term::say(&term::bold(
            "No instruction counts: this target does not report them.",
        )),
        None => term::say(&term::bold(
            "Every cart size is within the instruction limit.",
        )),
    }
}

fn check_settings(
    settings: Option<PathBuf>,
    extension: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<ExitKind> {
    let overrides = Overrides {
        extension,
        config,
        ..Overrides::default()
    };
    let resolved = resolve(settings.as_deref(), &overrides)?;
    let items = resolved.catalog.items().len();
    term::say(&format!(
        "{} settings · {} catalog items · {} currencies · seed {}",
        term::muted("✓"),
        items,
        resolved.generate.currencies.len(),
        resolved.generate.seed
    ));
    if let ResolvedTarget::Shopify(spec) = &resolved.target {
        let ext = promochaos_shopify::discover::discover(&spec.extension_dir)?;
        let schema = promochaos_shopify::schema::FunctionSchema::load(&ext.schema_path)?;
        for t in ext.run_targets() {
            if let Some(q) = &t.input_query {
                schema.load_query(&t.target, &t.export, q)?;
                term::say(&format!(
                    "{} {} · {}",
                    term::muted("✓"),
                    t.target,
                    q.display()
                ));
            }
        }
        let wasm = ext.wasm_path(spec.wasm.as_deref());
        if wasm.exists() {
            term::say(&format!("{} {}", term::muted("✓"), wasm.display()));
        } else {
            term::say(&format!(
                "{} {} is not built yet: shopify app function build",
                term::caution("▲"),
                wasm.display()
            ));
        }
    }
    if !resolved.rules.tiers.is_empty() {
        term::say(&format!(
            "{} {} tiers read from the config",
            term::muted("✓"),
            resolved.rules.tiers.len()
        ));
    }
    Ok(ExitKind::Clean)
}

fn replay(dir: &Path) -> Result<ExitKind> {
    let (finding, settings) = out::read_finding(dir)?;
    let settings_path = settings.map(PathBuf::from).ok_or_else(|| {
        Error::bad_input(format!(
            "{} does not say which promochaos.toml it came from",
            dir.join("finding.toml").display()
        ))
    })?;
    // finding.toml stores the settings path relative to the finding folder.
    let settings_path = if settings_path.is_relative() {
        dir.join(settings_path)
    } else {
        settings_path
    };
    let resolved = Resolved::load(&settings_path, &Overrides::default())?;
    let work = dir.join(".work");
    let targets = open_targets(&resolved, 1, &work)?;
    let run_target = finding_run_target(dir);
    let target = targets
        .iter()
        .find(|t| run_target.is_none() || t.info().run_target == run_target)
        .ok_or_else(|| {
            Error::bad_input("the finding's run target is no longer in the extension")
        })?;
    let (eval, still) = engine::replay(&resolved, target.as_ref(), &finding)?;
    if let Some(i) = eval.instructions() {
        term::stage(
            "run",
            &format!("1 run · {} instructions", group_int(i)),
            None,
        );
    }
    if still {
        term::say(&format!(
            "{} {}",
            term::fail_bold(&format!("✗ {} {} still fails.", finding.id, finding.rule)),
            finding.sentence
        ));
        Ok(ExitKind::Failures)
    } else {
        term::say(&format!(
            "{} {}",
            term::muted("✓"),
            term::bold(&format!("{} {} passes now.", finding.id, finding.rule))
        ));
        Ok(ExitKind::Clean)
    }
}

fn finding_run_target(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("finding.toml")).ok()?;
    let t: toml::Table = toml::from_str(&text).ok()?;
    t.get("run_target")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn report(dir: &Path, format: Format) -> Result<ExitKind> {
    let path = dir.join("results.json");
    let text = promochaos_core::util::read_input_file(&path)?;
    let results: RunResults = serde_json::from_str(&text).map_err(|e| {
        Error::bad_input(format!("{} is not promochaos results: {e}", path.display()))
    })?;
    match format {
        Format::Json => println!("{text}"),
        Format::Html => {
            let html =
                promochaos_report::html(&results).map_err(|e| Error::internal(e.to_string()))?;
            let out = dir.join("report.html");
            write_file(&out, html)?;
            term::say(&format!("{}{}", term::muted("report    "), out.display()));
        }
        Format::Junit => {
            let xml =
                promochaos_report::junit(&results).map_err(|e| Error::internal(e.to_string()))?;
            let out = dir.join("junit.xml");
            write_file(&out, xml)?;
            term::say(&format!("{}{}", term::muted("junit     "), out.display()));
        }
    }
    Ok(if results.exit_code == 1 {
        ExitKind::Failures
    } else {
        ExitKind::Clean
    })
}
