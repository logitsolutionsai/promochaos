//! Shopify Function target for promochaos.
//!
//! Discovery reads `shopify.extension.toml`, the input query and
//! `schema.graphql`; each cart is rendered as the function's input with
//! exactly the queried fields; the function runs through a [`runner::Runner`]
//! (in-process function-runner by default); the output is checked against
//! the schema and worked through the application model.

pub mod discover;
pub mod export;
pub mod render;
pub mod runner;
pub mod schema;
pub mod validate;

use promochaos_core::apply::apply_shopify_output;
use promochaos_core::cart::Cart;
use promochaos_core::error::{Error, Result};
use promochaos_core::eval::{Evaluation, RunMetrics, Target, TargetInfo};
use promochaos_core::settings::{Resolved, ResolvedTarget, RunnerKind};
use promochaos_core::util::display_path;
use std::path::PathBuf;
use std::time::Duration;

pub use discover::{CART_LINES_RUN, DELIVERY_RUN, Extension};
pub use runner::{FUNCTION_RUNNER_VERSION, RawRun, Runner};

pub struct ShopifyTarget {
    pub extension: Extension,
    pub schema: schema::FunctionSchema,
    pub query: schema::TargetQuery,
    pub wasm: PathBuf,
    pub runner: Box<dyn Runner>,
    pub configured_codes: Vec<String>,
    pub label: String,
}

impl ShopifyTarget {
    /// Discovers the extension and prepares one run target. `jobs` is the
    /// number of runs that can happen at once.
    pub fn open(resolved: &Resolved, jobs: usize) -> Result<Vec<ShopifyTarget>> {
        let ResolvedTarget::Shopify(spec) = &resolved.target else {
            return Err(Error::internal("not a Shopify function target"));
        };
        let extension = discover::discover(&spec.extension_dir)?;
        let available = extension.run_targets();
        if available.is_empty() {
            let fetch = extension
                .targets
                .iter()
                .any(|t| t.target.ends_with(".generate.fetch"));
            let hint = if fetch {
                "fetch targets are not supported in v1; promochaos runs the .generate.run targets"
            } else {
                "add a cart.lines.discounts.generate.run or cart.delivery-options.discounts.generate.run target"
            };
            return Err(Error::bad_input_hint(
                format!(
                    "{} has no discount run target",
                    extension.toml_path.display()
                ),
                hint,
            ));
        }
        let wanted: Vec<String> = if spec.run_targets.is_empty() {
            available.iter().map(|t| t.target.clone()).collect()
        } else {
            spec.run_targets.clone()
        };
        let wasm = promochaos_core::util::clean_path(&extension.wasm_path(spec.wasm.as_deref()));
        if !wasm.exists() {
            return Err(Error::unavailable(
                format!("{} does not exist", wasm.display()),
                "build the function first: shopify app function build (promochaos always runs the CLI's built output)",
            ));
        }
        check_wasm_magic(&wasm)?;
        let mut out = Vec::new();
        for target in wanted {
            let Some(t) = extension.targeting(&target) else {
                return Err(Error::bad_input(format!(
                    "[target] run = {target:?}, but {} has no such target",
                    extension.toml_path.display()
                )));
            };
            let Some(query_path) = &t.input_query else {
                return Err(Error::bad_input(format!(
                    "{target} has no input_query in {}",
                    extension.toml_path.display()
                )));
            };
            let fs = schema::FunctionSchema::load(&extension.schema_path)?;
            let query = fs.load_query(&target, &t.export, query_path)?;
            let timeout = Duration::from_millis(spec.timeout_ms);
            let runner: Box<dyn Runner> = match spec.runner {
                RunnerKind::Subprocess => Box::new(runner::SubprocessRunner::new(
                    &spec.function_runner,
                    &wasm,
                    &t.export,
                    &extension.schema_path,
                    query_path,
                    timeout,
                    &extension.dir,
                )),
                RunnerKind::InProcess => in_process(
                    &wasm,
                    &t.export,
                    jobs,
                    timeout,
                    &spec.function_runner,
                    &extension,
                    query_path,
                )?,
            };
            out.push(ShopifyTarget {
                // The extension folder, never "." (an extension at the
                // settings folder showed as "seed 42 · . · ..." in reports).
                label: {
                    let shown = display_path(&extension.dir, &resolved.base_dir);
                    if shown.is_empty() || shown == "." {
                        std::path::absolute(&extension.dir)
                            .ok()
                            .map(|p| promochaos_core::util::clean_path(&p))
                            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                            .unwrap_or(shown)
                    } else {
                        shown
                    }
                },
                extension: extension.clone(),
                schema: fs,
                query,
                wasm: wasm.clone(),
                runner,
                configured_codes: resolved.codes.clone(),
            });
        }
        Ok(out)
    }

    pub fn render(&self, cart: &Cart) -> serde_json::Value {
        render::Renderer {
            schema: &self.schema.schema,
            doc: &self.query.doc,
            cart,
            configured_codes: &self.configured_codes,
        }
        .render()
    }
}

/// A WebAssembly binary starts with `\0asm`. Without this check a corrupt
/// or empty file reached wasmtime, which tried to read it as WAT text and
/// printed a multi-line, misleading parse error.
fn check_wasm_magic(wasm: &std::path::Path) -> Result<()> {
    use std::io::Read;
    let mut head = [0_u8; 4];
    let ok = std::fs::File::open(wasm)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && head == *b"\0asm";
    if ok {
        Ok(())
    } else {
        Err(Error::unavailable(
            format!(
                "{} is not a WebAssembly module (it does not start with the bytes 00 61 73 6d)",
                wasm.display()
            ),
            "rebuild the function with `shopify app function build`",
        ))
    }
}

#[cfg(feature = "in-process")]
fn in_process(
    wasm: &std::path::Path,
    export: &str,
    jobs: usize,
    timeout: Duration,
    _binary: &str,
    _ext: &Extension,
    _query: &std::path::Path,
) -> Result<Box<dyn Runner>> {
    Ok(Box::new(runner::InProcessRunner::new(
        wasm, export, jobs, timeout,
    )?))
}

#[cfg(not(feature = "in-process"))]
fn in_process(
    wasm: &std::path::Path,
    export: &str,
    _jobs: usize,
    timeout: Duration,
    binary: &str,
    ext: &Extension,
    query: &std::path::Path,
) -> Result<Box<dyn Runner>> {
    // Built without function-runner: fall back to the subprocess runner.
    Ok(Box::new(runner::SubprocessRunner::new(
        binary,
        wasm,
        export,
        &ext.schema_path,
        query,
        timeout,
        &ext.dir,
    )))
}

impl Target for ShopifyTarget {
    fn info(&self) -> TargetInfo {
        let wasm_kib = std::fs::metadata(&self.wasm).ok().map(|m| m.len() / 1024);
        TargetInfo {
            kind: "shopify-function".to_string(),
            label: self.label.clone(),
            run_target: Some(self.query.target.clone()),
            export: Some(self.query.export.clone()),
            wasm: Some(promochaos_core::util::portable_path(&self.wasm)),
            wasm_kib,
            language: Some(self.extension.language.clone()),
            runner: Some(self.runner.describe()),
            api_version: self
                .extension
                .api_version
                .clone()
                .or_else(|| self.schema.api_version.clone()),
            function_runner_version: Some(FUNCTION_RUNNER_VERSION.to_string()),
            rust_tests: self.extension.language == "Rust",
        }
    }

    fn evaluate(&self, cart: &Cart) -> Result<Evaluation> {
        let input = self.render(cart);
        let bytes = serde_json::to_vec(&input)?;
        let scale = render::scale_factor(&self.query.doc, &input);
        let raw = self.runner.run(&bytes, scale)?;
        let crash = if raw.success {
            None
        } else if raw.timed_out {
            Some(runner::crash_message(&raw.logs).replace("promochaos: ", ""))
        } else {
            Some(runner::crash_message(&raw.logs))
        };
        let mut schema_errors = Vec::new();
        let mut output = None;
        if raw.success {
            match (&raw.output_error, &raw.output) {
                (Some(e), _) => schema_errors.push(format!("the output is not valid JSON: {e}")),
                (None, Some(o)) => {
                    schema_errors =
                        validate::validate_output(&self.schema.schema, &self.query.result_type, o);
                    output = Some(o.clone());
                }
                (None, None) => schema_errors.push("the function produced no output".to_string()),
            }
        }
        let ledger = output.as_ref().map(|o| apply_shopify_output(cart, o));
        Ok(Evaluation {
            input,
            input_bytes: if raw.input_bytes > 0 {
                raw.input_bytes
            } else {
                bytes.len()
            },
            output,
            output_text: raw.output_text,
            output_bytes: raw.output_bytes,
            crash,
            logs: raw.logs,
            metrics: Some(RunMetrics {
                instructions: raw.instructions,
                memory_kib: raw.memory_kib,
                module_kib: raw.module_kib,
                scale_factor: scale,
            }),
            schema_errors,
            ledger,
            totals: None,
            order: None,
            timed_out: raw.timed_out,
        })
    }
}

#[cfg(test)]
mod magic_tests {
    use super::check_wasm_magic;

    /// Regression (QA 2026-11-15): corrupt and empty .wasm files gave a WAT
    /// parse error ("input bytes aren't valid utf-8") over several lines.
    #[test]
    fn corrupt_and_empty_modules_are_refused_in_one_line() {
        let dir = std::env::temp_dir().join(format!("promochaos-magic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let empty = dir.join("empty.wasm");
        let junk = dir.join("junk.wasm");
        let good = dir.join("good.wasm");
        std::fs::write(&empty, b"").ok();
        std::fs::write(&junk, b"not a module at all").ok();
        std::fs::write(&good, b"\0asm\x01\0\0\0").ok();
        for p in [&empty, &junk] {
            let e = check_wasm_magic(p)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(e.contains("is not a WebAssembly module"), "{e}");
            assert!(!e.contains('\n'));
        }
        assert!(check_wasm_magic(&good).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
