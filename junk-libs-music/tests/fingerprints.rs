#![cfg(feature = "fingerprint")]
use junk_libs_music::{
    Error,
    fingerprint::{ByteSource, MediaHint, compute},
};
fn source(bytes: &[u8], extension: &str) -> ByteSource {
    ByteSource::new(
        std::io::Cursor::new(bytes.to_vec()),
        MediaHint::new(Some(bytes.len() as u64), Some(extension), None),
    )
}
#[test]
fn real_flac_and_mp3_decode_with_full_duration_and_distinct_fingerprints() {
    let a = compute(
        source(include_bytes!("fixtures/01.flac"), "flac"),
        &mut || Ok(()),
    )
    .unwrap();
    let b = compute(
        source(include_bytes!("fixtures/02.mp3"), "mp3"),
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(a.decoded_frames, 44100 * 12);
    assert_eq!(a.analyzed_frames, a.decoded_frames);
    assert_eq!(a.duration_ms, 12000);
    assert!((b.duration_ms as i64 - 12000).abs() < 100);
    assert!(a.usable() && b.usable());
    assert_ne!(a.fingerprint, b.fingerprint);
    // Independent native Chromaprint 1.6.2 Test2 vectors from the decoded PCM.
    assert_eq!(
        a.fingerprint,
        "AQAAS4moRcql4NClnHASJziu33gSU_gHT_HxH98H_XiQSsl0XCOe4_oRhn8ilMyi4od6dFWSo4ks_LicG49MCT88pTn-4_ugHw9SKZlyXCP-4z9SfkyEM8vxQz0aVdFRyfiDK73xmRJueEqDH9_yCTouI3WY4wzx68GRcowo4dpw5tDRqNNRCX9wpZbxeZFQO2imFD8-foLQyUEYp8cZHr-OHylXLRIAABAxBADEFEDCAKgkZIQiYAgACmpmAAGICoCEAVBJqAgwihgCCFBQMwOIQAgbgJBQBBhCCICkGUAQNgApgoAhxAE"
    );
    assert_eq!(
        b.fingerprint,
        "AQAAS56SqIkSJfiSNdTgZE6O48ypwmwURG4gHXmI5_gzFQ3HHpV-CVxoPFGO54iUdYd25Hhy5SiX8BGaZMeF51RhWkHkItHxED_KZw_MsRsq_SG4UMSV5NiPdN2hI8eTXDmaSEkfocrx48lVmI2CyEWi4yF-lPke-N9Q6Q_BhTquBDvSdUiOH5-ucHAYP3iO_-iTC1PYKMiqIXmEt_ipHO3zoBm_4ZnzEBwjHl-O_UhXJAcCAAFCgUEEEIQ5yJRiiQAhAAOCiSGAEMwppyBLAghEFDOCiSEAIEJYpiDCRAiAiELCEEaEAIYA4JhyCgI"
    );
    a.validate().unwrap();
    b.validate().unwrap();
}
#[test]
fn cancellation_during_decoding_never_returns_a_fingerprint() {
    let mut checks = 0;
    let result = compute(
        source(include_bytes!("fixtures/01.flac"), "flac"),
        &mut || {
            checks += 1;
            if checks >= 8 {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(checks, 8);
}
#[test]
fn truncated_flac_and_inconsistent_envelopes_fail() {
    let bytes = include_bytes!("fixtures/01.flac");
    assert!(compute(source(&bytes[..bytes.len() / 2], "flac"), &mut || Ok(())).is_err());
    let mut a = compute(source(bytes, "flac"), &mut || Ok(())).unwrap();
    a.decoded_frames += 1;
    assert!(a.validate().is_err());
    assert!(
        !a.usable(),
        "inconsistent coordinates cannot be advertised as searchable"
    );
}

#[test]
fn prefix_decode_keeps_recorded_duration_and_stops_before_tail() {
    use junk_libs_music::fingerprint::compute_window;
    let bytes = include_bytes!("fixtures/prefix-125s.flac");
    let full = compute(source(bytes, "flac"), &mut || Ok(())).unwrap();
    let mut checks = 0;
    let prefix = compute_window(source(bytes, "flac"), 8000 * 125, &mut || {
        checks += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(prefix.fingerprint, full.fingerprint);
    assert_eq!(prefix.total_frames(), 8000 * 125);
    assert_eq!(prefix.decoded_frames, 8000 * 120);
    assert_eq!(prefix.duration_ms, 125000);
    assert!(checks < 2 * (8000 * 125 / 4096));
    let mut invalid = prefix.clone();
    invalid.total_frames = Some(8000 * 119);
    assert!(invalid.validate().is_err());
    assert!(compute_window(source(bytes, "flac"), 8000 * 126, &mut || Ok(())).is_err());
    assert!(compute_window(source(bytes, "flac"), 0, &mut || Ok(())).is_err());
    assert!(compute_window(source(bytes, "flac"), u64::MAX, &mut || Ok(())).is_err());
    assert!(matches!(
        compute_window(source(bytes, "flac"), 8000 * 125, &mut || Err(
            Error::Cancelled
        )),
        Err(Error::Cancelled)
    ));
}
