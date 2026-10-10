//! Content-only identification. Length is a layout constraint, never a system signature.
use crate::*;
use std::io::SeekFrom;

// Standard cartridge-header logo bytes, also used by the existing Retro Junk inspector.
const GBA_LOGO: [u8; 156] = [
    0x24, 0xFF, 0xAE, 0x51, 0x69, 0x9A, 0xA2, 0x21, 0x3D, 0x84, 0x82, 0x0A, 0x84, 0xE4, 0x09, 0xAD,
    0x11, 0x24, 0x8B, 0x98, 0xC0, 0x81, 0x7F, 0x21, 0xA3, 0x52, 0xBE, 0x19, 0x93, 0x09, 0xCE, 0x20,
    0x10, 0x46, 0x4A, 0x4A, 0xF8, 0x27, 0x31, 0xEC, 0x58, 0xC7, 0xE8, 0x33, 0x82, 0xE3, 0xCE, 0xBF,
    0x85, 0xF4, 0xDF, 0x94, 0xCE, 0x4B, 0x09, 0xC1, 0x94, 0x56, 0x8A, 0xC0, 0x13, 0x72, 0xA7, 0xFC,
    0x9F, 0x84, 0x4D, 0x73, 0xA3, 0xCA, 0x9A, 0x61, 0x58, 0x97, 0xA3, 0x27, 0xFC, 0x03, 0x98, 0x76,
    0x23, 0x1D, 0xC7, 0x61, 0x03, 0x04, 0xAE, 0x56, 0xBF, 0x38, 0x84, 0x00, 0x40, 0xA7, 0x0E, 0xFD,
    0xFF, 0x52, 0xFE, 0x03, 0x6F, 0x95, 0x30, 0xF1, 0x97, 0xFB, 0xC0, 0x85, 0x60, 0xD6, 0x80, 0x25,
    0xA9, 0x63, 0xBE, 0x03, 0x01, 0x4E, 0x38, 0xE2, 0xF9, 0xA2, 0x34, 0xFF, 0xBB, 0x3E, 0x03, 0x44,
    0x78, 0x00, 0x90, 0xCB, 0x88, 0x11, 0x3A, 0x94, 0x65, 0xC0, 0x7C, 0x63, 0x87, 0xF0, 0x3C, 0xAF,
    0xD6, 0x25, 0xE4, 0x8B, 0x38, 0x0A, 0xAC, 0x72, 0x21, 0xD4, 0xF8, 0x07,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectionStatus {
    Recognized,
    Ambiguous,
    Unrecognized,
    Malformed,
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedFormat {
    Ines,
    N64,
    RawSystem(&'static str),
    Headerless,
}
#[derive(Clone, Debug)]
pub struct FormatDetection {
    pub status: DetectionStatus,
    pub format: Option<DetectedFormat>,
    pub reason: String,
}

pub fn inspect_game_format<R: ReadSeek + ?Sized>(
    reader: &mut R,
) -> Result<FormatDetection, AnalysisError> {
    let bytes = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut head = [0; 512];
    let count = bytes.min(head.len() as u64) as usize;
    reader.read_exact(&mut head[..count])?;
    reader.seek(SeekFrom::Start(0))?;
    let result = if count >= 4 && &head[..4] == b"NES\x1a" {
        match inspect_ines(reader) {
            Ok(_) => FormatDetection {
                status: DetectionStatus::Recognized,
                format: Some(DetectedFormat::Ines),
                reason: "Validated iNES/NES 2.0 header and exact payload length".into(),
            },
            Err(AnalysisError::UnsupportedVariant(reason)) => FormatDetection {
                status: DetectionStatus::Unsupported,
                format: Some(DetectedFormat::Ines),
                reason,
            },
            Err(AnalysisError::Io(error)) => return Err(AnalysisError::Io(error)),
            Err(error) => FormatDetection {
                status: DetectionStatus::Malformed,
                format: Some(DetectedFormat::Ines),
                reason: error.to_string(),
            },
        }
    } else if count >= 4 && detect_n64_format(&head[..4]).is_some() {
        if bytes < 4096 || bytes % 4 != 0 {
            FormatDetection {
                status: DetectionStatus::Malformed,
                format: Some(DetectedFormat::N64),
                reason: "N64 header is truncated or payload is not word aligned".into(),
            }
        } else {
            // Validate reserved header words in normalized byte order. A byte-order magic alone is insufficient.
            let format = detect_n64_format(&head[..4]).unwrap();
            let mut header = [0; 64];
            header.copy_from_slice(&head[..64]);
            normalize_n64_in_place(&mut header, format);
            if header[0x18..0x20].iter().any(|b| *b != 0)
                || header[0x34..0x38].iter().any(|b| *b != 0)
            {
                FormatDetection {
                    status: DetectionStatus::Malformed,
                    format: Some(DetectedFormat::N64),
                    reason: "N64 reserved header fields are inconsistent".into(),
                }
            } else {
                FormatDetection {
                    status: DetectionStatus::Recognized,
                    format: Some(DetectedFormat::N64),
                    reason: "Validated N64 byte order, header boundaries and reserved fields"
                        .into(),
                }
            }
        }
    } else if count >= 0x104 && &head[0x100..0x104] == b"SEGA" {
        if count < 512 {
            FormatDetection {
                status: DetectionStatus::Malformed,
                format: Some(DetectedFormat::RawSystem("megadrive")),
                reason: "Truncated Mega Drive header".into(),
            }
        } else {
            let start = u32::from_be_bytes(head[0x1a0..0x1a4].try_into().unwrap()) as u64;
            let end = u32::from_be_bytes(head[0x1a4..0x1a8].try_into().unwrap()) as u64;
            if start != 0 || end < 511 || end >= bytes || bytes % 2 != 0 {
                FormatDetection {
                    status: DetectionStatus::Malformed,
                    format: Some(DetectedFormat::RawSystem("megadrive")),
                    reason: "Mega Drive header declares impossible ROM boundaries".into(),
                }
            } else {
                FormatDetection {status:DetectionStatus::Recognized,format:Some(DetectedFormat::RawSystem("megadrive")),reason:"Mega Drive header and ROM boundaries are consistent; catalog matching establishes identity".into()}
            }
        }
    } else if count >= 8 && head[4..8] == [0x24, 0xff, 0xae, 0x51] {
        if count < 0xc0
            || head[4..0xa0] != GBA_LOGO
            || head[0xb2] != 0x96
            || head[0xbe..0xc0].iter().any(|b| *b != 0)
            || head[0xa0..0xbd]
                .iter()
                .fold(0u8, |sum, b| sum.wrapping_sub(*b))
                .wrapping_sub(0x19)
                != head[0xbd]
            || head[0xb5..0xbc].iter().any(|b| *b != 0)
        {
            FormatDetection {
                status: DetectionStatus::Malformed,
                format: Some(DetectedFormat::RawSystem("gba")),
                reason: "GBA header is truncated or its checksum/reserved fields are inconsistent"
                    .into(),
            }
        } else {
            FormatDetection {status:DetectionStatus::Recognized,format:Some(DetectedFormat::RawSystem("gba")),reason:"GBA header checksum and boundaries are consistent; catalog matching establishes identity".into()}
        }
    } else if count >= 4 && (&head[..4] == b"UNIF" || (count >= 8 && &head[..8] == b"MComprHD")) {
        FormatDetection {
            status: DetectionStatus::Unsupported,
            format: None,
            reason: "Recognized container has no qualified automatic reader".into(),
        }
    } else if (count >= 4 && &head[..4] == b"PK\x03\x04")
        || (count >= 6 && &head[..6] == b"7z\xbc\xaf\x27\x1c")
        || (count >= 4 && &head[..4] == b"Rar!")
        || (count >= 2 && head[..2] == [0x1f, 0x8b])
    {
        FormatDetection {
            status: DetectionStatus::Unsupported,
            format: None,
            reason: "Compressed game packages have no qualified automatic reader".into(),
        }
    } else if bytes == 0 {
        FormatDetection {
            status: DetectionStatus::Malformed,
            format: None,
            reason: "Empty payload".into(),
        }
    } else {
        FormatDetection {
            status: DetectionStatus::Unrecognized,
            format: Some(DetectedFormat::Headerless),
            reason: "No supported identifying header; exact catalog evidence is required".into(),
        }
    };
    reader.seek(SeekFrom::Start(0))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn malformed_and_unsupported_headers_never_become_raw() {
        let mut nes = vec![0; 16];
        nes[..4].copy_from_slice(b"NES\x1a");
        nes[4] = 1;
        assert_eq!(
            inspect_game_format(&mut Cursor::new(nes.clone()))
                .unwrap()
                .status,
            DetectionStatus::Malformed
        );
        nes.resize(16 + 16384, 0);
        nes[6] = 4;
        assert_eq!(
            inspect_game_format(&mut Cursor::new(nes)).unwrap().status,
            DetectionStatus::Unsupported
        );
        let mut n64 = vec![0; 64];
        n64[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        n64[0x18] = 1;
        assert_eq!(
            inspect_game_format(&mut Cursor::new(n64)).unwrap().status,
            DetectionStatus::Malformed
        );
    }
    #[test]
    fn lengths_do_not_identify_systems() {
        for size in [1, 8192, 32768, 33280] {
            assert_eq!(
                inspect_game_format(&mut Cursor::new(vec![0x55; size]))
                    .unwrap()
                    .status,
                DetectionStatus::Unrecognized
            );
        }
    }
    #[test]
    fn valid_ines_and_n64_orders_use_contents() {
        let mut nes = vec![0; 16 + 16384];
        nes[..4].copy_from_slice(b"NES\x1a");
        nes[4] = 1;
        assert_eq!(
            inspect_game_format(&mut Cursor::new(nes)).unwrap().format,
            Some(DetectedFormat::Ines)
        );
        for magic in [
            [0x80, 0x37, 0x12, 0x40],
            [0x37, 0x80, 0x40, 0x12],
            [0x40, 0x12, 0x37, 0x80],
        ] {
            let mut rom = vec![0; 4096];
            rom[..4].copy_from_slice(&magic);
            assert_eq!(
                inspect_game_format(&mut Cursor::new(rom)).unwrap().status,
                DetectionStatus::Recognized
            );
        }
    }
}
