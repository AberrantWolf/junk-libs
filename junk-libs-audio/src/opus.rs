//! Ogg Opus decoding through libopus with RFC 7845 timing applied by
//! `AudioDecoder`.

use symphonia::core::{
    audio::AudioBufferRef,
    codecs::{
        CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, Decoder, DecoderOptions, FinalizeResult,
    },
    errors::Result as SymphoniaResult,
    formats::{Packet, Track},
    support_codec,
};
use symphonia_adapter_libopus::OpusDecoder;

use crate::{AudioDecodeError, media::Presentation, ogg::OggOpusLayout};

/// Frames to decode and discard before a seek target so the decoder state
/// converges (RFC 7845 §4.6: at least 80 ms at 48 kHz).
const SEEK_PREROLL_FRAMES: u64 = 3_840;

/// Pins the adapter to plain packet decoding across its 0.2.x releases, so
/// `Presentation` is the only place Ogg Opus timing is applied:
/// - 0.2.9 trims pre-skip from the first packet it decodes (after open, and
///   again after a recreate), so it never sees the `OpusHead` extra data;
/// - 0.2.0's `reset` keeps libopus state, so `reset` recreates the decoder and
///   seek pre-roll starts from the same clean state as the stream start.
pub(crate) struct OpusCodec {
    params: CodecParameters,
    inner: OpusDecoder,
}

impl OpusCodec {
    fn make_inner(params: &CodecParameters) -> SymphoniaResult<OpusDecoder> {
        let mut plain = params.clone();
        plain.extra_data = None;
        OpusDecoder::try_new(&plain, &DecoderOptions::default())
    }
}

impl Decoder for OpusCodec {
    fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> SymphoniaResult<Self> {
        Ok(Self {
            params: params.clone(),
            inner: Self::make_inner(params)?,
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        // Descriptors carry their constructor, so the adapter's would bypass this wrapper.
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus (libopus)")]
    }

    fn reset(&mut self) {
        // Creation only depends on parameters that already succeeded once.
        if let Ok(fresh) = Self::make_inner(&self.params) {
            self.inner = fresh;
        }
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> SymphoniaResult<AudioBufferRef<'_>> {
        self.inner.decode(packet)
    }

    fn finalize(&mut self) -> FinalizeResult {
        self.inner.finalize()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.inner.last_decoded()
    }
}

/// Map a single-link Ogg Opus stream onto track-local frames.
///
/// Symphonia's Ogg reader must run with gapless trimming disabled: it does not
/// apply pre-skip, and on single-page streams it trims the start instead of
/// the end. Packet timestamps are then raw granule positions, which this window
/// interprets per RFC 7845 §4.
pub(crate) fn presentation(
    layout: &OggOpusLayout,
    track: &Track,
    track_count: usize,
) -> Result<Presentation, AudioDecodeError> {
    if track_count != 1 || track.id != layout.serial || layout.last_page.serial != layout.serial {
        return Err(AudioDecodeError::UnsupportedOggLayout);
    }
    if layout.head.mapping_family != 0 {
        return Err(AudioDecodeError::UnsupportedOpusChannelMapping(
            layout.head.mapping_family,
        ));
    }
    let invalid = |message: &str| AudioDecodeError::Decoder(message.into());
    if layout.last_page.granule == crate::ogg::PageHeader::NO_GRANULE {
        return Err(invalid("final Ogg Opus page has no granule position"));
    }
    let decode_from_ts = track.codec_params.start_ts;
    let start_ts = decode_from_ts
        .checked_add(u64::from(layout.head.pre_skip))
        .ok_or_else(|| invalid("Opus pre-skip overflows the timeline"))?;
    if layout.last_page.granule < start_ts {
        return Err(invalid("Opus final granule precedes pre-skip"));
    }
    let gain = layout.head.linear_gain();
    Ok(Presentation {
        decode_from_ts,
        start_ts,
        end_ts: Some(layout.last_page.granule),
        preroll_frames: SEEK_PREROLL_FRAMES,
        gain: (gain != 1.0).then_some(gain),
        delay_frames: Some(u32::from(layout.head.pre_skip)),
        padding_frames: None,
    })
}
