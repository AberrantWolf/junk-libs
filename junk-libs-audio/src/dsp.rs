use thiserror::Error;

/// Apply an explicit linear gain without silently clipping and return the peak.
pub fn apply_gain(samples: &mut [f32], linear_gain: f32) -> Result<f32, DspError> {
    if !linear_gain.is_finite() || linear_gain < 0.0 {
        return Err(DspError::InvalidGain(linear_gain));
    }
    let mut peak = 0.0_f32;
    for sample in samples {
        *sample *= linear_gain;
        peak = peak.max(sample.abs());
    }
    Ok(peak)
}

/// Clamp samples symmetrically and return the number which were changed.
pub fn hard_clip(samples: &mut [f32], limit: f32) -> Result<usize, DspError> {
    if !limit.is_finite() || limit <= 0.0 {
        return Err(DspError::InvalidClipLimit(limit));
    }
    let mut clipped = 0;
    for sample in samples {
        let bounded = sample.clamp(-limit, limit);
        if bounded != *sample {
            clipped += 1;
            *sample = bounded;
        }
    }
    Ok(clipped)
}

#[derive(Debug, Error, Clone, Copy, PartialEq)]
pub enum DspError {
    #[error("linear gain must be finite and non-negative; got {0}")]
    InvalidGain(f32),
    #[error("clip limit must be finite and positive; got {0}")]
    InvalidClipLimit(f32),
}
