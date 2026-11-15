//! Checks a function's output against the schema's output (input-object)
//! types: required fields, unknown fields, enums, scalars and `@oneOf`.
//!
//! `Decimal` accepts a decimal string or a JSON number: Shopify's own
//! JavaScript template emits `percentage: { value: 10 }`. A single value
//! where a list is expected is accepted, as GraphQL input coercion does.

use crate::schema::named_type;
use apollo_compiler::Schema;
use apollo_compiler::schema::{ExtendedType, Type};
use promochaos_core::money;
use serde_json::Value;

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 40 {
        let cut: String = s.chars().take(40).collect();
        format!("{cut}…")
    } else {
        s
    }
}

struct Checker<'a> {
    schema: &'a Schema,
    errors: Vec<String>,
}

impl Checker<'_> {
    fn push(&mut self, path: &str, msg: String) {
        if self.errors.len() < 50 {
            let p = if path.is_empty() { "output" } else { path };
            self.errors.push(format!("{p} {msg}"));
        }
    }

    fn value(&mut self, ty: &Type, v: &Value, path: &str, depth: usize) {
        if depth > 64 {
            return;
        }
        match ty {
            Type::NonNullNamed(_) | Type::NonNullList(_) if v.is_null() => {
                self.push(path, "is required but is null".to_string());
            }
            _ if v.is_null() => {}
            Type::List(inner) | Type::NonNullList(inner) => match v {
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.value(inner, item, &format!("{path}[{i}]"), depth + 1);
                    }
                }
                single => self.value(inner, single, path, depth + 1),
            },
            Type::Named(n) | Type::NonNullNamed(n) => self.named(n.as_str(), v, path, depth),
        }
    }

    fn named(&mut self, name: &str, v: &Value, path: &str, depth: usize) {
        match self.schema.types.get(name) {
            Some(ExtendedType::InputObject(obj)) => {
                let Value::Object(map) = v else {
                    self.push(
                        path,
                        format!("should be an object ({name}) but is {}", kind_of(v)),
                    );
                    return;
                };
                for key in map.keys() {
                    if !obj.fields.contains_key(key.as_str()) {
                        self.push(
                            path,
                            format!("has a field {key:?} that {name} does not have"),
                        );
                    }
                }
                let one_of = obj.directives.get("oneOf").is_some();
                if one_of {
                    let set = map.values().filter(|x| !x.is_null()).count();
                    if set != 1 {
                        self.push(
                            path,
                            format!(
                                "must set exactly one field of {name} (@oneOf), but sets {set}"
                            ),
                        );
                    }
                }
                for (fname, def) in &obj.fields {
                    let child_path = if path.is_empty() {
                        fname.as_str().to_string()
                    } else {
                        format!("{path}.{}", fname.as_str())
                    };
                    match map.get(fname.as_str()) {
                        Some(child) => self.value(&def.ty, child, &child_path, depth + 1),
                        None => {
                            let required =
                                matches!(*def.ty, Type::NonNullNamed(_) | Type::NonNullList(_));
                            if required && def.default_value.is_none() && !one_of {
                                self.push(&child_path, "is required but missing".to_string());
                            }
                        }
                    }
                }
            }
            Some(ExtendedType::Enum(e)) => match v {
                Value::String(s) if e.values.contains_key(s.as_str()) => {}
                Value::String(s) => {
                    self.push(path, format!("is {s:?}, which is not a {name} value"))
                }
                other => self.push(
                    path,
                    format!("should be a {name} value but is {}", kind_of(other)),
                ),
            },
            _ => self.scalar(name, v, path),
        }
    }

    fn scalar(&mut self, name: &str, v: &Value, path: &str) {
        let ok = match name {
            "String"
            | "Handle"
            | "URL"
            | "Date"
            | "DateTime"
            | "DateTimeWithoutTimezone"
            | "TimeWithoutTimezone" => v.is_string(),
            "ID" => v.is_string() || v.as_i64().is_some(),
            "Int" => v.as_i64().is_some_and(|i| i32::try_from(i).is_ok()),
            "Float" => v.is_number(),
            "Boolean" => v.is_boolean(),
            "Decimal" => money::decimal_from_json(v).is_some(),
            _ => true,
        };
        if !ok {
            let what = match name {
                "Decimal" => "a decimal string or number".to_string(),
                "Int" => "a 32-bit integer".to_string(),
                other => format!("a {other}"),
            };
            self.push(path, format!("is {}, which is not {what}", short(v)));
        }
    }
}

/// Errors in `output` against the result type (for example
/// `CartLinesDiscountsGenerateRunResult`). Empty when the output is valid.
pub fn validate_output(schema: &Schema, result_type: &str, output: &Value) -> Vec<String> {
    let mut c = Checker {
        schema,
        errors: Vec::new(),
    };
    c.named(result_type, output, "", 0);
    c.errors
}

/// The result type's name as a non-null type, for callers with a `Type`.
pub fn result_named(ty: &Type) -> &str {
    named_type(ty).as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = r#"
schema { query: Q }
directive @oneOf on INPUT_OBJECT
scalar Decimal
type Q { x: Int }
input Result { operations: [Operation!]! }
input Operation @oneOf { orderDiscountsAdd: OrderAdd productDiscountsAdd: ProductAdd }
input OrderAdd { candidates: [Candidate!]! selectionStrategy: Strategy! }
input ProductAdd { candidates: [Candidate!]! selectionStrategy: Strategy! }
input Candidate { message: String value: Value! }
input Value @oneOf { percentage: Percentage }
input Percentage { value: Decimal! }
enum Strategy { FIRST MAXIMUM ALL }
"#;

    fn schema() -> Option<apollo_compiler::validation::Valid<Schema>> {
        crate::schema::parse_schema(SCHEMA, std::path::Path::new("s.graphql")).ok()
    }

    #[test]
    fn accepts_numbers_and_strings_for_decimal() {
        let Some(s) = schema() else {
            panic!("schema did not parse")
        };
        let out = json!({ "operations": [ { "orderDiscountsAdd": {
            "selectionStrategy": "FIRST",
            "candidates": [ { "value": { "percentage": { "value": 10 } } },
                            { "value": { "percentage": { "value": "10.0" } } } ] } } ] });
        assert_eq!(validate_output(&s, "Result", &out), Vec::<String>::new());
    }

    #[test]
    fn reports_enums_unknown_fields_and_one_of() {
        let Some(s) = schema() else {
            panic!("schema did not parse")
        };
        let out = json!({ "operations": [ {
            "orderDiscountsAdd": { "selectionStrategy": "All", "candidates": [], "extra": 1 },
            "productDiscountsAdd": { "selectionStrategy": "ALL", "candidates": [] }
        } ] });
        let errs = validate_output(&s, "Result", &out);
        assert!(errs.iter().any(|e| e.contains("\"All\"")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("\"extra\"")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("@oneOf")), "{errs:?}");
    }

    #[test]
    fn reports_missing_required_fields() {
        let Some(s) = schema() else {
            panic!("schema did not parse")
        };
        let errs = validate_output(&s, "Result", &json!({}));
        assert_eq!(errs, vec!["operations is required but missing".to_string()]);
    }
}
