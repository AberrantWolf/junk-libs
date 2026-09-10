//! Headless byte-source and PCM coordinate contracts.
//!
//! AccurateRip and other preservation calculations intentionally do not use
//! this floating-point playback boundary; they remain on integer CD PCM.

use std::{
    io::{Read, Seek},
    num::{NonZeroU16, NonZeroU32},
};

use thiserror::Error;

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
