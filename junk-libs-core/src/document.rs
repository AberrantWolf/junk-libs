//! Document classification and plain-text previews.
//!
//! PDF recognition is a signature check only; nothing is parsed or validated.
use crate::AnalysisError;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentFormat {
    Pdf,
    Utf8Text,
}

impl DocumentFormat {
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Utf8Text => "text/plain; charset=utf-8",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Utf8Text => "txt",
        }
    }
}

/// Classify a document as PDF (by signature) or UTF-8 text without NUL bytes.
pub fn classify(bytes: &[u8]) -> Result<DocumentFormat, AnalysisError> {
    if bytes.starts_with(b"%PDF-") {
        return Ok(DocumentFormat::Pdf);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(DocumentFormat::Utf8Text),
        _ => Err(AnalysisError::invalid_format(
            "document is neither PDF nor UTF-8 text",
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    /// More text follows the preview.
    pub truncated: bool,
}

/// Read at most `max_bytes` of UTF-8 text (plus a few bytes of lookahead).
///
/// A codepoint split by the preview boundary is dropped; invalid UTF-8 fails.
/// A leading byte-order mark is removed.
pub fn text_preview(reader: impl Read, max_bytes: usize) -> Result<TextPreview, AnalysisError> {
    let mut bytes = Vec::new();
    reader.take(max_bytes as u64 + 4).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > max_bytes;
    let prefix = &bytes[..bytes.len().min(max_bytes)];
    let text = match std::str::from_utf8(prefix) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            // Only an incomplete codepoint at the cut; the prefix before it is valid.
            std::str::from_utf8(&prefix[..error.valid_up_to()]).unwrap()
        }
        Err(_) => return Err(AnalysisError::invalid_format("invalid UTF-8 text")),
    };
    Ok(TextPreview {
        text: text.strip_prefix('\u{feff}').unwrap_or(text).into(),
        truncated,
    })
}

#[cfg(test)]
#[path = "tests/document_tests.rs"]
mod tests;
