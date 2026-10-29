//! Human-readable status lines for `doctor` and `health` (example format).

use std::fmt::Display;

/// Outcome symbol for a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Warn,
    Fail,
    Info,
}

impl Mark {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "⚠",
            Self::Fail => "✗",
            Self::Info => "·",
        }
    }
}

/// Prints `<mark> <label>: <detail>`.
pub fn line(mark: Mark, label: &str, detail: impl Display) {
    println!("{} {label}: {detail}", mark.symbol());
}

/// Formats a byte count as GiB with one decimal.
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gib_formats_one_decimal() {
        assert_eq!(gib(8 * 1024 * 1024 * 1024), "8.0 GiB");
    }
}
