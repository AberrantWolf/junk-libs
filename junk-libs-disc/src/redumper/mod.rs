//! Redumper sidecar discovery and parsing.
//!
//! Redumper is a CD/DVD imaging tool that writes a set of sidecar files
//! alongside the CUE it produces, sharing the CUE's basename. The `.log`
//! carries drive info, read offset, MCN/UPC, per-track ISRCs, and the
//! ripper version; the `.cdtext` carries a binary dump of the disc's
//! CD-TEXT strings (album + per-track titles, performer, ISRC, etc.).
//!
//! This module exposes three entry points:
//!
//! - [`find_sidecars`] — filesystem discovery, no parsing.
//! - [`parse_log`] — parses a redumper log into [`RedumperLog`].
//! - [`parse_cdtext`] — parses a binary CD-TEXT dump into [`CdText`].
//!
//! Everything here is pure parsing — no domain types from phono-junk or
//! retro-junk leak in. Both workspaces consume the output.
//!
//! Upstream reference: redumper project, <https://github.com/superg/redumper>.
//! CD-TEXT format: IEC 60908 (Red Book) annex and Sony/Philips CD-TEXT spec.

pub mod cdtext;
pub mod log;
pub mod sidecars;

pub use cdtext::{CdText, CdTextBlock, CdTextTrack, parse_cdtext, parse_cdtext_bytes};
pub use log::{DriveInfo, RedumperLog, parse_log, parse_log_text};
pub use sidecars::{
    CdRawStructure, Sidecars, find_sidecars, sniff_ripper, validate_current_cd_raw,
};

/// Which CD-ripping tool produced a given `.log` sidecar.
///
/// Detected by sniffing the log header. `Unknown` covers both "no log
/// present" and "log present but format not recognised" — callers
/// distinguish via the presence of an accompanying `log_path`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Ripper {
    /// <https://github.com/superg/redumper>
    Redumper,
    /// Exact Audio Copy (`.log` format differs from redumper).
    Eac,
    /// CUERipper (dBpoweramp's companion tool).
    Cueripper,
    /// dBpoweramp CD Ripper.
    DbPoweramp,
    /// X Lossless Decoder (macOS).
    Xld,
    /// No ripper-specific marker found.
    Unknown,
}

impl Ripper {
    /// Stable short identifier suitable for logs / DB serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Redumper => "redumper",
            Self::Eac => "eac",
            Self::Cueripper => "cueripper",
            Self::DbPoweramp => "dbpoweramp",
            Self::Xld => "xld",
            Self::Unknown => "unknown",
        }
    }

    /// Inverse of [`Ripper::as_str`]. Any unrecognised value maps to
    /// [`Ripper::Unknown`] rather than erroring — we'd rather round-trip
    /// an old DB row through a newer binary than hard-fail on it.
    pub fn from_id(s: &str) -> Self {
        match s {
            "redumper" => Self::Redumper,
            "eac" => Self::Eac,
            "cueripper" => Self::Cueripper,
            "dbpoweramp" => Self::DbPoweramp,
            "xld" => Self::Xld,
            _ => Self::Unknown,
        }
    }

    /// Backward-compatible alias retained for existing junk-libs consumers.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self::from_id(s)
    }
}
