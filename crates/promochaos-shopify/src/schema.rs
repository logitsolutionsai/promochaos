//! Parses `schema.graphql` and each target's input query with
//! apollo-compiler 1.x.

use apollo_compiler::schema::{ExtendedType, NamedType, Type};
use apollo_compiler::validation::Valid;
use apollo_compiler::{ExecutableDocument, Schema};
use promochaos_core::error::{Error, Result};
use promochaos_core::util::read_input_file;
use std::path::{Path, PathBuf};

pub struct FunctionSchema {
    pub schema: Valid<Schema>,
    pub path: PathBuf,
    pub api_version: Option<String>,
}

pub struct TargetQuery {
    pub target: String,
    pub export: String,
    pub doc: Valid<ExecutableDocument>,
    pub path: PathBuf,
    /// The output type, e.g. `CartLinesDiscountsGenerateRunResult`.
    pub result_type: String,
}

/// `cart.lines.discounts.generate.run` to `cartLinesDiscountsGenerateRun`.
pub fn mutation_field(target: &str) -> String {
    let mut out = String::new();
    for (i, part) in target
        .split(['.', '-'])
        .filter(|p| !p.is_empty())
        .enumerate()
    {
        if i == 0 {
            out.push_str(part);
        } else {
            let mut chars = part.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}

/// The named type inside list and non-null wrappers.
pub fn named_type(ty: &Type) -> &NamedType {
    match ty {
        Type::Named(n) | Type::NonNullNamed(n) => n,
        Type::List(inner) | Type::NonNullList(inner) => named_type(inner),
    }
}

/// The first line of the schema, `# api_version: 2026-07`, when present.
fn schema_api_version(text: &str) -> Option<String> {
    text.lines()
        .take(5)
        .find_map(|l| l.trim().strip_prefix("# api_version:"))
        .map(|v| v.trim().to_string())
}

/// Parses and validates a Shopify Function schema. Shopify's schema files
/// declare `scalar ID`, which apollo-compiler reports as a redefinition of a
/// built-in scalar unless `ignore_builtin_redefinitions` is set.
pub fn parse_schema(
    text: impl Into<String>,
    path: &Path,
) -> std::result::Result<Valid<Schema>, String> {
    let built = Schema::builder()
        .ignore_builtin_redefinitions()
        .parse(text, path)
        .build()
        .map_err(|e| e.errors.to_string())?;
    built.validate().map_err(|e| e.errors.to_string())
}

impl FunctionSchema {
    pub fn load(path: &Path) -> Result<FunctionSchema> {
        if !path.exists() {
            return Err(Error::bad_input_hint(
                format!("{} is missing", path.display()),
                "run `shopify app function schema` in the extension folder to download it",
            ));
        }
        let text = read_input_file(path)?;
        let api_version = schema_api_version(&text);
        let schema = parse_schema(text, path).map_err(|errors| {
            Error::bad_input(format!(
                "{} is not a valid GraphQL schema: {errors}",
                path.display()
            ))
        })?;
        Ok(FunctionSchema {
            schema,
            path: path.to_path_buf(),
            api_version,
        })
    }

    /// The output input-object type for a run target, from the mutation
    /// field's `result` argument.
    pub fn result_type(&self, target: &str) -> Option<String> {
        let field = mutation_field(target);
        for ty in self.schema.types.values() {
            if let ExtendedType::Object(obj) = ty
                && let Some(def) = obj.fields.get(field.as_str())
            {
                return def
                    .arguments
                    .first()
                    .map(|a| named_type(&a.ty).as_str().to_string());
            }
        }
        None
    }

    pub fn load_query(&self, target: &str, export: &str, path: &Path) -> Result<TargetQuery> {
        let text = read_input_file(path)?;
        let doc =
            ExecutableDocument::parse_and_validate(&self.schema, text, path).map_err(|e| {
                Error::bad_input(format!(
                    "{} does not match schema.graphql: {}",
                    path.display(),
                    e.errors
                ))
            })?;
        // Variables come from a metafield at checkout (input variables),
        // which promochaos does not model yet. Rendering them as no value
        // would quietly change what the function sees, so refuse instead.
        if let Ok(op) = doc.operations.get(None)
            && !op.variables.is_empty()
        {
            let names: Vec<String> = op
                .variables
                .iter()
                .map(|v| format!("${}", v.name))
                .collect();
            return Err(Error::bad_input_hint(
                format!(
                    "{} uses query variables ({}), which promochaos does not support yet",
                    path.display(),
                    names.join(", ")
                ),
                "test a copy of the query with the variable values written in",
            ));
        }
        let result_type = self.result_type(target).ok_or_else(|| {
            Error::bad_input(format!(
                "schema.graphql has no mutation field for {target}; is the schema for the Discount API?"
            ))
        })?;
        Ok(TargetQuery {
            target: target.to_string(),
            export: export.to_string(),
            doc,
            path: path.to_path_buf(),
            result_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_field_names() {
        assert_eq!(
            mutation_field("cart.lines.discounts.generate.run"),
            "cartLinesDiscountsGenerateRun"
        );
        assert_eq!(
            mutation_field("cart.delivery-options.discounts.generate.run"),
            "cartDeliveryOptionsDiscountsGenerateRun"
        );
    }

    #[test]
    fn queries_with_variables_are_refused_in_one_line() {
        let dir = std::env::temp_dir().join(format!("promochaos-vars-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp folder");
        let schema_text = "type Query { cart: Cart }\ntype Cart { inAnyCollection(ids: [ID!]): Boolean! }\ntype Mutation { cartLinesDiscountsGenerateRun(result: Boolean): Void }\nscalar Void\n";
        let fs = FunctionSchema {
            schema: parse_schema(schema_text, Path::new("schema.graphql")).expect("schema"),
            path: dir.join("schema.graphql"),
            api_version: None,
        };
        let query = dir.join("run.graphql");
        std::fs::write(
            &query,
            "query Input($ids: [ID!]) { cart { inAnyCollection(ids: $ids) } }",
        )
        .expect("write the query");
        let err = fs
            .load_query("cart.lines.discounts.generate.run", "run", &query)
            .err()
            .expect("a query with variables is refused");
        let text = err.to_string();
        assert!(text.contains("query variables ($ids)"), "{text}");
        assert!(!text.contains('\n'), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_the_api_version_comment() {
        assert_eq!(
            schema_api_version("# api_version: 2026-07\n\nschema {"),
            Some("2026-07".to_string())
        );
    }
}
