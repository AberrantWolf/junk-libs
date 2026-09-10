//! Compare computed [`TrackCrc`] values against a parsed [`DbarFile`].
//!
//! The verification loop is deliberately stateless: given a dBAR and one
//! or more computed CRCs, it reports every matching pressing for v1 and
//! v2 independently. Callers (CLI `verify`, library cache writer) decide
//! how to present the outcome.

use crate::crc::TrackCrc;
use crate::dbar::DbarFile;

/// Which local algorithm matched the dBAR's unlabeled primary checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumVersion {
    V1,
    V2,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackVerificationStatus {
    Verified,
    Mismatched,
    NoData,
    Ambiguous,
}

/// One hit: the pressing index within the `DbarFile` and its submitter count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrcMatch {
    pub pressing: usize,
    pub confidence: u8,
    pub checksum: u32,
    pub version: ChecksumVersion,
}

/// Outcome of checking one track's computed CRC against a dBAR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackVerification {
    pub position: u8,
    pub computed: TrackCrc,
    pub v1_matches: Vec<CrcMatch>,
    pub v2_matches: Vec<CrcMatch>,
    pub sample_shift: Option<i32>,
    pub frame_450_support: bool,
    pub status: TrackVerificationStatus,
}

impl TrackVerification {
    /// Highest submitter count across all matches (either version).
    pub fn best_confidence(&self) -> Option<u8> {
        self.v1_matches
            .iter()
            .chain(self.v2_matches.iter())
            .map(|m| m.confidence)
            .max()
    }

    pub fn is_verified(&self) -> bool {
        self.status == TrackVerificationStatus::Verified
    }

    /// Best full-track match, preferring ARv2 when confidence is equal.
    pub fn best_match(&self) -> Option<&CrcMatch> {
        self.v2_matches
            .iter()
            .chain(self.v1_matches.iter())
            .max_by_key(|matched| matched.confidence)
    }

    /// Short human-readable summary suitable for presentation. The format is
    /// stable and grep-friendly; durable state is the structured verification
    /// run rather than this derived string.
    ///
    /// - `"v2 confidence 8"` — v2 matched, best confidence 8
    /// - `"v1 confidence 3 (v2 no match)"` — only v1 matched
    /// - `"no match"` — dBAR loaded, neither version matched
    pub fn status_string(&self) -> String {
        match self.status {
            TrackVerificationStatus::NoData => return "no data".to_string(),
            TrackVerificationStatus::Ambiguous => return "ambiguous offset".to_string(),
            TrackVerificationStatus::Verified | TrackVerificationStatus::Mismatched => {}
        }
        let v1_best = self.v1_matches.iter().map(|m| m.confidence).max();
        let v2_best = self.v2_matches.iter().map(|m| m.confidence).max();
        match (v1_best, v2_best) {
            (_, Some(c)) => format!("v2 confidence {c}"),
            (Some(c), None) => format!("v1 confidence {c} (v2 no match)"),
            (None, None) => "no match".to_string(),
        }
    }
}

/// Check one track's computed CRC against every pressing in a dBAR.
///
/// The primary dBAR checksum is not version-labelled. Compare the locally
/// computed ARv2 and ARv1 values independently, preferring ARv2 for display
/// when both happen to be equal.
pub fn verify_track(dbar: &DbarFile, position: u8, computed: TrackCrc) -> TrackVerification {
    let mut v1_matches = Vec::new();
    let mut v2_matches = Vec::new();
    for (pressing, entry) in dbar.entries_for_track(position) {
        let v2 = entry.checksum == computed.v2;
        let v1 = entry.checksum == computed.v1;
        if v1 {
            v1_matches.push(CrcMatch {
                pressing,
                confidence: entry.confidence,
                checksum: entry.checksum,
                version: if v2 {
                    ChecksumVersion::Both
                } else {
                    ChecksumVersion::V1
                },
            });
        }
        if v2 {
            v2_matches.push(CrcMatch {
                pressing,
                confidence: entry.confidence,
                checksum: entry.checksum,
                version: if v1 {
                    ChecksumVersion::Both
                } else {
                    ChecksumVersion::V2
                },
            });
        }
    }
    let status = if v1_matches.is_empty() && v2_matches.is_empty() {
        if dbar.entries_for_track(position).next().is_some() {
            TrackVerificationStatus::Mismatched
        } else {
            TrackVerificationStatus::NoData
        }
    } else {
        TrackVerificationStatus::Verified
    };
    TrackVerification {
        position,
        computed,
        v1_matches,
        v2_matches,
        sample_shift: Some(0),
        frame_450_support: false,
        status,
    }
}

/// Verify every computed track CRC on a disc against the dBAR.
/// `crcs` is a slice of `(position, crc)` tuples in any order.
pub fn verify_disc(dbar: &DbarFile, crcs: &[(u8, TrackCrc)]) -> Vec<TrackVerification> {
    crcs.iter()
        .map(|&(pos, crc)| verify_track(dbar, pos, crc))
        .collect()
}
