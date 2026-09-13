#![cfg(feature = "chd")]

use std::fs::File;
use std::io::BufReader;
use std::process::Command;

use junk_libs_game_formats::{CUE_RAW_2352_TRACK_DOMAIN, hash_chd_raw_2352};

#[test]
#[ignore = "requires explicitly provisioned chdman 0.285; run with --ignored"]
fn chdman_0285_round_trip_matches_every_unpadded_raw_track() {
    let tool = std::env::var("CHDMAN").unwrap_or_else(|_| "chdman".into());
    let version = Command::new(&tool)
        .arg("--version")
        .output()
        .expect("required chdman oracle is unavailable");
    let banner = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        banner.split_whitespace().any(|token| token == "0.285"),
        "required chdman 0.285, got {banner}"
    );

    let directory = tempfile::tempdir().unwrap();
    let cue = directory.path().join("disc.cue");
    let chd = directory.path().join("disc.chd");
    let first = vec![0x11_u8; 3 * 2352];
    let second = vec![0x22_u8; 5 * 2352];
    std::fs::write(directory.path().join("track01.bin"), &first).unwrap();
    std::fs::write(directory.path().join("track02.bin"), &second).unwrap();
    std::fs::write(
        &cue,
        "FILE \"track01.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n\
         FILE \"track02.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    let output = Command::new(&tool)
        .args(["createcd", "-i"])
        .arg(&cue)
        .arg("-o")
        .arg(&chd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut reader = BufReader::new(File::open(chd).unwrap());
    let tracks = hash_chd_raw_2352(&mut reader, &|_, _| {}).unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].frames, 3);
    assert_eq!(tracks[1].frames, 5);
    assert_eq!(tracks[0].hashes.bytes, first.len() as u64);
    assert_eq!(tracks[1].hashes.bytes, second.len() as u64);
    assert_eq!(tracks[0].hashes.domain, CUE_RAW_2352_TRACK_DOMAIN);
    assert_eq!(
        tracks[0].hashes.sha256,
        "e75a34b718be8826bd01312d55980c3c6d898e346ab4bf2fda123791737fa12f"
    );
    assert_eq!(
        tracks[1].hashes.sha256,
        "e8d9b3b43740b26ce576e9b8922fa5264c3ccc2a73585a97c985b1b05ca687fb"
    );
}
