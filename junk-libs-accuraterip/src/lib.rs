//! Headless AccurateRip checksum, dBAR, and sample-offset primitives.
//!
//! This crate operates on packed integer CD PCM and in-memory database bytes.
//! It owns no filesystem, HTTP, catalog, GUI, or audio-device policy.

use thiserror::Error;

pub mod crc;
pub mod dbar;
pub mod offset;
pub mod verify;

pub use crc::{
    PcmSector, SAMPLES_PER_SECTOR, SKIP_SAMPLES, TrackCrc, TrackPosition, skip_bounds,
    track_crc_samples, track_crc_streaming,
};
pub use dbar::{DbarFile, DbarResponse, ExpectedChecksum};
pub use offset::{
    DEFAULT_MAX_SAMPLE_SHIFT, DiscTrackSamples, OffsetCandidate, VerificationOptions,
    VerificationStatus, VerificationSummary, verify_with_offsets,
};
pub use verify::{
    ChecksumVersion, CrcMatch, TrackVerification, TrackVerificationStatus, verify_disc,
    verify_track,
};

#[derive(Debug, Error)]
pub enum AccurateRipError {
    #[error(transparent)]
    Analysis(#[from] junk_libs_core::AnalysisError),

    #[error("AccurateRip CRC: iterator produced {actual} samples, expected {expected}")]
    SampleCount { actual: u32, expected: u32 },

    #[error("dBAR parse: {0}")]
    Parse(String),
}
