//! Runs a program without a shell, with a timeout and an output cap.

use promochaos_core::error::{Error, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    /// stdout went past the cap.
    pub truncated: bool,
}

fn read_capped<R: Read>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut buf = [0_u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() + n > cap {
                    let take = cap.saturating_sub(out.len());
                    out.extend_from_slice(buf.get(..take).unwrap_or(&[]));
                    truncated = true;
                    // Keep draining so the child does not block on a full pipe.
                    continue;
                }
                out.extend_from_slice(buf.get(..n).unwrap_or(&[]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (out, truncated)
}

pub fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    stdin: &[u8],
    timeout: Duration,
    cap: usize,
) -> Result<ProcessOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::unavailable(
                format!("could not start {}: {e}", program.display()),
                "check [target] run in promochaos.toml and build the engine first",
            )
        } else {
            Error::unavailable(
                format!("could not start {}: {e}", program.display()),
                "check that the program is executable",
            )
        }
    })?;

    let mut stdin_pipe = child.stdin.take();
    let input = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(mut p) = stdin_pipe.take() {
            // A child that exits early closes the pipe; that is not our error.
            let _ = p.write_all(&input);
        }
    });
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Readers report through channels so a stuck pipe cannot block us: a
    // child that spawned its own children (a shell running `sleep`) keeps
    // the pipes open after it is killed, and joining the readers then
    // waited for the grandchild, far past the timeout.
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = match stdout {
            Some(s) => read_capped(s, cap),
            None => (Vec::new(), false),
        };
        let _ = out_tx.send(r);
    });
    std::thread::spawn(move || {
        let r = match stderr {
            Some(s) => read_capped(s, 64 * 1024),
            None => (Vec::new(), false),
        };
        let _ = err_tx.send(r);
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => {
                return Err(Error::internal(format!(
                    "waiting for {} failed: {e}",
                    program.display()
                )));
            }
        }
    };
    // After the process ends, give the pipes a short grace period to drain.
    let grace = if timed_out {
        Duration::from_millis(250)
    } else {
        timeout.saturating_sub(start.elapsed()) + Duration::from_millis(250)
    };
    let deadline = Instant::now() + grace;
    let left = || deadline.saturating_duration_since(Instant::now());
    let (stdout, truncated) = out_rx.recv_timeout(left()).unwrap_or_default();
    let (stderr, _) = err_rx.recv_timeout(left()).unwrap_or_default();
    if !timed_out {
        let _ = writer.join();
    }
    Ok(ProcessOutput {
        stdout,
        stderr,
        exit_code: status.and_then(|s| s.code()),
        success: status.map(|s| s.success()).unwrap_or(false),
        timed_out,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (QA 2026-11-15): a hanging command run through a shell
    /// (`bash -c "sleep 30"`) took the full 30 s per run, not the timeout,
    /// because the killed shell's child kept stdout open.
    #[cfg(unix)]
    #[test]
    fn a_hanging_grandchild_does_not_outlive_the_timeout() {
        let started = Instant::now();
        let out = run(
            Path::new("/bin/sh"),
            &["-c".to_string(), "sleep 20".to_string()],
            Path::new("."),
            &[],
            b"",
            Duration::from_millis(300),
            1024,
        );
        let out = out.expect("sh starts");
        assert!(out.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn caps_output() {
        let data = [b'x'; 100];
        let (out, truncated) = read_capped(&data[..], 10);
        assert_eq!(out.len(), 10);
        assert!(truncated);
        let (out, truncated) = read_capped(&data[..], 1000);
        assert_eq!(out.len(), 100);
        assert!(!truncated);
    }
}
