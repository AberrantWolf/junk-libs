//! CD-TEXT binary parser for redumper `.cdtext` sidecars.
//!
//! CD-TEXT is defined by the Sony/Philips CD-TEXT spec and referenced by
//! IEC 60908. The lead-in carries up to 8 language blocks, each a sequence
//! of 18-byte packs carrying title/performer/songwriter/etc. strings for
//! the album and each track.
//!
//! Redumper's `.cdtext` file is the raw READ-TOC-with-CD-TEXT-format
//! response: a 4-byte response header followed by 18-byte packs. Some
//! rippers strip the 4-byte header; we handle both cases transparently.
//!
//! ## Pack layout (18 bytes)
//!
//! | Offset | Size | Meaning                                        |
//! |--------|------|------------------------------------------------|
//! | 0      | 1    | Pack type (0x80 = title, 0x81 = performer, …)  |
//! | 1      | 1    | Track number (0 = album; 1–99 = track)         |
//! | 2      | 1    | Sequence number within the block               |
//! | 3      | 1    | DBCS flag (bit 7), block# (bits 4-6), char pos |
//! | 4..16  | 12   | Text data (null-terminated strings)            |
//! | 16..18 | 2    | CRC-16 (optional)                              |
//!
//! Strings for a given (block, pack-type) are concatenated across all
//! sequence-ordered packs of that type, then split on null bytes. The
//! resulting vector's index *is* the track number — `strings[0]` is the
//! album-wide value, `strings[N]` is track `N`'s value.

use std::path::Path;

use junk_libs_core::AnalysisError;

const PACK_SIZE: usize = 18;

// Pack-type IDs (Sony/Philips CD-TEXT spec).
const PT_TITLE: u8 = 0x80;
const PT_PERFORMER: u8 = 0x81;
const PT_SONGWRITER: u8 = 0x82;
const PT_COMPOSER: u8 = 0x83;
const PT_ARRANGER: u8 = 0x84;
const PT_MESSAGE: u8 = 0x85;
const PT_UPC_ISRC: u8 = 0x8E;
const PT_BLOCK_SIZE_INFO: u8 = 0x8F;

/// Parsed CD-TEXT content, one entry per language block present on the disc.
#[derive(Debug, Clone, Default)]
pub struct CdText {
    /// One entry per non-empty language block. Index 0 is the primary
    /// block (typically English or the disc's native language); subsequent
    /// entries expose alternate-language data when present.
    pub blocks: Vec<CdTextBlock>,
}

impl CdText {
    /// Convenience: primary block, the one most consumers care about.
    pub fn primary(&self) -> Option<&CdTextBlock> {
        self.blocks.first()
    }
}

/// One language's worth of CD-TEXT.
#[derive(Debug, Clone, Default)]
pub struct CdTextBlock {
    /// Block number within the disc's CD-TEXT (0–7).
    pub block: u8,
    /// Character-set code from the block-size-info pack. Common values:
    /// `0x00` = ISO-8859-1, `0x01` = ASCII, `0x80` = MS-JIS,
    /// `0x81` = KSC 5601 (Korean), `0x82` = GB 2312-80 (Simplified Chinese).
    pub charset_code: u8,
    /// Language code (EBU/Tech 3258) from the block-size-info pack.
    pub language_code: u8,
    /// Album title (pack type 0x80, track 0).
    pub title: Option<String>,
    /// Album performer / artist.
    pub performer: Option<String>,
    /// Album songwriter.
    pub songwriter: Option<String>,
    /// Album composer.
    pub composer: Option<String>,
    /// Album arranger.
    pub arranger: Option<String>,
    /// Album message / disc-level free-text annotation.
    pub message: Option<String>,
    /// UPC/EAN from pack type 0x8E at track 0.
    pub upc_ean: Option<String>,
    /// Per-track fields, indexed by 1-based track number.
    pub tracks: Vec<CdTextTrack>,
}

/// Per-track CD-TEXT fields. All optional — only the fields the disc
/// actually carries are populated.
#[derive(Debug, Clone, Default)]
pub struct CdTextTrack {
    pub track_number: u8,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub songwriter: Option<String>,
    pub composer: Option<String>,
    pub arranger: Option<String>,
    pub message: Option<String>,
    /// Per-track ISRC from pack type 0x8E when track > 0.
    pub isrc: Option<String>,
}

/// Parse a redumper `.cdtext` file from disk.
pub fn parse_cdtext(path: &Path) -> Result<CdText, AnalysisError> {
    let bytes = std::fs::read(path)?;
    parse_cdtext_bytes(&bytes)
}

/// Parse CD-TEXT from a raw byte slice.
///
/// Strips an optional 4-byte READ-TOC response header when present. An
/// empty input — or one with no valid packs — yields an empty [`CdText`]
/// with no blocks; that's not an error, just "this disc carried no CD-TEXT."
pub fn parse_cdtext_bytes(bytes: &[u8]) -> Result<CdText, AnalysisError> {
    let packs_bytes = strip_response_header(bytes);
    if packs_bytes.is_empty() {
        return Ok(CdText::default());
    }
    if !packs_bytes.len().is_multiple_of(PACK_SIZE) {
        return Err(AnalysisError::InvalidFormat(format!(
            "CD-TEXT body length {} is not a multiple of {}",
            packs_bytes.len(),
            PACK_SIZE
        )));
    }

    let packs: Vec<Pack> = packs_bytes
        .chunks_exact(PACK_SIZE)
        .map(Pack::from_bytes)
        .collect();

    // Bucket packs by (block, pack_type) and keep them in sequence order.
    let mut by_block: [[Vec<&Pack>; 16]; 8] = Default::default();
    for p in &packs {
        let block = p.block as usize;
        let ty_idx = (p.pack_type & 0x0F) as usize;
        if block < 8 && p.pack_type >= 0x80 && ty_idx < 16 {
            by_block[block][ty_idx].push(p);
        }
    }
    for block in by_block.iter_mut() {
        for bucket in block.iter_mut() {
            bucket.sort_by_key(|p| p.sequence);
        }
    }

    let mut out = CdText::default();
    for (block_idx, block_buckets) in by_block.iter().enumerate() {
        if block_buckets.iter().all(|b| b.is_empty()) {
            continue;
        }
        let block = assemble_block(block_idx as u8, block_buckets);
        out.blocks.push(block);
    }

    Ok(out)
}

fn strip_response_header(bytes: &[u8]) -> &[u8] {
    if bytes.len().is_multiple_of(PACK_SIZE) {
        return bytes;
    }
    if bytes.len() >= 4 && (bytes.len() - 4).is_multiple_of(PACK_SIZE) {
        // `READ TOC` wraps the packs in a 4-byte response header
        // (2-byte length BE + 2 reserved). Skip it.
        return &bytes[4..];
    }
    bytes
}

struct Pack {
    pack_type: u8,
    track_number: u8,
    sequence: u8,
    block: u8,
    dbcs: bool,
    data: [u8; 12],
}

impl Pack {
    fn from_bytes(b: &[u8]) -> Self {
        let flags = b[3];
        let mut data = [0u8; 12];
        data.copy_from_slice(&b[4..16]);
        Self {
            pack_type: b[0],
            track_number: b[1],
            sequence: b[2],
            block: (flags >> 4) & 0x07,
            dbcs: (flags & 0x80) != 0,
            data,
        }
    }
}

fn assemble_block(block_idx: u8, buckets: &[Vec<&Pack>; 16]) -> CdTextBlock {
    let mut block = CdTextBlock {
        block: block_idx,
        ..Default::default()
    };

    // Block-size-info packs (0x8F) carry charset + language codes, spread
    // across 3 packs. Pack 0 data layout (12 bytes):
    //   byte 0: charset code
    //   byte 1: first track
    //   byte 2: last track
    //   byte 3: copy protection flags
    //   bytes 4..: pack counts per type
    // Packs 1-2 carry language codes per block and last-sequence counters.
    let info_packs = &buckets[(PT_BLOCK_SIZE_INFO & 0x0F) as usize];
    if let Some(p0) = info_packs.iter().find(|p| p.sequence == 0) {
        block.charset_code = p0.data[0];
    }
    if let Some(p2) = info_packs.iter().find(|p| p.sequence == 2) {
        // Language codes for blocks 0..=7 at bytes 4..=11 of the third
        // info pack. Pick our own block.
        let idx = 4 + block_idx as usize;
        if idx < p2.data.len() {
            block.language_code = p2.data[idx];
        }
    }

    let decode = |ty: u8| -> Vec<String> {
        let bucket = &buckets[(ty & 0x0F) as usize];
        if bucket.is_empty() {
            return Vec::new();
        }
        let mut raw: Vec<u8> = Vec::with_capacity(bucket.len() * 12);
        for p in bucket {
            raw.extend_from_slice(&p.data);
        }
        split_strings(&raw, block.charset_code, bucket[0].dbcs)
    };

    let assign = |slot: &mut Option<String>,
                  tracks: &mut Vec<CdTextTrack>,
                  strings: Vec<String>,
                  setter: fn(&mut CdTextTrack, String)| {
        for (i, s) in strings.into_iter().enumerate() {
            if s.is_empty() {
                continue;
            }
            if i == 0 {
                *slot = Some(s);
            } else {
                let tn = i as u8;
                let t = ensure_track(tracks, tn);
                setter(t, s);
            }
        }
    };

    assign(
        &mut block.title,
        &mut block.tracks,
        decode(PT_TITLE),
        |t, s| t.title = Some(s),
    );
    assign(
        &mut block.performer,
        &mut block.tracks,
        decode(PT_PERFORMER),
        |t, s| t.performer = Some(s),
    );
    assign(
        &mut block.songwriter,
        &mut block.tracks,
        decode(PT_SONGWRITER),
        |t, s| t.songwriter = Some(s),
    );
    assign(
        &mut block.composer,
        &mut block.tracks,
        decode(PT_COMPOSER),
        |t, s| t.composer = Some(s),
    );
    assign(
        &mut block.arranger,
        &mut block.tracks,
        decode(PT_ARRANGER),
        |t, s| t.arranger = Some(s),
    );
    assign(
        &mut block.message,
        &mut block.tracks,
        decode(PT_MESSAGE),
        |t, s| t.message = Some(s),
    );

    // Pack 0x8E carries UPC at track 0 and ISRC at track > 0. Each value
    // may span multiple packs (UPC/EAN is 13 chars but each pack holds
    // only 12 bytes of payload). Group by track_number, concatenate
    // sequence-ordered data, then trim.
    use std::collections::BTreeMap;
    let mut by_track: BTreeMap<u8, Vec<&Pack>> = BTreeMap::new();
    for p in &buckets[(PT_UPC_ISRC & 0x0F) as usize] {
        by_track.entry(p.track_number).or_default().push(p);
    }
    for (tn, mut packs_for_track) in by_track {
        packs_for_track.sort_by_key(|p| p.sequence);
        let dbcs = packs_for_track.first().is_some_and(|p| p.dbcs);
        let mut raw = Vec::with_capacity(packs_for_track.len() * 12);
        for p in &packs_for_track {
            raw.extend_from_slice(&p.data);
        }
        let value = decode_string(&raw, block.charset_code, dbcs);
        let value = value.trim_end_matches('\0').trim().to_string();
        if value.is_empty() {
            continue;
        }
        if tn == 0 {
            block.upc_ean = Some(value);
        } else {
            let t = ensure_track(&mut block.tracks, tn);
            t.isrc = Some(value);
        }
    }

    block.tracks.sort_by_key(|t| t.track_number);
    block
}

fn ensure_track(tracks: &mut Vec<CdTextTrack>, n: u8) -> &mut CdTextTrack {
    if let Some(idx) = tracks.iter().position(|t| t.track_number == n) {
        &mut tracks[idx]
    } else {
        tracks.push(CdTextTrack {
            track_number: n,
            ..Default::default()
        });
        tracks.last_mut().unwrap()
    }
}

fn split_strings(raw: &[u8], charset: u8, dbcs: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<u8> = Vec::new();

    if dbcs {
        // Double-byte: null terminator is two 0x00 bytes on an even boundary.
        let mut i = 0;
        while i + 1 < raw.len() {
            if raw[i] == 0 && raw[i + 1] == 0 {
                out.push(decode_string(&current, charset, true));
                current.clear();
            } else {
                current.push(raw[i]);
                current.push(raw[i + 1]);
            }
            i += 2;
        }
    } else {
        for &b in raw {
            if b == 0 {
                out.push(decode_string(&current, charset, false));
                current.clear();
            } else {
                current.push(b);
            }
        }
    }

    // Drop a trailing empty string from the final null padding.
    while out.last().is_some_and(|s| s.is_empty()) {
        out.pop();
    }
    out
}

fn decode_string(bytes: &[u8], charset: u8, dbcs: bool) -> String {
    // Trim trailing NULs that may survive the split on short final strings.
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..end];
    if bytes.is_empty() {
        return String::new();
    }

    match (charset, dbcs) {
        // 0x00 = ISO 8859-1, 0x01 = ASCII. Both round-trip through
        // Latin-1 (ASCII is a strict subset).
        (0x00, false) | (0x01, false) => bytes.iter().map(|&b| b as char).collect(),
        _ => {
            // Unknown / multibyte: best-effort UTF-8, then fall back to
            // lossy Latin-1 so we never panic on non-UTF-8 CJK data.
            // Proper MS-JIS / Shift-JIS / KSC 5601 / GB 2312 decoding is
            // deferred until a real fixture surfaces demand.
            match std::str::from_utf8(bytes) {
                Ok(s) => s.to_string(),
                Err(_) => bytes.iter().map(|&b| b as char).collect(),
            }
        }
    }
}
