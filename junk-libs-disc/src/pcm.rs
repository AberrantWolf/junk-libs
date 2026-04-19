//! Per-track raw PCM sample iteration from BIN/CUE and CHD audio tracks.
//!
//! Yields stereo 16-bit samples packed as `u32` (left channel in the low
//! 16 bits, right channel in the high 16 bits, little-endian) in CDDA
//! frames of 588 samples per sector. This layout matches the byte order
//! of raw audio-CD sectors, so decoding is a direct reinterpretation of
//! the 2352-byte sector as 588 little-endian `u32` values.
//!
//! Consumers: phono-junk-accuraterip (CRC v1/v2), future FLAC export,
//! and any retro-junk CDDA consumer (GD-ROM audio, PCE-CD audio).
//!
//! Two source constructors mirror the layout readers in this crate:
//! [`TrackPcmReader::from_bin`] for a single-BIN CUE image, and
//! [`TrackPcmReader::from_chd`] for a CHD image. Both emit the same
//! [`PcmSector`] type so downstream code is source-agnostic.
//!
//! The iterator refuses to operate on non-audio tracks
//! ([`TrackKind::Data`] or [`TrackKind::Unknown`]) — misidentified tracks
//! would produce garbage PCM that silently corrupts AccurateRip CRCs.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use junk_libs_core::AnalysisError;

use crate::chd::read_chd_raw_sector;
use crate::layout::{LEAD_IN_FRAMES, TrackKind, TrackLayout};
use crate::sector::RAW_SECTOR_SIZE;

/// Number of stereo samples in one CDDA sector (one "frame" in the
/// AccurateRip sense: `2352 bytes / 4 bytes per stereo sample = 588`).
pub const PCM_SAMPLES_PER_SECTOR: usize = 588;

/// One CDDA sector's worth of stereo samples packed as `u32`.
///
/// Each entry is `left | (right << 16)`, both channels signed 16-bit in
/// little-endian byte order (reinterpreted as `u16` for packing).
pub type PcmSector = [u32; PCM_SAMPLES_PER_SECTOR];

#[derive(Debug)]
enum PcmSource {
    Bin(BufReader<File>),
    Chd(PathBuf),
}

/// Streaming per-track PCM iterator over BIN or CHD sources.
#[derive(Debug)]
pub struct TrackPcmReader {
    source: PcmSource,
    absolute_start_sector: u32,
    total_sectors: u32,
    next_index: u32,
}

impl TrackPcmReader {
    /// Open a BIN file and position the iterator at the start of `layout`.
    ///
    /// The BIN is assumed to contain the whole disc image, starting at
    /// absolute disc sector [`LEAD_IN_FRAMES`] (= sector 150). The byte
    /// offset of `layout.absolute_offset` is therefore
    /// `(absolute_offset - LEAD_IN_FRAMES) * RAW_SECTOR_SIZE`.
    ///
    /// Returns an error if `layout.kind` is not [`TrackKind::Audio`].
    pub fn from_bin(bin_path: &Path, layout: &TrackLayout) -> Result<Self, AnalysisError> {
        guard_audio(layout)?;
        let file = File::open(bin_path)?;
        Ok(Self {
            source: PcmSource::Bin(BufReader::new(file)),
            absolute_start_sector: layout.absolute_offset,
            total_sectors: layout.length_sectors,
            next_index: 0,
        })
    }

    /// Open a CHD file and position the iterator at the start of `layout`.
    ///
    /// Internally delegates to [`read_chd_raw_sector`] per sector; the CHD
    /// file is re-opened on each call (matching existing CHD read behaviour
    /// in this crate). Performance-sensitive callers should batch reads at
    /// a higher layer — see the TODO around hunk caching.
    ///
    /// Returns an error if `layout.kind` is not [`TrackKind::Audio`].
    pub fn from_chd(chd_path: &Path, layout: &TrackLayout) -> Result<Self, AnalysisError> {
        guard_audio(layout)?;
        Ok(Self {
            source: PcmSource::Chd(chd_path.to_path_buf()),
            absolute_start_sector: layout.absolute_offset,
            total_sectors: layout.length_sectors,
            next_index: 0,
        })
    }

    /// Total number of stereo samples this iterator will emit across the
    /// whole track. Useful for callers that need to know sample-count
    /// bounds without exhausting the iterator (e.g. AccurateRip's
    /// last-track skip computation).
    pub fn total_samples(&self) -> u64 {
        self.total_sectors as u64 * PCM_SAMPLES_PER_SECTOR as u64
    }

    fn read_next_raw(&mut self) -> Result<[u8; RAW_SECTOR_SIZE as usize], AnalysisError> {
        let absolute = self.absolute_start_sector + self.next_index;
        match &mut self.source {
            PcmSource::Bin(reader) => {
                let bin_sector = absolute.checked_sub(LEAD_IN_FRAMES).ok_or_else(|| {
                    AnalysisError::invalid_format(
                        "BIN PCM read requested a sector inside the lead-in region",
                    )
                })?;
                let byte_offset = bin_sector as u64 * RAW_SECTOR_SIZE;
                reader.seek(SeekFrom::Start(byte_offset))?;
                let mut buf = [0u8; RAW_SECTOR_SIZE as usize];
                reader.read_exact(&mut buf)?;
                Ok(buf)
            }
            PcmSource::Chd(path) => {
                let bin_sector = absolute.checked_sub(LEAD_IN_FRAMES).ok_or_else(|| {
                    AnalysisError::invalid_format(
                        "CHD PCM read requested a sector inside the lead-in region",
                    )
                })?;
                let file = File::open(path)?;
                let mut reader = BufReader::new(file);
                read_chd_raw_sector(&mut reader, bin_sector as u64)
            }
        }
    }
}

impl Iterator for TrackPcmReader {
    type Item = Result<PcmSector, AnalysisError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index >= self.total_sectors {
            return None;
        }
        let raw = match self.read_next_raw() {
            Ok(buf) => buf,
            Err(e) => {
                // Drain the iterator on error so repeated poll doesn't retry.
                self.next_index = self.total_sectors;
                return Some(Err(e));
            }
        };
        self.next_index += 1;
        Some(Ok(sector_to_samples(&raw)))
    }
}

/// Reinterpret a 2352-byte raw CD audio sector as 588 stereo `u32` samples.
///
/// Each sample packs `left | (right << 16)` where both channels are
/// signed 16-bit PCM read as little-endian `u16`. This is the sole point
/// in the codebase where byte-level audio layout is decoded; every other
/// consumer (BIN, CHD, future MDS/MDF) flows through here.
pub fn sector_to_samples(raw: &[u8; RAW_SECTOR_SIZE as usize]) -> PcmSector {
    let mut out = [0u32; PCM_SAMPLES_PER_SECTOR];
    for (i, out_sample) in out.iter_mut().enumerate() {
        let base = i * 4;
        let left = u16::from_le_bytes([raw[base], raw[base + 1]]);
        let right = u16::from_le_bytes([raw[base + 2], raw[base + 3]]);
        *out_sample = (left as u32) | ((right as u32) << 16);
    }
    out
}

fn guard_audio(layout: &TrackLayout) -> Result<(), AnalysisError> {
    match layout.kind {
        TrackKind::Audio => Ok(()),
        TrackKind::Data | TrackKind::Unknown => Err(AnalysisError::invalid_format(format!(
            "TrackPcmReader rejects non-audio track {} (kind = {:?}, mode = {:?})",
            layout.number, layout.kind, layout.mode
        ))),
    }
}

#[cfg(test)]
#[path = "tests/pcm_tests.rs"]
mod tests;
