//! A PDF → text.
//!
//! Pure Rust, with no outside programs. Checked 2026-10-16 on the real Revolut
//! (12 pages) and iCard (6 pages) statements: on Revolut the chain of balances
//! agreed 198 out of 198, and the out/in/total sums matched those printed in
//! the statement itself. `pdftotext` from poppler works too and is likewise not
//! needed — see §7 of the specification.
use std::path::Path;

use anyhow::{Context, Result};

pub fn extract(path: &Path) -> Result<String> {
    pdf_extract::extract_text(path)
        .with_context(|| crate::tr!("cannot read the PDF {}", "не читается PDF {}", path.display()))
}

/// The text as it is, for the `.txt` samples in the tests and for debugging.
pub fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| crate::tr!("cannot read {}", "не читается {}", path.display()))
}

/// Reads the file, choosing the way by its extension.
pub fn text_of(path: &Path) -> Result<String> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
        Some(ext) if ext == "pdf" => extract(path),
        _ => read_text(path),
    }
}
