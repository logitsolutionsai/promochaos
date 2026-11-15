//! The `Runner` trait and its two implementations:
//! - `InProcessRunner`: Shopify's function-runner 9.2.3 used as a library.
//!   The compiled function module is cached per worker slot and reused for
//!   every run. A watchdog thread per slot interrupts a run that goes past
//!   the timeout (function-runner itself starts fuel at `u64::MAX` and sets
//!   no deadline).
//! - `SubprocessRunner`: `function-runner --json`, the fallback when the
//!   library API changes.
//!
//! Everything else talks to the trait, because function-runner's library API
//! is not promised to be stable.

use promochaos_core::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One run, as either runner reports it.
#[derive(Debug, Clone, Default)]
pub struct RawRun {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub output_text: String,
    pub output_bytes: usize,
    /// The output was not valid JSON (or MessagePack).
    pub output_error: Option<String>,
    pub instructions: u64,
    pub memory_kib: u64,
    pub module_kib: u64,
    pub logs: String,
    pub input_bytes: usize,
    pub timed_out: bool,
}

pub trait Runner: Send + Sync {
    /// `function-runner 9.2.3, in-process`.
    fn describe(&self) -> String;
    fn run(&self, input_json: &[u8], scale_factor: f64) -> Result<RawRun>;
}

pub const FUNCTION_RUNNER_VERSION: &str = "9.2.3";

/// The crash message promochaos shows: a Rust panic line if there is one,
/// else a JavaScript error line, else the first line function-runner added.
pub fn crash_message(logs: &str) -> String {
    let lines: Vec<&str> = logs
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(i) = lines.iter().position(|l| l.contains("panicked at")) {
        let line = lines.get(i).copied().unwrap_or_default();
        // Since Rust 1.73 the message is on the line after `panicked at file:line:col:`.
        let joined = match (line.ends_with(':'), lines.get(i + 1)) {
            (true, Some(next)) => format!("{line} {next}"),
            _ => line.to_string(),
        };
        return promochaos_core::util::first_line(&joined);
    }
    if let Some(l) = lines
        .iter()
        .find(|l| l.starts_with("Error") || l.contains("Error:") || l.contains("TypeError"))
    {
        return promochaos_core::util::first_line(l);
    }
    if let Some(l) = lines.iter().find(|l| l.contains("module exited with code")) {
        return promochaos_core::util::first_line(l);
    }
    if let Some(l) = lines
        .iter()
        .find(|l| l.contains("wasm trap") || l.contains("error while executing"))
    {
        return promochaos_core::util::first_line(l);
    }
    lines
        .last()
        .map(|l| promochaos_core::util::first_line(l))
        .unwrap_or_else(|| "the function trapped".to_string())
}

#[cfg(feature = "in-process")]
pub use in_process::InProcessRunner;

#[cfg(feature = "in-process")]
mod in_process {
    use super::{FUNCTION_RUNNER_VERSION, RawRun, Runner};
    use function_runner::engine::{FunctionRunParams, new_engine, run, uses_msgpack_provider};
    use function_runner::{BytesContainer, BytesContainerType, Codec};
    use promochaos_core::error::{Error, Result};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
    use std::time::{Duration, Instant};
    use wasmtime::{Engine, Module};

    struct Watch {
        base: Instant,
        running: AtomicBool,
        started_ms: AtomicU64,
        fired: AtomicBool,
    }

    impl Watch {
        fn now_ms(&self) -> u64 {
            self.base.elapsed().as_millis() as u64
        }
        fn start(&self) {
            self.fired.store(false, Ordering::SeqCst);
            self.started_ms.store(self.now_ms(), Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
        }
        /// Stops the watch; true when the watchdog interrupted this run.
        fn stop(&self) -> bool {
            self.running.store(false, Ordering::SeqCst);
            self.fired.load(Ordering::SeqCst)
        }
    }

    struct Slot {
        engine: Engine,
        module: Module,
        watch: Arc<Watch>,
    }

    pub struct InProcessRunner {
        wasm: PathBuf,
        export: String,
        msgpack: bool,
        timeout: Duration,
        slots: Vec<Mutex<Slot>>,
        stop: Arc<AtomicBool>,
    }

    fn load(wasm: &Path) -> Result<(Engine, Module)> {
        let engine = new_engine()
            .map_err(|e| Error::internal(format!("could not start the Wasm engine: {e}")))?;
        let module = Module::from_file(&engine, wasm).map_err(|e| {
            Error::unavailable(
                format!("could not load {}: {e}", wasm.display()),
                "rebuild the function with `shopify app function build`",
            )
        })?;
        Ok((engine, module))
    }

    impl InProcessRunner {
        /// Compiles the module once per worker slot. `slots` is the number
        /// of runs that can happen at once (`--jobs`).
        pub fn new(wasm: &Path, export: &str, slots: usize, timeout: Duration) -> Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let mut list = Vec::new();
            let mut msgpack = false;
            for i in 0..slots.max(1) {
                let (engine, module) = load(wasm)?;
                if i == 0 {
                    msgpack = uses_msgpack_provider(&module);
                    if module.get_export(export).is_none() {
                        let exports: Vec<String> =
                            module.exports().map(|e| e.name().to_string()).collect();
                        return Err(Error::bad_input_hint(
                            format!("{} has no export named {export:?}", wasm.display()),
                            format!("exports in the module: {}", exports.join(", ")),
                        ));
                    }
                }
                let watch = Arc::new(Watch {
                    base: Instant::now(),
                    running: AtomicBool::new(false),
                    started_ms: AtomicU64::new(0),
                    fired: AtomicBool::new(false),
                });
                let (w, st, eng) = (watch.clone(), stop.clone(), engine.clone());
                let limit_ms = timeout.as_millis() as u64;
                std::thread::Builder::new()
                    .name(format!("promochaos-watchdog-{i}"))
                    .spawn(move || {
                        while !st.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(20));
                            if w.running.load(Ordering::SeqCst) {
                                let elapsed = w
                                    .now_ms()
                                    .saturating_sub(w.started_ms.load(Ordering::SeqCst));
                                if elapsed > limit_ms && !w.fired.swap(true, Ordering::SeqCst) {
                                    // Wakes the store, whose epoch deadline is 1 tick.
                                    eng.increment_epoch();
                                }
                            }
                        }
                    })
                    .map_err(|e| {
                        Error::internal(format!("could not start a watchdog thread: {e}"))
                    })?;
                list.push(Mutex::new(Slot {
                    engine,
                    module,
                    watch,
                }));
            }
            Ok(InProcessRunner {
                wasm: wasm.to_path_buf(),
                export: export.to_string(),
                msgpack,
                timeout,
                slots: list,
                stop,
            })
        }

        fn acquire(&self) -> MutexGuard<'_, Slot> {
            loop {
                for s in &self.slots {
                    match s.try_lock() {
                        Ok(g) => return g,
                        Err(TryLockError::Poisoned(p)) => return p.into_inner(),
                        Err(TryLockError::WouldBlock) => {}
                    }
                }
                std::thread::yield_now();
            }
        }

        /// One run, with one retry when the watchdog fired while
        /// function-runner was still setting up the module (see
        /// `SetupInterrupted`). Two in a row are reported as an error that
        /// says so, not as a crash or an over-limit finding.
        fn run_checked(
            &self,
            slot: &Slot,
            input_json: &[u8],
            scale_factor: f64,
        ) -> Result<(RawRun, bool, Duration)> {
            let attempt = match self.run_once(slot, input_json, scale_factor)? {
                Err(SetupInterrupted(_)) => self.run_once(slot, input_json, scale_factor)?,
                done => done,
            };
            attempt.map_err(|SetupInterrupted(elapsed)| {
                Error::unavailable(
                    format!(
                        "function-runner was still setting up the module after {} ms, so the timeout (timeout_ms = {}) stopped it before the function ran",
                        elapsed.as_millis(),
                        self.timeout.as_millis()
                    ),
                    "raise [target] timeout_ms, or use a release build of promochaos: a debug build compiles the JavaScript provider module slowly on every run",
                )
            })
        }

        fn run_once(
            &self,
            slot: &Slot,
            input_json: &[u8],
            scale_factor: f64,
        ) -> Result<std::result::Result<(RawRun, bool, Duration), SetupInterrupted>> {
            let codec = if self.msgpack {
                Codec::Messagepack
            } else {
                Codec::Json
            };
            let input = BytesContainer::new(BytesContainerType::Input, codec, input_json.to_vec())
                .map_err(|e| Error::internal(format!("function-runner refused the input: {e}")))?;
            let params = FunctionRunParams {
                function_path: self.wasm.clone(),
                input,
                export: &self.export,
                profile_opts: None,
                scale_factor,
                module: slot.module.clone(),
                engine: slot.engine.clone(),
            };
            let started = Instant::now();
            slot.watch.start();
            let result = catch_unwind(AssertUnwindSafe(|| run(params)));
            let fired = slot.watch.stop();
            let elapsed = started.elapsed();
            let r = match result {
                Err(payload) => {
                    // Name the panic: "function-runner panicked" alone gave no
                    // way to tell a missing trampoline from a runner bug.
                    let why = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .map(|m| promochaos_core::util::first_line(&m))
                        .unwrap_or_else(|| "no message".to_string());
                    // function-runner instantiates the provider module with
                    // `expect`, so the watchdog's epoch interrupt during that
                    // setup panics ("Failed to instantiate imported instance:
                    // wasm trap: interrupt") instead of returning an error.
                    if fired && why.contains("interrupt") {
                        return Ok(Err(SetupInterrupted(elapsed)));
                    }
                    return Err(Error::unavailable(
                        format!("function-runner panicked while running the module: {why}"),
                        "check that the module was built by `shopify app function build` (Wasm API functions need the trampoline step)",
                    ));
                }
                Ok(Err(e)) => {
                    return Err(Error::unavailable(
                        format!("function-runner could not run the module: {e}"),
                        "check the export name and rebuild with `shopify app function build`",
                    ));
                }
                Ok(Ok(r)) => r,
            };
            let output_text = match (&r.output.json_value, self.msgpack) {
                (Some(v), true) => serde_json::to_string(v).unwrap_or_default(),
                _ => String::from_utf8_lossy(&r.output.raw).to_string(),
            };
            Ok(Ok((
                RawRun {
                    success: r.success,
                    output: r.output.json_value.clone(),
                    output_text,
                    output_bytes: r.output.raw.len(),
                    output_error: r.output.encoding_error.clone(),
                    instructions: r.instructions,
                    memory_kib: r.memory_usage,
                    module_kib: r.size,
                    logs: r.logs.clone(),
                    input_bytes: r.input.raw.len(),
                    timed_out: false,
                },
                fired,
                elapsed,
            )))
        }
    }

    /// The watchdog interrupted function-runner before the function ran,
    /// while it compiled and instantiated the provider module. Carries the
    /// time the attempt took.
    struct SetupInterrupted(Duration);

    impl Drop for InProcessRunner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    impl Runner for InProcessRunner {
        fn describe(&self) -> String {
            format!("function-runner {FUNCTION_RUNNER_VERSION}, in-process")
        }

        fn run(&self, input_json: &[u8], scale_factor: f64) -> Result<RawRun> {
            let slot = self.acquire();
            let (mut raw, fired, elapsed) = self.run_checked(&slot, input_json, scale_factor)?;
            if fired && !raw.success {
                if elapsed < self.timeout {
                    // The watchdog fired for the previous run on this slot
                    // just as it ended: run again.
                    let (again, fired2, _) = self.run_checked(&slot, input_json, scale_factor)?;
                    raw = again;
                    if !fired2 || raw.success {
                        return Ok(raw);
                    }
                }
                raw.timed_out = true;
                raw.logs.push_str(&format!(
                    "\npromochaos: run interrupted after {} ms (timeout_ms)",
                    self.timeout.as_millis()
                ));
            }
            Ok(raw)
        }
    }
}

/// `function-runner --json` as a subprocess.
pub struct SubprocessRunner {
    pub binary: PathBuf,
    pub wasm: PathBuf,
    pub export: String,
    pub schema: PathBuf,
    pub query: PathBuf,
    pub timeout: Duration,
    pub cwd: PathBuf,
}

impl SubprocessRunner {
    pub fn new(
        binary: &str,
        wasm: &Path,
        export: &str,
        schema: &Path,
        query: &Path,
        timeout: Duration,
        cwd: &Path,
    ) -> SubprocessRunner {
        // Absolute paths: the subprocess runs in the extension folder.
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        SubprocessRunner {
            binary: PathBuf::from(binary),
            wasm: abs(wasm),
            export: export.to_string(),
            schema: abs(schema),
            query: abs(query),
            timeout,
            cwd: abs(cwd),
        }
    }
}

impl Runner for SubprocessRunner {
    fn describe(&self) -> String {
        format!("{} --json, subprocess", self.binary.display())
    }

    fn run(&self, input_json: &[u8], _scale_factor: f64) -> Result<RawRun> {
        let args: Vec<String> = vec![
            "-f".to_string(),
            self.wasm.display().to_string(),
            "-e".to_string(),
            self.export.clone(),
            "--json".to_string(),
            "-s".to_string(),
            self.schema.display().to_string(),
            "-q".to_string(),
            self.query.display().to_string(),
        ];
        let out = promochaos_command::process::run(
            &self.binary,
            &args,
            &self.cwd,
            &[],
            input_json,
            self.timeout,
            16 * 1024 * 1024,
        )
        .map_err(|e| match e {
            Error::TargetUnavailable { message, .. } => Error::unavailable(
                message,
                "install function-runner 9.2.3 from its GitHub releases, or set [target] function_runner",
            ),
            other => other,
        })?;
        if out.timed_out {
            return Ok(RawRun {
                success: false,
                logs: format!(
                    "promochaos: run interrupted after {} ms (timeout_ms)",
                    self.timeout.as_millis()
                ),
                input_bytes: input_json.len(),
                timed_out: true,
                ..RawRun::default()
            });
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
            Error::unavailable(
                format!(
                    "function-runner did not print JSON ({e}): {}",
                    promochaos_core::util::first_line(&String::from_utf8_lossy(&out.stderr))
                ),
                "check that the binary is function-runner 9.2.3",
            )
        })?;
        let output = v.get("output").cloned();
        let output_text = output
            .as_ref()
            .and_then(|o| serde_json::to_string(o).ok())
            .unwrap_or_default();
        let input_bytes = v
            .get("input")
            .and_then(|i| serde_json::to_vec(i).ok())
            .map(|b| b.len())
            .unwrap_or(input_json.len());
        Ok(RawRun {
            success: v
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            output_bytes: output_text.len(),
            output,
            output_text,
            output_error: None,
            instructions: v
                .get("instructions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            memory_kib: v
                .get("memory_usage")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            module_kib: v
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            logs: v
                .get("logs")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_bytes,
            timed_out: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_messages_prefer_the_panic_line() {
        let logs = "computing tiers\npanicked at src/run.rs:31:10:\ncalled `Option::unwrap()` on a `None` value\nerror while executing at wasm backtrace:\n    0: 0x1a2b";
        assert_eq!(
            crash_message(logs),
            "panicked at src/run.rs:31:10: called `Option::unwrap()` on a `None` value"
        );
        assert_eq!(
            crash_message("module exited with code: 1"),
            "module exited with code: 1"
        );
        assert_eq!(crash_message(""), "the function trapped");
    }
}
