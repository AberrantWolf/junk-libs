//! Bounded file-audio inspection. Consumers own package identity and publication.
use crate::{AudioDecodeError, AudioDecoder, ByteSource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use symphonia::core::meta::{MetadataRevision, StandardTagKey};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: Option<u32>,
    pub decoded_frames: Option<u64>,
    pub delay_frames: Option<u32>,
    pub padding_frames: Option<u32>,
    pub tags: BTreeMap<String, Vec<String>>,
    // Encoded artwork is consumed by a caller-owned Asset pipeline, not a PCM API.
    #[serde(skip)]
    pub pictures: Vec<(String, Vec<u8>)>,
}
impl FileAudioInfo {
    pub(crate) fn read_metadata(
        &mut self,
        metadata: &MetadataRevision,
    ) -> Result<(), AudioDecodeError> {
        if metadata.tags().len() > 256 || metadata.visuals().len() > 16 {
            return Err(AudioDecodeError::Decoder(
                "metadata entry limit exceeded".into(),
            ));
        }
        for tag in metadata.tags() {
            let key = match tag.std_key {
                Some(StandardTagKey::TrackTitle) => "title",
                Some(StandardTagKey::Artist) => "artist",
                Some(StandardTagKey::Album) => "album",
                Some(StandardTagKey::AlbumArtist) => "album_artist",
                Some(StandardTagKey::DiscNumber) => "disc_number",
                Some(StandardTagKey::TrackNumber) => "track_number",
                Some(StandardTagKey::Date | StandardTagKey::ReleaseDate) => "date",
                Some(StandardTagKey::MusicBrainzAlbumId) => "musicbrainz_release_id",
                Some(StandardTagKey::MusicBrainzRecordingId) => "musicbrainz_recording_id",
                _ => continue,
            };
            let value = tag.value.to_string();
            if value.len() > 4096
                || value.contains('\u{fffd}')
                || value.chars().any(char::is_control)
            {
                return Err(AudioDecodeError::Decoder(
                    "invalid or oversized audio tag".into(),
                ));
            }
            let values = self.tags.entry(key.into()).or_default();
            if !values.contains(&value) {
                values.push(value);
            }
        }
        for picture in metadata.visuals() {
            if picture.data.len() > 20 * 1024 * 1024 || self.pictures.len() >= 16 {
                return Err(AudioDecodeError::Decoder(
                    "embedded picture limit exceeded".into(),
                ));
            }
            self.pictures
                .push((picture.media_type.clone(), picture.data.to_vec()));
        }
        Ok(())
    }
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .get(key)
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}
/// Decode every frame with cooperative cancellation. The byte reader must also
/// bound/cancel I/O during probing and individual decoder reads.
pub fn inspect_file_audio(
    mut source: ByteSource,
    checkpoint: &mut dyn FnMut() -> Result<(), AudioDecodeError>,
) -> Result<FileAudioInfo, AudioDecodeError> {
    checkpoint()?;
    validate_structure(source.reader_mut(), checkpoint)?;
    #[cfg(feature = "decode-compressed")]
    let footer = read_id3_footer(source.reader_mut())?;
    let mut decoder = AudioDecoder::open(source)?;
    if !matches!(decoder.inspection.codec.as_str(), "flac" | "mp3") {
        return Err(AudioDecodeError::UnsupportedSource(
            decoder.inspection.codec,
        ));
    }
    if !matches!(decoder.format().channels(), 1 | 2) {
        return Err(AudioDecodeError::TooManyChannels);
    }
    let mut frames = 0u64;
    while let Some(chunk) = {
        checkpoint()?;
        decoder.read_frames(4096)?
    } {
        frames = frames
            .checked_add(chunk.frame_count())
            .ok_or(AudioDecodeError::ReadSizeOverflow)?;
        // Bound CPU even for tiny highly compressed inputs: at most 24h per track.
        if frames > u64::from(decoder.format().sample_rate()) * 86_400 {
            return Err(AudioDecodeError::Decoder(
                "audio duration exceeds 24 hour limit".into(),
            ));
        }
    }
    checkpoint()?;
    if frames == 0 {
        return Err(AudioDecodeError::Decoder("empty audio stream".into()));
    }
    #[cfg(feature = "decode-compressed")]
    for (key, values) in footer.tags {
        decoder.inspection.tags.entry(key).or_insert(values);
    }
    decoder.inspection.decoded_frames = Some(frames);
    Ok(decoder.inspection)
}

/// Check lengths before the codec/metadata library can allocate from untrusted
/// headers. MPEG traversal validates every frame, including headerless-duration
/// streams, so EOF cannot conceal an incomplete final frame.
fn validate_structure(
    reader: &mut dyn crate::AudioReadSeek,
    checkpoint: &mut dyn FnMut() -> Result<(), AudioDecodeError>,
) -> Result<(), AudioDecodeError> {
    use std::io::SeekFrom;
    let invalid = || AudioDecodeError::Decoder("malformed or truncated audio structure".into());
    let end = reader
        .seek(SeekFrom::End(0))
        .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?;
    let mut signature = [0; 10];
    reader.read_exact(&mut signature).map_err(|_| invalid())?;
    if &signature[..4] == b"fLaC" {
        let mut position = 4u64;
        let mut first = true;
        let mut metadata_bytes = 0u64;
        loop {
            checkpoint()?;
            reader
                .seek(SeekFrom::Start(position))
                .map_err(|_| invalid())?;
            let mut header = [0; 4];
            reader.read_exact(&mut header).map_err(|_| invalid())?;
            let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as u64;
            if first && (header[0] & 0x7f != 0 || length != 34) {
                return Err(invalid());
            }
            first = false;
            metadata_bytes = metadata_bytes.checked_add(length).ok_or_else(invalid)?;
            if metadata_bytes > 32 * 1024 * 1024 {
                return Err(AudioDecodeError::Decoder(
                    "FLAC metadata exceeds 32 MiB".into(),
                ));
            }
            position = position.checked_add(4 + length).ok_or_else(invalid)?;
            if position >= end {
                return Err(invalid());
            }
            if header[0] & 0x80 != 0 {
                break;
            }
        }
    } else {
        let mut position = 0u64;
        if &signature[..3] == b"ID3" {
            if !matches!(signature[3], 2..=4) || signature[6..10].iter().any(|b| b & 0x80 != 0) {
                return Err(invalid());
            }
            let length = signature[6..10]
                .iter()
                .fold(0u64, |n, b| (n << 7) | u64::from(*b));
            if length > 32 * 1024 * 1024 {
                return Err(AudioDecodeError::Decoder(
                    "ID3 metadata exceeds 32 MiB".into(),
                ));
            }
            position = 10
                + length
                + if signature[3] == 4 && signature[5] & 0x10 != 0 {
                    10
                } else {
                    0
                };
        }
        let mut frames = 0u64;
        while position < end {
            checkpoint()?;
            reader
                .seek(SeekFrom::Start(position))
                .map_err(|_| invalid())?;
            let mut h = [0; 4];
            reader.read_exact(&mut h).map_err(|_| invalid())?;
            if end - position == 128 && &h[..3] == b"TAG" {
                break;
            }
            let version = (h[1] >> 3) & 3;
            let rate_index = (h[2] >> 2) & 3;
            let bit_index = h[2] >> 4;
            if h[0] != 0xff
                || h[1] & 0xe0 != 0xe0
                || version == 1
                || (h[1] >> 1) & 3 != 1
                || rate_index == 3
                || bit_index == 0
                || bit_index == 15
            {
                return Err(invalid());
            }
            let rates = [44100u64, 48000, 32000];
            let rate = rates[rate_index as usize]
                / match version {
                    3 => 1,
                    2 => 2,
                    _ => 4,
                };
            let bitrates1 = [
                0u64, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ];
            let bitrates2 = [
                0u64, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160,
            ];
            let bitrate = if version == 3 {
                bitrates1[bit_index as usize]
            } else {
                bitrates2[bit_index as usize]
            };
            let length = (if version == 3 { 144000 } else { 72000 }) * bitrate / rate
                + u64::from((h[2] >> 1) & 1);
            position = position.checked_add(length).ok_or_else(invalid)?;
            if position > end {
                return Err(invalid());
            }
            frames += 1;
            if frames > 20_000_000 {
                return Err(AudioDecodeError::Decoder(
                    "MPEG frame limit exceeded".into(),
                ));
            }
        }
        if frames == 0 {
            return Err(invalid());
        }
    }
    reader.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
    Ok(())
}

#[cfg(feature = "decode-compressed")]
fn read_id3_footer(
    reader: &mut dyn crate::AudioReadSeek,
) -> Result<FileAudioInfo, AudioDecodeError> {
    use std::io::SeekFrom;
    let error = |e: std::io::Error| AudioDecodeError::Decoder(e.to_string());
    let end = reader.seek(SeekFrom::End(0)).map_err(error)?;
    let mut info = FileAudioInfo::default();
    if end >= 128 {
        reader.seek(SeekFrom::End(-128)).map_err(error)?;
        let mut bytes = [0; 128];
        reader.read_exact(&mut bytes).map_err(error)?;
        if &bytes[..3] == b"TAG" {
            let mut builder = symphonia::core::meta::MetadataBuilder::new();
            symphonia_metadata::id3v1::read_id3v1(
                &mut symphonia::core::io::BufReader::new(&bytes),
                &mut builder,
            )
            .map_err(AudioDecodeError::from)?;
            info.read_metadata(&builder.metadata())?;
        }
    }
    reader.seek(SeekFrom::Start(0)).map_err(error)?;
    Ok(info)
}

/// Validate metadata counts and lengths before third-party parsers allocate.
/// This deliberately does not traverse media payload during playback startup.
type LegacyPictures = Vec<(String, Vec<u8>)>;
pub(crate) fn validate_metadata_prefix(
    reader: &mut dyn crate::AudioReadSeek,
) -> Result<Option<LegacyPictures>, AudioDecodeError> {
    use std::io::SeekFrom;
    let mut legacy_pictures = None;
    let invalid = || AudioDecodeError::Decoder("malformed or oversized audio metadata".into());
    let io_error = |_| invalid();
    let end = reader.seek(SeekFrom::End(0)).map_err(io_error)?;
    reader.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut signature = [0u8; 10];
    reader.read_exact(&mut signature).map_err(io_error)?;
    if &signature[..4] == b"fLaC" {
        let mut pos = 4u64;
        let mut blocks = 0;
        let mut total = 0u64;
        loop {
            reader.read(&mut []).map_err(io_error)?;
            reader.seek(SeekFrom::Start(pos)).map_err(io_error)?;
            let mut h = [0u8; 4];
            reader.read_exact(&mut h).map_err(io_error)?;
            let size = u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
            blocks += 1;
            total = total.checked_add(size).ok_or_else(invalid)?;
            if blocks > 256
                || total > 32 * 1024 * 1024
                || pos.checked_add(4 + size).is_none_or(|n| n >= end)
            {
                return Err(invalid());
            }
            // Vorbis comments: vendor and field counts must be bounded before decoding.
            if h[0] & 0x7f == 4 {
                let mut buf = [0u8; 4];
                reader.read_exact(&mut buf).map_err(io_error)?;
                let vendor = u32::from_le_bytes(buf) as u64;
                if vendor > 65536 || vendor + 8 > size {
                    return Err(invalid());
                }
                reader
                    .seek(SeekFrom::Current(vendor as i64))
                    .map_err(io_error)?;
                reader.read_exact(&mut buf).map_err(io_error)?;
                let fields = u32::from_le_bytes(buf);
                if fields > 256 {
                    return Err(invalid());
                }
                let mut consumed = 8 + vendor;
                for _ in 0..fields {
                    reader.read(&mut []).map_err(io_error)?;
                    reader.read_exact(&mut buf).map_err(io_error)?;
                    let field = u32::from_le_bytes(buf) as u64;
                    consumed = consumed.checked_add(4 + field).ok_or_else(invalid)?;
                    if field > 65536 || consumed > size {
                        return Err(invalid());
                    }
                    reader
                        .seek(SeekFrom::Current(field as i64))
                        .map_err(io_error)?;
                }
                if consumed != size {
                    return Err(invalid());
                }
            }
            if h[0] & 0x7f == 6 {
                // PICTURE has internal lengths; bound them before the metadata
                // library can allocate, even when the outer block is tiny.
                let mut word = [0u8; 4];
                let mut consumed = 4u64;
                reader.read_exact(&mut word).map_err(io_error)?; // picture type
                for limit in [256u64, 65536] {
                    if consumed + 4 > size {
                        return Err(invalid());
                    }
                    reader.read_exact(&mut word).map_err(io_error)?;
                    let length = u32::from_be_bytes(word) as u64;
                    consumed += 4 + length;
                    if length > limit || consumed > size {
                        return Err(invalid());
                    }
                    reader
                        .seek(SeekFrom::Current(length as i64))
                        .map_err(io_error)?;
                }
                if consumed + 20 > size {
                    return Err(invalid());
                }
                reader.seek(SeekFrom::Current(16)).map_err(io_error)?; // dimensions/depth/palette
                reader.read_exact(&mut word).map_err(io_error)?;
                let data = u32::from_be_bytes(word) as u64;
                if data == 0 || data > 20 * 1024 * 1024 || consumed + 20 + data != size {
                    return Err(invalid());
                }
            }
            pos += 4 + size;
            if h[0] & 0x80 != 0 {
                break;
            }
        }
    } else if &signature[..3] == b"ID3" {
        let version = signature[3];
        if !(2..=4).contains(&version) || signature[6..10].iter().any(|b| b & 0x80 != 0) {
            return Err(invalid());
        }
        let allowed_flags = match version {
            2 => 0xc0,
            3 => 0xe0,
            _ => 0xf0,
        };
        if signature[4] != 0 || signature[5] & !allowed_flags != 0 {
            return Err(invalid());
        }
        let size = signature[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(*b));
        if size > 32 * 1024 * 1024 || size as u64 + 10 > end {
            return Err(invalid());
        }
        if version == 4 && signature[5] & 0x10 != 0 {
            if size as u64 + 20 > end {
                return Err(invalid());
            }
            reader
                .seek(SeekFrom::Start(size as u64 + 10))
                .map_err(io_error)?;
            let mut footer = [0; 10];
            reader.read_exact(&mut footer).map_err(io_error)?;
            if &footer[..3] != b"3DI" || footer[3..] != signature[3..] {
                return Err(invalid());
            }
            reader.seek(SeekFrom::Start(10)).map_err(io_error)?;
        }
        let mut bytes = vec![0; size];
        for chunk in bytes.chunks_mut(65536) {
            reader.read_exact(chunk).map_err(io_error)?;
        }
        if signature[5] & 0x80 != 0 && version < 4 {
            // Whole-tag unsynchronization precedes interpreting v2.2/v2.3 sizes.
            let mut write = 0;
            let mut read = 0;
            while read < bytes.len() {
                if read % 65536 == 0 {
                    reader.read(&mut []).map_err(io_error)?;
                }
                bytes[write] = bytes[read];
                let ff = bytes[read] == 0xff;
                write += 1;
                read += 1;
                if ff && bytes.get(read) == Some(&0) {
                    read += 1;
                }
            }
            bytes.truncate(write);
        }
        let mut pos = 0usize;
        if signature[5] & 0x40 != 0 {
            if version == 2 || bytes.len() < 4 {
                return Err(invalid());
            }
            pos = if version == 3 {
                u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize + 4
            } else {
                if bytes[..4].iter().any(|b| b & 0x80 != 0) {
                    return Err(invalid());
                }
                bytes[..4]
                    .iter()
                    .fold(0usize, |n, b| (n << 7) | usize::from(*b))
            };
            if pos < 4 || pos > bytes.len() {
                return Err(invalid());
            }
        }
        let mut fields = 0;
        while pos < bytes.len() {
            reader.read(&mut []).map_err(io_error)?;
            if bytes[pos] == 0 {
                if bytes[pos..].iter().any(|b| *b != 0) {
                    return Err(invalid());
                }
                break;
            }
            let header = if version == 2 { 6 } else { 10 };
            let ident = if version == 2 { 3 } else { 4 };
            if bytes.len() - pos < header {
                return Err(invalid());
            }
            let id = &bytes[pos..pos + ident];
            if !id
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                return Err(invalid());
            }
            let value = &bytes[pos + ident..pos + ident + if version == 2 { 3 } else { 4 }];
            let length = if version == 4 {
                if value.iter().any(|b| b & 0x80 != 0) {
                    return Err(invalid());
                }
                value.iter().fold(0usize, |n, b| (n << 7) | usize::from(*b))
            } else {
                value.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b))
            };
            if version >= 3 {
                let flags = bytes[pos + 9];
                let unsupported = if version == 3 {
                    flags & 0xc0 != 0
                } else {
                    flags & 0x0c != 0
                };
                if unsupported {
                    return Err(AudioDecodeError::UnsupportedSource(
                        "compressed or encrypted ID3 frame".into(),
                    ));
                }
                if (version == 3 && flags & 0x1f != 0) || (version == 4 && flags & 0xb0 != 0) {
                    return Err(invalid());
                }
            }
            fields += 1;
            let limit = if id == b"APIC" || id == b"PIC" {
                20 * 1024 * 1024
            } else {
                65536
            };
            if fields > 256 || length == 0 || length > limit || length > bytes.len() - pos - header
            {
                return Err(invalid());
            }
            // Symphonia maps v2.2 PIC to APIC but interprets its fixed 3-byte
            // format as a MIME string. Extract this legacy frame here instead.
            if version == 2 && id == b"PIC" {
                let value = &bytes[pos + header..pos + header + length];
                if value.len() < 6 {
                    return Err(invalid());
                }
                let media_type = match &value[1..4] {
                    b"PNG" => "image/png",
                    b"JPG" => "image/jpeg",
                    _ => {
                        return Err(AudioDecodeError::UnsupportedSource(
                            "ID3v2.2 picture format".into(),
                        ));
                    }
                };
                let description = &value[5..];
                let offset = match value[0] {
                    0 => description.iter().position(|b| *b == 0).map(|i| i + 1),
                    1 => description
                        .chunks_exact(2)
                        .position(|pair| pair == [0, 0])
                        .map(|i| (i + 1) * 2),
                    _ => None,
                }
                .ok_or_else(invalid)?;
                let picture = &description[offset..];
                let pictures = legacy_pictures.get_or_insert_with(Vec::new);
                if pictures.len() >= 16 || picture.is_empty() {
                    return Err(invalid());
                }
                pictures.push((media_type.to_owned(), picture.to_vec()));
            }
            pos += header + length;
        }
    }
    reader.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok(legacy_pictures)
}
