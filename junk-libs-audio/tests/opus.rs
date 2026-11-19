#![cfg(feature = "decode-opus")]
//! Ogg Opus fixtures are encoded here with the bundled libopus and muxed per
//! RFC 7845, so expectations come from the stream construction rather than
//! from committed binaries. The reference PCM is a plain libopus decode from
//! the first packet with pre-skip, end trim and output gain applied by hand.
//! The adapter may decode through i16, so exact-path comparisons allow one
//! 16-bit quantization step; misalignment would show as ~0.5.

use std::{ffi::c_int, io::Cursor};

use junk_libs_audio::{AudioDecodeError, AudioDecoder, ByteSource, FramePosition, MediaHint};

const RATE: usize = 48_000;
const PACKET_FRAMES: usize = 960;
const PACKETS_PER_PAGE: usize = 8;
/// One i16 step plus float rounding (see module docs).
const EXACT: f32 = 1.0 / 16_384.0;

struct Spec {
    channels: u8,
    frames: usize,
    gain_q78: i16,
    mapping_family: u8,
    serial: u32,
}

impl Spec {
    fn new(channels: u8, frames: usize) -> Self {
        Self {
            channels,
            frames,
            gain_q78: 0,
            mapping_family: 0,
            serial: 0x5eed,
        }
    }
}

struct Fixture {
    bytes: Vec<u8>,
    packets: Vec<Vec<u8>>,
    pre_skip: usize,
    channels: usize,
    frames: usize,
    gain: f32,
}

/// Distinct deterministic tones per channel with a slow amplitude ramp.
fn signal(channels: usize, frames: usize) -> Vec<i16> {
    (0..frames)
        .flat_map(|frame| {
            (0..channels).map(move |channel| {
                let t = frame as f64 / RATE as f64;
                let hz = 330.0 * (channel + 1) as f64;
                let envelope = 0.2 + 0.5 * (frame as f64 / frames as f64);
                ((2.0 * std::f64::consts::PI * hz * t).sin() * envelope * 20_000.0) as i16
            })
        })
        .collect()
}

fn encode(spec: &Spec) -> Fixture {
    let channels = usize::from(spec.channels);
    let mut error = 0;
    let encoder = unsafe {
        opusic_sys::opus_encoder_create(
            RATE as i32,
            c_int::from(spec.channels),
            opusic_sys::OPUS_APPLICATION_AUDIO,
            &mut error,
        )
    };
    assert_eq!(error, opusic_sys::OPUS_OK);
    let mut lookahead: i32 = 0;
    assert_eq!(
        unsafe {
            opusic_sys::opus_encoder_ctl(
                encoder,
                opusic_sys::OPUS_GET_LOOKAHEAD_REQUEST,
                &mut lookahead as *mut i32,
            )
        },
        opusic_sys::OPUS_OK
    );
    let pre_skip = usize::try_from(lookahead).unwrap();
    // The encoder delays its input by pre-skip; code enough packets to cover it.
    let coded = (pre_skip + spec.frames).div_ceil(PACKET_FRAMES) * PACKET_FRAMES;
    let mut input = signal(channels, spec.frames);
    input.resize(coded * channels, 0);
    let packets = input
        .chunks_exact(PACKET_FRAMES * channels)
        .map(|pcm| {
            let mut packet = vec![0; 4_000];
            let length = unsafe {
                opusic_sys::opus_encode(
                    encoder,
                    pcm.as_ptr(),
                    PACKET_FRAMES as c_int,
                    packet.as_mut_ptr(),
                    packet.len() as i32,
                )
            };
            packet.truncate(usize::try_from(length).expect("encode"));
            packet
        })
        .collect::<Vec<_>>();
    unsafe { opusic_sys::opus_encoder_destroy(encoder) };

    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(spec.channels);
    head.extend_from_slice(&u16::try_from(pre_skip).unwrap().to_le_bytes());
    head.extend_from_slice(&48_000_u32.to_le_bytes());
    head.extend_from_slice(&spec.gain_q78.to_le_bytes());
    head.push(spec.mapping_family);
    if spec.mapping_family != 0 {
        // One stream per channel, none coupled, identity mapping.
        head.extend_from_slice(&[spec.channels, 0]);
        head.extend(0..spec.channels);
    }
    let mut tags = b"OpusTags".to_vec();
    let vendor = b"junk-libs test";
    let comment = b"TITLE=Generated tone";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&1_u32.to_le_bytes());
    tags.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    tags.extend_from_slice(comment);

    let mut bytes = Vec::new();
    let mut sequence = 0;
    let mut page = |packets: &[&[u8]], granule: u64, flags: u8, bytes: &mut Vec<u8>| {
        write_page(bytes, spec.serial, sequence, granule, flags, packets);
        sequence += 1;
    };
    page(&[&head], 0, 2, &mut bytes);
    page(&[&tags], 0, 0, &mut bytes);
    let final_granule = (pre_skip + spec.frames) as u64;
    let pages = packets.chunks(PACKETS_PER_PAGE).collect::<Vec<_>>();
    let mut coded_so_far = 0_u64;
    for (index, group) in pages.iter().enumerate() {
        coded_so_far += (group.len() * PACKET_FRAMES) as u64;
        let last = index + 1 == pages.len();
        let granule = if last { final_granule } else { coded_so_far };
        let refs = group.iter().map(Vec::as_slice).collect::<Vec<_>>();
        page(&refs, granule, if last { 4 } else { 0 }, &mut bytes);
    }

    Fixture {
        bytes,
        packets,
        pre_skip,
        channels,
        frames: spec.frames,
        gain: 10_f32.powf(f32::from(spec.gain_q78) / (20.0 * 256.0)),
    }
}

fn write_page(
    out: &mut Vec<u8>,
    serial: u32,
    sequence: u32,
    granule: u64,
    flags: u8,
    packets: &[&[u8]],
) {
    let mut lacing = Vec::new();
    for packet in packets {
        lacing.extend(std::iter::repeat_n(255, packet.len() / 255));
        lacing.push((packet.len() % 255) as u8);
    }
    let start = out.len();
    out.extend_from_slice(b"OggS\0");
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(u8::try_from(lacing.len()).unwrap());
    out.extend_from_slice(&lacing);
    for packet in packets {
        out.extend_from_slice(packet);
    }
    let crc = out[start..].iter().fold(0_u32, |mut crc, &byte| {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
        crc
    });
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

impl Fixture {
    /// Continuous libopus decode with RFC 7845 trimming and gain.
    fn reference(&self) -> Vec<f32> {
        let mut error = 0;
        let decoder = unsafe {
            opusic_sys::opus_decoder_create(RATE as i32, self.channels as c_int, &mut error)
        };
        assert_eq!(error, opusic_sys::OPUS_OK);
        let mut pcm = Vec::new();
        let mut buffer = vec![0_i16; 5_760 * self.channels];
        for packet in &self.packets {
            let frames = unsafe {
                opusic_sys::opus_decode(
                    decoder,
                    packet.as_ptr(),
                    packet.len() as i32,
                    buffer.as_mut_ptr(),
                    5_760,
                    0,
                )
            };
            pcm.extend_from_slice(&buffer[..usize::try_from(frames).unwrap() * self.channels]);
        }
        unsafe { opusic_sys::opus_decoder_destroy(decoder) };
        pcm[self.pre_skip * self.channels..][..self.frames * self.channels]
            .iter()
            .map(|&sample| f32::from(sample) / 32_768.0 * self.gain)
            .collect()
    }

    fn open(&self, extension: &str) -> Result<AudioDecoder, AudioDecodeError> {
        open_bytes(self.bytes.clone(), extension)
    }
}

fn open_bytes(bytes: Vec<u8>, extension: &str) -> Result<AudioDecoder, AudioDecodeError> {
    let length = bytes.len() as u64;
    AudioDecoder::open(ByteSource::new(
        Cursor::new(bytes),
        MediaHint::new(Some(length), Some(extension), Some("audio/ogg")),
    ))
}

fn read_all(decoder: &mut AudioDecoder, chunk: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    while let Some(pcm) = decoder.read_frames(chunk).unwrap() {
        assert_eq!(
            pcm.start_frame.0 as usize,
            decoder.position().0 as usize - pcm.frame_count() as usize
        );
        samples.extend(pcm.samples);
    }
    samples
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32, context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: sample count");
    let worst = actual
        .iter()
        .zip(expected)
        .map(|(a, e)| (a - e).abs())
        .fold(0.0_f32, f32::max);
    assert!(worst <= tolerance, "{context}: max deviation {worst}");
}

/// The squared error against the reference is smallest with no frame offset.
/// Tails shorter than 10 ms are too short to discriminate and are skipped.
fn assert_aligned(actual: &[f32], expected: &[f32], channels: usize, context: &str) {
    let frames = (actual.len() / channels).min(4_800);
    if frames < 480 {
        return;
    }
    let frames = frames - 2;
    let error = |lag: usize| -> f32 {
        (2..frames)
            .flat_map(|frame| (0..channels).map(move |c| (frame, c)))
            .map(|(frame, c)| {
                actual[frame * channels + c] - expected[(frame + lag - 2) * channels + c]
            })
            .map(|d| d * d)
            .sum()
    };
    let aligned = error(2);
    for lag in [0, 1, 3, 4] {
        assert!(
            aligned < error(lag),
            "{context}: offset {} fits better",
            lag as i64 - 2
        );
    }
}

#[test]
fn sample_exact_length_applies_pre_skip_and_end_trim() {
    for (channels, extension) in [(1, "opus"), (2, "opus"), (1, "ogg"), (2, "ogg")] {
        // Not a packet multiple, so the last page granule trims the final packet.
        let fixture = encode(&Spec::new(channels, 30_001));
        assert!(fixture.pre_skip > 0);
        let mut decoder = fixture.open(extension).unwrap();
        assert_eq!(decoder.format().sample_rate(), 48_000);
        assert_eq!(decoder.format().channels(), u16::from(channels));
        assert_eq!(decoder.total_frames(), Some(30_001));
        assert_eq!(decoder.inspection.codec, "opus");
        assert_eq!(
            decoder.inspection.delay_frames,
            Some(fixture.pre_skip as u32)
        );
        assert_eq!(decoder.inspection.tag("title"), Some("Generated tone"));
        let decoded = read_all(&mut decoder, 777);
        assert_close(
            &decoded,
            &fixture.reference(),
            EXACT,
            &format!("{channels}ch .{extension}"),
        );
    }
}

#[test]
fn seeks_to_arbitrary_frames_with_preroll() {
    for channels in [1, 2] {
        let fixture = encode(&Spec::new(channels, 50_000));
        let reference = fixture.reference();
        let mut decoder = fixture.open("opus").unwrap();
        let ch = usize::from(channels);
        // Start, inside pre-roll reach of the start, page edges, deep and end.
        for target in [
            0, 1, 959, 961, 3_000, 4_500, 7_679, 7_680, 7_681, 24_321, 49_999, 50_000, 12,
        ] {
            decoder.seek_exact(FramePosition(target)).unwrap();
            assert_eq!(decoder.position(), FramePosition(target));
            let decoded = read_all(&mut decoder, 4_096);
            assert_eq!(
                decoded.len(),
                (50_000 - target as usize) * ch,
                "seek {target}"
            );
            let expected = &reference[target as usize * ch..];
            let context = format!("{channels}ch seek {target}");
            if target as usize + fixture.pre_skip < 3_840 {
                // Pre-roll reaches back to the stream start: same decode path.
                assert_close(&decoded, expected, EXACT, &context);
            } else {
                // libopus is not bit-exact after a reset; 80 ms of pre-roll
                // converges to within a few percent, so check alignment too.
                assert_close(&decoded, expected, 0.05, &context);
                assert_aligned(&decoded, expected, ch, &context);
            }
        }
        assert!(matches!(
            decoder.seek_exact(FramePosition(50_001)),
            Err(AudioDecodeError::SeekOutOfRange { .. })
        ));
    }
}

#[test]
fn output_gain_is_applied() {
    for gain_q78 in [-6 * 256, 3 * 256 + 128] {
        let fixture = encode(&Spec {
            gain_q78,
            ..Spec::new(2, 9_600)
        });
        let reference = fixture.reference();
        let decoded = read_all(&mut fixture.open("opus").unwrap(), 1_000);
        assert_close(&decoded, &reference, EXACT * fixture.gain, "gain");
    }
}

#[test]
fn single_page_stream_trims_end_not_start() {
    // One audio page whose granule (952) is below its coded duration (960);
    // Symphonia alone would treat that as start delay.
    let bytes = include_bytes!("fixtures/silence-588.opus").to_vec();
    let mut decoder = open_bytes(bytes, "opus").unwrap();
    assert_eq!(decoder.total_frames(), Some(640));
    assert_eq!(decoder.inspection.delay_frames, Some(312));
    let decoded = read_all(&mut decoder, 100);
    assert_eq!(decoded.len(), 640 * 2);
    assert!(decoded.iter().all(|sample| sample.abs() < 1e-3));
    decoder.seek_exact(FramePosition(600)).unwrap();
    assert_eq!(read_all(&mut decoder, 100).len(), 40 * 2);
}

#[test]
fn unsupported_layouts_fail_clearly() {
    let family_one = encode(&Spec {
        mapping_family: 1,
        ..Spec::new(2, 4_800)
    });
    assert!(matches!(
        family_one.open("opus"),
        Err(AudioDecodeError::UnsupportedOpusChannelMapping(1))
    ));

    let mut chained = encode(&Spec::new(2, 4_800)).bytes;
    chained.extend(
        encode(&Spec {
            serial: 0xbeef,
            ..Spec::new(2, 4_800)
        })
        .bytes,
    );
    assert!(matches!(
        open_bytes(chained, "opus"),
        Err(AudioDecodeError::UnsupportedOggLayout)
    ));
}

#[test]
fn truncated_stream_is_not_successful_eof() {
    let fixture = encode(&Spec::new(2, 30_000));
    let truncated = fixture.bytes[..fixture.bytes.len() - 20].to_vec();
    let result = open_bytes(truncated, "opus").and_then(|mut decoder| {
        while decoder.read_frames(4_096)?.is_some() {}
        Ok(())
    });
    assert!(result.is_err());
}
