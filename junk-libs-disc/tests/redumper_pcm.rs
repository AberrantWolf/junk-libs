use junk_libs_disc::redumper::pcm::{InvalidSamplePolicy, RawAudioReader, parse_audio_toc};
use std::io::{Seek, SeekFrom, Write};

fn sources(states: &[u8]) -> (Box<std::fs::File>, Box<std::fs::File>) {
    let origin = 45150 * 588;
    let mut pcm = tempfile::tempfile().unwrap();
    let mut state = tempfile::tempfile().unwrap();
    pcm.set_len((origin + states.len() as u64) * 4).unwrap();
    state.set_len(origin + states.len() as u64).unwrap();
    pcm.seek(SeekFrom::Start(origin * 4)).unwrap();
    state.seek(SeekFrom::Start(origin)).unwrap();
    for i in 0..states.len() {
        pcm.write_all(&(i as i16).to_le_bytes()).unwrap();
        pcm.write_all(&(-(i as i16) - 1).to_le_bytes()).unwrap();
    }
    state.write_all(states).unwrap();
    (Box::new(pcm), Box::new(state))
}
#[test]
fn exact_seek_stereo_eof_and_contiguous_tracks() {
    let states = vec![4; 1201];
    let (pcm, state) = sources(&states);
    let mut a =
        RawAudioReader::from_sources(pcm, state, 0, 601, InvalidSamplePolicy::Reject).unwrap();
    let mut out = [[0; 2]; 588];
    assert_eq!(a.read_frames(&mut out).unwrap().0, 588);
    assert_eq!(out[587], [587, -588]);
    assert_eq!(a.read_frames(&mut out).unwrap().0, 13);
    assert_eq!(out[12], [600, -601]);
    assert_eq!(a.read_frames(&mut out).unwrap().0, 0);
    a.seek_frame(587).unwrap();
    assert_eq!(a.read_frames(&mut out).unwrap().0, 14);
    assert_eq!(out[0], [587, -588]);
    a.seek_frame(0).unwrap();
    assert_eq!(a.read_frames(&mut out[..1]).unwrap().0, 1);
    assert_eq!(out[0], [0, -1]);
    assert!(a.seek_frame(602).is_err());
    assert_eq!(a.position_frames(), 1);
    let (pcm, state) = sources(&states);
    let mut b =
        RawAudioReader::from_sources(pcm, state, 601, 1201, InvalidSamplePolicy::Reject).unwrap();
    b.read_frames(&mut out).unwrap();
    assert_eq!(out[0], [601, -602]);
}
#[test]
fn quality_policy_is_explicit_and_error_does_not_advance_or_mutate_output() {
    for policy in [
        InvalidSamplePolicy::Reject,
        InvalidSamplePolicy::Silence,
        InvalidSamplePolicy::Preserve,
    ] {
        let (pcm, state) = sources(&[0, 1, 2, 3, 4]);
        let mut r = RawAudioReader::from_sources(pcm, state, 0, 5, policy).unwrap();
        let mut out = [[123; 2]; 5];
        let result = r.read_frames(&mut out);
        if policy == InvalidSamplePolicy::Reject {
            assert!(result.is_err());
            assert_eq!(out, [[123; 2]; 5]);
            assert_eq!(r.position_frames(), 0);
            r.seek_frame(2).unwrap();
            assert_eq!(r.read_frames(&mut out).unwrap().0, 3);
        } else {
            let (n, q) = result.unwrap();
            assert_eq!(n, 5);
            assert_eq!(
                (
                    q.skipped,
                    q.c2_errors,
                    q.c2_unchecked,
                    q.scsi_unchecked,
                    q.verified
                ),
                (1, 1, 1, 1, 1)
            );
            assert_eq!(out[2], [2, -3]);
            assert_eq!(
                out[1],
                if policy == InvalidSamplePolicy::Silence {
                    [0, 0]
                } else {
                    [1, -2]
                }
            );
        }
    }
    let (pcm, state) = sources(&[5]);
    let mut r =
        RawAudioReader::from_sources(pcm, state, 0, 1, InvalidSamplePolicy::Preserve).unwrap();
    assert!(r.read_frames(&mut [[0; 2]; 1]).is_err());
}
fn toc() -> Vec<u8> {
    let mut b = vec![0, 26, 1, 2];
    for (track, lba) in [(1, 0i32), (2, 123), (0xaa, 456)] {
        b.extend([0, 0x10, track, 0]);
        b.extend(lba.to_be_bytes());
    }
    b
}
#[test]
fn toc_uses_binary_lba_and_rejects_ambiguous_or_malformed_layouts() {
    let tracks = parse_audio_toc(&toc()).unwrap();
    assert_eq!(tracks[0].end_frame, tracks[1].start_frame);
    assert_eq!(tracks[1].end_frame, 456 * 588);
    for at in [0, 1, 2, 3, 5, 6, 13, 14, 22] {
        let mut bytes = toc();
        bytes[at] = 255;
        assert!(
            parse_audio_toc(&bytes).is_err(),
            "accepted corrupt byte {at}"
        );
    }
    let mut bytes = toc();
    bytes[5] = 0x14;
    assert!(parse_audio_toc(&bytes).is_err());
    let mut bytes = toc();
    bytes[5] = 0x11;
    assert!(parse_audio_toc(&bytes).unwrap()[0].pre_emphasis);
}

#[test]
fn missing_frames_and_truncated_sources_fail_instead_of_padding() {
    let (pcm, state) = sources(&[4; 10]);
    assert!(
        RawAudioReader::from_sources(pcm, state, 0, 11, InvalidSamplePolicy::Preserve).is_err()
    );
    let (pcm, state) = sources(&[4; 10]);
    pcm.set_len(pcm.metadata().unwrap().len() - 4).unwrap();
    assert!(RawAudioReader::from_sources(pcm, state, 0, 9, InvalidSamplePolicy::Preserve).is_err());
    let (pcm, state) = sources(&[4; 10]);
    assert!(
        RawAudioReader::from_signed_sources(
            pcm,
            state,
            -45150 * 588 - 1,
            0,
            InvalidSamplePolicy::Preserve
        )
        .is_err()
    );
}

#[test]
#[ignore = "set JUNK_LIBS_REDUMPER_AUDIO_PREFIX to the known-good Octopath b736 package"]
fn real_audio_raw_matches_split_bin_on_the_recording_timeline() {
    use junk_libs_disc::{
        pcm::TrackPcmReader,
        redumper::{find_sidecars, pcm::read_audio_layout},
    };
    let prefix = std::env::var_os("JUNK_LIBS_REDUMPER_AUDIO_PREFIX").expect("fixture prefix");
    let cue = std::path::PathBuf::from(format!("{}.cue", prefix.to_string_lossy()));
    let sidecars = find_sidecars(&cue);
    assert_eq!(
        junk_libs_disc::redumper::parse_log(sidecars.log.as_ref().unwrap())
            .unwrap()
            .disc_write_offset,
        Some(0),
        "this fixture oracle requires zero split offset"
    );
    let layout = read_audio_layout(&sidecars).unwrap();
    assert_eq!(layout.len(), 18);
    assert_eq!(layout.last().unwrap().end_frame, 268234 * 588);
    for track in &layout {
        let mut raw =
            RawAudioReader::open(&sidecars, track.number, InvalidSamplePolicy::Reject).unwrap();
        let mut bin = TrackPcmReader::from_cue(&cue, track.number).unwrap();
        assert_eq!(raw.total_frames(), bin.total_samples());
        let mut output = [[0; 2]; 588];
        for sector in &mut bin {
            let sector = sector.unwrap();
            assert_eq!(raw.read_frames(&mut output).unwrap().0, 588);
            for (i, (sample, pair)) in sector.into_iter().zip(output).enumerate() {
                assert_eq!(
                    [sample as u16 as i16, (sample >> 16) as u16 as i16],
                    pair,
                    "track {} stereo frame {} (if split applied disc-write correction, record that explicit offset before comparing)",
                    track.number,
                    raw.position_frames() - 588 + i as u64
                );
            }
        }
        assert_eq!(raw.read_frames(&mut output).unwrap().0, 0);
    }
}
