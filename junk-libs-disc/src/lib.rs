//! Shared CD-ROM and optical disc utilities.
//!
//! Generic disc image handling — parsing and reading, no domain semantics:
//! - Disc format detection (ISO, raw BIN, CUE, CHD)
//! - ISO 9660 filesystem parsing (PVD, directory walking, file reading)
//! - CUE sheet parsing (standard + CDRWin compatibility)
//! - CHD compressed disc reading
//! - CD sector constants and helpers
//!
//! Redump-specific track-aware hashing lives in `retro-junk` and is not
//! extracted here. For per-track audio PCM iteration (phono-junk's
//! AccurateRip CRC path, future FLAC export, retro-junk's CDDA tracks),
//! see the [`pcm`] module.

pub mod chd;
pub mod cue;
pub mod format;
pub mod iso9660;
pub mod layout;
pub mod pcm;
pub mod sector;

#[cfg(test)]
#[path = "tests/layout_tests.rs"]
mod layout_tests;

pub use chd::{compute_chd_layout, read_chd_layout, read_chd_raw_sector};
pub use cue::{
    CueCompatReport, CueFile, CueIndex, CueSheet, CueTrack, check_cue_compat, compute_cue_layout,
    convert_cue_to_standard, read_cue_layout,
};
pub use format::{DiscFormat, detect_disc_format};
pub use iso9660::{DirectoryRecord, PrimaryVolumeDescriptor, find_file_in_root, read_pvd};
pub use layout::{LEAD_IN_FRAMES, TrackKind, TrackLayout, classify_mode};
pub use pcm::{PCM_SAMPLES_PER_SECTOR, PcmSector, TrackPcmReader, sector_to_samples};
pub use sector::{
    CD_SYNC_PATTERN, CHD_MAGIC, ISO_SECTOR_SIZE, MODE1_DATA_OFFSET, MODE2_FORM1_DATA_OFFSET,
    PVD_SECTOR, RAW_SECTOR_SIZE, read_sector_data,
};

/// Test helpers for constructing synthetic disc images.
pub mod test_helpers;
