use std::cell::Cell;

use junk_libs_accuraterip::{
    BoundedDiscReader, DbarFile, DbarResponse, DiscPcmTrack, ExpectedChecksum, VerificationOptions,
    VerificationStatus, verify_with_offset_reader,
};
use junk_libs_core::AnalysisError;
use junk_libs_disc::CdPcmReader;

struct GeneratedReader {
    frames: u64,
    position: u64,
    maximum_request: Cell<usize>,
}

impl GeneratedReader {
    fn new(frames: u64) -> Self {
        Self {
            frames,
            position: 0,
            maximum_request: Cell::new(0),
        }
    }

    fn frame_at(position: u64) -> [i16; 2] {
        let left = position.wrapping_mul(17).wrapping_add(3) as i16;
        let right = position.wrapping_mul(31).wrapping_add(7) as i16;
        [left, right]
    }
}

impl CdPcmReader for GeneratedReader {
    fn total_frames(&self) -> u64 {
        self.frames
    }

    fn seek_frame(&mut self, frame: u64) -> Result<(), AnalysisError> {
        if frame > self.frames {
            return Err(AnalysisError::invalid_format("seek beyond generated PCM"));
        }
        self.position = frame;
        Ok(())
    }

    fn read_frames(&mut self, output: &mut [[i16; 2]]) -> Result<usize, AnalysisError> {
        self.maximum_request
            .set(self.maximum_request.get().max(output.len()));
        let count = output.len().min((self.frames - self.position) as usize);
        for (index, frame) in output[..count].iter_mut().enumerate() {
            *frame = Self::frame_at(self.position + index as u64);
        }
        self.position += count as u64;
        Ok(count)
    }
}

fn packed(frame: [i16; 2]) -> u32 {
    u32::from(frame[0] as u16) | (u32::from(frame[1] as u16) << 16)
}

fn dbar(checksums: &[u32]) -> DbarFile {
    DbarFile {
        responses: vec![DbarResponse {
            track_count: checksums.len() as u8,
            ar_id1: 1,
            ar_id2: 2,
            cddb_id: 3,
            tracks: checksums
                .iter()
                .map(|&checksum| ExpectedChecksum {
                    confidence: 12,
                    checksum,
                    checksum_450: 0,
                })
                .collect(),
        }],
    }
}

#[test]
fn reader_path_preserves_i16_extremes_and_both_offset_signs_across_tracks() {
    assert_eq!(packed([i16::MIN, i16::MAX]), 0x7fff_8000);
    assert_eq!(packed([-1, 1]), 0x0001_ffff);
    // Fixed independent vectors evaluated from the CUETools/ARver equations
    // over GeneratedReader::frame_at, outside this implementation. Keeping
    // v1 and v2 literal prevents the test oracle from sharing checksum code.
    let vectors = [
        (
            -6,
            [0x74db_0f72, 0x5b26_074c, 0x7493_fd62],
            [0x75ee_dd52, 0x5c4f_8d23, 0x7519_0488],
        ),
        (
            6,
            [0x28b4_e3ea, 0x17f4_26bc, 0xe119_e74a],
            [0x29c8_5822, 0x191d_b58d, 0xe19e_cbf1],
        ),
    ];
    for (shift, v1, v2) in vectors {
        let lengths = [9_000_u64, 9_000, 9_000];
        let tracks = lengths
            .iter()
            .enumerate()
            .scan(0_u64, |start, (index, &length)| {
                let track = DiscPcmTrack {
                    position: index as u8 + 1,
                    start_frame: *start,
                    frame_count: length,
                };
                *start += length;
                Some(track)
            })
            .collect::<Vec<_>>();
        let reader = GeneratedReader::new(lengths.iter().sum());
        let mut disc = BoundedDiscReader::new(reader, tracks).unwrap();
        let summary = verify_with_offset_reader(
            &dbar(&v1),
            &mut disc,
            VerificationOptions {
                max_sample_shift: 20,
            },
            || Ok(()),
        )
        .unwrap();

        assert_eq!(summary.status, VerificationStatus::Verified);
        assert_eq!(summary.chosen_sample_shift, Some(shift));
        assert_eq!(summary.evaluated_sample_shift_start, -20);
        assert_eq!(summary.evaluated_sample_shift_end, 20);
        assert_eq!(
            summary
                .tracks
                .iter()
                .map(|track| track.computed.v1)
                .collect::<Vec<_>>(),
            v1
        );
        assert_eq!(
            summary
                .tracks
                .iter()
                .map(|track| track.computed.v2)
                .collect::<Vec<_>>(),
            v2
        );
    }
}

#[test]
fn long_disc_uses_bounded_reads_and_observes_cancellation() {
    let frames = 80_u64 * 60 * 44_100;
    let reader = GeneratedReader::new(frames);
    let tracks = vec![DiscPcmTrack {
        position: 1,
        start_frame: 0,
        frame_count: frames,
    }];
    let mut disc = BoundedDiscReader::new(reader, tracks).unwrap();
    let mut checkpoints = 0_u32;
    let error = verify_with_offset_reader(
        &dbar(&[0x1234_5678]),
        &mut disc,
        VerificationOptions {
            max_sample_shift: 0,
        },
        || {
            checkpoints += 1;
            if checkpoints == 3 {
                Err(AnalysisError::other("cancelled"))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("cancelled"));
    assert!(disc.reader().maximum_request.get() <= 16 * 1024);
}

#[test]
fn reader_contract_rejects_gaps_overlap_and_truncation() {
    let invalid = [
        vec![DiscPcmTrack {
            position: 1,
            start_frame: 1,
            frame_count: 100,
        }],
        vec![
            DiscPcmTrack {
                position: 1,
                start_frame: 0,
                frame_count: 100,
            },
            DiscPcmTrack {
                position: 2,
                start_frame: 99,
                frame_count: 100,
            },
        ],
        vec![DiscPcmTrack {
            position: 1,
            start_frame: 0,
            frame_count: 101,
        }],
    ];
    for tracks in invalid {
        assert!(BoundedDiscReader::new(GeneratedReader::new(100), tracks).is_err());
    }
}

#[test]
fn v2_only_catalog_without_anchor_does_not_claim_exhaustive_mismatch() {
    let tracks = (0..3)
        .map(|index| DiscPcmTrack {
            position: index + 1,
            start_frame: u64::from(index) * 9000,
            frame_count: 9000,
        })
        .collect();
    let mut disc = BoundedDiscReader::new(GeneratedReader::new(27000), tracks).unwrap();
    // Independent +6 vectors above, with no v1 or frame-450 anchor.
    let result = verify_with_offset_reader(
        &dbar(&[0x29c8_5822, 0x191d_b58d, 0xe19e_cbf1]),
        &mut disc,
        VerificationOptions {
            max_sample_shift: 20,
        },
        || Ok(()),
    )
    .unwrap();
    assert_eq!(result.status, VerificationStatus::IncompleteSearch);
    assert_eq!(result.v2_evaluated_sample_shifts, vec![0]);
    assert_eq!(result.chosen_sample_shift, None);
}

#[test]
fn frame_450_probe_never_requires_pcm_beyond_short_disc() {
    let frames = 451 * 588;
    let mut disc = BoundedDiscReader::new(
        GeneratedReader::new(frames),
        vec![DiscPcmTrack {
            position: 1,
            start_frame: 0,
            frame_count: frames,
        }],
    )
    .unwrap();
    let result = verify_with_offset_reader(
        &dbar(&[123]),
        &mut disc,
        VerificationOptions {
            max_sample_shift: 20,
        },
        || Ok(()),
    );
    assert!(
        result.is_ok(),
        "optional anchor must not read beyond physical PCM: {result:?}"
    );
}
