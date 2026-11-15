//! Reads `shopify.extension.toml`: targets, exports, input queries, the
//! build output path and the schema.

use promochaos_core::error::{Error, Result};
use promochaos_core::util::read_input_file;
use std::path::{Path, PathBuf};

pub const CART_LINES_RUN: &str = "cart.lines.discounts.generate.run";
pub const DELIVERY_RUN: &str = "cart.delivery-options.discounts.generate.run";

/// Targets of the deprecated Product, Order and Shipping Discount APIs.
const OLD_API_TARGETS: &[&str] = &[
    "purchase.product-discount.run",
    "purchase.order-discount.run",
    "purchase.shipping-discount.run",
];
const OLD_API_TYPES: &[&str] = &["product_discounts", "order_discounts", "shipping_discounts"];

/// Where to read about moving to the current Discount API.
pub const MIGRATION_GUIDE: &str = "https://shopify.dev/docs/apps/build/discounts";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targeting {
    pub target: String,
    pub export: String,
    pub input_query: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    pub dir: PathBuf,
    pub toml_path: PathBuf,
    pub api_version: Option<String>,
    pub name: Option<String>,
    pub handle: Option<String>,
    pub targets: Vec<Targeting>,
    pub build_path: Option<PathBuf>,
    pub schema_path: PathBuf,
    /// `Rust`, `JavaScript` or `Wasm`.
    pub language: String,
}

impl Extension {
    /// Discount run targets in the extension, in file order.
    pub fn run_targets(&self) -> Vec<&Targeting> {
        self.targets
            .iter()
            .filter(|t| t.target == CART_LINES_RUN || t.target == DELIVERY_RUN)
            .collect()
    }

    pub fn targeting(&self, target: &str) -> Option<&Targeting> {
        self.targets.iter().find(|t| t.target == target)
    }

    /// The wasm promochaos runs: the override, else `build.path`, else
    /// `dist/function.wasm`.
    pub fn wasm_path(&self, override_path: Option<&Path>) -> PathBuf {
        if let Some(p) = override_path {
            return p.to_path_buf();
        }
        match &self.build_path {
            Some(p) => p.clone(),
            None => self.dir.join("dist").join("function.wasm"),
        }
    }
}

fn str_of<'a>(v: &'a toml::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(toml::Value::as_str)
}

pub fn discover(dir: &Path) -> Result<Extension> {
    let toml_path = dir.join("shopify.extension.toml");
    if !toml_path.exists() {
        return Err(Error::bad_input_hint(
            format!("{} has no shopify.extension.toml", dir.display()),
            "pass the folder of one function extension, such as extensions/my-discount",
        ));
    }
    let text = read_input_file(&toml_path)?;
    let doc: toml::Value = toml::from_str(&text)
        .map_err(|e| Error::bad_input(format!("{} is not valid TOML: {e}", toml_path.display())))?;
    let api_version = str_of(&doc, "api_version").map(str::to_string);
    let ext = doc
        .get("extensions")
        .and_then(toml::Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| {
            Error::bad_input(format!(
                "{} has no [[extensions]] entry",
                toml_path.display()
            ))
        })?;
    if let Some(t) = str_of(ext, "type")
        && OLD_API_TYPES.contains(&t)
    {
        return Err(Error::bad_input_hint(
            format!(
                "this function uses the deprecated {t} API; promochaos supports only the current Discount API"
            ),
            format!(
                "move it to the Discount API (cart.lines.discounts.generate.run); see {MIGRATION_GUIDE}"
            ),
        ));
    }
    let mut targets = Vec::new();
    for t in ext
        .get("targeting")
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(target) = str_of(t, "target") else {
            continue;
        };
        if OLD_API_TARGETS.contains(&target) {
            return Err(Error::bad_input_hint(
                format!(
                    "{target} belongs to a deprecated discount API; promochaos supports only the current Discount API"
                ),
                format!("move it to cart.lines.discounts.generate.run; see {MIGRATION_GUIDE}"),
            ));
        }
        let export = str_of(t, "export")
            .map(str::to_string)
            .unwrap_or_else(|| target.replace(['.', '-'], "_"));
        targets.push(Targeting {
            target: target.to_string(),
            export,
            input_query: str_of(t, "input_query").map(|q| dir.join(q)),
        });
    }
    let build_path = ext
        .get("build")
        .and_then(|b| str_of(b, "path"))
        .filter(|p| !p.is_empty())
        .map(|p| dir.join(p));
    let language = if dir.join("Cargo.toml").exists() {
        "Rust"
    } else if dir.join("package.json").exists() {
        "JavaScript"
    } else {
        "Wasm"
    };
    Ok(Extension {
        dir: dir.to_path_buf(),
        toml_path,
        api_version,
        name: str_of(ext, "name").map(str::to_string),
        handle: str_of(ext, "handle").map(str::to_string),
        targets,
        build_path,
        schema_path: dir.join("schema.graphql"),
        language: language.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_ext(dir: &Path, body: &str) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("shopify.extension.toml"), body);
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("promochaos-discover-{name}-{}", std::process::id()))
    }

    #[test]
    fn reads_targets_and_build_path() {
        let dir = temp("ok");
        write_ext(
            &dir,
            r#"api_version = "2026-07"
[[extensions]]
name = "Tiered"
handle = "tiered"
type = "function"
  [[extensions.targeting]]
  target = "cart.lines.discounts.generate.run"
  input_query = "src/run.graphql"
  export = "cart_lines_discounts_generate_run"
  [extensions.build]
  command = "cargo build --target=wasm32-unknown-unknown --release"
  path = "target/wasm32-unknown-unknown/release/tiered.wasm"
"#,
        );
        let e = discover(&dir);
        assert!(e.is_ok(), "{e:?}");
        if let Ok(e) = e {
            assert_eq!(e.api_version.as_deref(), Some("2026-07"));
            assert_eq!(e.run_targets().len(), 1);
            assert!(e.wasm_path(None).ends_with("tiered.wasm"));
        }
    }

    #[test]
    fn refuses_the_old_discount_apis() {
        let dir = temp("old");
        write_ext(
            &dir,
            r#"[[extensions]]
type = "function"
  [[extensions.targeting]]
  target = "purchase.product-discount.run"
"#,
        );
        assert!(matches!(discover(&dir), Err(Error::BadInput { .. })));
    }
}
