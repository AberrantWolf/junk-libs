#![cfg(feature = "decode-compressed")]
use junk_libs_audio::{
    AudioDecodeError, AudioDecoder, ByteSource, FramePosition, MediaHint, inspect_file_audio,
};
use std::{fs, io::Cursor, path::PathBuf};
fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/purchases")
            .join(name),
    )
    .unwrap()
}
fn source(bytes: Vec<u8>, extension: &str) -> ByteSource {
    let length = bytes.len() as u64;
    ByteSource::new(
        Cursor::new(bytes),
        MediaHint::new(Some(length), Some(extension), None),
    )
}
#[test]
fn native_flac_vectors_preserve_all_advertised_rates_depths_and_channels() {
    for rate in [44100, 48000, 88200, 96000, 176400, 192000] {
        for bits in [16, 24] {
            for channels in [1, 2] {
                let bytes = fixture(&format!("flac-{rate}-{bits}-{channels}.flac"));
                let info =
                    inspect_file_audio(source(bytes.clone(), "flac"), &mut || Ok(())).unwrap();
                assert_eq!(
                    (
                        info.sample_rate,
                        info.bits_per_sample,
                        info.channels,
                        info.decoded_frames
                    ),
                    (rate, Some(bits), channels, Some(u64::from(rate / 100)))
                );
                let mut decoder = AudioDecoder::open(source(bytes, "flac")).unwrap();
                decoder.seek_exact(FramePosition(17)).unwrap();
                let chunk = decoder.read_frames(5).unwrap().unwrap();
                assert_eq!(chunk.start_frame, FramePosition(17));
                assert_eq!(chunk.frame_count(), 5);
                for (index, sample) in chunk.samples.iter().enumerate() {
                    let frame = 17 + index / usize::from(channels);
                    let integer =
                        ((frame as f64 / 7.0).sin() * ((1u64 << (bits - 2)) as f64)) as i32;
                    let sign = if channels == 2 && index % 2 == 1 {
                        -1.0
                    } else {
                        1.0
                    };
                    let expected = sign * integer as f32 / (1u64 << (bits - 1)) as f32;
                    assert!(
                        (sample - expected).abs() <= 1.0 / (1u64 << (bits - 1)) as f32,
                        "rate={rate} bits={bits} channels={channels} sample={index}: {sample} vs {expected}"
                    );
                }
            }
        }
    }
}
#[test]
fn mpeg_variants_decode_seek_and_apply_available_delay_padding() {
    for rate in [8000, 16000, 44100] {
        for channels in [1, 2] {
            for mode in ["cbr", "vbr", "no-xing"] {
                let bytes = fixture(&format!(
                    "mp3-{rate}-{mode}{}.mp3",
                    if channels == 2 { "-stereo" } else { "" }
                ));
                let info =
                    inspect_file_audio(source(bytes.clone(), "mp3"), &mut || Ok(())).unwrap();
                assert_eq!(
                    (info.codec.as_str(), info.sample_rate, info.channels),
                    ("mp3", rate, channels)
                );
                assert_eq!(info.tag("title"), Some("音楽"));
                if mode != "no-xing" {
                    assert_eq!(info.decoded_frames, Some(u64::from(rate / 5)));
                }
                let mut decoder = AudioDecoder::open(source(bytes, "mp3")).unwrap();
                decoder.seek_exact(FramePosition(100)).unwrap();
                assert_eq!(
                    decoder.read_frames(7).unwrap().unwrap().start_frame,
                    FramePosition(100)
                );
            }
        }
    }
}
#[test]
fn truncation_and_cancelled_decoding_never_return_success() {
    for name in [
        "flac-96000-24-2.flac",
        "mp3-44100-cbr.mp3",
        "mp3-8000-no-xing.mp3",
    ] {
        let mut bytes = fixture(name);
        bytes.truncate(bytes.len() - 7);
        assert!(
            inspect_file_audio(source(bytes, name.rsplit('.').next().unwrap()), &mut || Ok(
                ()
            ))
            .is_err(),
            "{name}"
        );
    }
    let bytes = fixture("flac-192000-24-2.flac");
    let mut calls = 0;
    let result = inspect_file_audio(source(bytes, "flac"), &mut || {
        calls += 1;
        if calls >= 3 {
            Err(AudioDecodeError::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(AudioDecodeError::Cancelled)));
}

fn mpeg_payload() -> Vec<u8> {
    let bytes = fixture("mp3-44100-no-xing.mp3");
    if bytes.starts_with(b"ID3") {
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(*b));
        bytes[10 + size..].to_vec()
    } else {
        bytes
    }
}
fn synchsafe(n: usize) -> [u8; 4] {
    [
        ((n >> 21) & 127) as u8,
        ((n >> 14) & 127) as u8,
        ((n >> 7) & 127) as u8,
        (n & 127) as u8,
    ]
}
fn id3_frame(version: u8, id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut frame = id.to_vec();
    if version == 2 {
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    } else {
        frame.extend_from_slice(&if version == 4 {
            synchsafe(payload.len())
        } else {
            (payload.len() as u32).to_be_bytes()
        });
        frame.extend_from_slice(&[0, 0]);
    }
    frame.extend_from_slice(payload);
    frame
}
fn tagged(version: u8, mut body: Vec<u8>, unsync: bool) -> Vec<u8> {
    if unsync {
        body = body
            .into_iter()
            .flat_map(|b| if b == 255 { vec![255, 0] } else { vec![b] })
            .collect();
    }
    let mut bytes = vec![b'I', b'D', b'3', version, 0, if unsync { 128 } else { 0 }];
    bytes.extend_from_slice(&synchsafe(body.len()));
    bytes.extend(body);
    bytes.extend(mpeg_payload());
    bytes
}
#[test]
fn id3_versions_unicode_unsynchronization_and_picture_variants() {
    // Independently assembled ID3 bytes, with real independently encoded MPEG audio.
    let picture = b"\x89PNG\r\n\x1a\nfixture";
    for version in [2, 3, 4] {
        let title = if version == 4 {
            b"\x03\xe9\x9f\xb3\xe6\xa5\xbd".to_vec()
        } else {
            vec![1, 255, 254, 0xf3, 0x97, 0x7d, 0x69]
        };
        let mut body = id3_frame(version, if version == 2 { b"TT2" } else { b"TIT2" }, &title);
        // PIC and APIC have different format fields; an empty description follows type.
        let mut image = if version == 2 {
            b"\0PNG\x03\0".to_vec()
        } else {
            b"\0image/png\0\x03\0".to_vec()
        };
        image.extend_from_slice(picture);
        body.extend(id3_frame(
            version,
            if version == 2 { b"PIC" } else { b"APIC" },
            &image,
        ));
        for unsync in [false, true] {
            if version == 4 && unsync {
                continue;
            }
            let info = inspect_file_audio(
                source(tagged(version, body.clone(), unsync), "mp3"),
                &mut || Ok(()),
            )
            .unwrap();
            assert_eq!(
                info.tag("title"),
                Some("音楽"),
                "ID3v2.{version} unsync={unsync}"
            );
            assert_eq!(info.pictures[0].1, picture);
        }
    }
    let mut bytes = mpeg_payload();
    let mut footer = [0; 128];
    footer[..3].copy_from_slice(b"TAG");
    footer[3..9].copy_from_slice(b"Legacy");
    bytes.extend(footer);
    assert_eq!(
        inspect_file_audio(source(bytes, "mp3"), &mut || Ok(()))
            .unwrap()
            .tag("title"),
        Some("Legacy")
    );
}
#[test]
fn hostile_metadata_is_rejected_before_allocating_from_inner_lengths() {
    let mut body = id3_frame(4, b"TIT2", b"\x03ok");
    body[4..8].copy_from_slice(&synchsafe(10_000));
    assert!(AudioDecoder::open(source(tagged(4, body, false), "mp3")).is_err());
    let repeated = (0..257)
        .flat_map(|_| id3_frame(3, b"TIT2", b"\0x"))
        .collect();
    assert!(AudioDecoder::open(source(tagged(3, repeated, false), "mp3")).is_err());
    // Tiny FLAC comment block advertises an impossible field count.
    let original = fixture("flac-44100-16-1.flac");
    let mut bytes = original[..42].to_vec();
    bytes[4] &= 127;
    bytes.extend_from_slice(&[0x84, 0, 0, 8, 0, 0, 0, 0, 255, 255, 255, 255]);
    bytes.extend_from_slice(&original[42..]);
    assert!(AudioDecoder::open(source(bytes, "flac")).is_err());
    let mut picture = original[..42].to_vec();
    picture[4] &= 127;
    picture.extend_from_slice(&[0x86, 0, 0, 8, 0, 0, 0, 3, 255, 255, 255, 255]);
    picture.extend_from_slice(&original[42..]);
    assert!(AudioDecoder::open(source(picture, "flac")).is_err());
}

#[test]
fn compressed_id3_frames_are_explicitly_unsupported_and_v24_frame_unsync_decodes() {
    let mut body = id3_frame(3, b"TIT2", b"\0title");
    body[9] = 0x80;
    assert!(matches!(
        AudioDecoder::open(source(tagged(3, body, false), "mp3")),
        Err(AudioDecodeError::UnsupportedSource(_))
    ));
    let mut body = id3_frame(4, b"TIT2", &[0, 255, 0, 224]);
    body[9] = 2;
    assert_eq!(
        inspect_file_audio(source(tagged(4, body, false), "mp3"), &mut || Ok(()))
            .unwrap()
            .tag("title"),
        Some("ÿà")
    );
}
