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
//! extracted here; consumers that need per-track PCM (e.g. phono-junk for
//! AccurateRip and FLAC export) build on top of [`chd`] and [`cue`] directly.

pub mod chd;
pub mod cue;
pub mod format;
pub mod iso9660;
pub mod sector;

pub use cue::{
    CueCompatReport, CueFile, CueIndex, CueSheet, CueTrack, check_cue_compat,
    convert_cue_to_standard,
};
pub use format::{DiscFormat, detect_disc_format};
pub use iso9660::{DirectoryRecord, PrimaryVolumeDescriptor, find_file_in_root, read_pvd};
pub use sector::{
    CD_SYNC_PATTERN, CHD_MAGIC, ISO_SECTOR_SIZE, MODE1_DATA_OFFSET, MODE2_FORM1_DATA_OFFSET,
    PVD_SECTOR, RAW_SECTOR_SIZE, read_sector_data,
};

/// Test helpers for constructing synthetic disc images.
pub mod test_helpers;
