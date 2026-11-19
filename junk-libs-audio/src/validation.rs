//! Bounded integer FLAC comparison and strict generated Ogg Opus inspection.
use std::io::{Error, ErrorKind, Read, Result};

use crate::ogg::{OpusHead, PAGE_HEADER_LEN, PageHeader, page_crc};
const IO_FRAMES: usize = 16384;
const MAX_OPUS_PACKET_BYTES: usize = 1024 * 1024;
fn invalid(message: String) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}
pub fn validate_flac(
    tracks: &[(u64, u64)],
    source: &mut (dyn junk_libs_disc::CdPcmReader + Send),
    scratch: &mut dyn Read,
    expected_frames: u64,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let mut decoder = claxon::FlacReader::new(&mut *scratch)
        .map_err(|error| invalid(format!("independent FLAC decode failed: {error}")))?;
    let info = decoder.streaminfo();
    if info.sample_rate != 44_100 || info.channels != 2 || info.bits_per_sample != 16 {
        return Err(invalid(
            "FLAC decoded format does not match CD-DA PCM".into(),
        ));
    }
    if info.samples.is_some_and(|frames| frames != expected_frames) {
        return Err(invalid("FLAC decoded frame length mismatch".into()));
    }
    let mut samples = decoder.samples();
    let mut frames = vec![[0_i16; 2]; IO_FRAMES];
    let mut compared = 0_u64;
    for &(start, length) in tracks {
        source
            .seek_frame(start)
            .map_err(|e| invalid(e.to_string()))?;
        let mut remaining = length;
        while remaining != 0 {
            checkpoint()?;
            let wanted = usize::try_from(remaining.min(IO_FRAMES as u64)).unwrap();
            let count = source
                .read_frames(&mut frames[..wanted])
                .map_err(|e| invalid(e.to_string()))?;
            if count > wanted {
                return Err(invalid("PCM reader exceeded requested frames".into()));
            }
            if count == 0 {
                return Err(invalid("source PCM ended during FLAC validation".into()));
            }
            for frame in &frames[..count] {
                for expected in frame {
                    let actual = samples
                        .next()
                        .ok_or_else(|| invalid("FLAC decoded PCM is truncated".into()))?
                        .map_err(|error| invalid(format!("FLAC decode failed: {error}")))?;
                    if actual != i32::from(*expected) {
                        return Err(invalid("FLAC decoded PCM differs from its source".into()));
                    }
                }
            }
            compared += count as u64;
            remaining -= count as u64;
        }
    }
    if compared != expected_frames || samples.next().is_some() {
        return Err(invalid("FLAC decoded frame length mismatch".into()));
    }
    Ok(())
}

pub struct OpusInspection {
    pub decoded_frames: u64,
    pub pre_skip: u32,
    pub end_padding: u32,
}

pub fn inspect_opus(
    scratch: &mut dyn Read,
    source_frames: u64,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<OpusInspection> {
    let mut packet = Vec::new();
    let mut packet_index = 0_u64;
    let mut audio_frames = 0_u64;
    let mut pre_skip = None;
    let mut final_granule = None;
    let mut serial = None;
    let mut sequence = 0_u32;
    let mut ended = false;
    loop {
        checkpoint()?;
        let mut raw = [0_u8; PAGE_HEADER_LEN];
        if scratch.read(&mut raw[..1])? == 0 {
            break;
        }
        scratch.read_exact(&mut raw[1..])?;
        if ended {
            return Err(invalid("data after Ogg EOS".into()));
        }
        let header =
            PageHeader::parse(&raw).ok_or_else(|| invalid("invalid Ogg Opus page".into()))?;
        let known_flags = PageHeader::CONTINUED | PageHeader::BOS | PageHeader::EOS;
        if header.flags & !known_flags != 0
            || header.sequence != sequence
            || serial.is_some_and(|s| s != header.serial)
            || header.is_bos() != (sequence == 0)
            || (header.flags & PageHeader::CONTINUED != 0) == packet.is_empty()
        {
            return Err(invalid("invalid Ogg stream continuity".into()));
        }
        serial = Some(header.serial);
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| invalid("Ogg sequence overflow".into()))?;
        ended = header.is_eos();
        let granule = header.granule;
        if granule != PageHeader::NO_GRANULE {
            if final_granule.is_some_and(|previous| granule < previous) {
                return Err(invalid("Ogg granule moved backwards".into()));
            }
            final_granule = Some(granule);
        }
        let mut lacing = vec![0_u8; usize::from(header.segments)];
        scratch.read_exact(&mut lacing)?;
        let mut body = vec![0; lacing.iter().map(|v| usize::from(*v)).sum()];
        scratch.read_exact(&mut body)?;
        if page_crc(&raw, &lacing, &body) != header.crc {
            return Err(invalid("Ogg checksum mismatch".into()));
        }
        let completed_before = packet_index;
        let mut body = &body[..];
        for length in lacing {
            let old = packet.len();
            let new = old
                .checked_add(usize::from(length))
                .filter(|bytes| *bytes <= MAX_OPUS_PACKET_BYTES)
                .ok_or_else(|| invalid("Ogg Opus packet exceeded its bound".into()))?;
            packet.resize(new, 0);
            body.read_exact(&mut packet[old..])?;
            if length < 255 {
                match packet_index {
                    0 => {
                        let head = OpusHead::parse(&packet)
                            .filter(|head| {
                                packet.len() == OpusHead::LEN
                                    && head.version == 1
                                    && head.channels == 2
                                    && head.mapping_family == 0
                                    && head.output_gain == 0
                            })
                            .filter(|_| sequence == 1 && header.segments == 1 && granule == 0)
                            .ok_or_else(|| invalid("invalid stereo OpusHead".into()))?;
                        pre_skip = Some(u32::from(head.pre_skip));
                    }
                    1 => {
                        if !valid_tags(&packet) {
                            return Err(invalid("missing OpusTags packet".into()));
                        }
                    }
                    _ => {
                        audio_frames = audio_frames
                            .checked_add(opus_packet_frames(&packet)?)
                            .ok_or_else(|| invalid("Opus duration overflow".into()))?
                    }
                }
                packet.clear();
                packet_index += 1;
            }
        }
        if ended && (granule == u64::MAX || !packet.is_empty() || packet_index < 3) {
            return Err(invalid("invalid Ogg EOS boundary".into()));
        }
        if packet_index > completed_before
            && packet_index > 2
            && ((!ended && granule != audio_frames) || (ended && granule > audio_frames))
        {
            return Err(invalid("Ogg granule disagrees with coded duration".into()));
        }
    }
    if !ended || !packet.is_empty() || packet_index < 3 {
        return Err(invalid("truncated Ogg Opus packet stream".into()));
    }
    let pre_skip = pre_skip.ok_or_else(|| invalid("missing Opus pre-skip".into()))?;
    let granule = final_granule.ok_or_else(|| invalid("missing Opus granule position".into()))?;
    let decoded_frames = granule
        .checked_sub(u64::from(pre_skip))
        .ok_or_else(|| invalid("Opus granule precedes pre-skip".into()))?;
    let expected_frames = source_frames
        .checked_mul(160)
        .map(|frames| frames.div_ceil(147))
        .ok_or_else(|| invalid("Opus duration conversion overflow".into()))?;
    if decoded_frames != expected_frames {
        return Err(invalid(
            "Opus decoded duration does not match the recipe".into(),
        ));
    }
    let end_padding = audio_frames
        .checked_sub(granule)
        .and_then(|frames| u32::try_from(frames).ok())
        .filter(|frames| *frames < 960)
        .ok_or_else(|| invalid("Opus end padding is outside one 20-ms frame".into()))?;
    checkpoint()?;
    Ok(OpusInspection {
        decoded_frames,
        pre_skip,
        end_padding,
    })
}

/// The generated derivative profile uses only 20-ms packets.
fn opus_packet_frames(packet: &[u8]) -> Result<u64> {
    if packet.len() < 2 {
        return Err(invalid("empty Opus coded frame".into()));
    }
    let frames = crate::ogg::opus_packet_frames(packet)
        .ok_or_else(|| invalid("short Opus packet".into()))?;
    if frames != 960 {
        return Err(invalid(
            "Opus packet does not use the fixed 20-ms profile".into(),
        ));
    }
    Ok(frames)
}

fn valid_tags(packet: &[u8]) -> bool {
    if !packet.starts_with(b"OpusTags") || packet.len() < 16 {
        return false;
    }
    let vendor = u32::from_le_bytes(packet[8..12].try_into().unwrap()) as usize;
    let Some(mut cursor) = 12_usize.checked_add(vendor) else {
        return false;
    };
    let Some(count_bytes) = packet.get(cursor..cursor.saturating_add(4)) else {
        return false;
    };
    let count = u32::from_le_bytes(count_bytes.try_into().unwrap());
    cursor += 4;
    for _ in 0..count {
        let Some(length) = packet.get(cursor..cursor.saturating_add(4)) else {
            return false;
        };
        let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
        let Some(end) = cursor.checked_add(4).and_then(|v| v.checked_add(length)) else {
            return false;
        };
        if end > packet.len() {
            return false;
        }
        cursor = end;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    const OPUS: &[u8] = include_bytes!("../tests/fixtures/silence-588.opus");
    #[test]
    fn independent_encoder_preskip_is_not_counted_twice() {
        let result = inspect_opus(&mut &OPUS[..], 588, &mut || Ok(())).unwrap();
        assert_eq!(
            (result.decoded_frames, result.pre_skip, result.end_padding),
            (640, 312, 8)
        );
    }
    #[test]
    fn truncated_corrupted_and_cancelled_streams_fail() {
        for length in [0, 1, OPUS.len() - 1] {
            assert!(inspect_opus(&mut &OPUS[..length], 588, &mut || Ok(())).is_err());
        }
        let mut corrupt = OPUS.to_vec();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(inspect_opus(&mut &corrupt[..], 588, &mut || Ok(())).is_err());
        let mut trailing = OPUS.to_vec();
        trailing.extend_from_slice(b"Ogg");
        assert!(inspect_opus(&mut &trailing[..], 588, &mut || Ok(())).is_err());
        assert!(inspect_opus(&mut &OPUS[..], 588, &mut || Err(Error::other("cancelled"))).is_err());
    }
}
