use std::io::{Cursor, SeekFrom};

use junk_libs_audio::{ByteSource, FramePosition, MediaHint, PcmChunk, PcmFormat};

#[test]
fn opaque_source_retains_hints_and_supports_read_and_seek() {
    let mut source = ByteSource::new(
        Cursor::new(vec![1, 2, 3, 4]),
        MediaHint::new(Some(4), Some("wav"), Some("audio/wav")),
    );
    source.reader_mut().seek(SeekFrom::Start(2)).unwrap();
    let mut byte = [0];
    source.reader_mut().read_exact(&mut byte).unwrap();
    assert_eq!(byte, [3]);
    assert_eq!(source.hint().length_bytes, Some(4));
}

#[test]
fn pcm_chunks_require_interleaved_frames_and_integer_coordinates() {
    let format = PcmFormat::new(48_000, 2).unwrap();
    let chunk = PcmChunk::new(format, FramePosition(7), vec![0.25, -0.25, 0.5, -0.5]).unwrap();
    assert_eq!(chunk.frame_count(), 2);
    assert_eq!(chunk.start_frame, FramePosition(7));
    assert!(PcmChunk::new(format, FramePosition(0), vec![0.0]).is_err());
}
