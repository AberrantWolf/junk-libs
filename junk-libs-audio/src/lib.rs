//! Headless byte-source and PCM coordinate contracts.
//!
//! AccurateRip and other preservation calculations intentionally do not use
//! this floating-point playback boundary; they remain on integer CD PCM.

use std::{
    io::{Read, Seek},
    num::{NonZeroU16, NonZeroU32},
};

use thiserror::Error;

#[cfg(feature = "decode-wav")]
mod wav;

#[cfg(feature = "decode-wav")]
pub use wav::WavDecoder;

#[cfg(any(feature = "decode-wav", feature = "decode-compressed"))]
mod media;
#[cfg(any(feature = "decode-wav", feature = "decode-compressed"))]
pub use media::{AudioDecodeError, AudioDecoder};

#[cfg(feature = "disc-readers")]
mod disc;

#[cfg(feature = "disc-readers")]
pub use disc::{CuePcmDecoder, DiscDecodeError, RedumperPcmChunk, RedumperPcmDecoder};

mod dsp;
pub use dsp::{DspError, apply_gain, hard_clip};

#[cfg(feature = "resample")]
mod resample;

#[cfg(feature = "resample")]
pub use resample::{ResampleError, SincResampler};

pub trait AudioReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send> AudioReadSeek for T {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaHint {
    pub length_bytes: Option<u64>,
    pub extension: Option<String>,
    pub media_type: Option<String>,
}

impl MediaHint {
    #[must_use]
    pub fn new(
        length_bytes: Option<u64>,
        extension: Option<&str>,
        media_type: Option<&str>,
    ) -> Self {
        Self {
            length_bytes,
            extension: extension.map(str::to_owned),
            media_type: media_type.map(str::to_owned),
        }
    }
}

pub struct ByteSource {
    reader: Box<dyn AudioReadSeek>,
    hint: MediaHint,
}

impl ByteSource {
    #[must_use]
    pub fn new(reader: impl AudioReadSeek + 'static, hint: MediaHint) -> Self {
        Self {
            reader: Box::new(reader),
            hint,
        }
    }

    pub fn reader_mut(&mut self) -> &mut dyn AudioReadSeek {
        self.reader.as_mut()
    }

    #[must_use]
    pub fn hint(&self) -> &MediaHint {
        &self.hint
    }

    pub fn into_parts(self) -> (Box<dyn AudioReadSeek>, MediaHint) {
        (self.reader, self.hint)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FramePosition(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    sample_rate: NonZeroU32,
    channels: NonZeroU16,
}

impl PcmFormat {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioContractError> {
        Ok(Self {
            sample_rate: NonZeroU32::new(sample_rate).ok_or(AudioContractError::ZeroSampleRate)?,
            channels: NonZeroU16::new(channels).ok_or(AudioContractError::ZeroChannels)?,
        })
    }

    #[must_use]
    pub fn sample_rate(self) -> u32 {
        self.sample_rate.get()
    }

    #[must_use]
    pub fn channels(self) -> u16 {
        self.channels.get()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PcmChunk {
    pub format: PcmFormat,
    pub start_frame: FramePosition,
    pub samples: Vec<f32>,
}

impl PcmChunk {
    pub fn new(
        format: PcmFormat,
        start_frame: FramePosition,
        samples: Vec<f32>,
    ) -> Result<Self, AudioContractError> {
        if !samples.len().is_multiple_of(usize::from(format.channels())) {
            return Err(AudioContractError::IncompleteFrame {
                samples: samples.len(),
                channels: format.channels(),
            });
        }
        Ok(Self {
            format,
            start_frame,
            samples,
        })
    }

    #[must_use]
    pub fn frame_count(&self) -> u64 {
        (self.samples.len() / usize::from(self.format.channels())) as u64
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AudioContractError {
    #[error("sample rate must be non-zero")]
    ZeroSampleRate,
    #[error("channel count must be non-zero")]
    ZeroChannels,
    #[error("{samples} interleaved samples do not form complete {channels}-channel frames")]
    IncompleteFrame { samples: usize, channels: u16 },
}

#[cfg(all(test, feature = "decode-wav"))]
mod decoder_contract_tests {
    use std::io::Cursor;

    use super::{ByteSource, FramePosition, MediaHint, WavDecoder};

    fn stereo_wav(frames: &[[i16; 2]], sample_rate: u32) -> Vec<u8> {
        let data_len = (frames.len() * 4) as u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for frame in frames {
            bytes.extend_from_slice(&frame[0].to_le_bytes());
            bytes.extend_from_slice(&frame[1].to_le_bytes());
        }
        bytes
    }

    #[test]
    fn wav_decode_and_seek_use_exact_track_local_frames() {
        let frames = (0..32)
            .map(|frame| [frame * 100, -(frame * 100)])
            .collect::<Vec<_>>();
        let bytes = stereo_wav(&frames, 8_000);
        let source = ByteSource::new(
            Cursor::new(bytes.clone()),
            MediaHint::new(Some(bytes.len() as u64), Some("wav"), Some("audio/wav")),
        );

        let mut decoder = WavDecoder::open(source).expect("open WAV");
        assert_eq!(decoder.format().sample_rate(), 8_000);
        assert_eq!(decoder.format().channels(), 2);
        assert_eq!(decoder.total_frames(), Some(32));

        let first = decoder.read_frames(3).expect("decode").expect("frames");
        assert_eq!(first.start_frame, FramePosition(0));
        assert_eq!(first.frame_count(), 3);

        decoder
            .seek_exact(FramePosition(17))
            .expect("exact frame seek");
        assert_eq!(decoder.position(), FramePosition(17));
        let sought = decoder.read_frames(2).expect("decode").expect("frames");
        assert_eq!(sought.start_frame, FramePosition(17));
        assert!((sought.samples[0] - (1_700.0 / 32_768.0)).abs() < 0.000_001);
        assert!((sought.samples[1] - (-1_700.0 / 32_768.0)).abs() < 0.000_001);

        decoder
            .seek_exact(FramePosition(32))
            .expect("exact EOF seek");
        assert!(decoder.read_frames(1).expect("read EOF").is_none());
    }

    #[test]
    fn decoder_rejects_non_wav_and_out_of_range_or_empty_requests() {
        let bytes = stereo_wav(&[[1, -1]; 4], 44_100);
        let source = ByteSource::new(
            Cursor::new(bytes.clone()),
            MediaHint::new(Some(bytes.len() as u64), Some("mp3"), Some("audio/mpeg")),
        );
        assert!(WavDecoder::open(source).is_err());

        let source = ByteSource::new(
            Cursor::new(bytes),
            MediaHint::new(None, Some("wav"), Some("audio/wav")),
        );
        let mut decoder = WavDecoder::open(source).expect("open WAV");
        assert!(decoder.read_frames(0).is_err());
        assert!(decoder.seek_exact(FramePosition(5)).is_err());
    }
}

#[cfg(feature = "derivative-validation")]
pub mod validation;

#[cfg(all(feature = "decode-compressed", feature = "resample"))]
mod sequence;
#[cfg(all(feature = "decode-compressed", feature = "resample"))]
pub use sequence::{SequenceSource, decode_sequence};
