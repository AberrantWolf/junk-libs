//! Absolute-sector disc layout shared between CUE and CHD sources.
//!
//! `TrackLayout` is the canonical "disc layout" shape: one entry per track,
//! with its absolute sector position on the disc (lead-in already applied)
//! and its length. Callers that care about audio-CD identification
//! (phono-junk) or game-disc extraction (retro-junk) both consume this
//! shape, so the MSF/sector arithmetic lives here exactly once.

/// CD lead-in length in frames (sectors).
///
/// Track 1 on an audio CD conventionally starts at absolute sector 150
/// (= 2 seconds × 75 frames/second). The disc origin at sector 0 is where
/// the lead-in begins; the lead-in itself contains no user-addressable
/// audio or data.
pub const LEAD_IN_FRAMES: u32 = 150;

/// Kind of track, classified from the source's mode/type string.
///
/// `Unknown` is a hedge for mode strings we don't explicitly recognise.
/// Downstream code is expected to treat `Unknown` conservatively (e.g.
/// phono-junk's CD-Extra detection treats `Unknown` as audio rather than
/// silently discarding the track).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Data,
    Unknown,
}

/// One track's position and length on a disc, in absolute sectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackLayout {
    /// 1-based track number as reported by the source.
    pub number: u8,
    /// Absolute sector offset from the disc origin. Lead-in (150 frames)
    /// is already added, so track 1 on a standard audio CD is at 150.
    pub absolute_offset: u32,
    /// Length of this track in sectors.
    pub length_sectors: u32,
    /// Whether this is an audio, data, or unrecognised track.
    pub kind: TrackKind,
    /// The original mode/type string from the source (CUE `MODE2/2352`,
    /// CHD `TYPE:MODE1_RAW`, etc.). Retained for debugging and to let
    /// callers distinguish variants when `kind == Unknown`.
    pub mode: String,
}

/// Classify a mode/type string into a `TrackKind`.
///
/// - `"AUDIO"` → `Audio`
/// - Any string starting with `"MODE1"` or `"MODE2"` → `Data`
/// - Anything else → `Unknown`
///
/// Comparison is ASCII-case-insensitive on the leading keyword.
pub fn classify_mode(mode: &str) -> TrackKind {
    let upper = mode.to_ascii_uppercase();
    if upper == "AUDIO" {
        TrackKind::Audio
    } else if upper.starts_with("MODE1") || upper.starts_with("MODE2") {
        TrackKind::Data
    } else {
        TrackKind::Unknown
    }
}
