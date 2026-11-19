use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{Mutex, OnceLock},
};

use symphonia::core::{
    audio::SampleBuffer,
    codecs::{
        CODEC_TYPE_FLAC, CODEC_TYPE_MP3, CODEC_TYPE_OPUS, CodecParameters, CodecRegistry, Decoder,
        DecoderOptions,
    },
    errors::Error as SymphoniaError,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};
use thiserror::Error;

use crate::{AudioReadSeek, ByteSource, FramePosition, PcmChunk, PcmFormat};

const MAX_FRAMES_PER_READ: usize = 65_536;

/// Codecs enabled by Cargo features, with this crate's replacements registered
/// over Symphonia's defaults.
fn codec_registry() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        // Symphonia PR #528 backport; see vendor provenance.
        #[cfg(feature = "decode-compressed")]
        registry.register_all::<symphonia_bundle_mp3::MpaDecoder>();
        #[cfg(feature = "decode-opus")]
        registry.register_all::<crate::opus::OpusCodec>();
        registry
    })
}

/// How a track's frames sit on its codec timeline. Frames decoded before
/// `start_ts` (Opus pre-skip, seek pre-roll) or at/after `end_ts` (end trim)
/// are decoded but never returned.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Presentation {
    /// Timestamp of the first frame the decoder produces after opening.
    pub decode_from_ts: u64,
    /// Timestamp of track-local frame 0.
    pub start_ts: u64,
    /// Timestamp one past the final track frame, when the codec layer does
    /// not already trim to it.
    pub end_ts: Option<u64>,
    /// Frames to decode and discard before a seek target.
    pub preroll_frames: u64,
    /// Linear gain the format requires players to apply.
    pub gain: Option<f32>,
    pub delay_frames: Option<u32>,
    pub padding_frames: Option<u32>,
}

impl Presentation {
    /// Symphonia's own gapless trimming already presents exact track frames.
    fn native(params: &CodecParameters) -> Self {
        Self {
            decode_from_ts: params.start_ts,
            start_ts: params.start_ts,
            end_ts: None,
            preroll_frames: 0,
            gain: None,
            delay_frames: params.delay,
            padding_frames: params.padding,
        }
    }

    fn total_frames(&self, params: &CodecParameters) -> Option<u64> {
        match self.end_ts {
            Some(end) => end.checked_sub(self.start_ts),
            None => params.n_frames,
        }
    }
}

/// Seekable media decoder with track-local PCM frame coordinates.
/// Codec support is selected by Cargo features.
pub struct AudioDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    presentation: Presentation,
    pcm_format: PcmFormat,
    pub inspection: crate::FileAudioInfo,
    total_frames: Option<u64>,
    position: FramePosition,
    buffered: VecDeque<f32>,
    /// Codec timestamp of the next frame the decoder will produce.
    next_ts: u64,
}

impl AudioDecoder {
    pub fn open(source: ByteSource) -> Result<Self, AudioDecodeError> {
        Self::open_checked(source, &mut || Ok(()))
    }

    pub(crate) fn open_checked(
        mut source: ByteSource,
        checkpoint: &mut dyn FnMut() -> Result<(), AudioDecodeError>,
    ) -> Result<Self, AudioDecodeError> {
        checkpoint()?;
        let legacy_pictures = crate::file_audio::validate_metadata_prefix(source.reader_mut())?;
        let (payload_end, suffix) = crate::file_audio::decoder_suffix(&mut source, checkpoint)?;
        let io_error = |error: io::Error| AudioDecodeError::Decoder(error.to_string());
        // Measure rather than trust the hint: Symphonia's Ogg seeking needs a length.
        let length = source
            .reader_mut()
            .seek(SeekFrom::End(0))
            .map_err(io_error)?;
        #[cfg(feature = "decode-opus")]
        let ogg_opus =
            crate::ogg::OggOpusLayout::scan(source.reader_mut(), payload_end.unwrap_or(length))
                .map_err(io_error)?;
        #[cfg(not(feature = "decode-opus"))]
        let ogg_opus = None::<()>;
        // `LockedSource` starts its cursor at byte 0.
        source
            .reader_mut()
            .seek(SeekFrom::Start(0))
            .map_err(io_error)?;
        checkpoint()?;
        let (reader, media_hint) = source.into_parts();

        let stream = MediaSourceStream::new(
            Box::new(LockedSource::new(reader, Some(length), payload_end)),
            Default::default(),
        );
        let mut hint = Hint::new();
        if let Some(extension) = media_hint.extension.as_deref() {
            hint.with_extension(extension);
        }
        let mut probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    // Ogg Opus timing is applied by `Presentation` instead.
                    enable_gapless: ogg_opus.is_none(),
                    ..Default::default()
                },
                &MetadataOptions {
                    limit_metadata_bytes: symphonia::core::meta::Limit::Maximum(32 * 1024 * 1024),
                    limit_visual_bytes: symphonia::core::meta::Limit::Maximum(20 * 1024 * 1024),
                },
            )
            .map_err(AudioDecodeError::from)?;
        let mut inspection = crate::FileAudioInfo::default();
        if let Some(metadata) = probed.metadata.get().as_ref().and_then(|m| m.current()) {
            inspection.read_metadata(metadata)?;
        }
        let mut format_reader = probed.format;
        if let Some(metadata) = format_reader.metadata().current() {
            inspection.read_metadata(metadata)?;
        }
        if let Some(pictures) = legacy_pictures {
            inspection.pictures = pictures;
        }
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

        let codec = track.codec_params.codec;
        let presentation = match ogg_opus {
            #[cfg(feature = "decode-opus")]
            Some(layout) if codec == CODEC_TYPE_OPUS => {
                crate::opus::presentation(&layout, track, format_reader.tracks().len())?
            }
            _ if codec == CODEC_TYPE_OPUS => {
                return Err(if cfg!(feature = "decode-opus") {
                    // Only a single-link stream from byte 0 is timed exactly.
                    AudioDecodeError::UnsupportedOggLayout
                } else {
                    AudioDecodeError::UnsupportedCodec("opus (enable decode-opus)")
                });
            }
            _ => Presentation::native(&track.codec_params),
        };
        inspection.codec = match codec {
            CODEC_TYPE_FLAC => "flac",
            CODEC_TYPE_MP3 => "mp3",
            CODEC_TYPE_OPUS => "opus",
            _ => "other",
        }
        .into();
        if inspection.codec == "mp3" {
            for (key, values) in suffix.tags {
                inspection.tags.entry(key).or_insert(values);
            }
            if inspection.pictures.is_empty() {
                inspection.pictures = suffix.pictures;
            }
        }
        inspection.sample_rate = sample_rate;
        inspection.channels = channels;
        inspection.bits_per_sample = track.codec_params.bits_per_sample;
        inspection.delay_frames = presentation.delay_frames;
        inspection.padding_frames = presentation.padding_frames;
        let track_id = track.id;
        let total_frames = presentation.total_frames(&track.codec_params);
        if track
            .codec_params
            .max_frames_per_packet
            .is_some_and(|frames| frames > MAX_FRAMES_PER_READ as u64)
        {
            return Err(AudioDecodeError::Decoder(
                "declared audio packet exceeds decoder bounds".into(),
            ));
        }
        let decoder = codec_registry()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(AudioDecodeError::from)?;

        Ok(Self {
            format_reader,
            decoder,
            track_id,
            next_ts: presentation.decode_from_ts,
            presentation,
            pcm_format,
            inspection,
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
        let unavailable = AudioDecodeError::ExactSeekUnavailable {
            requested: requested.0,
        };
        let target_ts = self
            .presentation
            .start_ts
            .checked_add(requested.0)
            .ok_or(AudioDecodeError::SeekTimestampOverflow)?;
        let seek_ts = target_ts
            .saturating_sub(self.presentation.preroll_frames)
            .max(self.presentation.decode_from_ts);
        let seeked = self
            .format_reader
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts: seek_ts,
                    track_id: self.track_id,
                },
            )
            .map_err(AudioDecodeError::from)?;
        if seeked.actual_ts > seeked.required_ts {
            return Err(unavailable);
        }

        self.decoder.reset();
        self.buffered.clear();
        self.position = requested;
        self.next_ts = seeked.actual_ts;
        // Decode through the pre-roll now so an unreachable target fails here.
        if !self.decode_packet()? {
            let reached = self
                .presentation
                .end_ts
                .map_or(self.next_ts, |end| end.min(self.next_ts));
            if reached < target_ts {
                return Err(unavailable);
            }
        }
        Ok(())
    }

    /// Decode packets until one contributes frames to the presentation
    /// window. Returns `false` at the end of the track.
    fn decode_packet(&mut self) -> Result<bool, AudioDecodeError> {
        let channels = usize::from(self.pcm_format.channels());
        loop {
            if self
                .presentation
                .end_ts
                .is_some_and(|end| self.next_ts >= end)
            {
                return Ok(false);
            }
            let packet = match self.format_reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    if self.total_frames.is_some_and(|total| {
                        self.position.0 + ((self.buffered.len() / channels) as u64) < total
                    }) {
                        return Err(AudioDecodeError::Decoder(
                            "truncated audio before declared frame count".into(),
                        ));
                    }
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
                || audio.spec().channels.count() != channels
            {
                return Err(AudioDecodeError::MidstreamFormatChange);
            }
            if audio.capacity() > MAX_FRAMES_PER_READ || audio.spec().channels.count() > 32 {
                return Err(AudioDecodeError::Decoder(
                    "audio packet exceeds decoder bounds".into(),
                ));
            }
            let frames = audio.frames() as u64;
            let first_ts = self.next_ts;
            self.next_ts = first_ts
                .checked_add(frames)
                .ok_or(AudioDecodeError::SeekTimestampOverflow)?;
            let wanted_ts = self.presentation.start_ts
                + self.position.0
                + (self.buffered.len() / channels) as u64;
            let keep_from = wanted_ts.saturating_sub(first_ts).min(frames) as usize;
            let keep_to = self
                .presentation
                .end_ts
                .map_or(frames, |end| end.saturating_sub(first_ts).min(frames))
                as usize;
            if keep_from >= keep_to {
                continue;
            }
            let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, *audio.spec());
            samples.copy_interleaved_ref(audio);
            let kept = &mut samples.samples_mut()[keep_from * channels..keep_to * channels];
            if let Some(gain) = self.presentation.gain {
                crate::apply_gain(kept, gain)
                    .map_err(|error| AudioDecodeError::Decoder(error.to_string()))?;
            }
            self.buffered.extend(kept.iter());
            return Ok(true);
        }
    }
}

struct LockedSource {
    reader: Mutex<Box<dyn AudioReadSeek>>,
    length_bytes: Option<u64>,
    payload_end: Option<u64>,
    position: u64,
}

impl LockedSource {
    fn new(
        reader: Box<dyn AudioReadSeek>,
        length_bytes: Option<u64>,
        payload_end: Option<u64>,
    ) -> Self {
        Self {
            reader: Mutex::new(reader),
            length_bytes: payload_end.or(length_bytes),
            payload_end,
            position: 0,
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
        let allowed = match self.payload_end {
            Some(end) => usize::try_from(
                end.checked_sub(self.position)
                    .ok_or_else(|| io::Error::other("audio cursor exceeds payload"))?
                    .min(buffer.len() as u64),
            )
            .unwrap(),
            None => buffer.len(),
        };
        let count = self.reader_mut()?.read(&mut buffer[..allowed])?;
        if count > allowed {
            return Err(io::Error::other("audio reader exceeded request"));
        }
        self.position = self
            .position
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("audio cursor overflow"))?;
        Ok(count)
    }
}

impl Seek for LockedSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = if let Some(end) = self.payload_end {
            let offset = match position {
                SeekFrom::Start(n) => i128::from(n),
                SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
                SeekFrom::End(n) => i128::from(end) + i128::from(n),
            };
            if offset < 0 || offset > i128::from(end) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "seek exceeds audio payload",
                ));
            }
            SeekFrom::Start(offset as u64)
        } else {
            position
        };
        let actual = self.reader_mut()?.seek(target)?;
        if let Some(end) = self.payload_end {
            if actual > end {
                return Err(io::Error::other("audio seek exceeds payload"));
            }
            if let SeekFrom::Start(expected) = target
                && actual != expected
            {
                return Err(io::Error::other(
                    "audio seek did not reach requested position",
                ));
            }
        }
        self.position = actual;
        Ok(actual)
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
    #[error("audio processing cancelled")]
    Cancelled,
    #[error("unsupported source extension {0}")]
    UnsupportedSource(String),
    #[error("unsupported codec {0}")]
    UnsupportedCodec(&'static str),
    #[error("chained, multiplexed or offset Ogg streams are unsupported")]
    UnsupportedOggLayout,
    #[error("Opus channel mapping family {0} is unsupported; only family 0 (mono/stereo) is")]
    UnsupportedOpusChannelMapping(u8),
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
        match error {
            // Symphonia's Ogg reader signals a new chain link (BOS page) this way.
            SymphoniaError::ResetRequired => Self::UnsupportedOggLayout,
            error => Self::Decoder(error.to_string()),
        }
    }
}
