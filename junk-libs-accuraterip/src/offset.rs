//! Offset-aware AccurateRip verification over a reconstructed disc stream.
//!
//! [`verify_with_offsets`] is a convenience adapter for callers holding
//! whole-track vectors; it delegates to [`verify_with_offset_reader`].

use crate::dbar::DbarFile;
use crate::verify::TrackVerification;
use crate::{BoundedDiscReader, DiscPcmTrack, verify_with_offset_reader};
use junk_libs_core::AnalysisError;
use junk_libs_disc::CdPcmReader;

pub const DEFAULT_MAX_SAMPLE_SHIFT: i32 = 2939;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationOptions {
    pub max_sample_shift: i32,
}

impl Default for VerificationOptions {
    fn default() -> Self {
        Self {
            max_sample_shift: DEFAULT_MAX_SAMPLE_SHIFT,
        }
    }
}

/// PCM for one physical audio track, in disc order. Each `u32` packs a
/// little-endian stereo sample as `L | (R << 16)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscTrackSamples {
    pub position: u8,
    pub samples: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetCandidate {
    pub sample_shift: i32,
    pub full_matches: usize,
    pub minimum_confidence: u8,
    pub total_confidence: u32,
    pub frame_450_matches: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    /// At least one full-track match; callers must inspect every track for whole-disc success.
    Verified,
    /// No full match established and not every v2 offset was evaluated.
    IncompleteSearch,
    Mismatched,
    NoData,
    AmbiguousOffsets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationSummary {
    pub status: VerificationStatus,
    pub chosen_sample_shift: Option<i32>,
    pub tracks: Vec<TrackVerification>,
    /// Every candidate tied for the winning evidence rank. Empty unless the
    /// result is ambiguous.
    pub ambiguous_offsets: Vec<OffsetCandidate>,
    /// Exact inclusive interval evaluated for v1 against physically available PCM.
    /// V2 is evaluated only at the explicitly recorded candidate offsets.
    pub evaluated_sample_shift_start: i32,
    pub evaluated_sample_shift_end: i32,
    pub v2_evaluated_sample_shifts: Vec<i32>,
    /// Physical boundary frames unavailable at the requested interval extremes.
    pub unavailable_leading_frames: u32,
    pub unavailable_trailing_frames: u32,
}

struct VectorPcmReader<'a> {
    tracks: &'a [DiscTrackSamples],
    starts: Vec<u64>,
    total: u64,
    position: u64,
}

impl CdPcmReader for VectorPcmReader<'_> {
    fn total_frames(&self) -> u64 {
        self.total
    }

    fn seek_frame(&mut self, frame: u64) -> Result<(), AnalysisError> {
        if frame > self.total {
            return Err(AnalysisError::invalid_format(
                "PCM vector seek out of bounds",
            ));
        }
        self.position = frame;
        Ok(())
    }

    fn read_frames(&mut self, output: &mut [[i16; 2]]) -> Result<usize, AnalysisError> {
        let mut written = 0;
        while written < output.len() && self.position < self.total {
            let index = self
                .starts
                .partition_point(|&start| start <= self.position)
                .saturating_sub(1);
            let local = usize::try_from(self.position - self.starts[index])
                .map_err(|_| AnalysisError::invalid_format("PCM vector index overflow"))?;
            let samples = &self.tracks[index].samples[local..];
            let count = samples.len().min(output.len() - written);
            for (target, &packed) in output[written..written + count]
                .iter_mut()
                .zip(&samples[..count])
            {
                *target = [packed as u16 as i16, (packed >> 16) as u16 as i16];
            }
            written += count;
            self.position += count as u64;
        }
        Ok(written)
    }
}

/// Adapter for callers that already hold whole-track vectors. Prefer
/// [`verify_with_offset_reader`] when the PCM source should stay bounded and
/// cancellation-aware.
pub fn verify_with_offsets(
    dbar: &DbarFile,
    tracks: &[DiscTrackSamples],
    options: VerificationOptions,
) -> VerificationSummary {
    let mut starts = Vec::with_capacity(tracks.len());
    let mut layouts = Vec::with_capacity(tracks.len());
    let mut total = 0_u64;
    for track in tracks {
        starts.push(total);
        let Ok(frame_count) = u64::try_from(track.samples.len()) else {
            return invalid_vector_summary();
        };
        layouts.push(DiscPcmTrack {
            position: track.position,
            start_frame: total,
            frame_count,
        });
        let Some(next) = total.checked_add(frame_count) else {
            return invalid_vector_summary();
        };
        total = next;
    }
    let reader = VectorPcmReader {
        tracks,
        starts,
        total,
        position: 0,
    };
    let Ok(mut disc) = BoundedDiscReader::new(reader, layouts) else {
        return invalid_vector_summary();
    };
    verify_with_offset_reader(dbar, &mut disc, options, || Ok(()))
        .unwrap_or_else(|_| invalid_vector_summary())
}

fn invalid_vector_summary() -> VerificationSummary {
    VerificationSummary {
        status: VerificationStatus::NoData,
        chosen_sample_shift: None,
        tracks: Vec::new(),
        ambiguous_offsets: Vec::new(),
        evaluated_sample_shift_start: 0,
        evaluated_sample_shift_end: 0,
        v2_evaluated_sample_shifts: Vec::new(),
        unavailable_leading_frames: 0,
        unavailable_trailing_frames: 0,
    }
}
