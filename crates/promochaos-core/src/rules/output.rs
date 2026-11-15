//! `valid_output`: the output matches the schema's output types (checked by
//! the target and passed in as `schema_errors`), every target id exists in
//! the input, percentages run from 0 to 100 and amounts are 0 or more.

use super::{Check, OutcomeKind, RuleContext, VALID_OUTPUT};
use crate::cart::Cart;
use crate::eval::Evaluation;
use crate::money;
use rust_decimal::Decimal;
use serde_json::Value;

/// Replaces list indexes in a path so errors group by cause:
/// `operations[3].productDiscountsAdd` becomes `operations[].productDiscountsAdd`.
pub fn normalize_error(e: &str) -> String {
    let mut out = String::with_capacity(e.len());
    let mut in_brackets = false;
    for ch in e.chars() {
        match ch {
            '[' => {
                in_brackets = true;
                out.push('[');
            }
            ']' => {
                in_brackets = false;
                out.push(']');
            }
            c if in_brackets && c.is_ascii_digit() => {}
            c => out.push(c),
        }
    }
    out
}

fn walk(v: &Value, path: &str, cart: &Cart, errors: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match k.as_str() {
                    "percentage" => {
                        if let Some(d) = child.get("value").and_then(money::decimal_from_json)
                            && (d < Decimal::ZERO || d > Decimal::ONE_HUNDRED)
                        {
                            errors.push((
                                format!("{p}.value is {}, outside 0 to 100", d.normalize()),
                                "percentage outside 0 to 100".to_string(),
                            ));
                        }
                    }
                    "fixedAmount" => {
                        if let Some(d) = child.get("amount").and_then(money::decimal_from_json)
                            && d < Decimal::ZERO
                        {
                            errors.push((
                                format!("{p}.amount is {}, below zero", d.normalize()),
                                "amount below zero".to_string(),
                            ));
                        }
                    }
                    "cartLine" => {
                        if let Some(id) = child.get("id").and_then(Value::as_str)
                            && cart.line(id).is_none()
                        {
                            errors.push((
                                format!("{p}.id {id} is not a line in the input"),
                                "target line id not in the input".to_string(),
                            ));
                        }
                    }
                    "excludedCartLineIds" => {
                        for id in child
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            if cart.line(id).is_none() {
                                errors.push((
                                    format!("{p} lists {id}, which is not a line in the input"),
                                    "excluded line id not in the input".to_string(),
                                ));
                            }
                        }
                    }
                    "deliveryGroup" => {
                        if let Some(id) = child.get("id").and_then(Value::as_str)
                            && !cart.delivery.iter().any(|g| g.id == id)
                        {
                            errors.push((
                                format!("{p}.id {id} is not a delivery group in the input"),
                                "delivery group id not in the input".to_string(),
                            ));
                        }
                    }
                    "deliveryOption" => {
                        if let Some(h) = child.get("handle").and_then(Value::as_str) {
                            let known = cart
                                .delivery
                                .iter()
                                .flat_map(|g| g.options.iter())
                                .any(|o| o.handle == h);
                            if !known {
                                errors.push((
                                    format!("{p}.handle {h} is not a delivery option in the input"),
                                    "delivery option handle not in the input".to_string(),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
                walk(child, &p, cart, errors);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk(child, &format!("{path}[{i}]"), cart, errors);
            }
        }
        _ => {}
    }
}

/// Value checks that need the input, for Shopify function output.
pub fn value_errors(cart: &Cart, output: &Value) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    walk(output, "", cart, &mut errors);
    errors
}

pub fn check(ctx: &RuleContext, cart: &Cart, eval: &Evaluation) -> Check {
    // Schema errors come first: they are the target's own validation.
    if let Some(first) = eval.schema_errors.first() {
        let n = eval.schema_errors.len();
        let more = if n > 1 {
            format!(" ({} more)", n - 1)
        } else {
            String::new()
        };
        let who = if ctx.is_command { "response" } else { "output" };
        return Check::fail(
            VALID_OUTPUT,
            OutcomeKind::RuleBroken,
            normalize_error(first),
            format!("The {who} does not match the expected shape: {first}{more}."),
        )
        .with_detail("First problem", first.clone());
    }
    if ctx.is_command {
        return Check::pass(VALID_OUTPUT);
    }
    let Some(output) = &eval.output else {
        return Check::not_applicable(VALID_OUTPUT, "produced no output");
    };
    let errors = value_errors(cart, output);
    match errors.first() {
        None => Check::pass(VALID_OUTPUT),
        Some((message, class)) => Check::fail(
            VALID_OUTPUT,
            OutcomeKind::RuleBroken,
            class.clone(),
            format!("The output is not valid: {message}."),
        )
        .with_detail("First problem", message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;
    use serde_json::json;

    #[test]
    fn finds_bad_percentages_and_ids() {
        let c = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 1000)]);
        let out = json!({ "operations": [ { "productDiscountsAdd": {
            "selectionStrategy": "ALL",
            "candidates": [
                { "targets": [ { "cartLine": { "id": "gid://shopify/CartLine/9" } } ],
                  "value": { "percentage": { "value": 150 } } }
            ] } } ] });
        let errs = value_errors(&c, &out);
        let classes: Vec<&str> = errs.iter().map(|(_, c)| c.as_str()).collect();
        assert!(classes.contains(&"percentage outside 0 to 100"));
        assert!(classes.contains(&"target line id not in the input"));
    }

    #[test]
    fn normalizes_indexes() {
        assert_eq!(
            normalize_error("operations[3].productDiscountsAdd.candidates[12]"),
            "operations[].productDiscountsAdd.candidates[]"
        );
    }
}
