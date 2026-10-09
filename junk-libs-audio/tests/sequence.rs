#![cfg(all(feature = "decode-compressed", feature = "resample"))]
use junk_libs_audio::{AudioDecodeError, ByteSource, MediaHint, SequenceSource, decode_sequence};
use std::{cell::Cell, io::Cursor};
fn source(frames: usize, rate: u32, start: usize) -> SequenceSource {
    let mut bytes = Vec::new();
    let data = (frames * 4) as u32;
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 4).to_le_bytes());
    bytes.extend(4u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for i in start..start + frames {
        let value = ((i % 800) as i32 - 400) as i16;
        bytes.extend(value.to_le_bytes());
        bytes.extend((-value).to_le_bytes());
    }
    let len = bytes.len() as u64;
    SequenceSource {
        bytes: ByteSource::new(
            Cursor::new(bytes),
            MediaHint::new(Some(len), Some("wav"), Some("audio/wav")),
        ),
        expected_duration: Some((frames as u64, rate)),
    }
}
fn capture(sources: Vec<SequenceSource>) -> Vec<f32> {
    let mut samples = Vec::new();
    decode_sequence(sources.into_iter().map(Ok), 48000, 0, &|| false, |chunk| {
        samples.extend(chunk.samples);
        Ok(())
    })
    .unwrap();
    samples
}
#[test]
fn adjacent_sources_preserve_fractional_phase_and_samples() {
    let joined = capture(vec![source(2002, 44100, 0)]);
    let split = capture(vec![source(1001, 44100, 0), source(1001, 44100, 1001)]);
    assert_eq!(split, joined);
    assert_eq!(split.len() / 2, 2179);
}
#[test]
fn different_rates_drain_each_exact_span() {
    assert_eq!(
        capture(vec![source(441, 44100, 0), source(480, 48000, 441)]).len() / 2,
        960
    );
}
#[test]
fn cancellation_and_inconsistent_duration_fail() {
    let stopped = Cell::new(false);
    let result = decode_sequence(
        vec![Ok(source(10000, 44100, 0))],
        48000,
        0,
        &|| stopped.get(),
        |_| {
            stopped.set(true);
            Ok(())
        },
    );
    assert!(matches!(result, Err(AudioDecodeError::Cancelled)));
    let mut invalid = source(441, 44100, 0);
    invalid.expected_duration = Some((999, 44100));
    assert!(
        decode_sequence(vec![Ok(invalid)], 48000, 0, &|| false, |_| panic!(
            "invalid envelope emitted audio"
        ))
        .is_err()
    );
}
