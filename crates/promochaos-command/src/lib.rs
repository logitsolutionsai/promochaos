//! The command target (protocol `promochaos-command/1`, docs/targets.md).
//!
//! promochaos starts the configured argv without a shell, writes one JSON
//! request to stdin, closes it, and reads one JSON response from stdout,
//! with a timeout and a 1 MB output cap. A non-zero exit, a timeout or a
//! response that is not JSON counts as a crash.

pub mod process;
pub mod protocol;

use promochaos_core::apply::build_rows;
use promochaos_core::cart::Cart;
use promochaos_core::error::{Error, Result};
use promochaos_core::eval::{Evaluation, Target, TargetInfo};
use promochaos_core::settings::CommandSpec;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Most bytes read from the engine's stdout (spec section 14).
pub const OUTPUT_CAP: usize = 1024 * 1024;

pub struct CommandTarget {
    spec: CommandSpec,
    program: PathBuf,
    args: Vec<String>,
    base_dir: PathBuf,
    work_dir: PathBuf,
}

/// Resolves the program: a path with a separator is relative to the
/// settings folder; on Windows `.exe` is tried when there is no extension.
pub fn resolve_program(program: &str, base_dir: &Path) -> PathBuf {
    let has_sep = program.contains('/') || program.contains('\\');
    if !has_sep {
        return PathBuf::from(program);
    }
    let p = PathBuf::from(program);
    let p = if p.is_absolute() { p } else { base_dir.join(p) };
    // Absolute, because a relative program path combined with
    // `current_dir` is resolved differently on each platform.
    let p = std::path::absolute(&p).unwrap_or(p);
    if cfg!(windows) && p.extension().is_none() && !p.exists() {
        let exe = p.with_extension("exe");
        if exe.exists() {
            return exe;
        }
    }
    p
}

impl CommandTarget {
    pub fn new(spec: &CommandSpec, base_dir: &Path, work_dir: &Path) -> Result<CommandTarget> {
        let Some((program, args)) = spec.run.split_first() else {
            return Err(Error::bad_input("[target] run is empty"));
        };
        let program_path = resolve_program(program, base_dir);
        let has_sep = program.contains('/') || program.contains('\\');
        if has_sep && !program_path.exists() {
            return Err(Error::unavailable(
                format!("the engine {} does not exist", program_path.display()),
                "build it first, for the example: cargo build -p homegrown-cart",
            ));
        }
        std::fs::create_dir_all(work_dir).map_err(|source| Error::Write {
            path: work_dir.to_path_buf(),
            source,
        })?;
        Ok(CommandTarget {
            spec: spec.clone(),
            program: program_path,
            args: args.to_vec(),
            base_dir: std::path::absolute(base_dir).unwrap_or_else(|_| base_dir.to_path_buf()),
            work_dir: std::path::absolute(work_dir).unwrap_or_else(|_| work_dir.to_path_buf()),
        })
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![(
            "PROMOCHAOS_WORK_DIR".to_string(),
            self.work_dir.display().to_string(),
        )]
    }

    fn call(&self, cart: &Cart, op: &str, key: Option<&str>) -> Result<Evaluation> {
        let request = protocol::request(cart, op, key);
        let bytes = serde_json::to_vec(&request)?;
        let out = process::run(
            &self.program,
            &self.args,
            &self.base_dir,
            &self.env(),
            &bytes,
            Duration::from_millis(self.spec.timeout_ms),
            OUTPUT_CAP,
        )?;
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if out.timed_out {
            return Ok(Evaluation::crashed(
                request,
                format!("timed out after {} ms", self.spec.timeout_ms),
                stderr,
            ));
        }
        if out.truncated {
            return Ok(Evaluation::crashed(
                request,
                format!("printed more than {OUTPUT_CAP} bytes"),
                stderr,
            ));
        }
        if let Some(code) = out.exit_code {
            if code != 0 {
                let first = promochaos_core::util::first_line(&stderr);
                let msg = if first.is_empty() {
                    format!("exit code {code}")
                } else {
                    format!("exit code {code}: {first}")
                };
                return Ok(Evaluation::crashed(request, msg, stderr));
            }
        } else if !out.success {
            return Ok(Evaluation::crashed(
                request,
                "killed by a signal".to_string(),
                stderr,
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let response: serde_json::Value = match serde_json::from_str(text.trim()) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Evaluation::crashed(
                    request,
                    format!("the response is not JSON: {e}"),
                    stderr,
                ));
            }
        };
        let parsed = protocol::parse_response(cart, &response);
        let mut ledger = parsed.ledger;
        build_rows(cart, &mut ledger);
        Ok(Evaluation {
            input_bytes: bytes.len(),
            input: request,
            output_bytes: text.len(),
            output: Some(response),
            output_text: text,
            crash: None,
            logs: stderr,
            metrics: None,
            schema_errors: parsed.errors,
            ledger: Some(ledger),
            totals: parsed.totals,
            order: parsed.order,
            timed_out: false,
        })
    }
}

impl Target for CommandTarget {
    fn info(&self) -> TargetInfo {
        let mut label = self.spec.run.join(" ");
        if label.len() > 120 {
            label.truncate(120);
        }
        TargetInfo {
            kind: "command".to_string(),
            label,
            runner: Some("command, no shell".to_string()),
            ..TargetInfo::default()
        }
    }

    fn evaluate(&self, cart: &Cart) -> Result<Evaluation> {
        self.call(cart, "price", None)
    }

    fn place_order(&self, cart: &Cart, idempotency_key: &str) -> Option<Result<Evaluation>> {
        Some(self.call(cart, "place_order", Some(idempotency_key)))
    }

    fn reset(&self) -> Result<()> {
        let Some(argv) = &self.spec.reset else {
            return Ok(());
        };
        let Some((program, args)) = argv.split_first() else {
            return Ok(());
        };
        let program = resolve_program(program, &self.base_dir);
        let out = process::run(
            &program,
            args,
            &self.base_dir,
            &self.env(),
            b"",
            Duration::from_millis(self.spec.timeout_ms),
            OUTPUT_CAP,
        )?;
        if out.timed_out || out.exit_code != Some(0) {
            // Say why: the message used to end in an empty stderr line.
            let why = if out.timed_out {
                format!("it timed out after {} ms", self.spec.timeout_ms)
            } else {
                let err = promochaos_core::util::first_line(&String::from_utf8_lossy(&out.stderr));
                match (out.exit_code, err.is_empty()) {
                    (Some(c), true) => format!("it exited with code {c}"),
                    (Some(c), false) => format!("it exited with code {c}: {err}"),
                    (None, true) => "it was stopped by a signal".to_string(),
                    (None, false) => format!("it was stopped by a signal: {err}"),
                }
            };
            return Err(Error::unavailable(
                format!("the reset command failed: {why}"),
                "check [target] reset and timeout_ms in promochaos.toml",
            ));
        }
        Ok(())
    }

    fn supports_orders(&self) -> bool {
        true
    }
}
