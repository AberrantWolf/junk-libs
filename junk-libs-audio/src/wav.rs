use crate::{AudioDecodeError, AudioDecoder, ByteSource, FramePosition, PcmChunk, PcmFormat};

/// WAV-only entry point retaining its explicit source contract.
pub struct WavDecoder(AudioDecoder);
impl WavDecoder {
    pub fn open(source: ByteSource) -> Result<Self, AudioDecodeError> {
        if let Some(ext) = source.hint().extension.as_deref()
            && !ext.eq_ignore_ascii_case("wav")
            && !ext.eq_ignore_ascii_case("wave")
        {
            return Err(AudioDecodeError::UnsupportedSource(ext.into()));
        }
        AudioDecoder::open(source).map(Self)
    }
    pub fn format(&self) -> PcmFormat {
        self.0.format()
    }
    pub fn total_frames(&self) -> Option<u64> {
        self.0.total_frames()
    }
    pub fn position(&self) -> FramePosition {
        self.0.position()
    }
    pub fn read_frames(&mut self, max: usize) -> Result<Option<PcmChunk>, AudioDecodeError> {
        self.0.read_frames(max)
    }
    pub fn seek_exact(&mut self, frame: FramePosition) -> Result<(), AudioDecodeError> {
        self.0.seek_exact(frame)
    }
}
