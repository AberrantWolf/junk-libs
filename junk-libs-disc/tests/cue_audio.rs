use junk_libs_disc::cue_audio::CueAudioDisc;
fn fixture(cue: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (name, values) in [
        ("one.bin", vec![1i16, 2, 3, 4]),
        ("two.bin", vec![5i16, 6, 7, 8]),
    ] {
        let bytes = values
            .into_iter()
            .flat_map(|v| [v.to_le_bytes(), (-v).to_le_bytes()].concat().repeat(588))
            .collect::<Vec<_>>();
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    let path = dir.path().join("disc.cue");
    std::fs::write(&path, cue).unwrap();
    (dir, path)
}
fn collect(mut r: junk_libs_disc::cue_audio::CueAudioReader) -> Vec<[i16; 2]> {
    let mut frames = vec![];
    let mut b = [[0; 2]; 588];
    loop {
        let n = r.read_frames(&mut b).unwrap();
        if n == 0 {
            break;
        }
        frames.extend(&b[..n]);
    }
    frames
}
#[test]
fn stored_and_synthetic_gaps_hidden_audio_and_exact_seeks() {
    let (_dir, path) = fixture(
        "FILE \"one.bin\" BINARY\n TRACK 01 AUDIO\n FLAGS PRE DCP\n INDEX 00 00:00:00\n INDEX 01 00:00:01\n POSTGAP 00:00:01\nFILE \"two.bin\" BINARY\n TRACK 02 AUDIO\n PREGAP 00:00:02\n INDEX 00 00:00:00\n INDEX 01 00:00:01\n",
    );
    let disc = CueAudioDisc::open(&path).unwrap();
    assert_eq!(disc.hidden_audio, Some((0, 588)));
    assert!(disc.tracks[0].pre_emphasis && disc.tracks[0].copy_permitted);
    let first = collect(disc.track_reader(1).unwrap());
    assert_eq!(first.len(), 7 * 588);
    for (n, v) in [2, 3, 4, 0, 0, 0, 5].into_iter().enumerate() {
        assert!(first[n * 588..(n + 1) * 588].iter().all(|f| *f == [v, -v]));
    }
    let second = collect(disc.track_reader(2).unwrap());
    assert_eq!(second[0], [6, -6]);
    assert_eq!(second.len(), 3 * 588);
    let mut r = disc.track_reader(1).unwrap();
    r.seek_frame(6 * 588 - 1).unwrap();
    let mut b = [[0; 2]; 2];
    assert_eq!(r.read_frames(&mut b).unwrap(), 1);
    assert_eq!(b[0], [0, 0]);
    assert_eq!(r.read_frames(&mut b).unwrap(), 2);
    assert_eq!(b[0], [5, -5]);
    r.seek_frame(0).unwrap();
    r.read_frames(&mut b).unwrap();
    assert_eq!(b[0], [2, -2]);
    r.seek_frame(r.total_frames()).unwrap();
    assert_eq!(r.read_frames(&mut b).unwrap(), 0);
}
#[test]
fn data_pregap_is_never_emitted_as_audio() {
    let (_dir, path) = fixture(
        "FILE \"one.bin\" BINARY\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n TRACK 02 MODE1/2352\n INDEX 00 00:00:02\n INDEX 01 00:00:03\n",
    );
    let d = CueAudioDisc::open(&path).unwrap();
    let pcm = collect(d.track_reader(1).unwrap());
    assert_eq!(pcm.len(), 2 * 588);
    assert!(d.track_reader(2).is_err());
    assert!(d.range_reader(0, 4 * 588).is_err());
}
#[test]
fn malformed_gaps_indexes_and_four_channel_flags_are_rejected() {
    for directives in [
        "PREGAP 00:60:00",
        "PREGAP 00:00:01\n PREGAP 00:00:01",
        "FLAGS 4CH",
        "INDEX 00 00:00:03",
        "INDEX 01 00:00:02",
        "INDEX 02 4294967295:00:00",
    ] {
        let (_dir, path) = fixture(&format!(
            "FILE \"one.bin\" BINARY\n TRACK 01 AUDIO\n INDEX 01 00:00:01\n {directives}\n"
        ));
        assert!(CueAudioDisc::open(&path).is_err(), "{directives}");
    }
}
