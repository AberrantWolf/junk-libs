//! Contiguous stereo playback conversion over lazy authorized byte sources.
use crate::{
    AudioDecodeError, AudioDecoder, ByteSource, FramePosition, PcmChunk, PcmFormat, SincResampler,
};

pub struct SequenceSource {
    pub bytes: ByteSource,
    /// Recorded duration in its original PCM clock, independent of encoded rate.
    pub expected_duration: Option<(u64, u32)>,
}
fn cancelled(check: &dyn Fn() -> bool) -> Result<(), AudioDecodeError> {
    if check() {
        Err(AudioDecodeError::Cancelled)
    } else {
        Ok(())
    }
}
fn drain(
    resampler: &mut SincResampler,
    check: &dyn Fn() -> bool,
    emit: &mut impl FnMut(PcmChunk) -> Result<(), AudioDecodeError>,
) -> Result<(), AudioDecodeError> {
    loop {
        cancelled(check)?;
        let Some(chunk) = resampler
            .read_frames(1024)
            .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?
        else {
            return Ok(());
        };
        cancelled(check)?;
        emit(chunk)?;
    }
}
/// Decode on a worker. Compatible adjacent sources share a resampler and its
/// fractional phase; a format change drains exactly before starting another.
/// Sources and sinks remain caller-owned, bounded and cancellable adapters.
pub fn decode_sequence(
    sources: impl IntoIterator<Item = Result<SequenceSource, AudioDecodeError>>,
    output_rate: u32,
    skip_output_frames: u64,
    check: &dyn Fn() -> bool,
    mut emit: impl FnMut(PcmChunk) -> Result<(), AudioDecodeError>,
) -> Result<(), AudioDecodeError> {
    let mut resampler: Option<SincResampler> = None;
    let mut input_offset = 0;
    let mut prior_format = None;
    for (index, source) in sources.into_iter().enumerate() {
        cancelled(check)?;
        let source = source?;
        let mut decoder = AudioDecoder::open(source.bytes)?;
        cancelled(check)?;
        let original = decoder.format();
        if original.channels() > 2 {
            return Err(AudioDecodeError::InvalidPcm(
                "stereo or mono audio required".into(),
            ));
        }
        if let Some((frames, rate)) = source.expected_duration {
            if rate == 0 {
                return Err(AudioDecodeError::InvalidPcm("zero source clock".into()));
            }
            let expected = (u128::from(frames) * u128::from(original.sample_rate())
                + u128::from(rate) / 2)
                / u128::from(rate);
            if decoder.total_frames().map(u128::from) != Some(expected) {
                return Err(AudioDecodeError::InvalidPcm(
                    "encoded audio differs from recorded duration".into(),
                ));
            }
        }
        let skip = if index == 0 {
            u64::try_from(
                u128::from(skip_output_frames) * u128::from(original.sample_rate())
                    / u128::from(output_rate.max(1)),
            )
            .map_err(|_| AudioDecodeError::SeekTimestampOverflow)?
        } else {
            0
        };
        if skip > 0 {
            decoder.seek_exact(FramePosition(skip))?;
        }
        let stereo = PcmFormat::new(original.sample_rate(), 2)
            .map_err(|e| AudioDecodeError::InvalidPcm(e.to_string()))?;
        if prior_format != Some(stereo) {
            if let Some(mut prior) = resampler.take() {
                prior
                    .finish()
                    .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?;
                drain(&mut prior, check, &mut emit)?;
            }
            cancelled(check)?;
            resampler = Some(
                SincResampler::new(stereo, output_rate, 1024)
                    .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?,
            );
            input_offset = 0;
            prior_format = Some(stereo);
        }
        let resampler = resampler.as_mut().ok_or(AudioDecodeError::NoAudioTrack)?;
        loop {
            cancelled(check)?;
            let Some(chunk) = decoder.read_frames(1024)? else {
                break;
            };
            cancelled(check)?;
            let channels = usize::from(chunk.format.channels());
            let samples = chunk
                .samples
                .chunks_exact(channels)
                .flat_map(|v| [v[0], v.get(1).copied().unwrap_or(v[0])])
                .collect::<Vec<_>>();
            let count = samples.len() as u64 / 2;
            let chunk = PcmChunk::new(stereo, FramePosition(input_offset), samples)
                .map_err(|e| AudioDecodeError::InvalidPcm(e.to_string()))?;
            input_offset = input_offset
                .checked_add(count)
                .ok_or(AudioDecodeError::SeekTimestampOverflow)?;
            resampler
                .push(chunk)
                .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?;
            drain(resampler, check, &mut emit)?;
        }
    }
    if let Some(mut resampler) = resampler {
        resampler
            .finish()
            .map_err(|e| AudioDecodeError::Decoder(e.to_string()))?;
        drain(&mut resampler, check, &mut emit)?;
    }
    cancelled(check)
}
