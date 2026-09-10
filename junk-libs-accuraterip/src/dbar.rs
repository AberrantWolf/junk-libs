//! Binary parser for AccurateRip's dBAR response files.
//!
//! A dBAR `.bin` is a concatenation of one or more "Responses", each
//! representing a single submitter's rip of a disc whose TOC happened to
//! hash to the same triple `(id1, id2, cddb)`. Wire format, all
//! little-endian:
//!
//! ```text
//! Response {
//!     u8   track_count
//!     u32  ar_id1
//!     u32  ar_id2
//!     u32  cddb_id
//!     TrackEntry[track_count] {
//!         u8   confidence
//!         u32  checksum      // primary ARv1 or ARv2, not labelled
//!         u32  checksum_450  // partial checksum used as offset evidence
//!     }
//! }
//! ```
//!
//! Format documented in Phono Junk's source-cited AccurateRip notes
//! and mirrors ARver's
//! [`arver/disc/database.py`](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py)
//! response parser.

use crate::AccurateRipError;

/// Header size in bytes: `u8 + 3 * u32`.
pub const HEADER_LEN: usize = 1 + 4 + 4 + 4;
/// Track entry size in bytes: `u8 + u32 + u32`.
pub const ENTRY_LEN: usize = 1 + 4 + 4;

/// One dBAR track entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedChecksum {
    /// Number of submitters whose rips produced this checksum. Saturates
    /// around 200+; interpret per the AccurateRip.md rubric.
    pub confidence: u8,
    /// The full-track checksum. The database does not say whether the
    /// submitting client calculated ARv1 or ARv2.
    pub checksum: u32,
    /// A one-frame partial checksum around frame 450. This can support an
    /// offset candidate, but can never verify a track by itself.
    pub checksum_450: u32,
}

/// One Response block — a single pressing's worth of expected CRCs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbarResponse {
    pub track_count: u8,
    pub ar_id1: u32,
    pub ar_id2: u32,
    pub cddb_id: u32,
    /// Per-track entries. `tracks.len() == track_count as usize`.
    pub tracks: Vec<ExpectedChecksum>,
}

/// A parsed dBAR file — all Responses stacked in submission order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbarFile {
    pub responses: Vec<DbarResponse>,
}

impl DbarFile {
    /// Parse the raw `.bin` bytes into its Responses. The file contains
    /// no length prefix; parsing runs to end-of-buffer and errors on any
    /// truncation.
    pub fn parse(bytes: &[u8]) -> Result<Self, AccurateRipError> {
        let mut responses = Vec::new();
        let mut cur = 0usize;

        while cur < bytes.len() {
            if bytes.len() - cur < HEADER_LEN {
                return Err(AccurateRipError::Parse(format!(
                    "truncated header at offset {cur}: {} bytes remaining, need {HEADER_LEN}",
                    bytes.len() - cur
                )));
            }
            let track_count = bytes[cur];
            let ar_id1 = read_u32_le(&bytes[cur + 1..cur + 5]);
            let ar_id2 = read_u32_le(&bytes[cur + 5..cur + 9]);
            let cddb_id = read_u32_le(&bytes[cur + 9..cur + 13]);
            cur += HEADER_LEN;

            let tc = track_count as usize;
            let need = tc * ENTRY_LEN;
            if bytes.len() - cur < need {
                return Err(AccurateRipError::Parse(format!(
                    "truncated entries at offset {cur}: {} bytes remaining, need {need} for {tc} tracks",
                    bytes.len() - cur
                )));
            }
            let mut tracks = Vec::with_capacity(tc);
            for _ in 0..tc {
                let confidence = bytes[cur];
                let checksum = read_u32_le(&bytes[cur + 1..cur + 5]);
                let checksum_450 = read_u32_le(&bytes[cur + 5..cur + 9]);
                cur += ENTRY_LEN;
                tracks.push(ExpectedChecksum {
                    confidence,
                    checksum,
                    checksum_450,
                });
            }

            responses.push(DbarResponse {
                track_count,
                ar_id1,
                ar_id2,
                cddb_id,
                tracks,
            });
        }

        Ok(DbarFile { responses })
    }

    /// Validate every response header against the disc that was requested.
    /// A body routed from the wrong cache key must fail closed rather than be
    /// used as verification evidence.
    pub fn validate_request(
        &self,
        track_count: u8,
        ar_id1: u32,
        ar_id2: u32,
        cddb_id: u32,
    ) -> Result<(), AccurateRipError> {
        for (index, response) in self.responses.iter().enumerate() {
            if response.track_count != track_count
                || response.ar_id1 != ar_id1
                || response.ar_id2 != ar_id2
                || response.cddb_id != cddb_id
            {
                return Err(AccurateRipError::Parse(format!(
                    "response {index} header does not match request: expected {track_count:03}-{ar_id1:08x}-{ar_id2:08x}-{cddb_id:08x}, got {:03}-{:08x}-{:08x}-{:08x}",
                    response.track_count, response.ar_id1, response.ar_id2, response.cddb_id
                )));
            }
        }
        Ok(())
    }

    /// Iterate every expected CRC for a given 1-indexed track position,
    /// across all pressings in the file. Yields `(pressing_index, entry)`
    /// pairs. Positions beyond a pressing's `track_count` are skipped
    /// rather than erroring — heterogeneous track counts within one file
    /// are rare but not illegal.
    pub fn entries_for_track(
        &self,
        position: u8,
    ) -> impl Iterator<Item = (usize, &ExpectedChecksum)> + '_ {
        self.responses.iter().enumerate().filter_map(move |(i, r)| {
            if position == 0 || position > r.track_count {
                None
            } else {
                Some((i, &r.tracks[(position - 1) as usize]))
            }
        })
    }
}

fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
