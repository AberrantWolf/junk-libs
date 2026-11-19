//! Pure Ogg page (RFC 3533) and Ogg Opus header (RFC 7845) parsing shared by
//! derivative validation and the Opus decode path.

#[cfg(feature = "decode-opus")]
use std::io::{self, Read, Seek, SeekFrom};

pub(crate) const PAGE_HEADER_LEN: usize = 27;
#[cfg(feature = "decode-opus")]
const MAX_PAGE_LEN: usize = PAGE_HEADER_LEN + 255 + 255 * 255;

/// Fixed 27-byte Ogg page header with the capture pattern and version checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageHeader {
    pub flags: u8,
    pub granule: u64,
    pub serial: u32,
    pub sequence: u32,
    pub crc: u32,
    pub segments: u8,
}

// Continuity flags are checked only by derivative validation.
#[cfg_attr(not(feature = "derivative-validation"), allow(dead_code))]
impl PageHeader {
    pub const CONTINUED: u8 = 1;
    pub const BOS: u8 = 2;
    pub const EOS: u8 = 4;
    /// Granule value meaning no packet completes on the page.
    pub const NO_GRANULE: u64 = u64::MAX;

    pub fn parse(bytes: &[u8; PAGE_HEADER_LEN]) -> Option<Self> {
        if &bytes[..4] != b"OggS" || bytes[4] != 0 {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Some(Self {
            flags: bytes[5],
            granule: u64::from_le_bytes(bytes[6..14].try_into().unwrap()),
            serial: word(14),
            sequence: word(18),
            crc: word(22),
            segments: bytes[26],
        })
    }

    pub fn is_bos(&self) -> bool {
        self.flags & Self::BOS != 0
    }

    pub fn is_eos(&self) -> bool {
        self.flags & Self::EOS != 0
    }
}

/// Ogg CRC-32 (polynomial 0x04c11db7, no reflection, zero init) of a whole
/// page with its checksum field treated as zero.
pub(crate) fn page_crc(header: &[u8; PAGE_HEADER_LEN], lacing: &[u8], body: &[u8]) -> u32 {
    let mut crc = 0_u32;
    let zeroed = header[..22]
        .iter()
        .chain(&[0; 4])
        .chain(&header[26..])
        .chain(lacing)
        .chain(body);
    for &byte in zeroed {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Identification header of an Ogg Opus logical stream (RFC 7845 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpusHead {
    pub version: u8,
    pub channels: u8,
    pub pre_skip: u16,
    /// Output gain in Q7.8 dB.
    pub output_gain: i16,
    pub mapping_family: u8,
}

impl OpusHead {
    pub const MAGIC: &'static [u8; 8] = b"OpusHead";
    pub const LEN: usize = 19;

    /// Parse structurally; channel mapping support is the caller's decision.
    /// Family 0 must be mono or stereo. Minor versions may append fields, so
    /// trailing bytes are ignored.
    pub fn parse(packet: &[u8]) -> Option<Self> {
        if packet.len() < Self::LEN || !packet.starts_with(Self::MAGIC) {
            return None;
        }
        let head = Self {
            version: packet[8],
            channels: packet[9],
            pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
            output_gain: i16::from_le_bytes([packet[16], packet[17]]),
            mapping_family: packet[18],
        };
        // Major version 0 is the only compatible one (RFC 7845 §5.1).
        let compatible = head.version >> 4 == 0 && head.channels != 0;
        let family_zero_ok = head.mapping_family != 0 || matches!(head.channels, 1 | 2);
        (compatible && family_zero_ok).then_some(head)
    }

    /// Linear factor for the header output gain, which RFC 7845 requires
    /// players to apply.
    #[cfg(feature = "decode-opus")]
    pub fn linear_gain(&self) -> f32 {
        10_f32.powf(f32::from(self.output_gain) / (20.0 * 256.0))
    }
}

/// Duration of an Opus packet in 48 kHz frames from its TOC (RFC 6716 §3.1),
/// or `None` when the packet is too short to declare one.
#[cfg(feature = "derivative-validation")]
pub(crate) fn opus_packet_frames(packet: &[u8]) -> Option<u64> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let per_frame = if config < 12 {
        [480_u64, 960, 1_920, 2_880][usize::from(config % 4)]
    } else if config < 16 {
        [480_u64, 960][usize::from(config % 2)]
    } else {
        [120_u64, 240, 480, 960][usize::from(config % 4)]
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(*packet.get(1)? & 0x3f),
    };
    Some(per_frame * count)
}

/// Bounds of a single-link Ogg Opus physical stream, read without walking its pages.
#[cfg(feature = "decode-opus")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OggOpusLayout {
    pub serial: u32,
    pub head: OpusHead,
    /// The page that ends exactly at the end of the stream.
    pub last_page: PageHeader,
}

#[cfg(feature = "decode-opus")]
impl OggOpusLayout {
    /// Return `Ok(None)` when the stream does not begin with an Ogg Opus BOS
    /// page. The reader position is unspecified afterwards.
    pub fn scan(reader: &mut (impl Read + Seek + ?Sized), end: u64) -> io::Result<Option<Self>> {
        let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message);
        reader.seek(SeekFrom::Start(0))?;
        let Some(first) = Page::read(reader)? else {
            return Ok(None);
        };
        let Some(packet) = first
            .first_packet()
            .filter(|packet| first.header.is_bos() && packet.starts_with(OpusHead::MAGIC))
        else {
            return Ok(None);
        };
        // Other Ogg codecs are left to Symphonia; only Opus is validated here.
        if !first.checksum_ok() {
            return Err(invalid("Ogg checksum mismatch"));
        }
        let head = OpusHead::parse(packet).ok_or_else(|| invalid("invalid OpusHead"))?;
        let last_page = last_page(reader, end)?
            .ok_or_else(|| invalid("Ogg stream does not end on a complete page"))?;
        Ok(Some(Self {
            serial: first.header.serial,
            head,
            last_page,
        }))
    }
}

#[cfg(feature = "decode-opus")]
struct Page {
    raw: [u8; PAGE_HEADER_LEN],
    header: PageHeader,
    lacing: Vec<u8>,
    body: Vec<u8>,
}

#[cfg(feature = "decode-opus")]
impl Page {
    /// Read one page at the reader position. `Ok(None)` when the bytes are
    /// too short or not an Ogg page; a truncated page body is an error.
    fn read(reader: &mut (impl Read + ?Sized)) -> io::Result<Option<Self>> {
        let mut raw = [0; PAGE_HEADER_LEN];
        let mut filled = 0;
        while filled < raw.len() {
            match reader.read(&mut raw[filled..])? {
                0 => return Ok(None),
                count => filled += count,
            }
        }
        let Some(header) = PageHeader::parse(&raw) else {
            return Ok(None);
        };
        let mut lacing = vec![0; usize::from(header.segments)];
        reader.read_exact(&mut lacing)?;
        let mut body = vec![0; lacing.iter().map(|&v| usize::from(v)).sum()];
        reader.read_exact(&mut body)?;
        Ok(Some(Self {
            raw,
            header,
            lacing,
            body,
        }))
    }

    fn checksum_ok(&self) -> bool {
        page_crc(&self.raw, &self.lacing, &self.body) == self.header.crc
    }

    /// The first packet when it completes on this page.
    fn first_packet(&self) -> Option<&[u8]> {
        let segments = self.lacing.iter().position(|&v| v < 255)? + 1;
        let length = self.lacing[..segments]
            .iter()
            .map(|&v| usize::from(v))
            .sum();
        self.body.get(..length)
    }
}

/// Find the checksummed page whose last byte is the final byte before `end`,
/// reading at most one maximum-size page.
#[cfg(feature = "decode-opus")]
fn last_page(reader: &mut (impl Read + Seek + ?Sized), end: u64) -> io::Result<Option<PageHeader>> {
    let window = end.min(MAX_PAGE_LEN as u64);
    reader.seek(SeekFrom::Start(end - window))?;
    let mut tail = vec![0; window as usize];
    reader.read_exact(&mut tail)?;
    for start in (0..tail.len().saturating_sub(PAGE_HEADER_LEN - 1)).rev() {
        let raw: &[u8; PAGE_HEADER_LEN] = tail[start..start + PAGE_HEADER_LEN].try_into().unwrap();
        let Some(header) = PageHeader::parse(raw) else {
            continue;
        };
        let lacing_end = start + PAGE_HEADER_LEN + usize::from(header.segments);
        let Some(lacing) = tail.get(start + PAGE_HEADER_LEN..lacing_end) else {
            continue;
        };
        let body_len: usize = lacing.iter().map(|&v| usize::from(v)).sum();
        if lacing_end + body_len == tail.len()
            && page_crc(raw, lacing, &tail[lacing_end..]) == header.crc
        {
            return Ok(Some(header));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_head_requires_compatible_version_and_family_zero_layout() {
        let mut head = *b"OpusHead\x01\x02\x38\x01\x44\xac\x00\x00\x00\xfa\x00";
        let parsed = OpusHead::parse(&head).unwrap();
        assert_eq!(
            (parsed.channels, parsed.pre_skip, parsed.output_gain),
            (2, 312, -1536)
        );
        head[9] = 3;
        assert!(
            OpusHead::parse(&head).is_none(),
            "family 0 is mono or stereo"
        );
        head[9] = 2;
        head[8] = 0x10;
        assert!(
            OpusHead::parse(&head).is_none(),
            "major version 1 is incompatible"
        );
        head[8] = 1;
        head[18] = 1;
        assert_eq!(OpusHead::parse(&head).unwrap().mapping_family, 1);
        assert!(OpusHead::parse(&head[..18]).is_none());
    }

    #[test]
    fn page_header_rejects_other_capture_patterns_and_versions() {
        let mut raw = [0; PAGE_HEADER_LEN];
        raw[..4].copy_from_slice(b"OggS");
        raw[5] = PageHeader::BOS | PageHeader::EOS;
        raw[6..14].copy_from_slice(&952_u64.to_le_bytes());
        let header = PageHeader::parse(&raw).unwrap();
        assert!(header.is_bos() && header.is_eos());
        assert_eq!(header.granule, 952);
        raw[4] = 1;
        assert!(PageHeader::parse(&raw).is_none());
        raw[4] = 0;
        raw[0] = b'X';
        assert!(PageHeader::parse(&raw).is_none());
    }
}
