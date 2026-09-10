//! AccurateRip CRC v1 / v2 computation over streaming PCM sectors.
//!
//! Consumes an iterator of [`PcmSector`] (588 stereo samples packed as
//! little-endian `L | (R << 16)` `u32` values per CDDA frame) and emits
//! a [`TrackCrc`]. Both variants are computed in a single pass to avoid
//! per-sample divergence between v1 and v2 implementations.
//!
//! Algorithm reference: Phono Junk's source-cited
//! `docs/knowledge/disc-formats/accuraterip.md`.
//! Formulas cross-verified against [leo-bogert/accuraterip-checksum]
//! (<https://github.com/leo-bogert/accuraterip-checksum>) and
//! [arcctgx/ARver](https://github.com/arcctgx/ARver).
//!
//! ## Skip bounds
//!
//! AccurateRip ignores the first 5 CDDA frames (2940 samples) of the
//! disc's first track and the last 5 CDDA frames of the disc's last
//! track to absorb drive-offset variance near the disc boundaries. On a
//! single-track disc both skips apply simultaneously.
//!
//! The check window is `check_start ..= check_end`, inclusive, on the
//! 1-indexed sample position that runs across the whole track.
//!
//! For first/only tracks, `check_start = SKIP_SAMPLES`, i.e. position
//! 2940 is the first *included* position — matching ARver's and
//! leo-bogert's `multiplier >= skip_frames` condition. (Some secondary
//! references describe the skip as "positions 1..=2940 excluded", which
//! is off-by-one; the reference implementations include 2940.)

use crate::AccurateRipError;

pub const SAMPLES_PER_SECTOR: usize = 588;
pub type PcmSector = [u32; SAMPLES_PER_SECTOR];

/// Where this track sits on the disc — selects which skip bounds apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPosition {
    /// First track of a multi-track disc. Skip positions `1..=2939`.
    First,
    /// Neither first nor last — no skip.
    Middle,
    /// Last track of a multi-track disc. Skip the final 2940 positions.
    Last,
    /// The only track on the disc — apply both skips.
    Only,
}

/// The pair of AccurateRip checksums produced for one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackCrc {
    /// v1: 32-bit truncated `position * sample` sum. Known flaw —
    /// roughly 3% of right-channel bits are ignored due to multiplication
    /// overflow. Retained for compatibility with legacy submissions.
    pub v1: u32,
    /// v2: same iteration, but with the full 64-bit product folded back
    /// as `hi + lo` into the 32-bit accumulator. Fixes v1's overflow flaw.
    pub v2: u32,
}

/// AccurateRip's skip constant: 5 CDDA frames * 588 samples/frame.
pub const SKIP_SAMPLES: u32 = 5 * 588;

/// Compute both AccurateRip CRC variants in a single pass.
///
/// `total_samples` is the expected sample count of the track (typically
/// `layout.length_sectors * 588`). Used to derive the last-track skip
/// upper bound. If the iterator produces a different sample count the
/// function returns [`AudioError::InvalidToc`] — AccurateRip verification
/// is meaningless when sample-count assumptions break.
pub fn track_crc_streaming<I>(
    samples: I,
    total_samples: u32,
    position: TrackPosition,
) -> Result<TrackCrc, AccurateRipError>
where
    I: IntoIterator<Item = Result<PcmSector, junk_libs_core::AnalysisError>>,
{
    let (check_start, check_end) = skip_bounds(position, total_samples);

    let mut crc = TrackCrc { v1: 0, v2: 0 };
    let mut pos: u32 = 1;
    let mut emitted: u32 = 0;

    for sector in samples {
        let sector = sector?;
        for &sample in sector.iter() {
            if pos >= check_start && pos <= check_end {
                accumulate(&mut crc, pos, sample);
            }
            pos += 1;
        }
        emitted += 588;
    }

    if emitted != total_samples {
        return Err(AccurateRipError::SampleCount {
            actual: emitted,
            expected: total_samples,
        });
    }

    Ok(crc)
}

/// Compute both variants from an already-materialized track. Offset search
/// and no-data evidence use this without repacking samples into sectors.
pub fn track_crc_samples(samples: &[u32], position: TrackPosition) -> TrackCrc {
    let total_samples = samples.len().min(u32::MAX as usize) as u32;
    let (check_start, check_end) = skip_bounds(position, total_samples);
    let mut crc = TrackCrc { v1: 0, v2: 0 };
    for (index, &sample) in samples.iter().enumerate() {
        let pos = index as u32 + 1;
        if pos >= check_start && pos <= check_end {
            accumulate(&mut crc, pos, sample);
        }
    }
    crc
}

fn accumulate(crc: &mut TrackCrc, position: u32, sample: u32) {
    crc.v1 = crc.v1.wrapping_add(position.wrapping_mul(sample));
    let product = position as u64 * sample as u64;
    crc.v2 = crc
        .v2
        .wrapping_add((product >> 32) as u32)
        .wrapping_add(product as u32);
}

/// Derive inclusive 1-indexed check bounds from the track's disc position.
///
/// Exposed for tests and for the (future) offset-compensated variant.
/// Returns `(check_start, check_end)`; positions outside that range
/// contribute zero. The function never errors — if the track is shorter
/// than the skip region the window simply becomes empty and the CRCs
/// are zero, mirroring ARver's C reference which has no short-track
/// guard.
pub fn skip_bounds(position: TrackPosition, total_samples: u32) -> (u32, u32) {
    let start = match position {
        TrackPosition::First | TrackPosition::Only => SKIP_SAMPLES,
        TrackPosition::Middle | TrackPosition::Last => 1,
    };
    let end = match position {
        TrackPosition::Last | TrackPosition::Only => total_samples.saturating_sub(SKIP_SAMPLES),
        TrackPosition::First | TrackPosition::Middle => total_samples,
    };
    (start, end)
}
