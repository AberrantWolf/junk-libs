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
    pub analyzed_frames: u64,
    pub duration_ms: u64,
    pub fingerprint: String,
}
impl AudioFingerprint {
    /// Short tracks can have valid computed coordinates but no searchable fingerprint.
    pub fn usable(&self) -> bool {
        chromaprint::decode_fingerprint(&self.fingerprint).is_ok_and(|(raw, _)| !raw.is_empty())
    }

    pub fn validate(&self) -> Result<()> {
        if self.algorithm != 1
            || self.implementation != IMPLEMENTATION
            || !(8000..=192000).contains(&self.sample_rate)
            || !matches!(self.channels, 1 | 2)
            || self.decoded_frames == 0
            || self.decoded_frames > u64::from(self.sample_rate) * MAX_SECONDS
            || self.analyzed_frames
                != self
                    .decoded_frames
                    .min(u64::from(self.sample_rate) * MAX_FINGERPRINT_SECONDS)
            || self.duration_ms != self.decoded_frames * 1000 / u64::from(self.sample_rate)
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
    let mut printer = chromaprint::Fingerprinter::new(chromaprint::Algorithm::Test2);
    printer
        .start(rate, channels)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let expected_frames = decoder.total_frames();
    let mut frames = 0u64;
    let cap = u64::from(rate) * MAX_FINGERPRINT_SECONDS;
    loop {
        checkpoint()?;
        let Some(chunk) = decoder
            .read_frames(4096)
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
    if expected_frames.is_some_and(|expected| expected != frames) {
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
        analyzed_frames: frames.min(cap),
        duration_ms: frames * 1000 / u64::from(rate),
        fingerprint: printer.encode(),
    };
    result.validate()?;
    checkpoint()?;
    Ok(result)
}
