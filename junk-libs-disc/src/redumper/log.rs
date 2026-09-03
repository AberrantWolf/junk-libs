//! Redumper `.log` text parser.
//!
//! Redumper's log is human-readable text with a fairly stable header and
//! section layout. This parser is deliberately forgiving — redumper's
//! format has drifted across versions and will continue to — so unknown
//! or missing fields degrade to `None` rather than failing the parse.
//!
//! Only [`sniff_ripper`](super::sniff_ripper) rejection (wrong ripper) and
//! I/O errors produce [`Err`]. A truncated or partial log still returns a
//! best-effort [`RedumperLog`].

use std::collections::BTreeMap;
use std::path::Path;

use junk_libs_core::AnalysisError;

use super::{Ripper, sniff_ripper};

/// CD/DVD drive identification as reported by redumper.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DriveInfo {
    /// Vendor / manufacturer, e.g. `ASUS`, `PLEXTOR`, `LG`.
    pub vendor: String,
    /// Product / model name, e.g. `BW-16D1HT`, `PX-712A`.
    pub product: String,
    /// Firmware revision, when the log records one.
    pub firmware: Option<String>,
}

/// Parsed redumper log.
///
/// Any field other than [`raw`](Self::raw) may be missing — redumper omits
/// MCN / ISRC entries when the disc carries none, and older versions log
/// some fields under different labels.
#[derive(Debug, Clone, Default)]
pub struct RedumperLog {
    /// Version string from the log header (for example, `(build: b736)`).
    pub version: Option<String>,
    /// Drive the rip was produced on.
    pub drive: Option<DriveInfo>,
    /// Drive read offset in four-byte stereo sample frames. Redumper applies
    /// this while writing the raw `.scram`/`.state` sample timeline.
    pub read_offset: Option<i32>,
    /// Disc write offset in four-byte stereo sample frames, detected during
    /// split from main-channel header MSF versus subchannel-Q timing.
    pub disc_write_offset: Option<i32>,
    /// Media Catalog Number (subchannel MCN, typically equals the UPC/EAN).
    ///
    /// `None` when the disc carries no MCN in its subchannel Q data.
    pub mcn: Option<String>,
    /// Per-track ISRCs keyed by 1-based track number.
    ///
    /// Empty when the disc carries no ISRCs. Tracks without an ISRC are
    /// simply absent from the map — no placeholder entries.
    pub isrcs: BTreeMap<u8, String>,
    /// Timestamp when the rip started, as an ISO-8601-ish string straight
    /// from the log (`YYYY-MM-DD HH:MM:SS`). Callers parse to a typed
    /// datetime if they need one — keeping this a string avoids forcing
    /// a chrono dependency on every consumer of `junk-libs-disc`.
    pub rip_date: Option<String>,
    /// Full log text, retained for forensic inspection and future reparse
    /// passes that want stretch fields (C2 counts, secure-mode details,
    /// read stats) without re-reading from disk.
    pub raw: String,
}

/// Parse a redumper log file at `path`.
///
/// Returns [`AnalysisError::UnsupportedVariant`] when the log's header
/// matches a different ripper (EAC, XLD, etc.) — the caller should treat
/// this as "wrong ripper," distinct from "corrupt log."
///
/// Returns [`AnalysisError::Io`] for filesystem failures. Malformed content
/// past the sniffer never errors — fields simply stay `None`.
pub fn parse_log(path: &Path) -> Result<RedumperLog, AnalysisError> {
    let ripper = sniff_ripper(path)?;
    if ripper != Ripper::Redumper {
        return Err(AnalysisError::UnsupportedVariant(format!(
            "log at {} was produced by {:?}, not redumper",
            path.display(),
            ripper
        )));
    }

    let raw = std::fs::read_to_string(path)?;
    Ok(parse_log_text(&raw))
}

/// Parse the text of a redumper log directly, without filesystem access.
///
/// Does not sniff — assumes the caller has already confirmed this is a
/// redumper log. Useful for testing with inline fixtures.
pub fn parse_log_text(raw: &str) -> RedumperLog {
    let mut out = RedumperLog {
        raw: raw.to_string(),
        ..Default::default()
    };

    out.version = extract_version(raw);
    out.rip_date = extract_rip_date(raw);
    out.drive = extract_drive(raw);
    out.read_offset = extract_read_offset(raw);
    out.disc_write_offset = extract_disc_write_offset(raw);
    out.mcn = extract_mcn(raw);
    out.isrcs = extract_isrcs(raw);

    out
}

fn extract_version(raw: &str) -> Option<String> {
    for line in raw.lines().take(10) {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.to_ascii_lowercase().starts_with("redumper") {
            // Strip the leading `redumper` token; keep the rest as the
            // version string (e.g. `v2024.03.01 build_123`).
            let after = trimmed[8..].trim();
            if !after.is_empty() {
                return Some(after.to_string());
            }
        }
    }
    None
}

fn extract_rip_date(raw: &str) -> Option<String> {
    // Redumper emits a line like `2024-01-15 14:23:45` early in the log.
    // We accept `YYYY-MM-DD HH:MM:SS` with either a space or `T` separator.
    for line in raw.lines().take(50) {
        let trimmed = line.trim();
        if looks_like_iso_timestamp(trimmed) {
            return Some(trimmed[..19].to_string());
        }
        if let Some(start) = trimmed.find(|c: char| c.is_ascii_digit()) {
            let candidate = &trimmed[start..];
            if looks_like_iso_timestamp(candidate) {
                return Some(candidate[..19].to_string());
            }
        }
    }
    None
}

fn looks_like_iso_timestamp(s: &str) -> bool {
    // Minimum shape: YYYY-MM-DD HH:MM:SS = 19 chars.
    let b = s.as_bytes();
    if b.len() < 19 {
        return false;
    }
    b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
        && (b[10] == b' ' || b[10] == b'T')
        && b[11].is_ascii_digit()
        && b[12].is_ascii_digit()
        && b[13] == b':'
        && b[14].is_ascii_digit()
        && b[15].is_ascii_digit()
        && b[16] == b':'
        && b[17].is_ascii_digit()
        && b[18].is_ascii_digit()
}

fn extract_drive(raw: &str) -> Option<DriveInfo> {
    // Redumper prints drive info in several known shapes across versions:
    //   (a)  drive: ASUS - BW-16D1HT (fw 3.00)
    //   (b)  drive:
    //          vendor: ASUS
    //          product: BW-16D1HT
    //          firmware: 3.00
    //   (c)  Drive:     ASUS     BW-16D1HT     3.00
    //
    // Try the indented-block shape first (most precise), then the
    // single-line shape.

    if let Some(info) = extract_redumper_inquiry(raw) {
        return Some(info);
    }
    if let Some(info) = extract_drive_block(raw) {
        return Some(info);
    }
    extract_drive_inline(raw)
}

fn extract_redumper_inquiry(raw: &str) -> Option<DriveInfo> {
    for line in raw.lines() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("inquiry:") else {
            continue;
        };
        let value = value.trim();
        let (vendor, rest) = value.split_once(" - ")?;
        let (product, firmware) =
            if let Some((product, detail)) = rest.split_once(" (revision level:") {
                let revision = detail
                    .split([',', ')'])
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                (product.trim(), revision)
            } else {
                (rest.trim(), None)
            };
        if !vendor.is_empty() && !product.is_empty() {
            return Some(DriveInfo {
                vendor: vendor.trim().to_string(),
                product: product.to_string(),
                firmware,
            });
        }
    }
    None
}

fn extract_drive_block(raw: &str) -> Option<DriveInfo> {
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower == "drive:" || lower.starts_with("drive:") && trimmed.ends_with(':') {
            // Consume indented follow-on lines.
            let mut vendor = None;
            let mut product = None;
            let mut firmware = None;
            while let Some(&next) = lines.peek() {
                if !next.starts_with(char::is_whitespace) || next.trim().is_empty() {
                    break;
                }
                let kv = next.trim();
                lines.next();
                if let Some((k, v)) = split_kv(kv) {
                    match k.to_ascii_lowercase().as_str() {
                        "vendor" => vendor = Some(v.to_string()),
                        "product" | "model" => product = Some(v.to_string()),
                        "firmware" | "revision" | "firmware revision" => {
                            firmware = Some(v.to_string())
                        }
                        _ => {}
                    }
                }
            }
            if vendor.is_some() || product.is_some() {
                return Some(DriveInfo {
                    vendor: vendor.unwrap_or_default(),
                    product: product.unwrap_or_default(),
                    firmware,
                });
            }
        }
    }
    None
}

fn extract_drive_inline(raw: &str) -> Option<DriveInfo> {
    for line in raw.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !(lower.starts_with("drive:") || lower.starts_with("drive ")) {
            continue;
        }
        let value = match trimmed.split_once(':') {
            Some((_, v)) => v.trim(),
            None => continue,
        };
        if value.is_empty() {
            // This was the header of a block form; handled elsewhere.
            continue;
        }
        return Some(parse_drive_inline(value));
    }
    None
}

fn parse_drive_inline(value: &str) -> DriveInfo {
    // Accept both `VENDOR - PRODUCT (fw X)` and `VENDOR PRODUCT FIRMWARE`
    // (whitespace-separated). Prefer the dash/paren form when present.
    if let Some((left, right)) = value.split_once(" - ") {
        let vendor = left.trim().to_string();
        let (product, firmware) = split_firmware_paren(right);
        return DriveInfo {
            vendor,
            product,
            firmware,
        };
    }

    // Fall back to whitespace columns.
    let parts: Vec<&str> = value.split_whitespace().collect();
    match parts.as_slice() {
        [v, p, f] => DriveInfo {
            vendor: (*v).to_string(),
            product: (*p).to_string(),
            firmware: Some((*f).to_string()),
        },
        [v, p] => DriveInfo {
            vendor: (*v).to_string(),
            product: (*p).to_string(),
            firmware: None,
        },
        _ => DriveInfo {
            vendor: String::new(),
            product: value.to_string(),
            firmware: None,
        },
    }
}

fn split_firmware_paren(right: &str) -> (String, Option<String>) {
    // `BW-16D1HT (fw 3.00)` -> ("BW-16D1HT", Some("3.00"))
    if let Some(open) = right.rfind('(')
        && let Some(close) = right.rfind(')')
        && close > open
    {
        let product = right[..open].trim().to_string();
        let paren = right[open + 1..close].trim();
        let fw = paren
            .trim_start_matches(|c: char| !c.is_ascii_digit() && c != 'v' && c != 'V')
            .trim_start_matches(['v', 'V'])
            .trim()
            .to_string();
        let fw = if fw.is_empty() { None } else { Some(fw) };
        return (product, fw);
    }
    (right.trim().to_string(), None)
}

fn extract_read_offset(raw: &str) -> Option<i32> {
    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        let marker = if lower.contains("read offset") {
            "read offset"
        } else {
            continue;
        };
        let start = lower.find(marker)? + marker.len();
        let after = line.get(start..)?.trim_start_matches([' ', ':']);
        if let Some(n) = parse_signed_prefix(after) {
            return Some(n);
        }
    }
    None
}

fn extract_disc_write_offset(raw: &str) -> Option<i32> {
    for line in raw.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let Some(after) = lower.strip_prefix("disc write offset:") else {
            continue;
        };
        let original_after = &trimmed[trimmed.len() - after.len()..];
        if let Some(n) = parse_signed_prefix(original_after.trim()) {
            return Some(n);
        }
    }
    None
}

fn parse_signed_prefix(value: &str) -> Option<i32> {
    let end = value
        .char_indices()
        .take_while(|(idx, ch)| ch.is_ascii_digit() || (*idx == 0 && matches!(ch, '+' | '-')))
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()?;
    value[..end].parse().ok()
}

fn extract_mcn(raw: &str) -> Option<String> {
    // Recognised shapes — redumper emits the CUE-sheet form; EAC-style
    // logs and some forks use the colon-prefixed label forms.
    //
    // - `MCN: 0123456789012`
    // - `media catalog number: 0123456789012`
    // - `catalog: 0123456789012`
    // - `CATALOG 0123456789012`    ← redumper embeds the CUE in its log;
    //                                this is the only MCN-bearing line
    //                                redumper actually writes.
    for line in raw.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let value = if let Some(rest) = lower.strip_prefix("mcn:") {
            &trimmed[trimmed.len() - rest.len()..]
        } else if let Some(rest) = lower.strip_prefix("media catalog number:") {
            &trimmed[trimmed.len() - rest.len()..]
        } else if let Some(rest) = lower.strip_prefix("catalog:") {
            &trimmed[trimmed.len() - rest.len()..]
        } else if let Some(rest) = lower.strip_prefix("catalog ") {
            // CUE-sheet shape, whitespace-separated, no colon. This is
            // what redumper writes via its inlined `printCUE` output.
            &trimmed[trimmed.len() - rest.len()..]
        } else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("none") || value == "-" {
            continue;
        }
        // MCN is 13 digits; tolerate stray whitespace / hyphens. The CUE
        // form sometimes wraps the value in quotes — strip them first.
        let unquoted = value.trim_matches('"');
        let cleaned: String = unquoted.chars().filter(|c| c.is_ascii_digit()).collect();
        if cleaned.len() == 13 {
            return Some(cleaned);
        }
    }
    None
}

fn extract_isrcs(raw: &str) -> BTreeMap<u8, String> {
    let mut out = BTreeMap::new();
    let mut in_block = false;
    let mut cue_track = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        // Redumper's actual output carries ISRCs in its embedded CUE:
        //   TRACK 01 AUDIO
        //     ISRC JPA842501138
        // Keep the current CUE track so the following unnumbered ISRC can
        // be assigned without inventing a track number.
        if let Some(track) = parse_cue_track_number(trimmed) {
            cue_track = Some(track);
            continue;
        }

        // Both `ISRC:` (block header) and `ISRCs:` are seen.
        if lower == "isrc:"
            || lower == "isrcs:"
            || lower.starts_with("isrc:") && trimmed.ends_with(':')
        {
            in_block = true;
            continue;
        }

        // Single-line form: `ISRC track 03: USRC17654321`.
        if lower.starts_with("isrc ") || lower.starts_with("isrc\t") {
            if let Some((num, code)) = parse_isrc_single_line(trimmed) {
                out.insert(num, code);
            } else if let Some(num) = cue_track {
                let code = clean_isrc(trimmed.get(4..).unwrap_or_default().trim());
                if is_valid_isrc(&code) {
                    out.insert(num, code);
                }
            }
            continue;
        }

        if !in_block {
            continue;
        }

        // Exit the block on a blank line or a non-indented line.
        if trimmed.is_empty() {
            in_block = false;
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            in_block = false;
            // Re-check this same line outside the block (it might be the
            // start of another section we care about). The loop's next
            // iteration will handle it.
            continue;
        }

        // In-block shape: `NN CODE` or `track NN: CODE` or `NN: CODE`.
        if let Some((num, code)) = parse_isrc_indented(trimmed) {
            out.insert(num, code);
        }
    }

    out
}

fn parse_isrc_single_line(line: &str) -> Option<(u8, String)> {
    // `ISRC track 03: USRC17654321`  or  `ISRC 03: USRC17654321`
    let after = line.get(4..)?.trim_start();
    let after = after.strip_prefix("track").unwrap_or(after).trim_start();
    let (num_str, rest) = after.split_once(':')?;
    let num: u8 = num_str.trim().parse().ok()?;
    let code = clean_isrc(rest.trim());
    is_valid_isrc(&code).then_some((num, code))
}

fn parse_isrc_indented(line: &str) -> Option<(u8, String)> {
    // Strip optional `track ` prefix.
    let line = line.strip_prefix("track ").unwrap_or(line);

    // Try `NN: CODE` first.
    if let Some((num_str, rest)) = line.split_once(':')
        && let Ok(num) = num_str.trim().parse::<u8>()
    {
        let code = clean_isrc(rest.trim());
        if is_valid_isrc(&code) {
            return Some((num, code));
        }
    }

    // Then `NN CODE` (whitespace-separated).
    let mut parts = line.split_whitespace();
    let num_str = parts.next()?;
    let num: u8 = num_str.parse().ok()?;
    let code = clean_isrc(parts.next().unwrap_or(""));
    is_valid_isrc(&code).then_some((num, code))
}

fn parse_cue_track_number(line: &str) -> Option<u8> {
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("TRACK") {
        return None;
    }
    let number = parts.next()?.parse::<u8>().ok()?;
    (1..=99).contains(&number).then_some(number)
}

fn is_valid_isrc(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() == 12
        && bytes[..2].iter().all(u8::is_ascii_alphabetic)
        && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

fn clean_isrc(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn split_kv(s: &str) -> Option<(&str, &str)> {
    let (k, v) = s.split_once(':')?;
    Some((k.trim(), v.trim()))
}
