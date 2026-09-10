use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::Mutex,
};

use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error as SymphoniaError,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};
use thiserror::Error;

use crate::{AudioReadSeek, ByteSource, FramePosition, PcmChunk, PcmFormat};

const MAX_FRAMES_PER_READ: usize = 65_536;

/// A qualified WAV decoder whose public coordinates are track-local PCM frames.
///
/// This decoder intentionally enables only Symphonia's WAV container and PCM
/// codec. Other formats are not advertised until they have independent vectors.
pub struct WavDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    track_start_ts: u64,
    pcm_format: PcmFormat,
    total_frames: Option<u64>,
    position: FramePosition,
    buffered: VecDeque<f32>,
}

impl WavDecoder {
    pub fn open(source: ByteSource) -> Result<Self, AudioDecodeError> {
        let (reader, media_hint) = source.into_parts();
        if let Some(extension) = media_hint.extension.as_deref()
            && !extension.eq_ignore_ascii_case("wav")
            && !extension.eq_ignore_ascii_case("wave")
        {
            return Err(AudioDecodeError::UnsupportedSource(extension.to_owned()));
        }

        let stream = MediaSourceStream::new(
            Box::new(LockedSource::new(reader, media_hint.length_bytes)),
            Default::default(),
        );
        let mut hint = Hint::new();
        if let Some(extension) = media_hint.extension.as_deref() {
            hint.with_extension(extension);
        }
        let format_reader = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(AudioDecodeError::from)?
            .format;
        let track = format_reader
            .default_track()
            .ok_or(AudioDecodeError::NoAudioTrack)?;
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or(AudioDecodeError::MissingSampleRate)?;
        let channels = track
            .codec_params
            .channels
            .ok_or(AudioDecodeError::MissingChannels)?
            .count();
        let channels = u16::try_from(channels).map_err(|_| AudioDecodeError::TooManyChannels)?;
        let pcm_format = PcmFormat::new(sample_rate, channels)
            .map_err(|error| AudioDecodeError::InvalidPcm(error.to_string()))?;

        let time_base = track
            .codec_params
            .time_base
            .ok_or(AudioDecodeError::MissingTimeBase)?;
        if u64::from(time_base.numer) * u64::from(sample_rate) != u64::from(time_base.denom) {
            return Err(AudioDecodeError::UnsupportedFrameTimeBase);
        }

        let track_id = track.id;
        let track_start_ts = track.codec_params.start_ts;
        let total_frames = track.codec_params.n_frames;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(AudioDecodeError::from)?;

        Ok(Self {
            format_reader,
            decoder,
            track_id,
            track_start_ts,
            pcm_format,
            total_frames,
            position: FramePosition(0),
            buffered: VecDeque::new(),
        })
    }

    #[must_use]
    pub fn format(&self) -> PcmFormat {
        self.pcm_format
    }

    #[must_use]
    pub fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    #[must_use]
    pub fn position(&self) -> FramePosition {
        self.position
    }

    pub fn read_frames(&mut self, max_frames: usize) -> Result<Option<PcmChunk>, AudioDecodeError> {
        if !(1..=MAX_FRAMES_PER_READ).contains(&max_frames) {
            return Err(AudioDecodeError::InvalidReadSize {
                requested: max_frames,
                maximum: MAX_FRAMES_PER_READ,
            });
        }

        let channels = usize::from(self.pcm_format.channels());
        let wanted_samples = max_frames
            .checked_mul(channels)
            .ok_or(AudioDecodeError::ReadSizeOverflow)?;
        while self.buffered.len() < wanted_samples {
            if !self.decode_packet()? {
                break;
            }
        }
        let count = wanted_samples.min(self.buffered.len());
        if count == 0 {
            return Ok(None);
        }
        let samples = self.buffered.drain(..count).collect::<Vec<_>>();
        let start = self.position;
        self.position.0 += (samples.len() / channels) as u64;
        PcmChunk::new(self.pcm_format, start, samples)
            .map(Some)
            .map_err(|error| AudioDecodeError::InvalidPcm(error.to_string()))
    }

    pub fn seek_exact(&mut self, requested: FramePosition) -> Result<(), AudioDecodeError> {
        if self.total_frames.is_some_and(|total| requested.0 > total) {
            return Err(AudioDecodeError::SeekOutOfRange {
                requested: requested.0,
                total: self.total_frames,
            });
        }
        let requested_ts = self
            .track_start_ts
            .checked_add(requested.0)
            .ok_or(AudioDecodeError::SeekTimestampOverflow)?;
        let seeked = self
            .format_reader
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: requested_ts,
                    track_id: self.track_id,
                },
            )
            .map_err(AudioDecodeError::from)?;
        if seeked.actual_ts > seeked.required_ts {
            return Err(AudioDecodeError::ExactSeekUnavailable {
                requested: requested.0,
            });
        }

        self.decoder.reset();
        self.buffered.clear();
        self.position = FramePosition(seeked.actual_ts.checked_sub(self.track_start_ts).ok_or(
            AudioDecodeError::ExactSeekUnavailable {
                requested: requested.0,
            },
        )?);
        let frames_to_discard = seeked.required_ts - seeked.actual_ts;
        self.discard_frames(frames_to_discard)?;
        if self.position != requested {
            return Err(AudioDecodeError::ExactSeekUnavailable {
                requested: requested.0,
            });
        }
        Ok(())
    }

    fn discard_frames(&mut self, mut frames: u64) -> Result<(), AudioDecodeError> {
        let channels = usize::from(self.pcm_format.channels());
        while frames > 0 {
            if self.buffered.is_empty() && !self.decode_packet()? {
                return Err(AudioDecodeError::ExactSeekUnavailable {
                    requested: self.position.0.saturating_add(frames),
                });
            }
            let available_frames = self.buffered.len() / channels;
            let discard = usize::try_from(frames)
                .unwrap_or(usize::MAX)
                .min(available_frames);
            self.buffered.drain(..discard * channels);
            self.position.0 += discard as u64;
            frames -= discard as u64;
        }
        Ok(())
    }

    fn decode_packet(&mut self) -> Result<bool, AudioDecodeError> {
        loop {
            let packet = match self.format_reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(AudioDecodeError::from(error)),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let audio = self
                .decoder
                .decode(&packet)
                .map_err(AudioDecodeError::from)?;
            if audio.spec().rate != self.pcm_format.sample_rate()
                || audio.spec().channels.count() != usize::from(self.pcm_format.channels())
            {
                return Err(AudioDecodeError::MidstreamFormatChange);
            }
            let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, *audio.spec());
            samples.copy_interleaved_ref(audio);
            self.buffered.extend(samples.samples());
            return Ok(!samples.samples().is_empty());
        }
    }
}

struct LockedSource {
    reader: Mutex<Box<dyn AudioReadSeek>>,
    length_bytes: Option<u64>,
}

impl LockedSource {
    fn new(reader: Box<dyn AudioReadSeek>, length_bytes: Option<u64>) -> Self {
        Self {
            reader: Mutex::new(reader),
            length_bytes,
        }
    }

    fn reader_mut(&mut self) -> io::Result<&mut Box<dyn AudioReadSeek>> {
        self.reader
            .get_mut()
            .map_err(|_| io::Error::other("audio source lock poisoned"))
    }
}

impl Read for LockedSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader_mut()?.read(buffer)
    }
}

impl Seek for LockedSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader_mut()?.seek(position)
    }
}

impl MediaSource for LockedSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.length_bytes
    }
}

#[derive(Debug, Error)]
pub enum AudioDecodeError {
    #[error("only WAV sources are qualified; got extension {0}")]
    UnsupportedSource(String),
    #[error("source has no audio track")]
    NoAudioTrack,
    #[error("audio track does not declare a sample rate")]
    MissingSampleRate,
    #[error("audio track does not declare channels")]
    MissingChannels,
    #[error("audio track has too many channels")]
    TooManyChannels,
    #[error("audio track does not declare a time base")]
    MissingTimeBase,
    #[error("audio time base does not map one timestamp to one PCM frame")]
    UnsupportedFrameTimeBase,
    #[error("invalid PCM format: {0}")]
    InvalidPcm(String),
    #[error("read size {requested} must be between 1 and {maximum} frames")]
    InvalidReadSize { requested: usize, maximum: usize },
    #[error("read size overflow")]
    ReadSizeOverflow,
    #[error("seek frame {requested} exceeds total frame count {total:?}")]
    SeekOutOfRange { requested: u64, total: Option<u64> },
    #[error("seek timestamp overflow")]
    SeekTimestampOverflow,
    #[error("decoder cannot position exactly at frame {requested}")]
    ExactSeekUnavailable { requested: u64 },
    #[error("midstream PCM format changes are unsupported")]
    MidstreamFormatChange,
    #[error("audio decoder error: {0}")]
    Decoder(String),
}

impl From<SymphoniaError> for AudioDecodeError {
    fn from(error: SymphoniaError) -> Self {
        Self::Decoder(error.to_string())
    }
}
