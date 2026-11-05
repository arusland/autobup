use std::fmt;

/// Exit codes, unchanged from the bash version.
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_CONFIG: i32 = 2;
pub const EXIT_PARTIAL: i32 = 3;
pub const EXIT_LOCKED: i32 = 4;

/// An error that ends the program with `code`. `reported` errors were already
/// logged, so `main` only exits with their code.
#[derive(Debug)]
pub struct AppError {
    pub code: i32,
    pub msg: String,
    pub reported: bool,
}

impl AppError {
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            reported: false,
        }
    }

    /// Runtime failure (exit 1).
    pub fn fail(msg: impl Into<String>) -> Self {
        Self::new(EXIT_FAILURE, msg)
    }

    /// Configuration error (exit 2).
    pub fn config(msg: impl Into<String>) -> Self {
        Self::new(EXIT_CONFIG, msg)
    }

    /// Exit with `code` without printing anything more.
    pub fn exit(code: i32) -> Self {
        Self {
            code,
            msg: String::new(),
            reported: true,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
