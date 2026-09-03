//! Per-track raw PCM sample iteration from CUE and CHD audio tracks.
//!
//! Yields stereo 16-bit samples packed as `u32` (left channel in the low
//! 16 bits, right channel in the high 16 bits, little-endian) in CDDA
//! frames of 588 samples per sector. CUE/BIN and CHD audio sectors expose
//! the channel words in this byte order, so decoding is a direct
//! reinterpretation as 588 little-endian `u32` values.
//!
//! Consumers: phono-junk-accuraterip (CRC v1/v2), phono-junk-gui playback,
//! the FLAC export pipeline, and any retro-junk CDDA consumer.
//!
//! Two source constructors mirror the two disc-image shapes this crate
//! understands:
//!
//! - [`TrackPcmReader::from_cue`] for CUE sheets. Handles single-BIN
//!   whole-disc images **and** multi-BIN per-track rips in the same path:
//!   the reader internally swaps the backing BIN when a track's sector
//!   range crosses a FILE boundary (which happens when one track's
//!   `compute_cue_layout` length extends into the pregap at the start of
//!   the next file).
//! - [`TrackPcmReader::from_chd`] for CHD images.
//!
//! Both refuse non-audio tracks ([`TrackKind::Data`] or
//! [`TrackKind::Unknown`]) — misidentified tracks would produce garbage
//! PCM that silently corrupts downstream consumers.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use junk_libs_core::AnalysisError;

use crate::chd::{ChdHunkCache, read_chd_layout};
use crate::cue::{
    check_cue_compat, compute_cue_resolved_layout, convert_cue_to_standard, parse_cue,
    resolve_local_file,
};
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

/// One backing BIN file for a CUE-backed [`TrackPcmReader`], with its
/// disc-sector span precomputed so sector lookups don't re-parse the CUE.
#[derive(Debug, Clone)]
struct CueBinFile {
    path: PathBuf,
    file_start_disc_sector: u32,
    file_sectors: u32,
    frame_bytes: u64,
}

impl CueBinFile {
    fn file_end_disc_sector(&self) -> u32 {
        self.file_start_disc_sector + self.file_sectors
    }

    fn contains(&self, disc_sector: u32) -> bool {
        disc_sector >= self.file_start_disc_sector && disc_sector < self.file_end_disc_sector()
    }
}

enum PcmSource {
    /// CUE-backed. Holds ordered metadata for every FILE directive plus a
    /// cached reader for whichever file was most recently touched, so
    /// sequential sector reads within the same BIN don't re-open the
    /// file. Transparently handles single-BIN whole-disc CUEs as the
    /// special case where `files.len() == 1`.
    Cue {
        files: Vec<CueBinFile>,
        current: Option<(usize, BufReader<File>)>,
    },
    Chd(Box<ChdHunkCache<BufReader<File>>>),
}

impl std::fmt::Debug for PcmSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cue { files, .. } => f
                .debug_struct("PcmSource::Cue")
                .field("files", files)
                .finish(),
            Self::Chd(_) => f.debug_tuple("PcmSource::Chd").finish(),
        }
    }
}

/// Streaming per-track PCM iterator over CUE or CHD sources.
#[derive(Debug)]
pub struct TrackPcmReader {
    source: PcmSource,
    /// Disc-absolute sector where the track begins (INDEX 01, lead-in
    /// applied). Uniform across CUE and CHD — the CUE path translates
    /// this into per-file byte offsets on read, the CHD path into the
    /// hunk-aligned BIN sector via `absolute - LEAD_IN_FRAMES`.
    absolute_start_sector: u32,
    total_sectors: u32,
    next_index: u32,
}

impl TrackPcmReader {
    /// Open a CUE-backed audio track. Handles single-BIN and multi-BIN
    /// rips uniformly. The reader iterates from the target track's
    /// INDEX 01 for the length `compute_cue_layout` assigns it — which
    /// may extend into the next FILE's start when there's a pregap there
    /// (the conventional INDEX 01-to-next-INDEX 01 track span). This fixes
    /// gap ownership only; source-specific sample-offset correction is a
    /// separate step.
    ///
    /// Errors if `track_number` is absent from the CUE, is not an audio
    /// track, or any referenced BIN is missing / misaligned.
    pub fn from_cue(cue_path: &Path, track_number: u8) -> Result<Self, AnalysisError> {
        let text = std::fs::read_to_string(cue_path)?;
        let cue_dir: PathBuf = cue_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        let compat = check_cue_compat(&text);
        let effective_text = if compat.is_standard() {
            text
        } else if let Some(reason) = compat.unfixable_reason.clone() {
            return Err(AnalysisError::invalid_format(reason));
        } else {
            convert_cue_to_standard(&text, &cue_dir)?
        };

        let sheet = parse_cue(&effective_text)?;

        let resolved = compute_cue_resolved_layout(&sheet, |filename| {
            let bin_path = resolve_local_file(&cue_dir, filename)?;
            let meta = std::fs::metadata(&bin_path).map_err(|e| {
                AnalysisError::Io(std::io::Error::new(
                    e.kind(),
                    format!("BIN '{}': {}", bin_path.display(), e),
                ))
            })?;
            Ok(meta.len())
        })?;

        let track = resolved
            .tracks
            .iter()
            .find(|t| t.number == track_number)
            .ok_or_else(|| {
                AnalysisError::invalid_format(format!(
                    "CUE has no track {track_number}; tracks present: {:?}",
                    resolved.tracks.iter().map(|t| t.number).collect::<Vec<_>>()
                ))
            })?;
        guard_audio(track)?;

        let files: Vec<CueBinFile> = resolved
            .files
            .iter()
            .map(|f| {
                Ok(CueBinFile {
                    path: resolve_local_file(&cue_dir, &f.filename)?,
                    file_start_disc_sector: f.file_start_disc_sector,
                    file_sectors: f.file_sectors,
                    frame_bytes: f.frame_bytes,
                })
            })
            .collect::<Result<_, AnalysisError>>()?;

        Ok(Self {
            source: PcmSource::Cue {
                files,
                current: None,
            },
            absolute_start_sector: track.absolute_offset,
            total_sectors: track.length_sectors,
            next_index: 0,
        })
    }

    /// Open a CHD-backed audio track, selecting by track number. The
    /// CHD's internal layout is authoritative; `track_number` indexes
    /// into it.
    ///
    /// The file handle and the most recently decompressed hunk are cached
    /// inside the reader (see [`ChdHunkCache`]), so sequential sector
    /// reads within the same hunk cost one decompression instead of one
    /// per sector.
    pub fn from_chd(chd_path: &Path, track_number: u8) -> Result<Self, AnalysisError> {
        let layouts = read_chd_layout(chd_path)?;
        let layout = layouts
            .iter()
            .find(|l| l.number == track_number)
            .ok_or_else(|| {
                AnalysisError::invalid_format(format!(
                    "CHD has no track {track_number}; tracks present: {:?}",
                    layouts.iter().map(|l| l.number).collect::<Vec<_>>()
                ))
            })?;
        guard_audio(layout)?;

        let file = File::open(chd_path)?;
        let cache = ChdHunkCache::open(BufReader::new(file))?;
        Ok(Self {
            source: PcmSource::Chd(Box::new(cache)),
            absolute_start_sector: layout.absolute_offset,
            total_sectors: layout.length_sectors,
            next_index: 0,
        })
    }

    /// Total number of stereo samples this iterator will emit across the
    /// whole track. Useful for callers that need to know sample-count
    /// bounds without exhausting the iterator (e.g. AccurateRip's
    /// last-track skip computation, FLAC encoder total-samples tag).
    pub fn total_samples(&self) -> u64 {
        self.total_sectors as u64 * PCM_SAMPLES_PER_SECTOR as u64
    }

    /// Total sector count for this track. Paired with
    /// [`sector_offset`](Self::sector_offset) it lets callers translate
    /// between reader position and external sample-index conventions.
    pub fn total_sectors(&self) -> u32 {
        self.total_sectors
    }

    /// Current playback position within the track, in sectors. Starts
    /// at 0 and advances by one on every successful `next()`.
    pub fn sector_offset(&self) -> u32 {
        self.next_index
    }

    /// Reposition the iterator to `sector_offset` within the track.
    /// Backend reads are random-access (BufReader `seek`, CHD hunk cache
    /// keyed by sector), so this is cheap — no re-parse, no re-decode.
    /// The per-FILE reader cached on the CUE source is kept; the next
    /// read auto-swaps it if the target sector lives in a different
    /// backing BIN.
    ///
    /// `sector_offset > total_sectors` is an error — callers should
    /// clamp before calling.
    pub fn seek_to_sector(&mut self, sector_offset: u32) -> Result<(), AnalysisError> {
        if sector_offset > self.total_sectors {
            return Err(AnalysisError::invalid_format(format!(
                "seek_to_sector({sector_offset}) exceeds track length {}",
                self.total_sectors
            )));
        }
        self.next_index = sector_offset;
        Ok(())
    }

    fn read_next_raw(&mut self) -> Result<[u8; RAW_SECTOR_SIZE as usize], AnalysisError> {
        let absolute = self
            .absolute_start_sector
            .checked_add(self.next_index)
            .ok_or_else(|| AnalysisError::invalid_format("PCM sector offset overflow"))?;
        match &mut self.source {
            PcmSource::Cue { files, current } => {
                let target_idx = files.iter().position(|f| f.contains(absolute)).ok_or_else(
                    || {
                        AnalysisError::invalid_format(format!(
                            "CUE PCM read requested disc sector {absolute}, which no FILE covers",
                        ))
                    },
                )?;

                // Swap the cached reader if we've moved to a different
                // FILE. Drop the old one first so we don't briefly hold
                // two handles on the same directory.
                let needs_swap = match current {
                    Some((idx, _)) => *idx != target_idx,
                    None => true,
                };
                if needs_swap {
                    *current = None;
                    let path = &files[target_idx].path;
                    let file = File::open(path).map_err(|e| {
                        AnalysisError::Io(std::io::Error::new(
                            e.kind(),
                            format!("BIN '{}': {}", path.display(), e),
                        ))
                    })?;
                    *current = Some((target_idx, BufReader::new(file)));
                }
                let (_, reader) = current.as_mut().expect("just set current");

                let file_info = &files[target_idx];
                if file_info.frame_bytes != RAW_SECTOR_SIZE {
                    return Err(AnalysisError::unsupported(format!(
                        "audio PCM requires 2352-byte frames, but '{}' stores {}-byte frames",
                        file_info.path.display(),
                        file_info.frame_bytes
                    )));
                }
                let byte_offset = u64::from(absolute - file_info.file_start_disc_sector)
                    .checked_mul(file_info.frame_bytes)
                    .ok_or_else(|| AnalysisError::invalid_format("PCM byte offset overflow"))?;
                reader.seek(SeekFrom::Start(byte_offset))?;
                let mut buf = [0u8; RAW_SECTOR_SIZE as usize];
                reader.read_exact(&mut buf)?;
                Ok(buf)
            }
            PcmSource::Chd(cache) => {
                let bin_sector = absolute.checked_sub(LEAD_IN_FRAMES).ok_or_else(|| {
                    AnalysisError::invalid_format(
                        "CHD PCM read requested a sector inside the lead-in region",
                    )
                })?;
                cache.read_raw_sector(bin_sector as u64)
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
