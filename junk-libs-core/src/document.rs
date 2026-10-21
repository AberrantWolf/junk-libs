//! Bounded opaque-document classification and plain-text preview mechanics.
//! PDF recognition is a signature check, not structural validation or rendering.
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
/// Classify bounded bytes for opaque preservation. Consumers retain their own
/// upload policy; PDFs are not parsed, decompressed, executed or called valid.
pub fn classify(bytes: &[u8], max_bytes: usize) -> Result<DocumentFormat, AnalysisError> {
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(AnalysisError::invalid_format(
            "document is empty or exceeds its byte limit",
        ));
    }
    if bytes.starts_with(b"%PDF-") {
        let version = bytes
            .get(5..8)
            .ok_or_else(|| AnalysisError::invalid_format("truncated PDF signature"))?;
        if !matches!(
            version,
            b"1.0" | b"1.1" | b"1.2" | b"1.3" | b"1.4" | b"1.5" | b"1.6" | b"1.7" | b"2.0"
        ) || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|w| w == b"%%EOF")
        {
            return Err(AnalysisError::invalid_format(
                "unsupported or incomplete PDF signature",
            ));
        }
        return Ok(DocumentFormat::Pdf);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| AnalysisError::invalid_format("document must be PDF or UTF-8 text"))?;
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(AnalysisError::invalid_format(
            "binary control characters in text document",
        ));
    }
    Ok(DocumentFormat::Utf8Text)
}
#[derive(Debug, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}
/// Read at most one MiB plus a UTF-8 boundary/lookahead. A partial final codepoint
/// is omitted only at an intentional preview boundary; malformed source bytes fail.
pub fn text_preview(reader: impl Read, max_bytes: usize) -> Result<TextPreview, AnalysisError> {
    if max_bytes == 0 || max_bytes > 1024 * 1024 {
        return Err(AnalysisError::invalid_format("invalid text preview bound"));
    }
    let mut bytes = Vec::with_capacity(max_bytes + 5);
    reader
        .take((max_bytes + 5) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > max_bytes;
    let prefix = &bytes[..bytes.len().min(max_bytes)];
    let text = match std::str::from_utf8(prefix) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&prefix[..error.valid_up_to()])
                .map_err(|_| AnalysisError::invalid_format("invalid UTF-8 preview"))?
        }
        Err(_) => return Err(AnalysisError::invalid_format("invalid UTF-8 preview")),
    };
    Ok(TextPreview {
        text: text.strip_prefix('\u{feff}').unwrap_or(text).into(),
        truncated,
    })
}
