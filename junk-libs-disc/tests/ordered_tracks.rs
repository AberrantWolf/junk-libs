use std::fs::File;
use std::io::BufReader;
use std::process::Command;

use junk_libs_disc::{RAW_SECTOR_SIZE, visit_chd_raw_tracks};

#[test]
fn chd_visits_every_true_ordered_track_frame_without_padding_or_subchannel() {
    if Command::new("chdman").arg("--help").output().is_err() {
        eprintln!("chdman is unavailable; synthetic CHD qualification skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cue = dir.path().join("disc.cue");
    let chd = dir.path().join("disc.chd");
    let first = vec![0x11_u8; 3 * RAW_SECTOR_SIZE as usize];
    let second = vec![0x22_u8; 5 * RAW_SECTOR_SIZE as usize];
    std::fs::write(dir.path().join("track01.bin"), &first).unwrap();
    std::fs::write(dir.path().join("track02.bin"), &second).unwrap();
    std::fs::write(
        &cue,
        "FILE \"track01.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n\
         FILE \"track02.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    assert!(
        Command::new("chdman")
            .args(["createcd", "-i"])
            .arg(&cue)
            .arg("-o")
            .arg(&chd)
            .output()
            .unwrap()
            .status
            .success()
    );

    let mut observed = vec![Vec::new(), Vec::new()];
    let mut reader = BufReader::new(File::open(chd).unwrap());
    let tracks = visit_chd_raw_tracks(&mut reader, &mut |track, raw| {
        observed[track.track_number as usize - 1].extend_from_slice(raw);
        Ok(())
    })
    .unwrap();

    assert_eq!(
        tracks.iter().map(|track| track.frames).collect::<Vec<_>>(),
        [3, 5]
    );
    assert_eq!(observed, [first, second]);
}
