use std::collections::VecDeque;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use thiserror::Error;

use crate::{FramePosition, PcmChunk, PcmFormat};

const MAX_CHUNK_FRAMES: usize = 16_384;
const MAX_READ_FRAMES: usize = 65_536;
const MAX_FLUSH_BLOCKS: usize = 128;

/// A bounded streaming sinc resampler with exact final output length.
///
/// Input chunks must be contiguous in source-frame coordinates. Output chunks
/// use a zero-based output-frame coordinate; queue/seek policy remains in the
/// playback service and never derives a source seek from resampled floats.
pub struct SincResampler {
    inner: SincFixedIn<f32>,
    input_format: PcmFormat,
    output_format: PcmFormat,
    chunk_frames: usize,
    input: VecDeque<f32>,
    ready: VecDeque<f32>,
    next_input_frame: u64,
    input_frames_received: u64,
    output_frames_emitted: u64,
    finished: bool,
    flush_blocks: usize,
}

impl SincResampler {
    pub fn new(
        input_format: PcmFormat,
        output_rate: u32,
        chunk_frames: usize,
    ) -> Result<Self, ResampleError> {
        if !(1..=MAX_CHUNK_FRAMES).contains(&chunk_frames) {
            return Err(ResampleError::InvalidChunkSize {
                requested: chunk_frames,
                maximum: MAX_CHUNK_FRAMES,
            });
        }
        let rate = input_format.sample_rate();
        if !(8000..=384000).contains(&rate)
            || !(8000..=384000).contains(&output_rate)
            || input_format.channels() > 8
            || u64::from(output_rate) * chunk_frames as u64 / u64::from(rate) > 65_536
        {
            return Err(ResampleError::InvalidFormat(
                "resampling exceeds supported rate/channel/buffer bounds".into(),
            ));
        }
        let output_format = PcmFormat::new(output_rate, input_format.channels())
            .map_err(|error| ResampleError::InvalidFormat(error.to_string()))?;
        let inner = SincFixedIn::new(
            f64::from(output_rate) / f64::from(input_format.sample_rate()),
            1.0,
            SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 128,
                window: WindowFunction::BlackmanHarris2,
            },
            chunk_frames,
            usize::from(input_format.channels()),
        )
        .map_err(|error| ResampleError::Engine(error.to_string()))?;
        Ok(Self {
            inner,
            input_format,
            output_format,
            chunk_frames,
            input: VecDeque::new(),
            ready: VecDeque::new(),
            next_input_frame: 0,
            input_frames_received: 0,
            output_frames_emitted: 0,
            finished: false,
            flush_blocks: 0,
        })
    }

    #[must_use]
    pub fn output_format(&self) -> PcmFormat {
        self.output_format
    }

    pub fn push(&mut self, chunk: PcmChunk) -> Result<(), ResampleError> {
        if self.finished {
            return Err(ResampleError::AlreadyFinished);
        }
        if chunk.format != self.input_format {
            return Err(ResampleError::FormatChanged);
        }
        if chunk.start_frame.0 != self.next_input_frame {
            return Err(ResampleError::NonContiguous {
                expected: self.next_input_frame,
                actual: chunk.start_frame.0,
            });
        }
        let channels = usize::from(self.input_format.channels());
        let buffered_frames = self.input.len() / channels;
        let incoming_frames = chunk.samples.len() / channels;
        let maximum = self.chunk_frames * 2;
        if buffered_frames + incoming_frames > maximum {
            return Err(ResampleError::InputBufferFull {
                buffered: buffered_frames,
                incoming: incoming_frames,
                maximum,
            });
        }
        self.next_input_frame += incoming_frames as u64;
        self.input_frames_received += incoming_frames as u64;
        self.input.extend(chunk.samples);
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), ResampleError> {
        if self.finished {
            return Err(ResampleError::AlreadyFinished);
        }
        self.finished = true;
        Ok(())
    }

    pub fn read_frames(&mut self, max_frames: usize) -> Result<Option<PcmChunk>, ResampleError> {
        if !(1..=MAX_READ_FRAMES).contains(&max_frames) {
            return Err(ResampleError::InvalidReadSize {
                requested: max_frames,
                maximum: MAX_READ_FRAMES,
            });
        }
        let channels = usize::from(self.output_format.channels());
        let final_remaining = self.finished.then(|| {
            self.target_output_frames()
                .saturating_sub(self.output_frames_emitted)
        });
        if final_remaining == Some(0) {
            self.ready.clear();
            return Ok(None);
        }
        let wanted = final_remaining
            .map(|remaining| remaining.min(max_frames as u64) as usize)
            .unwrap_or(max_frames);
        self.pump(wanted)?;
        let available = self.ready.len() / channels;
        let count = available.min(wanted);
        if count == 0 {
            return Ok(None);
        }
        let samples = self.ready.drain(..count * channels).collect();
        let start_frame = FramePosition(self.output_frames_emitted);
        self.output_frames_emitted += count as u64;
        PcmChunk::new(self.output_format, start_frame, samples)
            .map(Some)
            .map_err(|error| ResampleError::InvalidFormat(error.to_string()))
    }

    fn pump(&mut self, wanted_frames: usize) -> Result<(), ResampleError> {
        let channels = usize::from(self.input_format.channels());
        while self.ready.len() / channels < wanted_frames {
            let input_frames = self.input.len() / channels;
            if input_frames >= self.chunk_frames {
                self.process_block(self.chunk_frames)?;
            } else if self.finished {
                if self.flush_blocks >= MAX_FLUSH_BLOCKS {
                    return Err(ResampleError::FlushLimit);
                }
                if self.output_frames_emitted + (self.ready.len() / channels) as u64
                    >= self.target_output_frames()
                {
                    break;
                }
                self.process_block(input_frames)?;
                self.flush_blocks += 1;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn process_block(&mut self, real_frames: usize) -> Result<(), ResampleError> {
        let channels = usize::from(self.input_format.channels());
        let mut planar = vec![vec![0.0_f32; self.chunk_frames]; channels];
        for frame in 0..real_frames {
            for channel in &mut planar {
                channel[frame] = self
                    .input
                    .pop_front()
                    .expect("buffered frame count was checked");
            }
        }
        let output = self
            .inner
            .process(&planar, None)
            .map_err(|error| ResampleError::Engine(error.to_string()))?;
        let output_frames = output.first().map(Vec::len).unwrap_or(0);
        for frame in 0..output_frames {
            for channel in &output {
                self.ready.push_back(channel[frame]);
            }
        }
        Ok(())
    }

    fn target_output_frames(&self) -> u64 {
        ((self.input_frames_received as f64 * f64::from(self.output_format.sample_rate())
            / f64::from(self.input_format.sample_rate()))
        .round()) as u64
    }
}

#[derive(Debug, Error)]
pub enum ResampleError {
    #[error("resampler chunk size {requested} must be between 1 and {maximum} frames")]
    InvalidChunkSize { requested: usize, maximum: usize },
    #[error("resampler read size {requested} must be between 1 and {maximum} frames")]
    InvalidReadSize { requested: usize, maximum: usize },
    #[error("invalid resampler format: {0}")]
    InvalidFormat(String),
    #[error("PCM format changed within one resampling stream")]
    FormatChanged,
    #[error("non-contiguous input: expected frame {expected}, got {actual}")]
    NonContiguous { expected: u64, actual: u64 },
    #[error(
        "input buffer would exceed {maximum} frames ({buffered} buffered + {incoming} incoming)"
    )]
    InputBufferFull {
        buffered: usize,
        incoming: usize,
        maximum: usize,
    },
    #[error("resampling stream is already finished")]
    AlreadyFinished,
    #[error("resampler flush exceeded its bounded block limit")]
    FlushLimit,
    #[error("resampler engine error: {0}")]
    Engine(String),
}
