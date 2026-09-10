use junk_libs_disc::{
    cue_audio::CueAudioReader,
    redumper::pcm::{RawAudioReader, SampleQuality},
};
use thiserror::Error;

use crate::{FramePosition, PcmChunk, PcmFormat};

const CD_SAMPLE_RATE: u32 = 44_100;
const CD_CHANNELS: u16 = 2;
const MAX_DISC_READ_FRAMES: usize = 588;

/// Playback adapter for an already-authorized CUE audio reader.
///
/// Opening CUE paths and choosing a track remain product-adapter policy.
pub struct CuePcmDecoder {
    reader: CueAudioReader,
}

impl CuePcmDecoder {
    #[must_use]
    pub fn new(reader: CueAudioReader) -> Self {
        Self { reader }
    }

    #[must_use]
    pub fn format(&self) -> PcmFormat {
        cd_format()
    }

    #[must_use]
    pub fn total_frames(&self) -> u64 {
        self.reader.total_frames()
    }

    #[must_use]
    pub fn position(&self) -> FramePosition {
        FramePosition(self.reader.position_frames())
    }

    pub fn seek_exact(&mut self, position: FramePosition) -> Result<(), DiscDecodeError> {
        self.reader
            .seek_frame(position.0)
            .map_err(|error| DiscDecodeError::Reader(error.to_string()))
    }

    pub fn read_frames(&mut self, max_frames: usize) -> Result<Option<PcmChunk>, DiscDecodeError> {
        validate_read_size(max_frames)?;
        let start = self.position();
        let mut integer = vec![[0_i16; 2]; max_frames];
        let count = self
            .reader
            .read_frames(&mut integer)
            .map_err(|error| DiscDecodeError::Reader(error.to_string()))?;
        pcm_chunk(start, &integer[..count])
    }
}

/// Playback adapter for an already-authorized redumper raw-audio reader.
/// Returned quality observations remain available to the consumer.
pub struct RedumperPcmDecoder {
    reader: RawAudioReader,
}

impl RedumperPcmDecoder {
    #[must_use]
    pub fn new(reader: RawAudioReader) -> Self {
        Self { reader }
    }

    #[must_use]
    pub fn format(&self) -> PcmFormat {
        cd_format()
    }

    #[must_use]
    pub fn total_frames(&self) -> u64 {
        self.reader.total_frames()
    }

    #[must_use]
    pub fn position(&self) -> FramePosition {
        FramePosition(self.reader.position_frames())
    }

    pub fn seek_exact(&mut self, position: FramePosition) -> Result<(), DiscDecodeError> {
        self.reader
            .seek_frame(position.0)
            .map_err(|error| DiscDecodeError::Reader(error.to_string()))
    }

    pub fn read_frames(
        &mut self,
        max_frames: usize,
    ) -> Result<Option<RedumperPcmChunk>, DiscDecodeError> {
        validate_read_size(max_frames)?;
        let start = self.position();
        let mut integer = vec![[0_i16; 2]; max_frames];
        let (count, quality) = self
            .reader
            .read_frames(&mut integer)
            .map_err(|error| DiscDecodeError::Reader(error.to_string()))?;
        Ok(pcm_chunk(start, &integer[..count])?.map(|pcm| RedumperPcmChunk { pcm, quality }))
    }
}

#[derive(Debug)]
pub struct RedumperPcmChunk {
    pub pcm: PcmChunk,
    pub quality: SampleQuality,
}

fn cd_format() -> PcmFormat {
    PcmFormat::new(CD_SAMPLE_RATE, CD_CHANNELS).expect("CD PCM dimensions are nonzero")
}

fn validate_read_size(max_frames: usize) -> Result<(), DiscDecodeError> {
    if !(1..=MAX_DISC_READ_FRAMES).contains(&max_frames) {
        return Err(DiscDecodeError::InvalidReadSize {
            requested: max_frames,
            maximum: MAX_DISC_READ_FRAMES,
        });
    }
    Ok(())
}

fn pcm_chunk(
    start: FramePosition,
    frames: &[[i16; 2]],
) -> Result<Option<PcmChunk>, DiscDecodeError> {
    if frames.is_empty() {
        return Ok(None);
    }
    let samples = frames
        .iter()
        .flat_map(|frame| frame.iter().map(|sample| f32::from(*sample) / 32_768.0))
        .collect();
    PcmChunk::new(cd_format(), start, samples)
        .map(Some)
        .map_err(|error| DiscDecodeError::InvalidPcm(error.to_string()))
}

#[derive(Debug, Error)]
pub enum DiscDecodeError {
    #[error("disc read size {requested} must be between 1 and {maximum} frames")]
    InvalidReadSize { requested: usize, maximum: usize },
    #[error("disc reader error: {0}")]
    Reader(String),
    #[error("invalid PCM output: {0}")]
    InvalidPcm(String),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use junk_libs_disc::cue_audio::CueAudioDisc;
    use junk_libs_disc::redumper::pcm::{InvalidSamplePolicy, RawAudioReader};

    use super::*;

    #[test]
    fn redumper_adapter_preserves_exact_frames_and_quality() {
        const ORIGIN: i64 = 45_150 * 588;
        let frames = [[100_i16, -100_i16], [200, -200], [300, -300]];
        let bytes = frames
            .iter()
            .flat_map(|frame| frame.iter().flat_map(|sample| sample.to_le_bytes()))
            .collect::<Vec<_>>();
        let states = vec![4_u8; frames.len()];
        let reader = RawAudioReader::from_signed_sources(
            Box::new(Cursor::new(bytes)),
            Box::new(Cursor::new(states)),
            -ORIGIN,
            -ORIGIN + frames.len() as i64,
            InvalidSamplePolicy::Reject,
        )
        .expect("in-memory redumper reader");
        let mut decoder = RedumperPcmDecoder::new(reader);

        decoder.seek_exact(FramePosition(1)).expect("exact seek");
        let chunk = decoder.read_frames(2).expect("read").expect("two frames");
        assert_eq!(chunk.pcm.start_frame, FramePosition(1));
        assert_eq!(chunk.pcm.frame_count(), 2);
        assert_eq!(chunk.quality.verified, 2);
        assert!((chunk.pcm.samples[0] - 200.0 / 32_768.0).abs() < 0.000_001);
        assert!(decoder.read_frames(1).expect("EOF").is_none());
    }

    #[test]
    fn cue_adapter_keeps_track_boundaries_and_exact_seeks() {
        let directory = tempfile::tempdir().expect("temporary CUE fixture");
        let bin_path = directory.path().join("disc.bin");
        let frames = (0..1_176_i16)
            .flat_map(|frame| [frame, -frame])
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        std::fs::write(&bin_path, frames).expect("write BIN");
        let cue_path = directory.path().join("disc.cue");
        std::fs::write(
            &cue_path,
            "FILE \"disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:00:01\n",
        )
        .expect("write CUE");
        let disc = CueAudioDisc::open(&cue_path).expect("open CUE fixture");
        let mut decoder = CuePcmDecoder::new(disc.track_reader(2).expect("second track"));

        assert_eq!(decoder.total_frames(), 588);
        decoder.seek_exact(FramePosition(100)).expect("exact seek");
        let chunk = decoder.read_frames(1).expect("read").expect("one frame");
        assert_eq!(chunk.start_frame, FramePosition(100));
        assert!((chunk.samples[0] - 688.0 / 32_768.0).abs() < 0.000_001);
        assert!(decoder.seek_exact(FramePosition(589)).is_err());
    }
}
