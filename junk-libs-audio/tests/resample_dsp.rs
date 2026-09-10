#![cfg(feature = "resample")]

use junk_libs_audio::{
    FramePosition, PcmFormat, ResampleError, SincResampler, apply_gain, hard_clip,
};

#[test]
fn resampler_is_bounded_contiguous_and_has_exact_output_length() {
    let format = PcmFormat::new(24_000, 2).unwrap();
    let mut resampler = SincResampler::new(format, 48_000, 128).unwrap();
    let samples = (0..200)
        .flat_map(|frame| {
            let value = if frame == 100 { 1.0 } else { 0.0 };
            [value, -value]
        })
        .collect();
    resampler
        .push(junk_libs_audio::PcmChunk::new(format, FramePosition(0), samples).unwrap())
        .unwrap();
    resampler.finish().unwrap();

    let mut output = Vec::new();
    while let Some(chunk) = resampler.read_frames(7).unwrap() {
        assert_eq!(chunk.start_frame.0, (output.len() / 2) as u64);
        output.extend(chunk.samples);
    }
    assert_eq!(output.len() / 2, 400);
    for frame in output.chunks_exact(2) {
        assert!((frame[0] + frame[1]).abs() < 0.000_001);
    }
    let peak_frame = output
        .chunks_exact(2)
        .enumerate()
        .max_by(|(_, left), (_, right)| left[0].abs().total_cmp(&right[0].abs()))
        .unwrap()
        .0;
    assert!(
        peak_frame.abs_diff(200) <= 1,
        "2x resampling must retain impulse time within one output frame; got {peak_frame}"
    );
    assert_eq!(resampler.read_frames(1).unwrap(), None);

    let mut wrong_start = SincResampler::new(format, 48_000, 128).unwrap();
    let error = wrong_start
        .push(junk_libs_audio::PcmChunk::new(format, FramePosition(1), vec![0.0; 2]).unwrap())
        .unwrap_err();
    assert!(matches!(error, ResampleError::NonContiguous { .. }));
}

#[test]
fn gain_and_clipping_match_independent_vectors() {
    let mut samples = [-0.75, -0.25, 0.25, 0.75];
    let peak = apply_gain(&mut samples, 2.0).unwrap();
    assert_eq!(samples, [-1.5, -0.5, 0.5, 1.5]);
    assert_eq!(peak, 1.5);
    assert_eq!(hard_clip(&mut samples, 1.0).unwrap(), 2);
    assert_eq!(samples, [-1.0, -0.5, 0.5, 1.0]);
    assert!(apply_gain(&mut samples, f32::NAN).is_err());
    assert!(hard_clip(&mut samples, 0.0).is_err());
}
