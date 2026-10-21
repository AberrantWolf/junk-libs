use junk_libs_core::AnalysisError;
use junk_libs_disc::pcm::{CdPcmReader, PcmSequenceReader};
struct Frames {
    values: Vec<[i16; 2]>,
    position: usize,
    advertised: u64,
}
impl CdPcmReader for Frames {
    fn total_frames(&self) -> u64 {
        self.advertised
    }
    fn seek_frame(&mut self, p: u64) -> Result<(), AnalysisError> {
        self.position = p as usize;
        Ok(())
    }
    fn read_frames(&mut self, out: &mut [[i16; 2]]) -> Result<usize, AnalysisError> {
        let available = self.values.get(self.position..).unwrap_or_default();
        let n = out.len().min(available.len());
        out[..n].copy_from_slice(&available[..n]);
        self.position += n;
        Ok(n)
    }
}
fn frames(v: Vec<[i16; 2]>, advertised: u64) -> Box<dyn CdPcmReader + Send> {
    Box::new(Frames {
        values: v,
        position: 0,
        advertised,
    })
}
#[test]
fn sequence_preserves_frames_at_boundaries_and_seek_in_both_directions() {
    let mut r = PcmSequenceReader::new(vec![
        frames(vec![[1, -1], [2, -2]], 2),
        frames(vec![[3, -3]], 1),
    ])
    .unwrap();
    assert_eq!(r.total_frames(), 3);
    let mut out = [[0; 2]; 4];
    assert_eq!(r.read_frames(&mut out).unwrap(), 2);
    assert_eq!(&out[..2], &[[1, -1], [2, -2]]);
    assert_eq!(r.read_frames(&mut out).unwrap(), 1);
    assert_eq!(out[0], [3, -3]);
    assert_eq!(r.read_frames(&mut out).unwrap(), 0);
    r.seek_frame(1).unwrap();
    assert_eq!(r.read_frames(&mut out).unwrap(), 1);
    assert_eq!(out[0], [2, -2]);
    assert_eq!(r.read_frames(&mut out).unwrap(), 1);
    assert_eq!(out[0], [3, -3]);
    r.seek_frame(0).unwrap();
    assert_eq!(r.read_frames(&mut out).unwrap(), 2);
    assert!(r.seek_frame(4).is_err());
}
#[test]
fn sequence_refuses_empty_and_truncated_ranges() {
    assert!(PcmSequenceReader::new(vec![]).is_err());
    assert!(PcmSequenceReader::new(vec![frames(vec![], 0)]).is_err());
    let mut r = PcmSequenceReader::new(vec![frames(vec![[1, 2]], 2)]).unwrap();
    let mut out = [[0; 2]; 2];
    assert_eq!(r.read_frames(&mut out).unwrap(), 1);
    assert!(r.read_frames(&mut out).is_err());
}
