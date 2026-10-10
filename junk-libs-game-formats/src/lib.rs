//! Reader-only normalization and hashing primitives for game dumps.
//!
//! This crate deliberately owns no platform naming, catalog selection, filesystem
//! access, or verification policy. Callers provide already-authorized readers and,
//! where byte shape alone is ambiguous, an explicit layout.

use std::io::SeekFrom;

pub use junk_libs_core::{AnalysisError, ReadSeek};
use sha1::Digest as _;

pub const RAW_DOMAIN: NormalizationDomain = NormalizationDomain::new("org.junk.game.raw-bytes", 1);
pub const INES_PAYLOAD_DOMAIN: NormalizationDomain =
    NormalizationDomain::new("org.junk.game.nes.ines-payload", 1);
pub const N64_BIG_ENDIAN_DOMAIN: NormalizationDomain =
    NormalizationDomain::new("org.junk.game.n64.z64-big-endian", 1);
pub const SNES_PAYLOAD_DOMAIN: NormalizationDomain =
    NormalizationDomain::new("org.junk.game.snes.headerless-payload", 1);
pub const PCE_PAYLOAD_DOMAIN: NormalizationDomain =
    NormalizationDomain::new("org.junk.game.pce.hucard-payload", 1);
pub const CUE_RAW_2352_TRACK_DOMAIN: NormalizationDomain =
    NormalizationDomain::new("org.junk.game.cd.raw-2352-track", 1);

const INES_HEADER_BYTES: u64 = 16;
const TRAINER_BYTES: u64 = 512;
const SNES_BANK_BYTES: u64 = 32 * 1024;
const PCE_BANK_BYTES: u64 = 8 * 1024;
const STREAM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizationDomain {
    pub name: &'static str,
    pub version: u32,
}

impl NormalizationDomain {
    pub const fn new(name: &'static str, version: u32) -> Self {
        Self { name, version }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedHashes {
    pub bytes: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
    pub domain: NormalizationDomain,
}

struct Hashers {
    crc32: crc32fast::Hasher,
    md5: md5::Context,
    sha1: sha1::Sha1,
    sha256: sha2::Sha256,
    bytes: u64,
}

impl Hashers {
    fn new() -> Self {
        Self {
            crc32: crc32fast::Hasher::new(),
            md5: md5::Context::new(),
            sha1: sha1::Sha1::new(),
            sha256: sha2::Sha256::new(),
            bytes: 0,
        }
    }

    fn update(&mut self, bytes: &[u8]) -> Result<(), AnalysisError> {
        self.crc32.update(bytes);
        self.md5.consume(bytes);
        self.sha1.update(bytes);
        self.sha256.update(bytes);
        self.bytes = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| AnalysisError::other("hashed byte count overflow"))?;
        Ok(())
    }

    fn finish(self, domain: NormalizationDomain) -> QualifiedHashes {
        QualifiedHashes {
            bytes: self.bytes,
            crc32: format!("{:08x}", self.crc32.finalize()),
            md5: format!("{:x}", self.md5.compute()),
            sha1: format!("{:x}", self.sha1.finalize()),
            sha256: format!("{:x}", self.sha256.finalize()),
            domain,
        }
    }
}

fn reader_len<R: ReadSeek + ?Sized>(reader: &mut R) -> Result<u64, AnalysisError> {
    let bytes = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    Ok(bytes)
}

fn hash_range<R: ReadSeek + ?Sized>(
    reader: &mut R,
    offset: u64,
    bytes: u64,
    domain: NormalizationDomain,
) -> Result<QualifiedHashes, AnalysisError> {
    hash_range_with_progress(reader, offset, bytes, domain, |_| {})
}

fn hash_range_with_progress<R: ReadSeek + ?Sized>(
    reader: &mut R,
    offset: u64,
    bytes: u64,
    domain: NormalizationDomain,
    mut progress: impl FnMut(u64),
) -> Result<QualifiedHashes, AnalysisError> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut remaining = bytes;
    let mut buffer = vec![0; STREAM_BUFFER_BYTES];
    let mut hashers = Hashers::new();
    while remaining != 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| AnalysisError::other("hash buffer length overflow"))?;
        let read = reader.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(AnalysisError::too_small(bytes, bytes - remaining));
        }
        hashers.update(&buffer[..read])?;
        remaining -= read as u64;
        progress(bytes - remaining);
    }
    Ok(hashers.finish(domain))
}

pub fn hash_raw<R: ReadSeek + ?Sized>(reader: &mut R) -> Result<QualifiedHashes, AnalysisError> {
    let bytes = reader_len(reader)?;
    hash_range(reader, 0, bytes, RAW_DOMAIN)
}

/// Candidate interpretation for standard headerless PRG/CHR DAT bytes. This is
/// not structural recognition; the caller must require a complete catalog match.
pub fn hash_headerless_ines<R: ReadSeek + ?Sized>(
    reader: &mut R,
) -> Result<QualifiedHashes, AnalysisError> {
    let bytes = reader_len(reader)?;
    if bytes < 8192 || bytes % 8192 != 0 {
        return Err(AnalysisError::other(
            "Unsupported headerless PRG/CHR layout",
        ));
    }
    hash_range(reader, 0, bytes, INES_PAYLOAD_DOMAIN)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InesVersion {
    INes,
    Nes2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InesLayout {
    pub version: InesVersion,
    pub mapper: u16,
    pub submapper: u8,
    pub prg_rom_bytes: u64,
    pub chr_rom_bytes: u64,
    pub payload_offset: u64,
    pub payload_bytes: u64,
}

fn nes2_rom_bytes(lsb: u8, msb: u8, unit: u64, label: &str) -> Result<u64, AnalysisError> {
    if msb == 0x0f {
        let exponent = lsb >> 2;
        let multiplier = u64::from((lsb & 0x03) * 2 + 1);
        1_u64
            .checked_shl(u32::from(exponent))
            .and_then(|value| value.checked_mul(multiplier))
            .ok_or_else(|| {
                AnalysisError::unsupported(format!("NES 2.0 {label} ROM size is too large"))
            })
    } else {
        (u64::from(lsb) | (u64::from(msb) << 8))
            .checked_mul(unit)
            .ok_or_else(|| AnalysisError::corrupted_header(format!("{label} ROM size overflow")))
    }
}

/// Validate the exact trainer-free iNES/NES 2.0 payload span.
///
/// Trainer-bearing images are preserved but deliberately unsupported for catalog
/// normalization: silently dropping or including the trainer would claim a byte
/// domain that this checkpoint has not qualified. NES 2.0 miscellaneous ROMs are
/// also unsupported because their lengths are not encoded by the header.
pub fn inspect_ines<R: ReadSeek + ?Sized>(reader: &mut R) -> Result<InesLayout, AnalysisError> {
    let file_bytes = reader_len(reader)?;
    if file_bytes < INES_HEADER_BYTES {
        return Err(AnalysisError::too_small(INES_HEADER_BYTES, file_bytes));
    }
    let mut header = [0; INES_HEADER_BYTES as usize];
    reader.read_exact(&mut header)?;
    if header[..4] != *b"NES\x1a" {
        return Err(AnalysisError::invalid_format("missing iNES magic"));
    }
    if header[6] & 0x04 != 0 {
        return Err(AnalysisError::unsupported(
            "trainer-bearing iNES normalization is unqualified",
        ));
    }

    let nes2 = header[7] & 0x0c == 0x08;
    let (version, mapper, submapper, prg_rom_bytes, chr_rom_bytes) = if nes2 {
        if header[14] & 0x03 != 0 {
            return Err(AnalysisError::unsupported(
                "NES 2.0 miscellaneous ROM normalization is unqualified",
            ));
        }
        let mapper = u16::from(header[6] >> 4)
            | (u16::from(header[7] & 0xf0))
            | (u16::from(header[8] & 0x0f) << 8);
        (
            InesVersion::Nes2,
            mapper,
            header[8] >> 4,
            nes2_rom_bytes(header[4], header[9] & 0x0f, 16 * 1024, "PRG")?,
            nes2_rom_bytes(header[5], header[9] >> 4, 8 * 1024, "CHR")?,
        )
    } else {
        (
            InesVersion::INes,
            u16::from(header[6] >> 4) | u16::from(header[7] & 0xf0),
            0,
            u64::from(header[4]) * 16 * 1024,
            u64::from(header[5]) * 8 * 1024,
        )
    };
    if prg_rom_bytes == 0 {
        return Err(AnalysisError::corrupted_header("iNES declares no PRG ROM"));
    }
    let payload_bytes = prg_rom_bytes
        .checked_add(chr_rom_bytes)
        .ok_or_else(|| AnalysisError::corrupted_header("iNES payload length overflow"))?;
    let expected = INES_HEADER_BYTES
        .checked_add(payload_bytes)
        .ok_or_else(|| AnalysisError::corrupted_header("iNES file length overflow"))?;
    if file_bytes != expected {
        return Err(AnalysisError::invalid_format(format!(
            "iNES length is {file_bytes} bytes; header requires exactly {expected}"
        )));
    }
    reader.seek(SeekFrom::Start(0))?;
    Ok(InesLayout {
        version,
        mapper,
        submapper,
        prg_rom_bytes,
        chr_rom_bytes,
        payload_offset: INES_HEADER_BYTES,
        payload_bytes,
    })
}

pub fn hash_ines_payload<R: ReadSeek + ?Sized>(
    reader: &mut R,
) -> Result<(InesLayout, QualifiedHashes), AnalysisError> {
    let layout = inspect_ines(reader)?;
    let hashes = hash_range(
        reader,
        layout.payload_offset,
        layout.payload_bytes,
        INES_PAYLOAD_DOMAIN,
    )?;
    Ok((layout, hashes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum N64Format {
    Z64,
    V64,
    N64,
}

pub const MAGIC_Z64: [u8; 4] = [0x80, 0x37, 0x12, 0x40];
pub const MAGIC_V64: [u8; 4] = [0x37, 0x80, 0x40, 0x12];
pub const MAGIC_N64: [u8; 4] = [0x40, 0x12, 0x37, 0x80];

pub fn detect_n64_format(magic: &[u8]) -> Option<N64Format> {
    match magic.get(..4)? {
        value if value == MAGIC_Z64 => Some(N64Format::Z64),
        value if value == MAGIC_V64 => Some(N64Format::V64),
        value if value == MAGIC_N64 => Some(N64Format::N64),
        _ => None,
    }
}

/// Normalize complete N64 byte-order words in place.
///
/// Callers that stream arbitrary read sizes should use [`hash_n64`] so partial
/// groups are carried across read boundaries and a truncated final group fails.
pub fn normalize_n64_in_place(bytes: &mut [u8], format: N64Format) {
    match format {
        N64Format::Z64 => {}
        N64Format::V64 => bytes
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .for_each(|pair| pair.swap(0, 1)),
        N64Format::N64 => bytes
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|word| word.reverse()),
    }
}

pub fn hash_n64<R: ReadSeek + ?Sized>(
    reader: &mut R,
) -> Result<(N64Format, QualifiedHashes), AnalysisError> {
    let file_bytes = reader_len(reader)?;
    if file_bytes < 4 {
        return Err(AnalysisError::too_small(4, file_bytes));
    }
    if !file_bytes.is_multiple_of(4) {
        return Err(AnalysisError::invalid_format(
            "N64 ROM length is not aligned to 32-bit words",
        ));
    }
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    let format = detect_n64_format(&magic)
        .ok_or_else(|| AnalysisError::invalid_format("unrecognized N64 byte order"))?;
    reader.seek(SeekFrom::Start(0))?;

    let mut hashers = Hashers::new();
    let mut input = vec![0; STREAM_BUFFER_BYTES + 3];
    let mut carried = 0;
    loop {
        let remaining = file_bytes - hashers.bytes - carried as u64;
        if remaining == 0 {
            break;
        }
        let wanted = remaining.min((input.len() - carried) as u64) as usize;
        let read = reader.read(&mut input[carried..carried + wanted])?;
        if read == 0 {
            break;
        }
        let available = carried + read;
        let complete = available - available % 4;
        normalize_n64_in_place(&mut input[..complete], format);
        hashers.update(&input[..complete])?;
        input.copy_within(complete..available, 0);
        carried = available - complete;
    }
    if carried != 0 {
        return Err(AnalysisError::invalid_format(
            "N64 ROM ended inside a 32-bit word",
        ));
    }
    if hashers.bytes != file_bytes {
        return Err(AnalysisError::invalid_format(
            "N64 stream length changed while hashing",
        ));
    }
    Ok((format, hashers.finish(N64_BIG_ENDIAN_DOMAIN)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopierHeader {
    Absent,
    Present512,
}

impl CopierHeader {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Absent => 0,
            Self::Present512 => 512,
        }
    }
}

fn explicit_banked_layout(
    file_bytes: u64,
    layout: CopierHeader,
    bank_bytes: u64,
    minimum_bytes: u64,
    label: &str,
) -> Result<(u64, u64), AnalysisError> {
    let offset = layout.bytes();
    let payload = file_bytes.checked_sub(offset).ok_or_else(|| {
        AnalysisError::too_small(offset.saturating_add(minimum_bytes), file_bytes)
    })?;
    if payload < minimum_bytes || !payload.is_multiple_of(bank_bytes) {
        return Err(AnalysisError::invalid_format(format!(
            "{label} payload is not an explicitly qualified {bank_bytes}-byte bank layout"
        )));
    }
    Ok((offset, payload))
}

pub fn detect_snes_copier_header(file_bytes: u64) -> Result<CopierHeader, AnalysisError> {
    if file_bytes >= SNES_BANK_BYTES && file_bytes.is_multiple_of(SNES_BANK_BYTES) {
        Ok(CopierHeader::Absent)
    } else if file_bytes >= SNES_BANK_BYTES + TRAINER_BYTES
        && (file_bytes - TRAINER_BYTES).is_multiple_of(SNES_BANK_BYTES)
    {
        Ok(CopierHeader::Present512)
    } else {
        Err(AnalysisError::invalid_format(
            "SNES file is neither whole 32-KiB banks nor that layout plus 512 bytes",
        ))
    }
}

pub fn hash_snes_payload<R: ReadSeek + ?Sized>(
    reader: &mut R,
    layout: CopierHeader,
) -> Result<QualifiedHashes, AnalysisError> {
    let file_bytes = reader_len(reader)?;
    let (offset, payload) =
        explicit_banked_layout(file_bytes, layout, SNES_BANK_BYTES, SNES_BANK_BYTES, "SNES")?;
    hash_range(reader, offset, payload, SNES_PAYLOAD_DOMAIN)
}

pub fn detect_pce_copier_header(file_bytes: u64) -> Result<CopierHeader, AnalysisError> {
    if file_bytes >= PCE_BANK_BYTES && file_bytes.is_multiple_of(PCE_BANK_BYTES) {
        Ok(CopierHeader::Absent)
    } else if file_bytes >= PCE_BANK_BYTES + TRAINER_BYTES
        && (file_bytes - TRAINER_BYTES).is_multiple_of(PCE_BANK_BYTES)
    {
        Ok(CopierHeader::Present512)
    } else {
        Err(AnalysisError::invalid_format(
            "PC Engine file is neither whole 8-KiB banks nor that layout plus 512 bytes",
        ))
    }
}

pub fn hash_pce_payload<R: ReadSeek + ?Sized>(
    reader: &mut R,
    layout: CopierHeader,
) -> Result<QualifiedHashes, AnalysisError> {
    let file_bytes = reader_len(reader)?;
    let (offset, payload) = explicit_banked_layout(
        file_bytes,
        layout,
        PCE_BANK_BYTES,
        PCE_BANK_BYTES,
        "PC Engine",
    )?;
    hash_range(reader, offset, payload, PCE_PAYLOAD_DOMAIN)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrackHashes {
    pub track_number: u8,
    pub is_data: bool,
    pub source_name: String,
    pub source_byte_offset: u64,
    pub synthetic_pregap_frames: u32,
    pub synthetic_postgap_frames: u32,
    pub hashes: QualifiedHashes,
}

/// Hash every ordered raw-2352 stored CUE track from caller-provided readers.
///
/// `PREGAP` and `POSTGAP` declarations remain metadata and never synthesize bytes.
/// An in-file `INDEX 00` is part of the following track's stored span, as computed
/// by `junk-libs-disc`. Every referenced source must be present and exactly sized.
pub fn hash_cue_raw_2352<R: ReadSeek>(
    cue_text: &str,
    source_len: impl Fn(&str) -> Result<u64, AnalysisError>,
    open_source: impl FnMut(&str) -> Result<R, AnalysisError>,
) -> Result<Vec<CueTrackHashes>, AnalysisError> {
    hash_cue_raw_2352_with_progress(cue_text, source_len, open_source, &|_, _| {})
}

pub fn hash_cue_raw_2352_with_progress<R: ReadSeek>(
    cue_text: &str,
    source_len: impl Fn(&str) -> Result<u64, AnalysisError>,
    mut open_source: impl FnMut(&str) -> Result<R, AnalysisError>,
    progress: &dyn Fn(u64, u64),
) -> Result<Vec<CueTrackHashes>, AnalysisError> {
    if cue_text.len() > 1024 * 1024 {
        return Err(AnalysisError::other("CUE exceeds 1 MiB"));
    }
    for (index, line) in cue_text.lines().enumerate() {
        if index >= 4096 || line.len() > 4096 {
            return Err(AnalysisError::other("CUE directive bounds exceeded"));
        }
        let Some(keyword) = line.split_whitespace().next() else {
            continue;
        };
        if !matches!(
            keyword.to_ascii_uppercase().as_str(),
            "FILE"
                | "TRACK"
                | "INDEX"
                | "PREGAP"
                | "POSTGAP"
                | "REM"
                | "TITLE"
                | "PERFORMER"
                | "SONGWRITER"
                | "CATALOG"
                | "ISRC"
                | "FLAGS"
        ) {
            return Err(AnalysisError::unsupported(format!(
                "Unsupported CUE directive: {keyword}"
            )));
        }
    }
    let sheet = junk_libs_disc::cue::parse_cue(cue_text)?;
    let mut source_names = std::collections::BTreeSet::new();
    for file in &sheet.files {
        if !file.file_type.eq_ignore_ascii_case("BINARY") {
            return Err(AnalysisError::unsupported(
                "qualified CUE hashes require BINARY sources",
            ));
        }
        let logical_name = file
            .filename
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect::<Vec<_>>();
        if !source_names.insert(logical_name) {
            return Err(AnalysisError::unsupported(
                "repeated CUE FILE sources have ambiguous byte ownership",
            ));
        }
        let bytes = source_len(&file.filename)?;
        for track in &file.tracks {
            if track.number == 0
                || track.number > 99
                || track
                    .indexes
                    .iter()
                    .filter(|index| index.number == 1)
                    .count()
                    != 1
            {
                return Err(AnalysisError::invalid_format(
                    "CUE track requires a valid number and exactly one INDEX 01",
                ));
            }
            let mut previous = None;
            for index in &track.indexes {
                let frame = index.to_sector_offset();
                if frame
                    .checked_mul(junk_libs_disc::RAW_SECTOR_SIZE)
                    .is_none_or(|offset| offset >= bytes)
                    || previous.is_some_and(|(number, position)| {
                        index.number <= number || frame < position
                    })
                {
                    return Err(AnalysisError::invalid_format(
                        "CUE indexes are unordered or outside their source",
                    ));
                }
                previous = Some((index.number, frame));
            }
        }
    }
    let spans = junk_libs_disc::compute_cue_track_spans(&sheet, &source_len)?;
    if spans.is_empty() {
        return Err(AnalysisError::invalid_format("CUE describes no tracks"));
    }
    for (track, span) in sheet.files.iter().flat_map(|file| &file.tracks).zip(&spans) {
        let end = span
            .byte_offset
            .checked_add(span.byte_len)
            .ok_or_else(|| AnalysisError::invalid_format("CUE span overflow"))?;
        if track.indexes.iter().any(|index| {
            index
                .to_sector_offset()
                .checked_mul(junk_libs_disc::RAW_SECTOR_SIZE)
                .is_none_or(|offset| offset < span.byte_offset || offset >= end)
        }) {
            return Err(AnalysisError::invalid_format(
                "CUE index falls outside its stored track span",
            ));
        }
    }
    let total = spans.iter().try_fold(0_u64, |total, span| {
        total
            .checked_add(span.byte_len)
            .ok_or_else(|| AnalysisError::invalid_format("CUE track byte total overflow"))
    })?;
    let mut completed = 0_u64;
    let mut tracks = Vec::with_capacity(spans.len());
    for span in spans {
        let frame_bytes = junk_libs_disc::checked_sector_size_for_mode(&span.mode)?;
        if frame_bytes != junk_libs_disc::RAW_SECTOR_SIZE {
            return Err(AnalysisError::unsupported(format!(
                "CUE TRACK {:02} uses {frame_bytes}-byte stored frames; only raw 2352 is qualified",
                span.track_number
            )));
        }
        let mut reader = open_source(&span.filename)?;
        let actual = reader_len(&mut reader)?;
        let declared = source_len(&span.filename)?;
        if actual != declared {
            return Err(AnalysisError::invalid_format(format!(
                "CUE source {} is {actual} bytes; frozen inventory declares {declared}",
                span.filename
            )));
        }
        let required_end = span
            .byte_offset
            .checked_add(span.byte_len)
            .ok_or_else(|| AnalysisError::invalid_format("CUE track byte span overflow"))?;
        if actual < required_end {
            return Err(AnalysisError::too_small(required_end, actual));
        }
        let base = completed;
        let hashes = hash_range_with_progress(
            &mut reader,
            span.byte_offset,
            span.byte_len,
            CUE_RAW_2352_TRACK_DOMAIN,
            |done| progress(base + done, total),
        )?;
        completed += span.byte_len;
        tracks.push(CueTrackHashes {
            track_number: span.track_number,
            is_data: span.kind == junk_libs_disc::TrackKind::Data,
            source_name: span.filename,
            source_byte_offset: span.byte_offset,
            synthetic_pregap_frames: span.synthetic_pregap_frames,
            synthetic_postgap_frames: span.synthetic_postgap_frames,
            hashes,
        });
    }
    Ok(tracks)
}

#[cfg(feature = "chd")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdTrackHashes {
    pub track_number: u32,
    pub is_data: bool,
    pub frames: u32,
    pub hashes: QualifiedHashes,
}

/// Hash every true ordered CHD CD track as raw 2352-byte main-channel frames.
/// CHD four-frame alignment padding and subchannel bytes are excluded by the
/// version-pinned `junk-libs-disc` reader.
#[cfg(feature = "chd")]
pub fn hash_chd_raw_2352<R: ReadSeek>(
    reader: &mut R,
    progress: &dyn Fn(u64, u64),
) -> Result<Vec<ChdTrackHashes>, AnalysisError> {
    let info = junk_libs_disc::read_chd_track_info(reader)?;
    if info.is_empty() {
        return Err(AnalysisError::invalid_format("CHD describes no tracks"));
    }
    let total = info.iter().try_fold(0_u64, |total, track| {
        u64::try_from(track.frames)
            .ok()
            .and_then(|frames| frames.checked_mul(junk_libs_disc::RAW_SECTOR_SIZE))
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or_else(|| AnalysisError::invalid_format("CHD track byte total overflow"))
    })?;
    let mut hashers = info
        .iter()
        .map(|track| (track.track_number, Hashers::new()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut completed = 0_u64;
    let visited = junk_libs_disc::visit_chd_raw_tracks(reader, &mut |track, raw| {
        hashers
            .get_mut(&track.track_number)
            .ok_or_else(|| AnalysisError::corrupted_header("CHD track metadata changed"))?
            .update(raw)?;
        completed = completed
            .checked_add(raw.len() as u64)
            .ok_or_else(|| AnalysisError::other("CHD progress overflow"))?;
        progress(completed, total);
        Ok(())
    })?;
    visited
        .into_iter()
        .map(|track| {
            let hashes = hashers
                .remove(&track.track_number)
                .ok_or_else(|| AnalysisError::corrupted_header("CHD track was not visited"))?
                .finish(CUE_RAW_2352_TRACK_DOMAIN);
            let frames = u32::try_from(track.frames)
                .map_err(|_| AnalysisError::unsupported("CHD track has too many frames"))?;
            Ok(ChdTrackHashes {
                track_number: track.track_number,
                is_data: track.is_data(),
                frames,
                hashes,
            })
        })
        .collect()
}

mod detection;
pub use detection::*;
