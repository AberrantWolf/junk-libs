#![cfg(feature = "decode-compressed")]
use junk_libs_audio::{AudioDecoder, ByteSource, FramePosition, MediaHint};
use std::io::Cursor;
fn open(bytes: Vec<u8>) -> AudioDecoder {
    let length = bytes.len() as u64;
    AudioDecoder::open(ByteSource::new(
        Cursor::new(bytes),
        MediaHint::new(Some(length), Some("flac"), Some("audio/flac")),
    ))
    .unwrap()
}
#[test]
fn independent_flac_vector_preserves_samples_count_and_exact_seek() {
    let mut decoder = open(include_bytes!("fixtures/ramp-1001.flac").to_vec());
    assert_eq!(decoder.total_frames(), Some(1001));
    let mut frames = 0;
    while let Some(chunk) = decoder.read_frames(73).unwrap() {
        for stereo in chunk.samples.chunks_exact(2) {
            assert_eq!(stereo[0], (frames as f32 - 500.0) / 32768.0);
            assert_eq!(stereo[1], (500.0 - frames as f32) / 32768.0);
            frames += 1;
        }
    }
    assert_eq!(frames, 1001);
    decoder.seek_exact(FramePosition(777)).unwrap();
    assert_eq!(
        decoder.read_frames(1).unwrap().unwrap().samples,
        [277.0 / 32768.0, -277.0 / 32768.0]
    );
}
#[test]
fn truncated_flac_is_not_successful_eof() {
    let bytes = include_bytes!("fixtures/ramp-1001.flac");
    let mut decoder = open(bytes[..bytes.len() - 20].to_vec());
    assert!(decoder.read_frames(2000).is_err());
}
