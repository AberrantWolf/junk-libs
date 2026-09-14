use junk_libs_core::AnalysisError;
use junk_libs_disc::CdPcmReader;

use crate::{
    AccurateRipError, ChecksumVersion, CrcMatch, DEFAULT_MAX_SAMPLE_SHIFT, DbarFile,
    OffsetCandidate, TrackCrc, TrackPosition, TrackVerification, TrackVerificationStatus,
    VerificationOptions, VerificationStatus, VerificationSummary, skip_bounds,
};

pub const ACCURATERIP_WORKING_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const READ_BUFFER_FRAMES: usize = 16 * 1024;
const FRAME_450_START: u64 = 450 * 588;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscPcmTrack {
    pub position: u8,
    pub start_frame: u64,
    pub frame_count: u64,
}

pub struct BoundedDiscReader<R> {
    reader: R,
    tracks: Vec<DiscPcmTrack>,
}

impl<R: CdPcmReader> BoundedDiscReader<R> {
    pub fn new(reader: R, tracks: Vec<DiscPcmTrack>) -> Result<Self, AccurateRipError> {
        if tracks.is_empty() || tracks.len() > 99 {
            return Err(AccurateRipError::Layout(
                "disc PCM requires between 1 and 99 tracks".into(),
            ));
        }
        let mut next_start = 0_u64;
        let mut previous = 0_u8;
        for track in &tracks {
            if track.position <= previous
                || track.position > 99
                || track.start_frame != next_start
                || track.frame_count == 0
                || track.frame_count > u64::from(u32::MAX)
            {
                return Err(AccurateRipError::Layout(
                    "PCM tracks must be ordered, nonempty, contiguous, and u32-addressable".into(),
                ));
            }
            next_start = next_start
                .checked_add(track.frame_count)
                .ok_or_else(|| AccurateRipError::Layout("disc PCM length overflow".into()))?;
            previous = track.position;
        }
        if next_start != reader.total_frames() {
            return Err(AccurateRipError::Layout(format!(
                "PCM layout describes {next_start} frames but reader declares {}",
                reader.total_frames()
            )));
        }
        Ok(Self { reader, tracks })
    }

    pub fn tracks(&self) -> &[DiscPcmTrack] {
        &self.tracks
    }

    pub fn reader(&self) -> &R {
        &self.reader
    }

    pub fn reader_mut(&mut self) -> &mut R {
        &mut self.reader
    }
}

#[derive(Clone, Copy)]
struct SearchBounds {
    start: i32,
    end: i32,
    missing_leading: u32,
    missing_trailing: u32,
}

impl SearchBounds {
    fn for_disc(tracks: &[DiscPcmTrack], total: u64, maximum: i32) -> Self {
        let requested_start = -maximum;
        let requested_end = maximum;
        let mut start = requested_start;
        let mut end = requested_end;
        for (index, track) in tracks.iter().enumerate() {
            let (first, last) = skip_bounds(
                track_position(index, tracks.len()),
                track.frame_count as u32,
            );
            if first > last {
                continue;
            }
            let source_start = track.start_frame + u64::from(first - 1);
            let source_end = track.start_frame + u64::from(last);
            let lower = i128::from(source_start)
                .checked_neg()
                .and_then(|value| i32::try_from(value).ok())
                .unwrap_or(i32::MIN);
            let upper = i32::try_from(total - source_end).unwrap_or(i32::MAX);
            start = start.max(lower);
            end = end.min(upper);
        }
        Self {
            start,
            end,
            missing_leading: u32::try_from(i64::from(start) - i64::from(requested_start))
                .unwrap_or(u32::MAX),
            missing_trailing: u32::try_from(i64::from(requested_end) - i64::from(end))
                .unwrap_or(u32::MAX),
        }
    }
}

fn empty_summary(status: VerificationStatus, bounds: SearchBounds) -> VerificationSummary {
    VerificationSummary {
        status,
        chosen_sample_shift: None,
        tracks: Vec::new(),
        ambiguous_offsets: Vec::new(),
        evaluated_sample_shift_start: bounds.start,
        evaluated_sample_shift_end: bounds.end,
        v2_evaluated_sample_shifts: Vec::new(),
        unavailable_leading_frames: bounds.missing_leading,
        unavailable_trailing_frames: bounds.missing_trailing,
    }
}

/// Verify exact integer PCM without retaining whole tracks or prefix arrays.
/// The largest allocation is a 64-KiB read buffer; the published 64-MiB budget
/// leaves room for adapter buffering and result bookkeeping.
pub fn verify_with_offset_reader<R, C>(
    dbar: &DbarFile,
    disc: &mut BoundedDiscReader<R>,
    options: VerificationOptions,
    mut checkpoint: C,
) -> Result<VerificationSummary, AccurateRipError>
where
    R: CdPcmReader,
    C: FnMut() -> Result<(), AnalysisError>,
{
    checkpoint()?;
    let maximum = options.max_sample_shift.clamp(0, DEFAULT_MAX_SAMPLE_SHIFT);
    let bounds = SearchBounds::for_disc(&disc.tracks, disc.reader.total_frames(), maximum);
    if dbar.responses.is_empty() || bounds.start > bounds.end {
        return Ok(empty_summary(VerificationStatus::NoData, bounds));
    }

    let tracks = disc.tracks.clone();
    let shift_count = usize::try_from(bounds.end - bounds.start + 1).unwrap();
    let mut v1_by_track = Vec::with_capacity(tracks.len());
    let mut frame_450_by_track = Vec::with_capacity(tracks.len());
    let mut frontier = vec![false; shift_count];
    frontier[usize::try_from(-bounds.start).unwrap()] = true;

    // Discover the candidate frontier with rolling weighted sums. Each full
    // checksum window is streamed once; only the samples crossing either
    // boundary are retained for the requested offset interval.
    for (index, track) in tracks.iter().enumerate() {
        let position = track_position(index, tracks.len());
        let (first, last) = skip_bounds(position, track.frame_count as u32);
        let v1 = if first > last {
            vec![0; shift_count]
        } else {
            rolling_weighted(
                disc.reader_mut(),
                track.start_frame + u64::from(first - 1),
                u64::from(last - first + 1),
                first,
                bounds,
                &mut checkpoint,
            )?
        };
        let frame_450 = if FRAME_450_START + 588 <= track.frame_count
            && track.start_frame + FRAME_450_START >= u64::from(bounds.start.unsigned_abs())
            && track.start_frame + FRAME_450_START + 588 + bounds.end as u64
                <= disc.reader.total_frames()
        {
            Some(rolling_weighted(
                disc.reader_mut(),
                track.start_frame + FRAME_450_START,
                588,
                1,
                bounds,
                &mut checkpoint,
            )?)
        } else {
            None
        };
        for shift_index in 0..shift_count {
            if primary_matches(dbar, track.position, v1[shift_index])
                || frame_450.as_ref().is_some_and(|checksums| {
                    frame_450_matches(dbar, track.position, checksums[shift_index])
                })
            {
                frontier[shift_index] = true;
            }
        }
        v1_by_track.push(v1);
        frame_450_by_track.push(frame_450);
    }

    let rank = |candidate: &OffsetCandidate| {
        (
            candidate.full_matches,
            candidate.minimum_confidence,
            candidate.total_confidence,
            candidate.frame_450_matches,
        )
    };
    let v2_offsets = frontier
        .iter()
        .enumerate()
        .filter_map(|(index, present)| present.then_some(bounds.start + index as i32))
        .collect::<Vec<_>>();
    let unmatched_status = if v2_offsets.len() == shift_count {
        VerificationStatus::Mismatched
    } else {
        VerificationStatus::IncompleteSearch
    };
    let mut best_rank = None;
    let mut best_candidates = Vec::new();
    let mut best_tracks = Vec::new();
    let mut zero_tracks = Vec::new();
    for (shift_index, has_evidence) in frontier.into_iter().enumerate() {
        if !has_evidence {
            continue;
        }
        checkpoint()?;
        let shift = bounds.start + i32::try_from(shift_index).unwrap();
        let mut results = Vec::with_capacity(tracks.len());
        let mut confidences = Vec::new();
        let mut frame_matches = 0;
        for (index, track) in tracks.iter().enumerate() {
            let position = track_position(index, tracks.len());
            let v1 = v1_by_track[index][shift_index];
            let v2 = track_v2_at_shift(disc.reader_mut(), track, position, shift, &mut checkpoint)?;
            let frame_support = frame_450_by_track[index].as_ref().is_some_and(|checksums| {
                frame_450_matches(dbar, track.position, checksums[shift_index])
            });
            if frame_support {
                frame_matches += 1;
            }
            let mut v1_matches = Vec::new();
            let mut v2_matches = Vec::new();
            for (pressing, expected) in dbar.entries_for_track(track.position) {
                let matches_v1 = expected.checksum == v1;
                let matches_v2 = expected.checksum == v2;
                let Some(version) = (match (matches_v1, matches_v2) {
                    (true, true) => Some(ChecksumVersion::Both),
                    (true, false) => Some(ChecksumVersion::V1),
                    (false, true) => Some(ChecksumVersion::V2),
                    (false, false) => None,
                }) else {
                    continue;
                };
                let matched = CrcMatch {
                    pressing,
                    confidence: expected.confidence,
                    checksum: expected.checksum,
                    version,
                };
                if matches_v1 {
                    v1_matches.push(matched);
                }
                if matches_v2 {
                    v2_matches.push(matched);
                }
            }
            let status = if v1_matches.is_empty() && v2_matches.is_empty() {
                TrackVerificationStatus::Mismatched
            } else {
                confidences.push(
                    v1_matches
                        .iter()
                        .chain(&v2_matches)
                        .map(|matched| matched.confidence)
                        .max()
                        .unwrap_or(0),
                );
                TrackVerificationStatus::Verified
            };
            results.push(TrackVerification {
                position: track.position,
                computed: TrackCrc { v1, v2 },
                v1_matches,
                v2_matches,
                sample_shift: Some(shift),
                frame_450_support: frame_support,
                status,
            });
        }
        let candidate = OffsetCandidate {
            sample_shift: shift,
            full_matches: confidences.len(),
            minimum_confidence: confidences.iter().copied().min().unwrap_or(0),
            total_confidence: confidences.iter().copied().map(u32::from).sum(),
            frame_450_matches: frame_matches,
        };
        if shift == 0 {
            zero_tracks.clone_from(&results);
        }
        let candidate_rank = rank(&candidate);
        match best_rank {
            None => {
                best_rank = Some(candidate_rank);
                best_candidates.push(candidate);
                best_tracks = results;
            }
            Some(current) if candidate_rank > current => {
                best_rank = Some(candidate_rank);
                best_candidates.clear();
                best_candidates.push(candidate);
                best_tracks = results;
            }
            Some(current) if candidate_rank == current => best_candidates.push(candidate),
            Some(_) => {}
        }
    }

    let Some(best_rank) = best_rank else {
        return Ok(empty_summary(unmatched_status, bounds));
    };
    if best_rank == (0, 0, 0, 0) {
        for track in &mut zero_tracks {
            track.sample_shift = None;
        }
        let mut summary = empty_summary(unmatched_status, bounds);
        summary.v2_evaluated_sample_shifts = v2_offsets;
        summary.tracks = zero_tracks;
        return Ok(summary);
    }
    if best_candidates.len() != 1 {
        for track in &mut zero_tracks {
            track.sample_shift = None;
            track.status = TrackVerificationStatus::Ambiguous;
        }
        let mut summary = empty_summary(VerificationStatus::AmbiguousOffsets, bounds);
        summary.tracks = zero_tracks;
        summary.ambiguous_offsets = best_candidates;
        summary.v2_evaluated_sample_shifts = v2_offsets;
        return Ok(summary);
    }

    let candidate = best_candidates[0];
    let mut summary = empty_summary(
        if candidate.full_matches > 0 {
            VerificationStatus::Verified
        } else {
            unmatched_status
        },
        bounds,
    );
    summary.v2_evaluated_sample_shifts = v2_offsets;
    summary.chosen_sample_shift = Some(candidate.sample_shift);
    summary.tracks = best_tracks;
    Ok(summary)
}

fn track_v2_at_shift<R: CdPcmReader, C: FnMut() -> Result<(), AnalysisError>>(
    reader: &mut R,
    track: &DiscPcmTrack,
    position: TrackPosition,
    shift: i32,
    checkpoint: &mut C,
) -> Result<u32, AccurateRipError> {
    let (first, last) = skip_bounds(position, track.frame_count as u32);
    if first > last {
        return Ok(0);
    }
    let start = shifted(track.start_frame + u64::from(first - 1), shift)?;
    let count = u64::from(last - first + 1);
    let mut multiplier = u64::from(first);
    let mut crc = 0_u32;
    visit_frames(reader, start, count, checkpoint, |frame| {
        let sample = pack(frame);
        let product = multiplier * u64::from(sample);
        crc = crc
            .wrapping_add(product as u32)
            .wrapping_add((product >> 32) as u32);
        multiplier += 1;
    })?;
    Ok(crc)
}

fn rolling_weighted<R: CdPcmReader, C: FnMut() -> Result<(), AnalysisError>>(
    reader: &mut R,
    start: u64,
    count: u64,
    first_multiplier: u32,
    bounds: SearchBounds,
    checkpoint: &mut C,
) -> Result<Vec<u32>, AccurateRipError> {
    let negative = usize::try_from(-bounds.start).unwrap();
    let positive = usize::try_from(bounds.end).unwrap();
    let mut weighted = 0_u32;
    let mut sum = 0_u32;
    let mut multiplier = first_multiplier;
    visit_frames(reader, start, count, checkpoint, |frame| {
        let sample = pack(frame);
        weighted = weighted.wrapping_add(multiplier.wrapping_mul(sample));
        sum = sum.wrapping_add(sample);
        multiplier = multiplier.wrapping_add(1);
    })?;

    let preceding = read_packed(reader, start - negative as u64, negative, checkpoint)?;
    let outgoing_tail = read_packed(
        reader,
        start + count - negative as u64,
        negative,
        checkpoint,
    )?;
    let outgoing_head = read_packed(reader, start, positive, checkpoint)?;
    let incoming_tail = read_packed(reader, start + count, positive, checkpoint)?;

    let mut values = vec![0_u32; negative + positive + 1];
    values[negative] = weighted;
    let last_multiplier = first_multiplier.wrapping_add(count as u32 - 1);

    let mut previous_weighted = weighted;
    let mut previous_sum = sum;
    for step in 1..=negative {
        let incoming = preceding[negative - step];
        let outgoing = outgoing_tail[negative - step];
        previous_weighted = previous_weighted
            .wrapping_add(previous_sum)
            .wrapping_add(first_multiplier.wrapping_mul(incoming))
            .wrapping_sub(last_multiplier.wrapping_add(1).wrapping_mul(outgoing));
        previous_sum = previous_sum.wrapping_add(incoming).wrapping_sub(outgoing);
        values[negative - step] = previous_weighted;
    }

    let mut next_weighted = weighted;
    let mut next_sum = sum;
    for step in 1..=positive {
        let outgoing = outgoing_head[step - 1];
        let incoming = incoming_tail[step - 1];
        next_weighted = next_weighted
            .wrapping_sub(next_sum)
            .wrapping_sub(first_multiplier.wrapping_sub(1).wrapping_mul(outgoing))
            .wrapping_add(last_multiplier.wrapping_mul(incoming));
        next_sum = next_sum.wrapping_sub(outgoing).wrapping_add(incoming);
        values[negative + step] = next_weighted;
    }
    Ok(values)
}

fn read_packed<R: CdPcmReader, C: FnMut() -> Result<(), AnalysisError>>(
    reader: &mut R,
    start: u64,
    count: usize,
    checkpoint: &mut C,
) -> Result<Vec<u32>, AccurateRipError> {
    let mut output = Vec::with_capacity(count);
    visit_frames(reader, start, count as u64, checkpoint, |frame| {
        output.push(pack(frame));
    })?;
    Ok(output)
}

fn visit_frames<R: CdPcmReader, C: FnMut() -> Result<(), AnalysisError>>(
    reader: &mut R,
    start: u64,
    count: u64,
    checkpoint: &mut C,
    mut visit: impl FnMut([i16; 2]),
) -> Result<(), AccurateRipError> {
    reader.seek_frame(start)?;
    let mut remaining = count;
    let mut actual = 0_u64;
    let mut buffer = vec![[0_i16; 2]; READ_BUFFER_FRAMES];
    while remaining != 0 {
        checkpoint()?;
        let wanted = usize::try_from(remaining.min(READ_BUFFER_FRAMES as u64)).unwrap();
        let read = reader.read_frames(&mut buffer[..wanted])?;
        if read == 0 || read > wanted {
            return Err(AccurateRipError::UnexpectedEof {
                expected: count,
                actual,
            });
        }
        for &frame in &buffer[..read] {
            visit(frame);
        }
        actual += read as u64;
        remaining -= read as u64;
    }
    Ok(())
}

fn shifted(base: u64, shift: i32) -> Result<u64, AccurateRipError> {
    u64::try_from(i128::from(base) + i128::from(shift))
        .map_err(|_| AccurateRipError::Layout("offset requires unavailable PCM".into()))
}

fn pack(frame: [i16; 2]) -> u32 {
    u32::from(frame[0] as u16) | (u32::from(frame[1] as u16) << 16)
}

fn frame_450_matches(dbar: &DbarFile, position: u8, checksum: u32) -> bool {
    dbar.entries_for_track(position)
        .any(|(_, expected)| expected.checksum_450 != 0 && expected.checksum_450 == checksum)
}

fn primary_matches(dbar: &DbarFile, position: u8, checksum: u32) -> bool {
    dbar.entries_for_track(position)
        .any(|(_, expected)| expected.checksum == checksum)
}

fn track_position(index: usize, count: usize) -> TrackPosition {
    match (index, count) {
        (_, 1) => TrackPosition::Only,
        (0, _) => TrackPosition::First,
        (index, count) if index + 1 == count => TrackPosition::Last,
        _ => TrackPosition::Middle,
    }
}
