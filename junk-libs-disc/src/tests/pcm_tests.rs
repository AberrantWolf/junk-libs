use std::fs::OpenOptions;
use std::io::Write;

use crate::layout::{LEAD_IN_FRAMES, TrackKind, TrackLayout};
use crate::pcm::{PCM_SAMPLES_PER_SECTOR, TrackPcmReader, sector_to_samples};
use crate::sector::RAW_SECTOR_SIZE;

fn audio_layout(number: u8, absolute_offset: u32, length_sectors: u32) -> TrackLayout {
    TrackLayout {
        number,
        absolute_offset,
        length_sectors,
        kind: TrackKind::Audio,
        mode: "AUDIO".to_string(),
    }
}

#[test]
fn sector_to_samples_packs_little_endian_u16_pairs() {
    let mut raw = [0u8; RAW_SECTOR_SIZE as usize];
    // Sample 0: L = 0x1234, R = 0x5678.
    raw[0] = 0x34;
    raw[1] = 0x12;
    raw[2] = 0x78;
    raw[3] = 0x56;
    // Sample 1: L = 0xFFFF (−1 signed), R = 0x8000 (min signed).
    raw[4] = 0xFF;
    raw[5] = 0xFF;
    raw[6] = 0x00;
    raw[7] = 0x80;

    let samples = sector_to_samples(&raw);
    assert_eq!(samples[0], 0x1234 | (0x5678u32 << 16));
    assert_eq!(samples[1], 0xFFFFu32 | (0x8000u32 << 16));
    // Remaining samples are zero.
    for s in &samples[2..] {
        assert_eq!(*s, 0);
    }
}

#[test]
fn sector_to_samples_emits_exactly_588_samples() {
    let raw = [0u8; RAW_SECTOR_SIZE as usize];
    let samples = sector_to_samples(&raw);
    assert_eq!(samples.len(), PCM_SAMPLES_PER_SECTOR);
    assert_eq!(samples.len(), 588);
}

#[test]
fn from_bin_rejects_data_track() {
    let tmp = std::env::temp_dir().join("phono_junk_pcm_data_track.bin");
    let _ = std::fs::remove_file(&tmp);
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 10).unwrap();

    let layout = TrackLayout {
        number: 1,
        absolute_offset: LEAD_IN_FRAMES,
        length_sectors: 10,
        kind: TrackKind::Data,
        mode: "MODE1/2352".to_string(),
    };
    let err = TrackPcmReader::from_bin(&tmp, &layout).expect_err("data track must error");
    assert!(format!("{}", err).contains("rejects non-audio"));

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn from_bin_rejects_unknown_track() {
    let tmp = std::env::temp_dir().join("phono_junk_pcm_unknown_track.bin");
    let _ = std::fs::remove_file(&tmp);
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 10).unwrap();

    let layout = TrackLayout {
        number: 1,
        absolute_offset: LEAD_IN_FRAMES,
        length_sectors: 10,
        kind: TrackKind::Unknown,
        mode: "MYSTERY".to_string(),
    };
    let err = TrackPcmReader::from_bin(&tmp, &layout).expect_err("unknown kind must error");
    assert!(format!("{}", err).contains("rejects non-audio"));

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn from_bin_reads_sectors_at_absolute_offset() {
    // Build a BIN with 4 sectors. Put a distinctive byte at the start of
    // sector index 2 (0-indexed within the BIN = absolute sector LEAD_IN_FRAMES + 2).
    let tmp = std::env::temp_dir().join("phono_junk_pcm_offset.bin");
    let _ = std::fs::remove_file(&tmp);
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 4).unwrap();

    // Write sector 2's first 4 bytes: L = 0xBEEF, R = 0xDEAD.
    let target = 2u64 * RAW_SECTOR_SIZE;
    f.seek_write_or_panic(target, &[0xEF, 0xBE, 0xAD, 0xDE]);
    drop(f);

    // Track 2 starts at absolute sector LEAD_IN_FRAMES + 2 and is one sector long.
    let layout = audio_layout(2, LEAD_IN_FRAMES + 2, 1);
    let mut iter = TrackPcmReader::from_bin(&tmp, &layout).unwrap();
    let sector = iter.next().unwrap().unwrap();
    assert_eq!(sector[0], 0xBEEFu32 | (0xDEADu32 << 16));
    // Iterator is exhausted after length_sectors emissions.
    assert!(iter.next().is_none());

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn from_bin_emits_exactly_length_sectors() {
    let tmp = std::env::temp_dir().join("phono_junk_pcm_count.bin");
    let _ = std::fs::remove_file(&tmp);
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 10).unwrap();
    drop(f);

    let layout = audio_layout(1, LEAD_IN_FRAMES, 7);
    let iter = TrackPcmReader::from_bin(&tmp, &layout).unwrap();
    let count = iter.count();
    assert_eq!(count, 7);

    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn total_samples_matches_length_sectors_times_588() {
    let tmp = std::env::temp_dir().join("phono_junk_pcm_total.bin");
    let _ = std::fs::remove_file(&tmp);
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .unwrap();
    f.set_len(RAW_SECTOR_SIZE * 10).unwrap();
    drop(f);

    let layout = audio_layout(1, LEAD_IN_FRAMES, 10);
    let reader = TrackPcmReader::from_bin(&tmp, &layout).unwrap();
    assert_eq!(reader.total_samples(), 10 * 588);

    let _ = std::fs::remove_file(&tmp);
}

// Small convenience for writing into a File at a byte offset without
// pulling in extra crates. Panics on I/O failure, which is fine in tests.
trait SeekWrite {
    fn seek_write_or_panic(&mut self, offset: u64, buf: &[u8]);
}

impl SeekWrite for std::fs::File {
    fn seek_write_or_panic(&mut self, offset: u64, buf: &[u8]) {
        use std::io::{Seek, SeekFrom};
        self.seek(SeekFrom::Start(offset)).unwrap();
        self.write_all(buf).unwrap();
    }
}
