//! Bounded APEv2 suffix parsing. Never changes the encoded source.
use crate::{AudioDecodeError, AudioReadSeek, FileAudioInfo};
use std::io::SeekFrom;

fn invalid() -> AudioDecodeError {
    AudioDecodeError::Decoder("malformed or oversized APEv2 metadata".into())
}
fn io_error(_: std::io::Error) -> AudioDecodeError {
    invalid()
}
fn word(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}

/// `end` excludes any final ID3v1 tag. Return the audio end and parsed metadata.
pub(crate) fn suffix(
    reader: &mut dyn AudioReadSeek,
    end: u64,
    checkpoint: &mut dyn FnMut() -> Result<(), AudioDecodeError>,
) -> Result<(u64, FileAudioInfo), AudioDecodeError> {
    let mut info = FileAudioInfo::default();
    if end < 32 {
        return Ok((end, info));
    }
    checkpoint()?;
    reader.seek(SeekFrom::Start(end - 32)).map_err(io_error)?;
    let mut footer = [0; 32];
    reader.read_exact(&mut footer).map_err(io_error)?;
    if &footer[..8] != b"APETAGEX" {
        return Ok((end, info));
    }
    let size = u64::from(word(&footer, 12));
    let count = word(&footer, 16);
    let flags = word(&footer, 20);
    if word(&footer, 8) != 2000
        || !(32..=32 * 1024 * 1024).contains(&size)
        || size > end
        || count > 256
        || flags & !0x8000_0001 != 0
        || footer[24..] != [0; 8]
    {
        return Err(invalid());
    }
    let mut start = end - size;
    if flags & 0x8000_0000 != 0 {
        start = start.checked_sub(32).ok_or_else(invalid)?;
        reader.seek(SeekFrom::Start(start)).map_err(io_error)?;
        let mut header = [0; 32];
        reader.read_exact(&mut header).map_err(io_error)?;
        let mut expected = footer;
        expected[20..24].copy_from_slice(&(flags | 0x2000_0000).to_le_bytes());
        if header != expected {
            return Err(invalid());
        }
    }
    let mut position = end - size;
    let items_end = end - 32;
    for _ in 0..count {
        checkpoint()?;
        if items_end - position < 8 {
            return Err(invalid());
        }
        reader.seek(SeekFrom::Start(position)).map_err(io_error)?;
        let mut item = [0; 8];
        reader.read_exact(&mut item).map_err(io_error)?;
        let length = u64::from(word(&item, 0));
        let flags = word(&item, 4);
        if flags & !7 != 0 || flags >> 1 == 3 {
            return Err(invalid());
        }
        position += 8;
        let mut key = Vec::new();
        loop {
            if position >= items_end || key.len() > 255 {
                return Err(invalid());
            }
            let mut b = [0];
            reader.read_exact(&mut b).map_err(io_error)?;
            position += 1;
            if b[0] == 0 {
                break;
            }
            if !(32..=126).contains(&b[0]) || b[0] == b'=' {
                return Err(invalid());
            }
            key.push(b[0]);
        }
        if key.len() < 2 || key.len() > 255 || length > items_end - position {
            return Err(invalid());
        }
        let key = String::from_utf8(key)
            .map_err(|_| invalid())?
            .to_ascii_lowercase();
        let mapped = match key.as_str() {
            "title" => Some("title"),
            "artist" => Some("artist"),
            "album" => Some("album"),
            "album artist" | "albumartist" => Some("album_artist"),
            "track" | "tracknumber" => Some("track_number"),
            "disc" | "discnumber" => Some("disc_number"),
            "year" | "date" => Some("date"),
            "musicbrainz_albumid" => Some("musicbrainz_release_id"),
            "musicbrainz_trackid" => Some("musicbrainz_recording_id"),
            _ => None,
        };
        let picture = key == "cover art (front)" && flags >> 1 == 1;
        if (mapped.is_some() && flags >> 1 == 0) || picture {
            let limit = if picture { 20 * 1024 * 1024 } else { 32 * 1024 };
            if length > limit {
                return Err(invalid());
            }
            let mut bytes = vec![0; length as usize];
            for chunk in bytes.chunks_mut(65536) {
                checkpoint()?;
                reader.read_exact(chunk).map_err(io_error)?;
            }
            if picture {
                let split = bytes.iter().position(|b| *b == 0).ok_or_else(invalid)?;
                let image = &bytes[split + 1..];
                let mime = if image.starts_with(&[0xff, 0xd8, 0xff]) {
                    Some("image/jpeg")
                } else if image.starts_with(b"\x89PNG\r\n\x1a\n") {
                    Some("image/png")
                } else {
                    None
                };
                if let Some(mime) = mime {
                    info.pictures.push((mime.into(), image.to_vec()));
                }
            } else {
                let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
                let values = info.tags.entry(mapped.unwrap().into()).or_default();
                for value in text.split('\0') {
                    if value.len() > 4096
                        || value.contains('\u{fffd}')
                        || value.chars().any(char::is_control)
                    {
                        return Err(invalid());
                    }
                    if !values.iter().any(|v| v == value) {
                        values.push(value.into());
                    }
                }
            }
        }
        position += length;
    }
    if position != items_end {
        return Err(invalid());
    }
    checkpoint()?;
    Ok((start, info))
}
