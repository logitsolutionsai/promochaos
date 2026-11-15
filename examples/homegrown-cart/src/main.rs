//! `homegrown_cart`: a small homegrown cart engine for the promochaos command target.
//!
//! It speaks the `promochaos-command/1` protocol: one JSON request on stdin, one JSON
//! response on stdout. Money is integer minor units (USD cents) everywhere.
//!
//! The default build has three seeded bugs. `--fixed` turns all three off:
//!
//! 1. The 30 percent cap is applied to each discount separately, so the 15 percent
//!    tier and the 20 percent code stack to 35 percent.
//! 2. Free shipping is decided on the subtotal before discounts.
//! 3. A retried `place_order` with the same idempotency key is processed again and
//!    uses the coupon a second time.
//!
//! Usage:
//!
//! ```text
//! homegrown_cart [--fixed] [--state <path>] < request.json
//! homegrown_cart --reset [--state <path>]
//! ```
//!
//! State (orders and coupon uses) lives in `--state`, else
//! `$PROMOCHAOS_WORK_DIR/homegrown-state.json`, else `./homegrown-state.json`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

const PROTOCOL: &str = "promochaos-command/1";
const CURRENCY: &str = "USD";
const CURRENCY_EXPONENT: u32 = 2;

const TIER_1_AT: i64 = 10_000;
const TIER_1_PERCENT: i64 = 10;
const TIER_2_AT: i64 = 20_000;
const TIER_2_PERCENT: i64 = 15;

const WELCOME_CODE: &str = "WELCOME20";
const WELCOME_PERCENT: i64 = 20;
const WELCOME_USAGE_LIMIT: u64 = 3;

const CAP_PERCENT: i64 = 30;
const FREE_SHIPPING_AT: i64 = 10_000;
const TAX_PERCENT: i64 = 8;

const STATE_FILE_NAME: &str = "homegrown-state.json";

// ---------------------------------------------------------------------------
// Protocol types

#[derive(Debug, Clone, Deserialize)]
struct Request {
    #[serde(default)]
    protocol: Option<String>,
    op: String,
    #[serde(default)]
    idempotency_key: Option<String>,
    cart: Cart,
}

#[derive(Debug, Clone, Deserialize)]
struct Cart {
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    currency: String,
    #[serde(default = "default_exponent")]
    currency_exponent: u32,
    #[serde(default)]
    lines: Vec<Line>,
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    shipping: Option<Shipping>,
}

fn default_exponent() -> u32 {
    CURRENCY_EXPONENT
}

#[derive(Debug, Clone, Deserialize)]
struct Line {
    id: String,
    quantity: i64,
    unit_price: i64,
    #[serde(default)]
    gift_card: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Shipping {
    #[serde(default)]
    #[allow(dead_code)]
    handle: Option<String>,
    price: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Response {
    subtotal: i64,
    discounts: Vec<DiscountEntry>,
    shipping: i64,
    tax: i64,
    total: i64,
    codes_applied: Vec<String>,
    codes_rejected: Vec<RejectedCode>,
    order: Option<OrderInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DiscountEntry {
    code: Option<String>,
    title: String,
    kind: String,
    amount: i64,
    line_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RejectedCode {
    code: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OrderInfo {
    id: String,
    coupon_uses: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    orders_placed: u64,
    #[serde(default)]
    orders: BTreeMap<String, Response>,
    #[serde(default)]
    coupon_uses: BTreeMap<String, u64>,
}

// ---------------------------------------------------------------------------
// Errors and options

#[derive(Debug)]
enum Failure {
    /// The request or the command line is wrong. Exit 2.
    BadRequest(String),
    /// The state file could not be read or written. Exit 1.
    State(String),
}

impl Failure {
    fn exit_code(&self) -> u8 {
        match self {
            Failure::BadRequest(_) => 2,
            Failure::State(_) => 1,
        }
    }

    fn message(&self) -> &str {
        match self {
            Failure::BadRequest(message) | Failure::State(message) => message,
        }
    }
}

fn bad(message: impl Into<String>) -> Failure {
    Failure::BadRequest(message.into())
}

#[derive(Debug, Default)]
struct Options {
    fixed: bool,
    reset: bool,
    state: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, Failure> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixed" => options.fixed = true,
            "--reset" => options.reset = true,
            "--state" => {
                let path = args.next().ok_or_else(|| bad("--state needs a path"))?;
                options.state = Some(PathBuf::from(path));
            }
            other => return Err(bad(format!("unknown argument {other:?}"))),
        }
    }
    Ok(options)
}

fn state_path(options: &Options) -> PathBuf {
    if let Some(path) = &options.state {
        return path.clone();
    }
    match std::env::var_os("PROMOCHAOS_WORK_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(STATE_FILE_NAME),
        _ => PathBuf::from(STATE_FILE_NAME),
    }
}

// ---------------------------------------------------------------------------
// Entry point

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("homegrown_cart: {}", failure.message());
            ExitCode::from(failure.exit_code())
        }
    }
}

fn real_main() -> Result<(), Failure> {
    let options = parse_args(std::env::args().skip(1))?;
    let path = state_path(&options);

    if options.reset {
        return save_state(&path, &State::default());
    }

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|error| bad(format!("could not read stdin: {error}")))?;
    let request: Request = serde_json::from_str(&raw)
        .map_err(|error| bad(format!("invalid request JSON: {error}")))?;

    let response = handle(&request, options.fixed, &path)?;
    let text = serde_json::to_string(&response)
        .map_err(|error| Failure::State(format!("could not write the response: {error}")))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{text}")
        .map_err(|error| Failure::State(format!("could not write the response: {error}")))?;
    Ok(())
}

fn handle(request: &Request, fixed: bool, path: &Path) -> Result<Response, Failure> {
    if let Some(protocol) = &request.protocol
        && protocol != PROTOCOL
    {
        return Err(bad(format!(
            "unsupported protocol {protocol:?}, expected {PROTOCOL:?}"
        )));
    }
    validate(&request.cart)?;
    match request.op.as_str() {
        "price" => price(&request.cart, fixed, None),
        "place_order" => {
            let key = request
                .idempotency_key
                .as_deref()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| bad("place_order needs a non-empty idempotency_key"))?;
            let mut state = load_state(path)?;
            let response = place_order(&mut state, key, &request.cart, fixed)?;
            save_state(path, &state)?;
            Ok(response)
        }
        other => Err(bad(format!("unknown op {other:?}"))),
    }
}

fn validate(cart: &Cart) -> Result<(), Failure> {
    if cart.currency != CURRENCY {
        return Err(bad(format!(
            "homegrown_cart prices in {CURRENCY} only, got {:?}",
            cart.currency
        )));
    }
    if cart.currency_exponent != CURRENCY_EXPONENT {
        return Err(bad(format!(
            "currency_exponent must be {CURRENCY_EXPONENT} for {CURRENCY}, got {}",
            cart.currency_exponent
        )));
    }
    for line in &cart.lines {
        if line.quantity < 0 {
            return Err(bad(format!("line {} has a negative quantity", line.id)));
        }
        if line.unit_price < 0 {
            return Err(bad(format!("line {} has a negative unit_price", line.id)));
        }
    }
    if let Some(shipping) = &cart.shipping
        && shipping.price < 0
    {
        return Err(bad("shipping price is negative"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pricing

/// `amount * percent / 100`, rounded half up, in integer arithmetic.
fn percent_of(amount: i64, percent: i64) -> i64 {
    let product = i128::from(amount) * i128::from(percent);
    let rounded = if product >= 0 {
        (product + 50) / 100
    } else {
        -((-product + 50) / 100)
    };
    // percent is at most 100 here, so the result fits in i64.
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

fn overflow() -> Failure {
    bad("cart amounts overflow")
}

/// Prices a cart. `coupon_uses` is `Some` only for `place_order`, where the
/// usage limit is enforced against the stored uses.
fn price(
    cart: &Cart,
    fixed: bool,
    coupon_uses: Option<&BTreeMap<String, u64>>,
) -> Result<Response, Failure> {
    let mut subtotal: i64 = 0;
    let mut eligible: i64 = 0;
    for line in &cart.lines {
        let amount = line
            .quantity
            .checked_mul(line.unit_price)
            .ok_or_else(overflow)?;
        subtotal = subtotal.checked_add(amount).ok_or_else(overflow)?;
        if !line.gift_card {
            eligible = eligible.checked_add(amount).ok_or_else(overflow)?;
        }
    }

    // Codes, de-duplicated without regard to case, in the order given.
    let mut codes_applied = Vec::new();
    let mut codes_rejected = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut welcome_applied = false;
    for code in &cart.codes {
        let normalized = code.trim().to_ascii_uppercase();
        if seen.contains(&normalized) {
            continue;
        }
        seen.push(normalized.clone());
        if normalized == WELCOME_CODE {
            let used = coupon_uses
                .and_then(|uses| uses.get(WELCOME_CODE).copied())
                .unwrap_or(0);
            if coupon_uses.is_some() && used >= WELCOME_USAGE_LIMIT {
                codes_rejected.push(RejectedCode {
                    code: code.clone(),
                    reason: "usage limit reached".to_string(),
                });
            } else {
                welcome_applied = true;
                codes_applied.push(WELCOME_CODE.to_string());
            }
        } else {
            codes_rejected.push(RejectedCode {
                code: code.clone(),
                reason: "unknown code".to_string(),
            });
        }
    }

    let tier_percent = if eligible >= TIER_2_AT {
        TIER_2_PERCENT
    } else if eligible >= TIER_1_AT {
        TIER_1_PERCENT
    } else {
        0
    };
    let cap = percent_of(eligible, CAP_PERCENT);
    let mut tier_amount = percent_of(eligible, tier_percent);
    let mut code_amount = if welcome_applied {
        percent_of(eligible, WELCOME_PERCENT)
    } else {
        0
    };

    if fixed {
        // The cap covers the order and product discounts together.
        tier_amount = tier_amount.min(cap);
        code_amount = code_amount.min(cap - tier_amount);
    } else {
        // Seeded bug 1: the cap is applied to each discount on its own, so a
        // 15 percent tier and the 20 percent code stack to 35 percent.
        tier_amount = tier_amount.min(cap);
        code_amount = code_amount.min(cap);
    }

    let mut discounts = Vec::new();
    if tier_amount > 0 {
        discounts.push(DiscountEntry {
            code: None,
            title: format!("Spend and save {tier_percent}% off"),
            kind: "order".to_string(),
            amount: tier_amount,
            line_ids: Vec::new(),
        });
    }
    if code_amount > 0 {
        discounts.push(DiscountEntry {
            code: Some(WELCOME_CODE.to_string()),
            title: format!("{WELCOME_CODE} {WELCOME_PERCENT}% off"),
            kind: "order".to_string(),
            amount: code_amount,
            line_ids: Vec::new(),
        });
    }

    let merchandise_discount = tier_amount + code_amount;
    let after_discounts = subtotal - merchandise_discount;
    let eligible_after_discounts = eligible - merchandise_discount;

    let shipping = cart.shipping.as_ref().map_or(0, |shipping| shipping.price);
    let free_shipping_basis = if fixed {
        eligible_after_discounts
    } else {
        // Seeded bug 2: free shipping is decided on the subtotal before
        // discounts, so a cart pushed under the threshold by its discounts
        // still ships free.
        eligible
    };
    let shipping_discount = if shipping > 0 && free_shipping_basis >= FREE_SHIPPING_AT {
        shipping
    } else {
        0
    };
    if shipping_discount > 0 {
        discounts.push(DiscountEntry {
            code: None,
            title: "Free shipping".to_string(),
            kind: "shipping".to_string(),
            amount: shipping_discount,
            line_ids: Vec::new(),
        });
    }

    let tax = percent_of(after_discounts, TAX_PERCENT);
    let total = after_discounts
        .checked_add(shipping - shipping_discount)
        .and_then(|total| total.checked_add(tax))
        .ok_or_else(overflow)?;

    Ok(Response {
        subtotal,
        discounts,
        shipping,
        tax,
        total,
        codes_applied,
        codes_rejected,
        order: None,
    })
}

fn place_order(
    state: &mut State,
    key: &str,
    cart: &Cart,
    fixed: bool,
) -> Result<Response, Failure> {
    let previous = if fixed {
        state.orders.get(key).cloned()
    } else {
        // Seeded bug 3: the idempotency key is never looked up, so a retried
        // order is placed again and uses the coupon a second time.
        None
    };
    if let Some(previous) = previous {
        return Ok(previous);
    }

    let mut response = price(cart, fixed, Some(&state.coupon_uses))?;
    for code in &response.codes_applied {
        *state.coupon_uses.entry(code.clone()).or_insert(0) += 1;
    }
    state.orders_placed += 1;
    response.order = Some(OrderInfo {
        id: format!("order-{}", state.orders_placed),
        coupon_uses: state.coupon_uses.clone(),
    });
    state.orders.insert(key.to_string(), response.clone());
    Ok(response)
}

// ---------------------------------------------------------------------------
// State file

fn load_state(path: &Path) -> Result<State, Failure> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(State::default());
        }
        Err(error) => {
            return Err(Failure::State(format!(
                "could not read state {}: {error}",
                path.display()
            )));
        }
    };
    if text.trim().is_empty() {
        return Ok(State::default());
    }
    serde_json::from_str(&text)
        .map_err(|error| Failure::State(format!("state {} is not valid: {error}", path.display())))
}

fn save_state(path: &Path, state: &State) -> Result<(), Failure> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            Failure::State(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| Failure::State(format!("could not encode state: {error}")))?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, text).map_err(|error| {
        Failure::State(format!("could not write {}: {error}", temporary.display()))
    })?;
    std::fs::rename(&temporary, path)
        .map_err(|error| Failure::State(format!("could not replace {}: {error}", path.display())))
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, unit_price: i64, quantity: i64, gift_card: bool) -> Line {
        Line {
            id: id.to_string(),
            quantity,
            unit_price,
            gift_card,
        }
    }

    fn cart(lines: Vec<Line>, codes: &[&str], shipping: Option<i64>) -> Cart {
        Cart {
            id: Some("cart-test".to_string()),
            currency: "USD".to_string(),
            currency_exponent: 2,
            lines,
            codes: codes.iter().map(|code| code.to_string()).collect(),
            shipping: shipping.map(|price| Shipping {
                handle: Some("standard".to_string()),
                price,
            }),
        }
    }

    fn merchandise_discount(response: &Response) -> i64 {
        response
            .discounts
            .iter()
            .filter(|discount| discount.kind != "shipping")
            .map(|discount| discount.amount)
            .sum()
    }

    fn shipping_discount(response: &Response) -> i64 {
        response
            .discounts
            .iter()
            .filter(|discount| discount.kind == "shipping")
            .map(|discount| discount.amount)
            .sum()
    }

    fn temp_state(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "homegrown-cart-test-{}-{name}-{nanos}.json",
            std::process::id()
        ))
    }

    fn order_request(key: &str, cart: Cart) -> Request {
        Request {
            protocol: Some(PROTOCOL.to_string()),
            op: "place_order".to_string(),
            idempotency_key: Some(key.to_string()),
            cart,
        }
    }

    #[test]
    fn percent_of_rounds_half_up() {
        assert_eq!(percent_of(25, 10), 3);
        assert_eq!(percent_of(15, 10), 2);
        assert_eq!(percent_of(14, 10), 1);
        assert_eq!(percent_of(25_000, 30), 7_500);
        assert_eq!(percent_of(0, 20), 0);
    }

    #[test]
    fn stacked_discounts_bypass_the_cap_in_the_buggy_build() {
        let cart = cart(
            vec![line("line-1", 25_000, 1, false)],
            &["WELCOME20"],
            Some(800),
        );
        let response = price(&cart, false, None).unwrap();
        // 15% tier (3750) + 20% code (5000) = 8750, which is 35% of 25000.
        assert_eq!(merchandise_discount(&response), 8_750);
        assert_eq!(response.tax, 1_300);
        assert_eq!(response.total, 17_550);
    }

    #[test]
    fn the_fixed_build_caps_the_total_at_30_percent() {
        let cart = cart(
            vec![line("line-1", 25_000, 1, false)],
            &["WELCOME20"],
            Some(800),
        );
        let response = price(&cart, true, None).unwrap();
        assert_eq!(merchandise_discount(&response), 7_500);
        assert_eq!(response.tax, 1_400);
        assert_eq!(response.total, 18_900);
        // 17500 after discounts is over 100.00, so shipping is free.
        assert_eq!(shipping_discount(&response), 800);
    }

    #[test]
    fn free_shipping_uses_the_pre_discount_subtotal_in_the_buggy_build() {
        let cart = cart(
            vec![line("line-1", 11_000, 1, false)],
            &["WELCOME20"],
            Some(800),
        );
        let response = price(&cart, false, None).unwrap();
        // 10% tier (1100) + 20% code (2200) = 3300, leaving 7700: under 100.00.
        assert_eq!(merchandise_discount(&response), 3_300);
        assert_eq!(shipping_discount(&response), 800);
        assert_eq!(response.tax, 616);
        assert_eq!(response.total, 8_316);
    }

    #[test]
    fn the_fixed_build_charges_shipping_under_the_threshold_after_discounts() {
        let cart = cart(
            vec![line("line-1", 11_000, 1, false)],
            &["WELCOME20"],
            Some(800),
        );
        let response = price(&cart, true, None).unwrap();
        assert_eq!(merchandise_discount(&response), 3_300);
        assert_eq!(shipping_discount(&response), 0);
        assert_eq!(response.shipping, 800);
        assert_eq!(response.tax, 616);
        assert_eq!(response.total, 9_116);
    }

    #[test]
    fn tax_rounds_half_up_to_the_cent() {
        let low = price(
            &cart(vec![line("line-1", 1_006, 1, false)], &[], None),
            true,
            None,
        )
        .unwrap();
        // 8% of 10.06 is 0.8048.
        assert_eq!(low.tax, 80);
        assert_eq!(low.total, 1_086);
        let high = price(
            &cart(vec![line("line-1", 1_007, 1, false)], &[], None),
            true,
            None,
        )
        .unwrap();
        // 8% of 10.07 is 0.8056.
        assert_eq!(high.tax, 81);
        assert_eq!(high.total, 1_088);
    }

    #[test]
    fn gift_cards_are_not_discounted_and_do_not_count_toward_thresholds() {
        for fixed in [false, true] {
            let cart = cart(
                vec![line("gift", 5_000, 1, true), line("shirt", 9_000, 1, false)],
                &["WELCOME20"],
                Some(800),
            );
            let response = price(&cart, fixed, None).unwrap();
            assert_eq!(response.subtotal, 14_000);
            // No tier: the eligible subtotal is 90.00. The code takes 20% of 9000.
            assert_eq!(merchandise_discount(&response), 1_800);
            assert_eq!(shipping_discount(&response), 0);
            assert_eq!(response.tax, 976);
            assert_eq!(response.total, 13_976);
        }
    }

    #[test]
    fn totals_reconcile_in_both_builds() {
        for fixed in [false, true] {
            let cart = cart(
                vec![
                    line("a", 4_500, 3, false),
                    line("b", 2_500, 2, true),
                    line("c", 1_999, 5, false),
                ],
                &["WELCOME20", "nope"],
                Some(800),
            );
            let response = price(&cart, fixed, None).unwrap();
            let merchandise = merchandise_discount(&response);
            let shipping_charged = response.shipping - shipping_discount(&response);
            assert_eq!(
                response.subtotal - merchandise + shipping_charged + response.tax,
                response.total
            );
            assert_eq!(response.codes_applied, vec!["WELCOME20".to_string()]);
            assert_eq!(response.codes_rejected.len(), 1);
            assert_eq!(response.codes_rejected[0].reason, "unknown code");
        }
    }

    #[test]
    fn a_retried_order_is_idempotent_in_the_fixed_build() {
        let path = temp_state("fixed-retry");
        let request = order_request(
            "key-1",
            cart(vec![line("line-1", 11_000, 1, false)], &["WELCOME20"], None),
        );
        let first = handle(&request, true, &path).unwrap();
        let second = handle(&request, true, &path).unwrap();
        assert_eq!(first, second);
        let order = second.order.unwrap();
        assert_eq!(order.id, "order-1");
        assert_eq!(order.coupon_uses.get("WELCOME20"), Some(&1));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_retried_order_uses_the_coupon_twice_in_the_buggy_build() {
        let path = temp_state("buggy-retry");
        let request = order_request(
            "key-1",
            cart(vec![line("line-1", 11_000, 1, false)], &["WELCOME20"], None),
        );
        let first = handle(&request, false, &path).unwrap();
        let second = handle(&request, false, &path).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.order.unwrap().id, "order-1");
        let order = second.order.unwrap();
        assert_eq!(order.id, "order-2");
        assert_eq!(order.coupon_uses.get("WELCOME20"), Some(&2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_coupon_stops_at_its_usage_limit_in_both_builds() {
        for fixed in [false, true] {
            let path = temp_state(if fixed { "fixed-limit" } else { "buggy-limit" });
            let mut applied = 0;
            for n in 1..=5 {
                let request = order_request(
                    &format!("key-{n}"),
                    cart(vec![line("line-1", 5_000, 1, false)], &["WELCOME20"], None),
                );
                let response = handle(&request, fixed, &path).unwrap();
                if response.codes_applied.contains(&"WELCOME20".to_string()) {
                    applied += 1;
                } else {
                    assert_eq!(response.codes_rejected[0].reason, "usage limit reached");
                }
            }
            assert_eq!(applied, 3);
            let state = load_state(&path).unwrap();
            assert_eq!(state.coupon_uses.get("WELCOME20"), Some(&3));
            assert_eq!(state.orders_placed, 5);
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn reset_clears_the_state() {
        let path = temp_state("reset");
        let request = order_request(
            "key-1",
            cart(vec![line("line-1", 5_000, 1, false)], &["WELCOME20"], None),
        );
        handle(&request, true, &path).unwrap();
        save_state(&path, &State::default()).unwrap();
        assert_eq!(load_state(&path).unwrap(), State::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_full_request_parses() {
        let raw = r#"{
            "protocol": "promochaos-command/1",
            "op": "price",
            "idempotency_key": null,
            "cart": {
                "id": "cart-0007",
                "currency": "USD",
                "currency_exponent": 2,
                "lines": [
                    {
                        "id": "line-1",
                        "variant_id": "gid://shopify/ProductVariant/2001",
                        "product_id": "gid://shopify/Product/1001",
                        "title": "Linen Shirt",
                        "quantity": 2,
                        "unit_price": 4500,
                        "tags": ["multi-buy", "shirt"],
                        "collections": ["gid://shopify/Collection/10"],
                        "gift_card": false,
                        "requires_shipping": true,
                        "attributes": { "engraving": "AB" }
                    }
                ],
                "codes": ["WELCOME20"],
                "buyer": { "kind": "consumer", "customer_id": "gid://shopify/Customer/1", "email": "a@example.com", "tags": [], "company_id": null },
                "shipping": { "handle": "standard", "price": 800 }
            },
            "config": null
        }"#;
        let request: Request = serde_json::from_str(raw).unwrap();
        let response = handle(&request, true, Path::new("unused-state.json")).unwrap();
        assert_eq!(response.subtotal, 9_000);
        // 20% of 9000; 7200 after discounts, so shipping is charged.
        assert_eq!(merchandise_discount(&response), 1_800);
        assert_eq!(response.tax, 576);
        assert_eq!(response.total, 9_000 - 1_800 + 800 + 576);
    }

    #[test]
    fn bad_requests_are_rejected() {
        let mut negative = cart(vec![line("line-1", -1, 1, false)], &[], None);
        assert!(matches!(validate(&negative), Err(Failure::BadRequest(_))));
        negative.lines[0].unit_price = 100;
        negative.currency = "EUR".to_string();
        assert!(matches!(validate(&negative), Err(Failure::BadRequest(_))));
        assert!(parse_args(vec!["--state".to_string()]).is_err());
        assert!(parse_args(vec!["--what".to_string()]).is_err());
        let options = parse_args(vec!["--fixed".to_string(), "--reset".to_string()]).unwrap();
        assert!(options.fixed && options.reset);
    }
}
