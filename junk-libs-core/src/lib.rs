//! Generic infrastructure shared by retro-junk and phono-junk.
//!
//! - Streaming multi-algorithm hashing ([`MultiHasher`])
//! - ASCII/byte utilities ([`util`])
//! - Multi-disc filename grouping ([`disc`])
//! - Checksum descriptors ([`checksum`])
//! - [`ReadSeek`] trait alias
//! - [`AnalysisError`] for I/O + format errors
//!
//! This crate holds only the genuinely domain-agnostic bits. Retro-game and
//! audio-specific concepts live in `retro-junk-core` and `phono-junk-core`
//! respectively.

use serde::{Deserialize, Serialize};

pub mod checksum;
pub mod disc;
pub mod error;
pub mod hasher;
pub mod read_seek;
pub mod util;

pub use checksum::{ChecksumAlgorithm, ExpectedChecksum};
pub use error::AnalysisError;
pub use hasher::MultiHasher;
pub use read_seek::ReadSeek;

/// Progress callback for streaming hash computation.
///
/// Receives `(bytes_processed, total_bytes)`. Called periodically by the
/// hasher so callers can report incremental progress for large files
/// (CHD decompression, multi-GB BIN hashes, etc.).
pub type HashProgressFn<'a> = Option<&'a dyn Fn(u64, u64)>;

/// Hash results for a file or container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHashes {
    pub crc32: String,
    pub sha1: Option<String>,
    pub md5: Option<String>,
    /// Size of the data that was hashed (after header stripping or container extraction).
    pub data_size: u64,
    /// Warnings about potential issues with the hash calculation.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Which hash algorithms to compute in a streaming pass.
///
/// CRC32 is always included. Higher modes add SHA1 and MD5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithms {
    /// CRC32 only.
    Crc32,
    /// CRC32 + SHA1.
    Crc32Sha1,
    /// CRC32 + SHA1 + MD5.
    All,
}

impl HashAlgorithms {
    pub fn crc32(&self) -> bool {
        true
    }
    pub fn sha1(&self) -> bool {
        matches!(self, Self::Crc32Sha1 | Self::All)
    }
    pub fn md5(&self) -> bool {
        matches!(self, Self::All)
    }
}
