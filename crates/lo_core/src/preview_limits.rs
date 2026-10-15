//! Trusted process configuration for preview allocation limits.
//! Values are positive decimal byte counts (or pixels), without unit suffixes.
use crate::{LoError, Result};

/// Allocation limits, read from the process environment for each import/decode.
#[derive(Clone, Copy, Debug)]
pub struct PreviewLimits {
    pub zip_entry_bytes: usize,
    pub zip_total_bytes: usize,
    pub inflate_bytes: usize,
    pub png_pixels: usize,
}

impl PreviewLimits {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            zip_entry_bytes: read("LIBREOFFICE_RS_MAX_ZIP_ENTRY_BYTES", 64 * 1024 * 1024)?,
            zip_total_bytes: read("LIBREOFFICE_RS_MAX_ZIP_TOTAL_BYTES", 256 * 1024 * 1024)?,
            inflate_bytes: read("LIBREOFFICE_RS_MAX_INFLATE_BYTES", 64 * 1024 * 1024)?,
            png_pixels: read("LIBREOFFICE_RS_MAX_PNG_PIXELS", 16_000_000)?,
        })
    }
}

fn read(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => parse(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(LoError::InvalidInput(format!(
            "{name} must be a positive decimal integer"
        ))),
    }
}
fn parse(name: &str, value: &str) -> Result<usize> {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(n) = value.parse::<usize>() {
            if n > 0 {
                return Ok(n);
            }
        }
    }
    Err(LoError::InvalidInput(format!(
        "{name} must be a positive decimal integer fitting usize"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_overrides_without_mutating_global_environment() {
        assert_eq!(parse("LIMIT", "134217728").unwrap(), 128 * 1024 * 1024);
        for value in [
            "",
            "0",
            "-1",
            "+1",
            "64MiB",
            " 64",
            "1.5",
            "999999999999999999999999",
        ] {
            assert!(parse("LIMIT", value)
                .unwrap_err()
                .to_string()
                .contains("LIMIT"));
        }
    }
}
