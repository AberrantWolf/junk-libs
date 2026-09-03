//! CHD (Compressed Hunks of Data) disc reading.

use junk_libs_core::AnalysisError;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::iso9660::{PrimaryVolumeDescriptor, parse_directory_record, parse_pvd_data};
use crate::layout::{LEAD_IN_FRAMES, TrackLayout, classify_mode};
use crate::sector::{MODE2_FORM1_DATA_OFFSET, RAW_SECTOR_SIZE};

/// Cached CHD reader that decompresses each hunk at most once for as long
/// as it lives.
///
/// Holds an open `chd::Chd<F>` plus the most recently decompressed hunk
/// (by number + bytes) and a reusable compressed-data scratch buffer. Each
/// call to [`ChdHunkCache::read_raw_sector`] only pays the decompression
/// cost when the requested sector lives in a different hunk from the
/// previous call — linear readers (PCM iteration, ISO directory walk) see
/// a roughly 10–40× speedup versus the original re-open-per-sector path.
///
/// This is the single implementation of "decompress a hunk and slice out
/// one raw sector". [`read_chd_raw_sector`] is a thin one-shot wrapper;
/// [`crate::pcm::TrackPcmReader`]'s CHD arm owns a long-lived instance.
pub struct ChdHunkCache<F: Read + Seek> {
    chd: chd::Chd<F>,
    hunk_size: u64,
    unit_bytes: u64,
    cached: Option<(u32, Vec<u8>)>,
    cmp_buf: Vec<u8>,
    #[cfg(test)]
    decompress_count: u32,
}

impl<F: Read + Seek> ChdHunkCache<F> {
    /// Seek `reader` to 0, parse the CHD header, and build a cache.
    pub fn open(mut reader: F) -> Result<Self, AnalysisError> {
        reader.seek(SeekFrom::Start(0))?;
        let chd = chd::Chd::open(reader, None)
            .map_err(|e| AnalysisError::other(format!("Failed to open CHD: {}", e)))?;
        // The header is the authoritative frame stride. Current MAME CD
        // CHDs use 2448-byte units even for SUBTYPE:NONE; older producers
        // may use 2352-byte units.
        let hunk_size = chd.header().hunk_size() as u64;
        let unit_bytes = chd.header().unit_bytes() as u64;
        let logical_bytes = chd.header().logical_bytes();
        if unit_bytes < RAW_SECTOR_SIZE {
            return Err(AnalysisError::corrupted_header(
                "CHD unit is smaller than a 2352-byte raw CD frame",
            ));
        }
        if hunk_size == 0 || !hunk_size.is_multiple_of(unit_bytes) {
            return Err(AnalysisError::corrupted_header(
                "CHD hunk size is not an integral number of frames",
            ));
        }
        if logical_bytes == 0 || !logical_bytes.is_multiple_of(unit_bytes) {
            return Err(AnalysisError::corrupted_header(
                "CHD logical size is not an integral number of frames",
            ));
        }
        Ok(Self {
            chd,
            hunk_size,
            unit_bytes,
            cached: None,
            cmp_buf: Vec::new(),
            #[cfg(test)]
            decompress_count: 0,
        })
    }

    /// Read a full 2352-byte raw CD sector. Decompresses the owning hunk
    /// only if it differs from the one already cached.
    pub fn read_raw_sector(
        &mut self,
        sector: u64,
    ) -> Result<[u8; RAW_SECTOR_SIZE as usize], AnalysisError> {
        let sector_byte_offset = sector
            .checked_mul(self.unit_bytes)
            .ok_or_else(|| AnalysisError::corrupted_header("CHD sector offset overflow"))?;
        if sector_byte_offset >= self.chd.header().logical_bytes() {
            return Err(AnalysisError::corrupted_header(
                "requested CHD sector is beyond the logical stream",
            ));
        }
        let hunk_num = u32::try_from(sector_byte_offset / self.hunk_size)
            .map_err(|_| AnalysisError::corrupted_header("CHD hunk number overflow"))?;
        let offset_in_hunk = (sector_byte_offset % self.hunk_size) as usize;

        let hunk_buf = self.ensure_hunk(hunk_num)?;

        let end = offset_in_hunk
            .checked_add(RAW_SECTOR_SIZE as usize)
            .ok_or_else(|| AnalysisError::corrupted_header("CHD sector window overflow"))?;
        if end > hunk_buf.len() {
            return Err(AnalysisError::corrupted_header(
                "CHD raw sector extends beyond hunk boundary",
            ));
        }
        let mut result = [0u8; RAW_SECTOR_SIZE as usize];
        result.copy_from_slice(&hunk_buf[offset_in_hunk..end]);
        Ok(result)
    }

    fn ensure_hunk(&mut self, hunk_num: u32) -> Result<&[u8], AnalysisError> {
        let needs_fetch = self.cached.as_ref().map(|(n, _)| *n) != Some(hunk_num);
        if needs_fetch {
            let mut hunk_buf = self.chd.get_hunksized_buffer();
            let mut hunk = self.chd.hunk(hunk_num).map_err(|e| {
                AnalysisError::other(format!("Failed to get CHD hunk {}: {}", hunk_num, e))
            })?;
            hunk.read_hunk_in(&mut self.cmp_buf, &mut hunk_buf)
                .map_err(|e| {
                    AnalysisError::other(format!(
                        "Failed to decompress CHD hunk {}: {}",
                        hunk_num, e
                    ))
                })?;
            #[cfg(test)]
            {
                self.decompress_count += 1;
            }
            self.cached = Some((hunk_num, hunk_buf));
        }
        Ok(&self.cached.as_ref().expect("cache populated above").1)
    }

    /// Number of hunks decompressed over this cache's lifetime. Test-only
    /// instrumentation for verifying reuse.
    #[cfg(test)]
    pub(crate) fn decompress_count(&self) -> u32 {
        self.decompress_count
    }
}

/// Read 2048 bytes of user data from a given sector in a CHD file.
///
/// CHD CD images store raw 2352-byte sectors plus 96 bytes of subchannel
/// data, for 2448 bytes per sector. This function decompresses the relevant
/// hunk and extracts user data at the Mode 2 Form 1 offset (24 bytes in).
///
/// Use [`read_chd_sector_mode1`] for Mode 1 sectors (offset 16).
pub fn read_chd_sector(
    reader: &mut dyn junk_libs_core::ReadSeek,
    sector: u64,
) -> Result<[u8; 2048], AnalysisError> {
    read_chd_sector_with_offset(reader, sector, MODE2_FORM1_DATA_OFFSET as usize)
}

/// Read 2048 bytes of user data from a Mode 1 sector in a CHD file.
///
/// Mode 1 sectors have user data at offset 16 (12 sync + 4 header).
/// Saturn uses Mode 1 sectors.
pub fn read_chd_sector_mode1(
    reader: &mut dyn junk_libs_core::ReadSeek,
    sector: u64,
) -> Result<[u8; 2048], AnalysisError> {
    read_chd_sector_with_offset(reader, sector, crate::sector::MODE1_DATA_OFFSET as usize)
}

/// Read a full 2352-byte raw sector from a CHD file.
///
/// Decompresses the hunk containing `sector` and returns the raw CD sector
/// bytes (sync + header + user data + ECC/EDC for Mode 1/2 data sectors;
/// straight PCM for audio sectors).
///
/// This is the low-level primitive underlying [`read_chd_sector`] /
/// [`read_chd_sector_mode1`] for data tracks, and is the preferred entry
/// point for audio-track consumers (e.g. PCM iteration) that need bytes
/// without a Mode-specific user-data offset applied.
pub fn read_chd_raw_sector(
    reader: &mut dyn junk_libs_core::ReadSeek,
    sector: u64,
) -> Result<[u8; RAW_SECTOR_SIZE as usize], AnalysisError> {
    let mut cache = ChdHunkCache::open(reader)?;
    cache.read_raw_sector(sector)
}

fn read_chd_sector_with_offset(
    reader: &mut dyn junk_libs_core::ReadSeek,
    sector: u64,
    data_offset: usize,
) -> Result<[u8; 2048], AnalysisError> {
    let raw = read_chd_raw_sector(reader, sector)?;
    if data_offset + 2048 > raw.len() {
        return Err(AnalysisError::corrupted_header(
            "CHD sector data offset exceeds raw sector size",
        ));
    }
    let mut result = [0u8; 2048];
    result.copy_from_slice(&raw[data_offset..data_offset + 2048]);
    Ok(result)
}

/// Read CHD header metadata for display purposes.
#[allow(dead_code)]
pub struct ChdInfo {
    pub version: u32,
    pub hunk_size: u32,
    pub total_hunks: u32,
    pub logical_size: u64,
}

/// Extract basic CHD file information without full decompression.
pub fn read_chd_info(reader: &mut dyn junk_libs_core::ReadSeek) -> Result<ChdInfo, AnalysisError> {
    reader.seek(SeekFrom::Start(0))?;

    let chd = chd::Chd::open(reader, None)
        .map_err(|e| AnalysisError::other(format!("Failed to open CHD: {}", e)))?;

    let header = chd.header();

    Ok(ChdInfo {
        version: header.version() as u32,
        hunk_size: header.hunk_size(),
        total_hunks: header.hunk_count(),
        logical_size: header.logical_bytes(),
    })
}

/// Read the ISO 9660 PVD from a CHD disc image.
///
/// Reads sector 16, parses as PVD, but does NOT check the system identifier.
/// Callers should validate the system identifier for their platform.
pub fn read_pvd_from_chd(
    reader: &mut dyn junk_libs_core::ReadSeek,
) -> Result<PrimaryVolumeDescriptor, AnalysisError> {
    let pvd_data = read_chd_sector(reader, crate::sector::PVD_SECTOR)?;
    parse_pvd_data(&pvd_data)
}

/// Find and read a file from a CHD disc image's ISO 9660 root directory.
///
/// This is a generic function that reads the PVD, walks the root directory,
/// and reads the specified file. It does NOT check the system identifier —
/// callers should validate that separately.
///
/// Returns both the PVD (for system identifier checking) and the file contents.
pub fn find_file_in_chd(
    reader: &mut dyn junk_libs_core::ReadSeek,
    filename: &str,
) -> Result<(PrimaryVolumeDescriptor, Vec<u8>), AnalysisError> {
    // Read PVD from sector 16
    let pvd_data = read_chd_sector(reader, crate::sector::PVD_SECTOR)?;

    // Verify PVD signature
    if pvd_data[0] != 0x01 || &pvd_data[1..6] != b"CD001" {
        return Err(AnalysisError::invalid_format(
            "CHD: Missing PVD at sector 16",
        ));
    }

    let pvd = parse_pvd_data(&pvd_data)?;

    // Walk root directory to find the file
    let dir_sectors = u64::from(pvd.root_dir_data_length).div_ceil(crate::sector::ISO_SECTOR_SIZE);
    let target_upper = filename.to_uppercase();

    for sector_offset in 0..dir_sectors {
        let sector = u64::from(pvd.root_dir_extent_lba) + sector_offset;
        let sector_data = read_chd_sector(reader, sector)?;

        let mut pos = 0;
        while pos < crate::sector::ISO_SECTOR_SIZE as usize {
            let record_len = sector_data[pos] as usize;
            if record_len == 0 {
                break;
            }
            if pos + record_len > crate::sector::ISO_SECTOR_SIZE as usize {
                break;
            }

            let record = &sector_data[pos..pos + record_len];
            if let Some(dir_rec) = parse_directory_record(record) {
                let id_upper = dir_rec.file_identifier.to_uppercase();
                let id_stripped = id_upper.split(';').next().unwrap_or(&id_upper);
                if id_stripped == target_upper {
                    let file_end = u64::from(dir_rec.extent_lba)
                        .checked_add(
                            u64::from(dir_rec.data_length).div_ceil(crate::sector::ISO_SECTOR_SIZE),
                        )
                        .ok_or_else(|| {
                            AnalysisError::corrupted_header("ISO 9660 file extent overflow")
                        })?;
                    if file_end > u64::from(pvd.volume_space_size) {
                        return Err(AnalysisError::corrupted_header(
                            "ISO 9660 file extends beyond the declared volume",
                        ));
                    }
                    // Read the file from CHD
                    let content = read_file_from_chd(reader, &dir_rec)?;
                    return Ok((pvd, content));
                }
            }

            pos += record_len;
        }
    }

    Err(AnalysisError::other(format!(
        "'{}' not found in CHD root directory",
        filename,
    )))
}

/// Maximum file size we'll read from an ISO 9660 filesystem (256 MB).
const MAX_ISO_FILE_SIZE: u32 = 256 * 1024 * 1024;

/// Read file content from a CHD image given a directory record.
pub fn read_file_from_chd(
    reader: &mut dyn junk_libs_core::ReadSeek,
    record: &crate::iso9660::DirectoryRecord,
) -> Result<Vec<u8>, AnalysisError> {
    if record.data_length > MAX_ISO_FILE_SIZE {
        return Err(AnalysisError::corrupted_header(
            "ISO 9660 file size exceeds safety limit",
        ));
    }
    let mut result = Vec::with_capacity(record.data_length as usize);
    let sectors_needed = u64::from(record.data_length).div_ceil(crate::sector::ISO_SECTOR_SIZE);
    let mut remaining = record.data_length as usize;

    for i in 0..sectors_needed {
        let sector = u64::from(record.extent_lba) + i;
        let sector_data = read_chd_sector(reader, sector)?;
        let to_copy = remaining.min(crate::sector::ISO_SECTOR_SIZE as usize);
        result.extend_from_slice(&sector_data[..to_copy]);
        remaining -= to_copy;
    }

    Ok(result)
}

/// Parsed CHD track metadata entry.
#[derive(Debug, Clone)]
pub struct ChdTrackInfo {
    /// Track number (1-based).
    pub track_number: u32,
    /// Track type string (e.g., "MODE1_RAW", "MODE2_RAW", "AUDIO").
    pub track_type: String,
    /// Number of data frames (sectors) in this track.
    pub frames: usize,
    /// Sector offset where this track starts in the CHD's linear sector space.
    /// Computed by summing the frames of all preceding tracks.
    pub start_sector: usize,
}

impl ChdTrackInfo {
    /// Returns true if this is a data track (MODE1 or MODE2, not AUDIO).
    pub fn is_data(&self) -> bool {
        self.track_type.contains("MODE")
    }
}

/// Parse all CHD track metadata entries.
///
/// CHD CD-ROM track metadata is stored as text strings like:
///   `TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:19560 PREGAP:0 ...`
///
/// Returns tracks sorted by track number with computed `start_sector` offsets.
pub fn parse_chd_tracks<F: std::io::Read + std::io::Seek>(
    chd: &mut chd::Chd<F>,
) -> Result<Vec<ChdTrackInfo>, AnalysisError> {
    use chd::metadata::{KnownMetadata, MetadataTag};

    let meta_refs: Vec<_> = chd.metadata_refs().collect();
    let mut tracks = Vec::new();

    for meta_ref in &meta_refs {
        let tag = meta_ref.metatag();
        if tag != KnownMetadata::CdRomTrack as u32 && tag != KnownMetadata::CdRomTrack2 as u32 {
            continue;
        }

        let meta = meta_ref
            .read(chd.inner())
            .map_err(|e| AnalysisError::other(format!("Failed to read CHD metadata: {}", e)))?;

        let text = std::str::from_utf8(&meta.value)
            .map_err(|_| AnalysisError::corrupted_header("CHD CD track metadata is not UTF-8"))?;
        tracks.push(parse_chd_track_text(text)?);
    }

    // Sort by track number and compute cumulative sector offsets
    tracks.sort_by_key(|t| t.track_number);
    let mut offset = 0usize;
    let mut previous = None;
    for track in &mut tracks {
        if previous == Some(track.track_number) {
            return Err(AnalysisError::corrupted_header(format!(
                "CHD declares track {} more than once",
                track.track_number
            )));
        }
        previous = Some(track.track_number);
        track.start_sector = offset;
        // MAME stores every CD track in an independently four-frame-padded
        // span. FRAMES remains the true track length; padding belongs to no
        // track but shifts the next track's internal start.
        let padded_frames = track
            .frames
            .checked_add(3)
            .map(|frames| frames / 4 * 4)
            .ok_or_else(|| AnalysisError::corrupted_header("CHD track length overflow"))?;
        offset = offset
            .checked_add(padded_frames)
            .ok_or_else(|| AnalysisError::corrupted_header("CHD track offset overflow"))?;
    }

    Ok(tracks)
}

fn parse_chd_track_text(text: &str) -> Result<ChdTrackInfo, AnalysisError> {
    let field = |name| {
        parse_meta_field(text, name).ok_or_else(|| {
            AnalysisError::corrupted_header(format!(
                "CHD CD track metadata is missing {name}: {text}"
            ))
        })
    };
    let track_number = field("TRACK")?.parse::<u32>().map_err(|_| {
        AnalysisError::corrupted_header(format!("invalid CHD track number: {text}"))
    })?;
    if !(1..=99).contains(&track_number) {
        return Err(AnalysisError::corrupted_header(format!(
            "CHD track number is outside 1..99: {text}"
        )));
    }
    let frames = field("FRAMES")?.parse::<usize>().map_err(|_| {
        AnalysisError::corrupted_header(format!("invalid CHD track frame count: {text}"))
    })?;
    if frames == 0 {
        return Err(AnalysisError::corrupted_header(format!(
            "CHD track has zero frames: {text}"
        )));
    }
    let track_type = field("TYPE")?.to_ascii_uppercase();
    if !matches!(
        track_type.as_str(),
        "AUDIO"
            | "MODE1"
            | "MODE1_RAW"
            | "MODE2"
            | "MODE2_FORM1"
            | "MODE2_FORM2"
            | "MODE2_FORM_MIX"
            | "MODE2_RAW"
    ) {
        return Err(AnalysisError::unsupported(format!(
            "unsupported CHD CD track type {track_type}"
        )));
    }
    Ok(ChdTrackInfo {
        track_number,
        track_type,
        frames,
        start_sector: 0,
    })
}

/// Select the largest data track from parsed CHD track metadata.
///
/// Returns the track with the most frames among data tracks (those whose
/// TYPE contains "MODE"). This handles both single-data-track discs (PS1/PS2
/// where Track 1 is the only data track) and multi-data-track discs (Saturn
/// where Track 2 is often the largest data track).
pub fn select_largest_data_track(tracks: &[ChdTrackInfo]) -> Option<&ChdTrackInfo> {
    tracks
        .iter()
        .filter(|t| t.is_data())
        .max_by_key(|t| t.frames)
}

/// Parse CHD track metadata (CHTR or CHT2) to find the number of frames
/// (sectors) in Track 1. Returns `None` if no track metadata is found.
///
/// Prefer [`parse_chd_tracks`] + [`select_largest_data_track`] for hash
/// matching, as some discs (Saturn) store the main data in Track 2.
pub fn parse_chd_track1_frames<F: std::io::Read + std::io::Seek>(
    chd: &mut chd::Chd<F>,
) -> Result<Option<usize>, AnalysisError> {
    let tracks = parse_chd_tracks(chd)?;
    if let Some(track) = tracks.iter().find(|t| t.track_number == 1) {
        log::info!("CHD track metadata: Track 1 has {} frames", track.frames);
        Ok(Some(track.frames))
    } else {
        Ok(None)
    }
}

/// Extract a field value from CHD metadata text (e.g., "FRAMES" from
/// `"TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:229020"`).
pub fn parse_meta_field<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    let prefix = format!("{}:", field);
    for token in text.split_whitespace() {
        if let Some(value) = token.strip_prefix(&prefix) {
            return Some(value);
        }
    }
    None
}

// -- Absolute-sector layout computation --

/// Convert a slice of parsed `ChdTrackInfo` entries to `TrackLayout`s by
/// adding the 150-frame lead-in to each track's linear start position.
///
/// CHD stores tracks in a linear sector space starting at 0; audio-CD
/// identification conventions treat track 1 as starting at absolute
/// sector 150. This helper bridges the two.
pub fn compute_chd_layout(tracks: &[ChdTrackInfo]) -> Vec<TrackLayout> {
    tracks
        .iter()
        .map(|t| TrackLayout {
            number: t.track_number as u8,
            absolute_offset: t.start_sector as u32 + LEAD_IN_FRAMES,
            length_sectors: t.frames as u32,
            kind: classify_mode(&t.track_type),
            mode: t.track_type.clone(),
        })
        .collect()
}

/// Open a CHD file at `path`, parse its track metadata, and return the
/// absolute-sector layout.
pub fn read_chd_layout(path: &Path) -> Result<Vec<TrackLayout>, AnalysisError> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut chd = chd::Chd::open(&mut reader, None)
        .map_err(|e| AnalysisError::other(format!("Failed to open CHD: {}", e)))?;
    let tracks = parse_chd_tracks(&mut chd)?;
    if tracks.is_empty() {
        return Err(AnalysisError::invalid_format(
            "CHD has no CD track metadata",
        ));
    }
    Ok(compute_chd_layout(&tracks))
}

#[cfg(test)]
#[path = "tests/chd_tests.rs"]
mod tests;
