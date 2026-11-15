//! The run: generate, evaluate, check, shrink, group. Target-agnostic; the
//! CLI writes the files.

use crate::cart::Cart;
use crate::error::{Error, Result};
use crate::eval::{Evaluation, Target};
use crate::generate::metamorphic::{Twin, make_twin, twins};
use crate::generate::{GenKind, GeneratedCart, generate, generate_sizes};
use crate::money::{self, group_int};
use crate::results::{
    BudgetPoint, CartRecord, CartRow, Counts, Expect, FindingRecord, RuleStat, RunResults,
    ToolInfo, TwinRecord, not_checked,
};
use crate::rules::{
    self, CODES_ACCEPTED, Check, DETERMINISTIC, Fingerprint, IDEMPOTENT_RETRY, MIN_SUBTOTAL,
    NEVER_DISCOUNT, NO_TRAP, OutcomeKind, RuleContext, Status, TIERS, USAGE_LIMIT,
    WITHIN_INSTRUCTION_LIMIT, check_cart,
};
use crate::settings::{Relation, Resolved};
use crate::shrink::{Hints, Probe, shrink};
use crate::util::{first_line, hash_hex, utc_now_rfc3339};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Clone)]
pub struct EngineOptions {
    pub jobs: usize,
    pub shrink_budget: usize,
    /// Results folder, as shown in replay commands (`promochaos-results`).
    pub results_dir_display: String,
    /// Only the size sweep (`promochaos budget`).
    pub budget_only: bool,
    /// Reports progress lines (stage name, text, seconds).
    pub progress: Option<fn(&str, &str, Option<f64>)>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions {
            jobs: 1,
            shrink_budget: 200,
            results_dir_display: "promochaos-results".to_string(),
            budget_only: false,
            progress: None,
        }
    }
}

/// Timings and counts for the terminal's stage lines.
#[derive(Debug, Clone, Default)]
pub struct Stages {
    pub run_secs: f64,
    pub shrink_secs: f64,
    pub shrink_runs: usize,
}

/// Evaluates every cart, `jobs` at a time. Results keep the input order.
pub fn evaluate_all(target: &dyn Target, carts: &[&Cart], jobs: usize) -> Result<Vec<Evaluation>> {
    let n = carts.len();
    let slots: Vec<Mutex<Option<Result<Evaluation>>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let workers = jobs.clamp(1, 64).min(n.max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let Some(cart) = carts.get(i) else { break };
                    let r = target.evaluate(cart);
                    let failed = r.is_err();
                    if let Some(slot) = slots.get(i)
                        && let Ok(mut guard) = slot.lock()
                    {
                        *guard = Some(r);
                    }
                    if failed {
                        // Stop handing out work; the error is reported below.
                        next.store(n, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    let mut out = Vec::with_capacity(n);
    for slot in slots {
        let value = slot
            .into_inner()
            .map_err(|_| Error::internal("a worker thread panicked"))?;
        match value {
            Some(Ok(e)) => out.push(e),
            Some(Err(e)) => return Err(e),
            None => {
                return Err(Error::internal(
                    "a cart was not evaluated because an earlier cart failed",
                ));
            }
        }
    }
    Ok(out)
}

/// A failure seen on one cart or pair, before grouping.
#[derive(Clone)]
enum Subject {
    Cart(usize),
    Twin(usize),
    Determinism(usize),
    Sequence(usize),
}

#[derive(Clone)]
struct Seen {
    subject: Subject,
    check: Check,
}

struct Group {
    fingerprint: Fingerprint,
    members: Vec<Seen>,
    of_checked: Option<usize>,
}

fn cart_rows(cart: &Cart) -> (Vec<CartRow>, usize) {
    let rows: Vec<CartRow> = cart
        .lines
        .iter()
        .take(10)
        .map(|l| CartRow {
            line: l
                .id
                .rsplit('/')
                .next()
                .map(|n| format!("CartLine/{n}"))
                .unwrap_or_default(),
            name: l.merchandise.display_name(),
            quantity: l.quantity,
            subtotal: cart.money(l.subtotal()),
        })
        .collect();
    let more = cart.lines.len().saturating_sub(rows.len());
    (rows, more)
}

fn config_summary(resolved: &Resolved) -> Option<String> {
    if resolved.rules.tiers.is_empty() {
        return None;
    }
    Some(
        resolved
            .rules
            .tiers
            .iter()
            .map(|t| {
                format!(
                    "{}% from {}",
                    t.percent,
                    money::format_display(t.threshold, &resolved.store_currency)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn median(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

fn expect_for(ctx: &RuleContext, check: &Check, cart: &Cart, relation: Option<Relation>) -> Expect {
    if relation.is_some() {
        return Expect::TwinKeepsDiscount;
    }
    match check.rule.as_str() {
        NO_TRAP => Expect::NoTrap,
        TIERS => {
            let eligible = rules::merchant::eligible_subtotal(ctx, cart);
            let percent = ctx
                .resolved
                .rules
                .tiers
                .iter()
                .rev()
                .find(|t| rules::merchant::to_presentment(ctx, cart, t.threshold) <= eligible)
                .map(|t| t.percent.clone());
            Expect::OrderPercent { percent }
        }
        MIN_SUBTOTAL => Expect::NoDiscount,
        NEVER_DISCOUNT => Expect::LinesNotDiscounted {
            line_ids: cart
                .lines
                .iter()
                .filter(|l| ctx.resolved.rules.never_discount.reason(&l.merchandise).is_some())
                .map(|l| l.id.clone())
                .collect(),
        },
        CODES_ACCEPTED => Expect::CodesOnly {
            allowed: ctx.resolved.codes.clone(),
        },
        WITHIN_INSTRUCTION_LIMIT => Expect::NotExportable {
            reason: "a native test cannot count Wasm instructions; the fixture and promochaos replay can".to_string(),
        },
        other => Expect::NotExportable {
            reason: format!("{other} has no exported assertion; use promochaos replay"),
        },
    }
}

/// Runs the whole pipeline against `target`.
pub fn run(
    resolved: &Resolved,
    target: &dyn Target,
    opts: &EngineOptions,
) -> Result<(RunResults, Stages)> {
    let started = Instant::now();
    let started_at = utc_now_rfc3339();
    let ctx = RuleContext::new(resolved);
    let progress = |stage: &str, text: &str, secs: Option<f64>| {
        if let Some(p) = opts.progress {
            p(stage, text, secs);
        }
    };

    let generated = if opts.budget_only {
        generate_sizes(resolved)
    } else {
        generate(resolved)
    };
    let carts: Vec<GeneratedCart> = generated.carts;
    let twin_list: Vec<Twin> = if opts.budget_only {
        Vec::new()
    } else {
        twins(resolved, &carts)
    };
    let mut by_generator: Vec<(String, usize)> = Vec::new();
    for kind in [
        GenKind::Boundary,
        GenKind::Size,
        GenKind::Edge,
        GenKind::Pairwise,
        GenKind::Random,
    ] {
        let n = carts.iter().filter(|c| c.origin.kind == kind).count();
        if n > 0 {
            by_generator.push((kind.as_str().to_string(), n));
        }
    }
    let gen_text = {
        let parts: Vec<String> = by_generator
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        let twins_text = if twin_list.is_empty() {
            String::new()
        } else {
            format!(" + {} twins", twin_list.len())
        };
        format!("{} carts{twins_text} · {}", carts.len(), parts.join(" · "))
    };
    progress("generate", &gen_text, None);

    // Evaluate carts and twins.
    let run_start = Instant::now();
    let cart_refs: Vec<&Cart> = carts.iter().map(|g| &g.cart).collect();
    let evals = evaluate_all(target, &cart_refs, opts.jobs)?;
    let twin_refs: Vec<&Cart> = twin_list.iter().map(|t| &t.cart).collect();
    let twin_evals = evaluate_all(target, &twin_refs, opts.jobs)?;
    let mut runs = evals.len() + twin_evals.len();

    // Determinism: a sample, run a second time.
    let sample_n = if opts.budget_only {
        0
    } else {
        resolved.rules.deterministic_sample.min(carts.len())
    };
    let step = carts.len().checked_div(sample_n).map_or(1, |s| s.max(1));
    let sample_idx: Vec<usize> = (0..carts.len()).step_by(step).take(sample_n).collect();
    let sample_refs: Vec<&Cart> = sample_idx
        .iter()
        .filter_map(|i| carts.get(*i))
        .map(|g| &g.cart)
        .collect();
    let second = evaluate_all(target, &sample_refs, opts.jobs)?;
    runs += second.len();
    let run_secs = run_start.elapsed().as_secs_f64();

    let instr: Vec<u64> = evals.iter().filter_map(Evaluation::instructions).collect();
    let run_text = if ctx.is_command {
        format!("{runs} runs of the command")
    } else {
        match median(instr.clone()) {
            Some(m) => format!(
                "{runs} runs in-process · median {} instructions",
                group_int(m)
            ),
            None => format!("{runs} runs"),
        }
    };
    progress("run", &run_text, Some(run_secs));

    // Single-cart rules.
    let mut stats: BTreeMap<String, RuleStat> = BTreeMap::new();
    let mut tally = |c: &Check| {
        let s = stats.entry(c.rule.clone()).or_insert_with(|| RuleStat {
            rule: c.rule.clone(),
            from: rules::rule_source(&c.rule).to_string(),
            ..RuleStat::default()
        });
        match c.status {
            Status::Pass => s.checked += 1,
            Status::Fail => {
                s.checked += 1;
                s.failed += 1;
            }
            Status::Warn => {
                s.checked += 1;
                s.warned += 1;
            }
            Status::NotApplicable => {
                s.not_applicable += 1;
                let reason = c.reason.clone().unwrap_or_default();
                if let Some(r) = s.reasons.iter_mut().find(|(t, _)| *t == reason) {
                    r.1 += 1;
                } else {
                    s.reasons.push((reason, 1));
                }
            }
        }
    };
    let mut seen: Vec<Seen> = Vec::new();
    let mut cart_problems: Vec<Vec<String>> = vec![Vec::new(); carts.len()];
    for (i, (g, e)) in carts.iter().zip(&evals).enumerate() {
        for c in check_cart(&ctx, &g.cart, e) {
            tally(&c);
            if c.is_problem() {
                if let Some(p) = cart_problems.get_mut(i) {
                    p.push(c.rule.clone());
                }
                seen.push(Seen {
                    subject: Subject::Cart(i),
                    check: c,
                });
            }
        }
    }

    // Twin rules.
    let mut twins_checked: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (ti, (t, te)) in twin_list.iter().zip(&twin_evals).enumerate() {
        let (Some(orig), Some(oe)) = (carts.get(t.original), evals.get(t.original)) else {
            continue;
        };
        let c = rules::metamorphic::check(t.relation, &orig.cart, oe, &t.cart, te, &t.note());
        tally(&c);
        if c.status != Status::NotApplicable {
            *twins_checked.entry(t.relation.id()).or_insert(0) += 1;
        }
        if c.is_problem() {
            seen.push(Seen {
                subject: Subject::Twin(ti),
                check: c,
            });
        }
    }

    // Determinism.
    for (idx, b) in sample_idx.iter().zip(&second) {
        let Some(a) = evals.get(*idx) else { continue };
        let c = if a.output_text == b.output_text
            && a.crash.as_deref().map(first_line) == b.crash.as_deref().map(first_line)
        {
            Check::pass(DETERMINISTIC)
        } else {
            Check::fail(
                DETERMINISTIC,
                OutcomeKind::RuleBroken,
                "differs",
                "The same input gave different output on a second run.",
            )
        };
        tally(&c);
        if c.is_problem() {
            seen.push(Seen {
                subject: Subject::Determinism(*idx),
                check: c,
            });
        }
    }

    // Command sequences: idempotent retry and usage limits.
    let mut sequences: Vec<(Cart, Check)> = Vec::new();
    let mut order_runs = 0;
    if ctx.is_command && target.supports_orders() && !opts.budget_only {
        let code_carts: Vec<Cart> = {
            let mut v: Vec<Cart> = carts
                .iter()
                .filter(|g| {
                    !g.cart.codes.is_empty()
                        && g.cart
                            .codes
                            .iter()
                            .all(|c| resolved.codes.iter().any(|a| a.eq_ignore_ascii_case(c)))
                        && g.cart.lines.len() <= 10
                })
                .map(|g| g.cart.clone())
                .take(5)
                .collect();
            if v.is_empty()
                && let (Some(g), Some(code)) = (carts.first(), resolved.codes.first())
            {
                let mut c = g.cart.clone();
                c.codes = vec![code.clone()];
                v.push(c);
            }
            v
        };
        if resolved.rules.idempotent_retry {
            for (n, cart) in code_carts.iter().enumerate() {
                target.reset()?;
                let key = format!("promochaos-retry-{}", n + 1);
                let first = target.place_order(cart, &key);
                let again = target.place_order(cart, &key);
                order_runs += 2;
                let (Some(first), Some(again)) = (first, again) else {
                    break;
                };
                let c = rules::command::idempotent_retry(cart, &first?, &again?);
                tally(&c);
                if c.is_problem() {
                    sequences.push((cart.clone(), c));
                }
            }
        }
        for (code, limit) in &resolved.rules.usage_limit {
            let Some(base) = code_carts.first() else {
                break;
            };
            let mut cart = base.clone();
            cart.codes = vec![code.clone()];
            target.reset()?;
            let orders = limit + 2;
            let mut applied = 0;
            for n in 0..orders {
                let key = format!("promochaos-usage-{code}-{}", n + 1);
                let Some(r) = target.place_order(&cart, &key) else {
                    break;
                };
                order_runs += 1;
                let e = r?;
                if e.ledger.as_ref().is_some_and(|l| {
                    l.codes_accepted
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(code))
                }) {
                    applied += 1;
                }
            }
            let c = rules::command::usage_limit(&cart, code, *limit, applied, orders);
            tally(&c);
            if c.is_problem() {
                sequences.push((cart, c));
            }
        }
        target.reset()?;
    }
    runs += order_runs;
    for (i, (_, c)) in sequences.iter().enumerate() {
        seen.push(Seen {
            subject: Subject::Sequence(i),
            check: c.clone(),
        });
    }

    // Group by fingerprint, in order of first sighting.
    let mut groups: Vec<Group> = Vec::new();
    for s in seen {
        let fp = s.check.fingerprint();
        match groups.iter_mut().find(|g| g.fingerprint == fp) {
            Some(g) => g.members.push(s),
            None => {
                let of_checked = match &s.subject {
                    Subject::Twin(_) => twins_checked.get(s.check.rule.as_str()).copied(),
                    _ => None,
                };
                groups.push(Group {
                    fingerprint: fp,
                    members: vec![s],
                    of_checked,
                });
            }
        }
    }
    // Failures first, then warnings; inside each, the order they were seen.
    groups.sort_by_key(|g| g.fingerprint.kind == OutcomeKind::Warning);

    // Shrink and build findings.
    let shrink_start = Instant::now();
    let mut shrink_runs_total = 0;
    let mut findings: Vec<FindingRecord> = Vec::new();
    let mut curve_points: Vec<BudgetPoint> = Vec::new();
    for (gi, group) in groups.iter().enumerate() {
        // Start from the smallest member.
        let Some(first) = group.members.iter().min_by_key(|m| match m.subject {
            Subject::Cart(i) | Subject::Determinism(i) => carts
                .get(i)
                .map(|g| g.cart.lines.len())
                .unwrap_or(usize::MAX),
            Subject::Twin(i) => twin_list
                .get(i)
                .map(|t| t.cart.lines.len())
                .unwrap_or(usize::MAX),
            Subject::Sequence(_) => 0,
        }) else {
            continue;
        };
        let fp = group.fingerprint.clone();
        let relation = match first.subject {
            Subject::Twin(i) => twin_list.get(i).map(|t| t.relation),
            _ => None,
        };
        let start_cart: Cart = match first.subject {
            Subject::Cart(i) | Subject::Determinism(i) => carts.get(i).map(|g| g.cart.clone()),
            Subject::Twin(i) => twin_list
                .get(i)
                .and_then(|t| carts.get(t.original))
                .map(|g| g.cart.clone()),
            Subject::Sequence(i) => sequences.get(i).map(|(c, _)| c.clone()),
        }
        .ok_or_else(|| Error::internal("finding without a cart"))?;

        let by_lines = fp.rule == WITHIN_INSTRUCTION_LIMIT;
        let boundaries: Vec<i64> = resolved
            .thresholds()
            .iter()
            .map(|t| rules::merchant::to_presentment(&ctx, &start_cart, *t))
            .collect();
        let hints = Hints {
            boundaries,
            by_line_count: by_lines,
        };
        let ineligible: Vec<crate::catalog::Item> = resolved
            .catalog
            .items()
            .into_iter()
            .filter(|i| {
                i.is_gift_card()
                    || resolved
                        .rules
                        .never_discount
                        .reason(&i.merchandise)
                        .is_some()
            })
            .collect();

        let sequence = matches!(first.subject, Subject::Sequence(_));
        let shrunk = if sequence {
            crate::shrink::Shrunk {
                cart: start_cart.clone(),
                runs: 0,
                flaky: false,
                original_lines: start_cart.lines.len(),
                curve: Vec::new(),
                steps: vec!["sequences are not shrunk".to_string()],
            }
        } else {
            let fp_c = fp.clone();
            let oracle = |c: &Cart| -> Probe {
                let Ok(e) = target.evaluate(c) else {
                    return Probe::no();
                };
                let instructions = e.instructions();
                let limit = e
                    .metrics
                    .map(|m| rules::limits::instruction_limit(m.scale_factor));
                let reproduces = match relation {
                    Some(rel) => match make_twin(
                        rel,
                        c,
                        &ineligible,
                        &resolved.rules.never_discount,
                        &resolved.store_currency,
                    ) {
                        Some((tc, title, qty)) => match target.evaluate(&tc) {
                            Ok(te) => {
                                let note = rules::metamorphic::TwinNote {
                                    title,
                                    quantity: qty,
                                };
                                rules::metamorphic::check(rel, c, &e, &tc, &te, &note).fingerprint()
                                    == fp_c
                            }
                            Err(_) => false,
                        },
                        None => false,
                    },
                    None if fp_c.rule == DETERMINISTIC => match target.evaluate(c) {
                        Ok(e2) => e2.output_text != e.output_text,
                        Err(_) => false,
                    },
                    None => check_cart(&ctx, c, &e)
                        .iter()
                        .any(|ch| ch.is_problem() && ch.fingerprint() == fp_c),
                };
                Probe {
                    reproduces,
                    instructions,
                    limit,
                }
            };
            shrink(&start_cart, opts.shrink_budget, &hints, oracle)
        };
        shrink_runs_total += shrunk.runs;

        // Final run of the shrunk cart for the record.
        let final_cart = shrunk.cart.clone();
        let final_eval = target.evaluate(&final_cart)?;
        shrink_runs_total += 1;
        let final_checks = check_cart(&ctx, &final_cart, &final_eval);
        let mut twin_record: Option<TwinRecord> = None;
        let mut likely_cause = None;
        let final_check: Check = if let Some(rel) = relation {
            likely_cause = Some(rules::metamorphic::likely_cause(rel).to_string());
            match make_twin(
                rel,
                &final_cart,
                &ineligible,
                &resolved.rules.never_discount,
                &resolved.store_currency,
            ) {
                Some((tc, title, qty)) => {
                    let te = target.evaluate(&tc)?;
                    shrink_runs_total += 1;
                    let note = rules::metamorphic::TwinNote {
                        title,
                        quantity: qty,
                    };
                    let c =
                        rules::metamorphic::check(rel, &final_cart, &final_eval, &tc, &te, &note);
                    twin_record = Some(TwinRecord {
                        relation: rel,
                        discount: rules::metamorphic::total_discount(&te),
                        cart: tc,
                        input: te.input.clone(),
                        output: te.output.clone(),
                    });
                    if c.fingerprint() == fp {
                        c
                    } else {
                        first.check.clone()
                    }
                }
                None => first.check.clone(),
            }
        } else if sequence {
            first.check.clone()
        } else {
            final_checks
                .iter()
                .find(|c| c.fingerprint() == fp)
                .cloned()
                .unwrap_or_else(|| first.check.clone())
        };

        let n = final_cart.lines.len();
        let noun = if n == 1 { "line" } else { "lines" };
        let shrunk_summary = if shrunk.flaky {
            "did not reproduce, marked flaky".to_string()
        } else if sequence {
            // Order sequences are replayed as placed; "4 sequences a
            // sequence of orders" read badly in the terminal.
            "not shrunk".to_string()
        } else if by_lines {
            format!("shrunk to {n} {noun}")
        } else if shrunk.original_lines == n {
            format!("already minimal, {n} {noun}")
        } else {
            format!("shrunk from {} lines to {n}", shrunk.original_lines)
        };

        let mut sentence = final_check.sentence.clone();
        let mut details = final_check.details.clone();
        let mut curve: Vec<BudgetPoint> = shrunk
            .curve
            .iter()
            .map(|p| BudgetPoint {
                lines: p.lines,
                instructions: p.instructions,
                limit: p.limit,
            })
            .collect();
        // A timed-out run has no measured count, so it keeps its own
        // sentence instead of the measured line-count summary.
        if by_lines && !shrunk.flaky && fp.key == "instructions" {
            sentence = format!(
                "Carts with {n} or more lines go over the instruction limit, so Shopify discards the discount."
            );
            details.clear();
            let at_200 = carts
                .iter()
                .zip(&evals)
                .find(|(g, _)| g.origin.kind == GenKind::Size && g.cart.lines.len() == 200)
                .and_then(|(_, e)| e.measured());
            if let Some(m) = at_200 {
                details.push(rules::Detail::new(
                    "At 200 lines",
                    format!(
                        "{} instructions against a limit of {}",
                        group_int(m.instructions),
                        group_int(rules::limits::instruction_limit(m.scale_factor))
                    ),
                ));
            }
            if let Some(m) = final_eval.measured() {
                details.push(rules::Detail::new(
                    "Smallest failing",
                    format!("{n} {noun}, {} instructions", group_int(m.instructions)),
                ));
            }
            let at = |lines: usize| {
                carts
                    .iter()
                    .zip(&evals)
                    .find(|(g, _)| g.origin.kind == GenKind::Size && g.cart.lines.len() == lines)
                    .and_then(|(_, e)| e.instructions())
            };
            if let (Some(a), Some(b)) = (at(100), at(200))
                && a > 0
            {
                let ratio = b as f64 / a as f64;
                let shape = if ratio >= 3.5 {
                    "Grows with the square of the line count"
                } else if ratio >= 2.4 {
                    "Grows faster than the line count, so part of the work grows with its square"
                } else if ratio >= 1.7 {
                    "Grows in line with the line count"
                } else {
                    "Grows slowly with the line count"
                };
                details.push(rules::Detail::new(
                    "Shape",
                    format!(
                        "{shape}: doubling 100 lines to 200 multiplied the count by {ratio:.1}"
                    ),
                ));
            }
            curve_points.extend(curve.iter().copied());
        }
        if !by_lines {
            curve.clear();
        }
        let (rows, rows_more) = cart_rows(&final_cart);
        let id = format!("F-{:03}", gi + 1);
        let results_dir = opts.results_dir_display.trim_end_matches('/');
        let severity = if fp.kind == OutcomeKind::Warning {
            "warning"
        } else {
            "failure"
        };
        let mut expect = expect_for(&ctx, &final_check, &final_cart, relation);
        // The exported twin test asserts only that the twin keeps a
        // discount. When the twin's discount changed but did not vanish,
        // that test would pass on the buggy build, so it is not exported.
        if expect == Expect::TwinKeepsDiscount {
            let lost = twin_record.as_ref().and_then(|t| t.discount) == Some(0)
                && rules::metamorphic::total_discount(&final_eval).unwrap_or(0) > 0;
            if !lost {
                expect = Expect::NotExportable {
                    reason: "the twin's discount changed but did not vanish, and a native test cannot compare amounts; use promochaos replay".to_string(),
                };
            }
        }
        let ledger = final_eval.ledger.clone();
        let expected_discount = final_check.expected_discount.map(|m| final_cart.money(m));
        let count_unit = match first.subject {
            Subject::Twin(_) => "pairs",
            Subject::Sequence(_) => "sequences",
            _ => "carts",
        };
        findings.push(FindingRecord {
            id: id.clone(),
            label: format!("F{}", gi + 1),
            rule: fp.rule.clone(),
            kind: fp.kind.as_str().to_string(),
            kind_label: fp.kind.label().to_string(),
            severity: severity.to_string(),
            fingerprint: fp.clone(),
            sentence,
            count: group.members.len(),
            count_unit: count_unit.to_string(),
            of_checked: group.of_checked,
            shrunk_summary,
            flaky: shrunk.flaky,
            shrink_runs: shrunk.runs,
            details,
            cart: final_cart.clone(),
            cart_rows: rows,
            cart_rows_more: rows_more,
            input: final_eval.input.clone(),
            output: final_eval.output.clone(),
            output_pretty: final_eval
                .output
                .as_ref()
                .and_then(|o| serde_json::to_string_pretty(o).ok())
                .unwrap_or_else(|| {
                    if final_eval.output_text.is_empty() {
                        "(no output)".to_string()
                    } else {
                        final_eval.output_text.clone()
                    }
                }),
            logs_first_line: if final_eval.logs.trim().is_empty() {
                None
            } else {
                Some(first_line(&final_eval.logs))
            },
            ledger,
            expected_discount,
            twin: twin_record,
            likely_cause,
            curve,
            expect,
            replay: format!("promochaos replay {results_dir}/findings/{id}"),
            test_path: None,
            fixture_path: None,
        });
    }
    let shrink_secs = shrink_start.elapsed().as_secs_f64();
    runs += shrink_runs_total;
    if !groups.is_empty() {
        progress(
            "shrink",
            &format!(
                "{} {}, {shrink_runs_total} runs",
                groups.len(),
                if groups.len() == 1 {
                    "failure"
                } else {
                    "failures"
                }
            ),
            Some(shrink_secs),
        );
    }

    // Budget points: the size sweep, plus every size tried while shrinking.
    let mut budget: Vec<BudgetPoint> = carts
        .iter()
        .zip(&evals)
        .filter(|(g, _)| g.origin.kind == GenKind::Size)
        .filter_map(|(g, e)| {
            e.measured().map(|m| BudgetPoint {
                lines: g.cart.lines.len(),
                instructions: m.instructions,
                limit: rules::limits::instruction_limit(m.scale_factor),
            })
        })
        .collect();
    budget.extend(curve_points);
    budget.sort_by_key(|p| (p.lines, p.instructions));
    budget.dedup_by_key(|p| p.lines);

    // Rules table in display order.
    if sample_n > 0
        && let Some(s) = stats.get_mut(DETERMINISTIC)
    {
        s.reasons
            .push((format!("A sample of {sample_n}, run twice"), 0));
    }
    let mut rule_stats: Vec<RuleStat> = stats.into_values().collect();
    rule_stats.sort_by_key(|s| rules::rule_index(&s.rule));
    for s in &mut rule_stats {
        s.reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    let records: Vec<CartRecord> = carts
        .iter()
        .zip(&evals)
        .zip(&cart_problems)
        .map(|((g, e), probs)| {
            let mut problems = probs.clone();
            problems.sort();
            problems.dedup();
            CartRecord {
                id: g.cart.id.clone(),
                generator: g.origin.kind.as_str().to_string(),
                label: g.origin.label.clone(),
                lines: g.cart.lines.len(),
                currency: g.cart.currency.clone(),
                subtotal: g.cart.subtotal(),
                instructions: e.instructions(),
                output_bytes: e.output_bytes,
                crashed: e.crash.is_some(),
                discount: e
                    .ledger
                    .as_ref()
                    .map(|l| l.merchandise_total() + l.shipping_total()),
                problems,
            }
        })
        .collect();

    let info = target.info();
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    let mut hash_src: Vec<u8> = Vec::new();
    if let Some(p) = &resolved.settings_path {
        inputs.insert("settings".to_string(), crate::util::portable_path(p));
        hash_src.extend(std::fs::read(p).unwrap_or_default());
    }
    if let Some(p) = &resolved.catalog_path {
        inputs.insert("catalog".to_string(), crate::util::portable_path(p));
        hash_src.extend(std::fs::read(p).unwrap_or_default());
    }
    if let Some(p) = &resolved.config_path {
        inputs.insert("config".to_string(), crate::util::portable_path(p));
        hash_src.extend(std::fs::read(p).unwrap_or_default());
    }
    if let Some(w) = &info.wasm {
        hash_src.extend(std::fs::read(w).unwrap_or_default());
    }
    let mut notes = Vec::new();
    if !ctx.is_command {
        notes.push("function-runner 9.2.3 hard-codes a 20,000-byte output limit; Shopify's documentation says 40 kB. promochaos checks against the documentation and warns above 20,000 bytes.".to_string());
    }
    let failures = findings.iter().filter(|f| f.severity == "failure").count();
    let results = RunResults {
        schema_version: crate::results::RESULTS_SCHEMA_VERSION,
        tool: ToolInfo {
            name: "promochaos".to_string(),
            version: crate::TOOL_VERSION.to_string(),
            function_runner: info.function_runner_version.clone(),
            api_version: info.api_version.clone(),
        },
        started_at: Some(started_at),
        duration_ms: Some(started.elapsed().as_millis() as u64),
        seed: resolved.generate.seed,
        config_summary: config_summary(resolved),
        config_label: resolved
            .config_path
            .as_ref()
            .map(|p| crate::util::display_path(p, &resolved.base_dir)),
        target: info,
        inputs_hash: hash_hex(&hash_src),
        counts: Counts {
            carts: carts.len(),
            twins: twin_list.len(),
            runs,
            shrink_runs: shrink_runs_total,
            determinism_runs: second.len(),
            order_runs,
            by_generator,
        },
        median_instructions: median(instr),
        budget,
        rules: rule_stats,
        findings,
        carts: records,
        not_checked: not_checked(ctx.is_command),
        notes,
        results_dir: opts.results_dir_display.clone(),
        exit_code: if failures > 0 { 1 } else { 0 },
        inputs,
    };
    let stages = Stages {
        run_secs,
        shrink_secs,
        shrink_runs: shrink_runs_total,
    };
    Ok((results, stages))
}

/// Re-runs one finding's shrunk cart and checks its fingerprint again.
/// Returns the evaluation and whether the failure still shows.
pub fn replay(
    resolved: &Resolved,
    target: &dyn Target,
    finding: &FindingRecord,
) -> Result<(Evaluation, bool)> {
    let ctx = RuleContext::new(resolved);
    let eval = target.evaluate(&finding.cart)?;
    let fp = &finding.fingerprint;
    let still = if let Some(t) = &finding.twin {
        let te = target.evaluate(&t.cart)?;
        let note = rules::metamorphic::TwinNote::default();
        rules::metamorphic::check(t.relation, &finding.cart, &eval, &t.cart, &te, &note)
            .is_problem()
    } else if fp.rule == DETERMINISTIC {
        let again = target.evaluate(&finding.cart)?;
        again.output_text != eval.output_text
    } else if fp.rule == IDEMPOTENT_RETRY {
        target.reset()?;
        let a = target.place_order(&finding.cart, "promochaos-replay");
        let b = target.place_order(&finding.cart, "promochaos-replay");
        target.reset()?;
        match (a, b) {
            (Some(a), Some(b)) => {
                rules::command::idempotent_retry(&finding.cart, &a?, &b?).is_problem()
            }
            _ => false,
        }
    } else if fp.rule == USAGE_LIMIT {
        let code = finding.cart.codes.first().cloned().unwrap_or_default();
        let limit = resolved.rules.usage_limit.get(&code).copied().unwrap_or(0);
        target.reset()?;
        let mut applied = 0;
        for n in 0..(limit + 2) {
            if let Some(r) = target.place_order(&finding.cart, &format!("promochaos-replay-{n}"))
                && r?.ledger.as_ref().is_some_and(|l| {
                    l.codes_accepted
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(&code))
                })
            {
                applied += 1;
            }
        }
        target.reset()?;
        applied > limit
    } else {
        check_cart(&ctx, &finding.cart, &eval)
            .iter()
            .any(|c| c.is_problem() && c.fingerprint() == *fp)
    };
    Ok((eval, still))
}
