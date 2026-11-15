//! Small helpers: bounded file reads, stable hashes, UTC timestamps.

use crate::error::{Error, Result};
use crate::hard_limits::MAX_INPUT_FILE_BYTES;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Reads a text input file, refusing files over 10 MB (spec section 14).
pub fn read_input_file(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if meta.len() > MAX_INPUT_FILE_BYTES {
        return Err(Error::bad_input(format!(
            "{} is {} bytes; promochaos reads input files up to {} bytes",
            path.display(),
            meta.len(),
            MAX_INPUT_FILE_BYTES
        )));
    }
    std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes a results file, creating parent folders.
pub fn write_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, contents).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// FNV-1a, 64 bit. Used to label inputs in reports, not for security.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn hash_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// `7c2e…90af` style short form of a hex hash.
pub fn short_hash(hex: &str) -> String {
    if hex.len() <= 8 {
        return hex.to_string();
    }
    format!("{}…{}", &hex[..4], &hex[hex.len() - 4..])
}

/// The current UTC time as `2026-11-14T10:02:11Z`.
pub fn utc_now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339_from_unix(secs)
}

/// Civil date from a Unix timestamp (Howard Hinnant's algorithm).
pub fn rfc3339_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// First line of a message, trimmed, at most 200 characters.
pub fn first_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(200).collect()
}

/// A path shown relative to `base` when it is inside it, with forward slashes.
pub fn display_path(path: &Path, base: &Path) -> String {
    let shown = path.strip_prefix(base).unwrap_or(path);
    portable_path(shown)
}

/// A TOML error on one line: `line 3, column 13: invalid type: integer
/// `5`, expected a string`. toml's own Display spans several lines with a
/// caret diagram, which breaks the one-line error rule.
pub fn toml_error_line(text: &str, e: &toml::de::Error) -> String {
    let message = e.message().trim().replace('\n', " ");
    match e.span() {
        Some(span) => {
            let start = span.start.min(text.len());
            let before = text.get(..start).unwrap_or("");
            let line = before.matches('\n').count() + 1;
            let column = before
                .rsplit('\n')
                .next()
                .map(|l| l.chars().count())
                .unwrap_or(0)
                + 1;
            format!("line {line}, column {column}: {message}")
        }
        None => message,
    }
}

/// Drops `.` components: `ext/./dist/function.wasm` is
/// `ext/dist/function.wasm`, and `./.` is `.`.
pub fn clean_path(p: &Path) -> std::path::PathBuf {
    let c: std::path::PathBuf = p
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    if c.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        c
    }
}

/// A path for files and reports that must read the same on every OS:
/// forward slashes and no `.` components.
pub fn portable_path(p: &Path) -> String {
    clean_path(p).to_string_lossy().replace('\\', "/")
}

/// `target` relative to the folder `from`, when both are absolute and on
/// the same drive or root; otherwise None.
pub fn relative_to(target: &Path, from: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let t: Vec<Component> = target.components().collect();
    let f: Vec<Component> = from.components().collect();
    if !target.is_absolute() || !from.is_absolute() || t.first() != f.first() {
        return None;
    }
    let common = t.iter().zip(&f).take_while(|(a, b)| a == b).count();
    let mut out = std::path::PathBuf::new();
    for c in &f[common..] {
        if matches!(c, Component::Normal(_)) {
            out.push("..");
        }
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_794_650_531), "2026-11-14T10:02:11Z");
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(hash_hex(b""), "cbf29ce484222325");
        assert_eq!(short_hash("7c2e00000000090af"), "7c2e…90af");
    }

    /// Regression (QA 2026-11-15): results.json showed
    /// `examples/functions/tiered-discount-rust\\.\\dist/function.wasm`.
    #[test]
    fn portable_paths_have_no_dot_components_or_backslashes() {
        assert_eq!(
            portable_path(Path::new("examples/ext/./dist/function.wasm")),
            "examples/ext/dist/function.wasm"
        );
        assert_eq!(portable_path(Path::new("./.")), ".");
        assert_eq!(
            portable_path(Path::new("./dist/function.wasm")),
            "dist/function.wasm"
        );
        #[cfg(windows)]
        assert_eq!(
            portable_path(Path::new("examples\\homegrown-cart\\catalog.toml")),
            "examples/homegrown-cart/catalog.toml"
        );
    }

    #[test]
    fn relative_paths_between_folders() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let a = Path::new(root).join("w").join("ex").join("promochaos.toml");
        let b = Path::new(root)
            .join("w")
            .join("out")
            .join("findings")
            .join("F-001");
        let r = relative_to(&a, &b).map(|p| portable_path(&p));
        assert_eq!(r.as_deref(), Some("../../../ex/promochaos.toml"));
        assert_eq!(relative_to(Path::new("rel/x"), &b), None);
    }

    /// Regression (QA 2026-11-15): a malformed promochaos.toml printed a
    /// five-line error with a caret diagram.
    #[test]
    fn toml_errors_fit_on_one_line() {
        let text = "[target]\nkind = \"shopify-function\"\nextension = 5\n";
        let e = toml::from_str::<toml::Table>("[target\nkind = 1\n").err();
        let line = e
            .map(|e| toml_error_line("[target\nkind = 1\n", &e))
            .unwrap_or_default();
        assert!(line.starts_with("line 1, column "), "{line}");
        assert!(!line.contains('\n'));
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct T {
            target: Inner,
        }
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct Inner {
            kind: String,
            extension: String,
        }
        let e = toml::from_str::<T>(text).err();
        let line = e.map(|e| toml_error_line(text, &e)).unwrap_or_default();
        assert!(
            line.starts_with("line 3, column 13: invalid type"),
            "{line}"
        );
    }

    #[test]
    fn first_lines() {
        assert_eq!(first_line("\n  panicked at x\nmore"), "panicked at x");
        assert_eq!(first_line(""), "");
    }
}
