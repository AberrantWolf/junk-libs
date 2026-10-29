//! Reusable mechanisms. No HTTP clients, application jobs, paths or metadata authority.
use serde::{Deserialize, Serialize};
#[cfg(feature = "fingerprint")]
pub mod fingerprint;
pub mod matching;
#[cfg(feature = "musicbrainz")]
pub mod provider;

pub const MAX_TRACKS: usize = 256;
pub const MAX_RELEASES: usize = 128;
pub const MAX_RECORDINGS_PER_TRACK: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Music matching cancelled")]
    Cancelled,
    #[error("Invalid music matching input: {0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_ids: Vec<String>,
    pub release_group_id: Option<String>,
    pub disambiguation: Option<String>,
    pub language: Option<String>,
    pub script: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    pub barcode: Option<String>,
    pub media: Vec<Medium>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Medium {
    pub position: Option<u16>,
    pub format: Option<String>,
    pub track_count: u32,
    pub disc_ids: Vec<String>,
    pub tracks: Vec<Track>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    pub position: u16,
    pub title: String,
    pub artist: String,
    pub recording_id: Option<String>,
    pub duration_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecordingMatch {
    pub recording_id: String,
    /// Provider score, not an album-match probability.
    pub score: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LocalTrack {
    /// Consumer-owned opaque file/source identity. No paths are sent to providers.
    pub key: String,
    pub duration_ms: u64,
    pub disc: Option<u16>,
    pub position: Option<u16>,
    pub recordings: Vec<RecordingMatch>,
}

pub(crate) fn text(value: &str) -> bool {
    value.len() <= 4096 && !value.chars().any(|c| c.is_control() && !c.is_whitespace())
}
pub fn valid_mbid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
