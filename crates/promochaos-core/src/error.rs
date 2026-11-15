//! Errors and their exit codes.
//!
//! Exit codes are shared with recordchaos and shelfcheck:
//! `0` no failures, `1` failures found, `2` invalid input or config,
//! `3` target unavailable, `4` internal error.

use std::path::PathBuf;

/// The process exit code a run ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitKind {
    Clean = 0,
    Failures = 1,
    BadInput = 2,
    TargetUnavailable = 3,
    Internal = 4,
}

impl ExitKind {
    pub fn code(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Settings, catalog, config, query or cart input is wrong. Exit 2.
    #[error("{message}")]
    BadInput {
        message: String,
        hint: Option<String>,
    },
    /// No wasm, no binary, no function-runner. Exit 3.
    #[error("{message}")]
    TargetUnavailable {
        message: String,
        hint: Option<String>,
    },
    /// A bug in promochaos or an unexpected runner failure. Exit 4.
    #[error("{0}")]
    Internal(String),
    /// Reading an input file failed. Exit 2.
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Writing a results file failed. Exit 4.
    #[error("could not write {}: {source}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn bad_input(message: impl Into<String>) -> Self {
        Error::BadInput {
            message: message.into(),
            hint: None,
        }
    }

    pub fn bad_input_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Error::BadInput {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn unavailable(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Error::TargetUnavailable {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Error::Internal(message.into())
    }

    pub fn exit_kind(&self) -> ExitKind {
        match self {
            Error::BadInput { .. } | Error::Read { .. } => ExitKind::BadInput,
            Error::TargetUnavailable { .. } => ExitKind::TargetUnavailable,
            Error::Internal(_) | Error::Write { .. } => ExitKind::Internal,
        }
    }

    /// The suggested fix, printed under the error.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Error::BadInput { hint, .. } | Error::TargetUnavailable { hint, .. } => hint.as_deref(),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(format!("JSON error: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_spec() {
        assert_eq!(Error::bad_input("x").exit_kind().code(), 2);
        assert_eq!(Error::unavailable("x", "y").exit_kind().code(), 3);
        assert_eq!(Error::internal("x").exit_kind().code(), 4);
        assert_eq!(ExitKind::Clean.code(), 0);
        assert_eq!(ExitKind::Failures.code(), 1);
    }
}
