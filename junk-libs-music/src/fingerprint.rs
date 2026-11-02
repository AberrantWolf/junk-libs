//! Chromaprint Test2 over the first 120 s; duration covers the complete decoded file.
use crate::{Error, Result};
use junk_libs_audio::AudioDecoder;
pub use junk_libs_audio::{ByteSource, MediaHint};
use serde::{Deserialize, Serialize};
pub const IMPLEMENTATION: &str = "chromaprint-next/7a384eee09caf17b4f6241f481dd784c2e937d9a";
pub const MAX_FINGERPRINT_SECONDS: u64 = 120;
const MAX_SECONDS: u64 = 24 * 60 * 60;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioFingerprint {
    pub algorithm: u8,
    pub implementation: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub decoded_frames: u64,
    /// Recorded full duration when only a prefix was decoded. None means a full decode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_frames: Option<u64>,
    pub analyzed_frames: u64,
    pub duration_ms: u64,
    pub fingerprint: String,
}
impl AudioFingerprint {
    /// Short tracks can have valid computed coordinates but no searchable fingerprint.
    pub fn usable(&self) -> bool {
        self.validate().is_ok()
            && chromaprint::decode_fingerprint(&self.fingerprint)
                .is_ok_and(|(raw, _)| !raw.is_empty())
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames.unwrap_or(self.decoded_frames)
    }

    pub fn validate(&self) -> Result<()> {
        if self.algorithm != 1
            || self.implementation != IMPLEMENTATION
            || !(8000..=192000).contains(&self.sample_rate)
            || !matches!(self.channels, 1 | 2)
            || self.decoded_frames == 0
            || self.total_frames() > u64::from(self.sample_rate) * MAX_SECONDS
            || self.total_frames.is_some_and(|n| {
                self.decoded_frames != n.min(u64::from(self.sample_rate) * MAX_FINGERPRINT_SECONDS)
            })
            || self.analyzed_frames
                != self
                    .decoded_frames
                    .min(u64::from(self.sample_rate) * MAX_FINGERPRINT_SECONDS)
            || self.duration_ms != self.total_frames() * 1000 / u64::from(self.sample_rate)
            || self.fingerprint.len() > 65536
            || self.fingerprint.is_empty()
        {
            return Err(Error::Invalid("fingerprint envelope".into()));
        }
        let (_raw, algorithm) = chromaprint::decode_fingerprint(&self.fingerprint)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if algorithm != chromaprint::Algorithm::Test2 {
            return Err(Error::Invalid("empty/unsupported fingerprint".into()));
        }
        Ok(())
    }
}
/// The reader must enforce byte bounds/cancellation during I/O; this function also
/// checks cancellation between bounded decode and CPU chunks. It never writes tags.
pub fn compute(
    source: ByteSource,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<AudioFingerprint> {
    compute_inner(source, None, checkpoint)
}

/// Decode at most the fingerprint window, using a duration from a previously
/// inspected, digest-bound source. The caller must validate the consumed source.
pub fn compute_window(
    source: ByteSource,
    total_frames: u64,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<AudioFingerprint> {
    if total_frames == 0 {
        return Err(Error::Invalid("empty recorded duration".into()));
    }
    compute_inner(source, Some(total_frames), checkpoint)
}

fn compute_inner(
    source: ByteSource,
    recorded: Option<u64>,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<AudioFingerprint> {
    checkpoint()?;
    let mut decoder = AudioDecoder::open(source).map_err(|e| Error::Invalid(e.to_string()))?;
    let format = decoder.format();
    let rate = format.sample_rate();
    let channels = format.channels();
    if !(8000..=192000).contains(&rate)
        || !matches!(channels, 1 | 2)
        || !matches!(decoder.inspection.codec.as_str(), "flac" | "mp3")
    {
        return Err(Error::Invalid(
            "fingerprinting requires supported mono/stereo FLAC or MP3".into(),
        ));
    }
    if recorded.is_some_and(|n| n > u64::from(rate) * MAX_SECONDS) {
        return Err(Error::Invalid("recorded duration exceeds 24 hours".into()));
    }
    let mut printer = chromaprint::Fingerprinter::new(chromaprint::Algorithm::Test2);
    printer
        .start(rate, channels)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let expected_frames = decoder.total_frames();
    if let (Some(recorded), Some(header)) = (recorded, expected_frames)
        && recorded != header
    {
        return Err(Error::Invalid(
            "recorded duration disagrees with header".into(),
        ));
    }
    let mut frames = 0u64;
    let cap = u64::from(rate) * MAX_FINGERPRINT_SECONDS;
    loop {
        checkpoint()?;
        let target = recorded.map(|n| n.min(cap));
        if target.is_some_and(|n| frames >= n) {
            break;
        }
        let wanted = target.map_or(4096, |n| (n - frames).min(4096) as usize);
        let Some(chunk) = decoder
            .read_frames(wanted)
            .map_err(|e| Error::Invalid(e.to_string()))?
        else {
            break;
        };
        checkpoint()?;
        if chunk.start_frame.0 != frames || chunk.format != format {
            return Err(Error::Invalid("discontinuous PCM".into()));
        }
        let take =
            chunk.frame_count().min(cap.saturating_sub(frames)) as usize * usize::from(channels);
        let mut samples = Vec::with_capacity(take);
        for &sample in &chunk.samples[..take] {
            if !sample.is_finite() {
                return Err(Error::Invalid("nonfinite PCM".into()));
            }
            // Saturating round-to-nearest conversion, including the negative full-scale endpoint.
            samples.push((f64::from(sample) * 32768.).round().clamp(-32768., 32767.) as i16);
        }
        printer
            .feed(&samples)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        frames = frames
            .checked_add(chunk.frame_count())
            .ok_or_else(|| Error::Invalid("frame overflow".into()))?;
        if frames > u64::from(rate) * MAX_SECONDS {
            return Err(Error::Invalid("audio exceeds 24 hours".into()));
        }
    }
    checkpoint()?;
    if recorded
        .map(|n| n.min(cap))
        .or(expected_frames)
        .is_some_and(|expected| expected != frames)
    {
        return Err(Error::Invalid(
            "decoded length disagrees with the stream header".into(),
        ));
    }
    printer
        .finish()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let result = AudioFingerprint {
        algorithm: 1,
        implementation: IMPLEMENTATION.into(),
        sample_rate: rate,
        channels,
        decoded_frames: frames,
        total_frames: recorded,
        analyzed_frames: frames.min(cap),
        duration_ms: recorded.unwrap_or(frames) * 1000 / u64::from(rate),
        fingerprint: printer.encode(),
    };
    result.validate()?;
    checkpoint()?;
    Ok(result)
}
