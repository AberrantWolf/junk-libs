//! Sample-addressed access to current redumper CD images.
//!
//! Positions and lengths are stereo frames (two signed little-endian i16 samples).
//! The raw timeline retains disc write offset; drive read offset is already applied.
//! No audio descrambling or implicit offset correction is performed.
use super::{Sidecars, validate_current_cd_raw};
use junk_libs_core::AnalysisError;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

const ORIGIN_SECTORS: u64 = 45_150;
const FRAMES_PER_SECTOR: u64 = 588;

/// An addressable byte source; remote range-backed sources can implement this.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Playback boundaries from the drive TOC, independent of split BIN files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAudioTrack {
    pub number: u8,
    /// INDEX 01, relative to disc LBA 0, in stereo frames.
    pub start_frame: u64,
    /// Exclusive end: next INDEX 01, or lead-out. Includes the following audio pregap.
    pub end_frame: u64,
    pub pre_emphasis: bool,
    pub copy_permitted: bool,
}

/// Parse the current redumper `.toc` (MMC format 0, LBA addressing).
/// Mixed-mode and multisession images are deliberately unsupported by this first reader:
/// a format-0 TOC alone cannot safely determine their audio/data pregap boundaries.
pub fn parse_audio_toc(bytes: &[u8]) -> Result<Vec<RawAudioTrack>, AnalysisError> {
    let bad = || AnalysisError::invalid_format("invalid redumper audio TOC");
    if bytes.len() < 20
        || usize::from(u16::from_be_bytes([bytes[0], bytes[1]])) + 2 != bytes.len()
        || !(bytes.len() - 4).is_multiple_of(8)
        || bytes[2] != 1
        || !(1..=99).contains(&bytes[3])
    {
        return Err(bad());
    }
    let mut points = Vec::new();
    for d in bytes[4..].chunks_exact(8) {
        if d[1] >> 4 != 1 {
            return Err(bad());
        }
        let lba = i32::from_be_bytes(d[4..8].try_into().unwrap());
        if lba < 0 || points.last().is_some_and(|p: &(u8, u8, i32)| p.2 >= lba) {
            return Err(bad());
        }
        points.push((d[2], d[1] & 15, lba));
    }
    if points.len() != usize::from(bytes[3]) + 1 || points.last().unwrap().0 != 0xaa {
        return Err(bad());
    }
    let mut tracks = Vec::new();
    for (i, pair) in points.windows(2).enumerate() {
        let (number, control, lba) = pair[0];
        if number as usize != i + 1 {
            return Err(bad());
        }
        if control & 12 != 0 {
            return Err(AnalysisError::unsupported(
                "redumper mixed-mode or four-channel CD audio",
            ));
        }
        tracks.push(RawAudioTrack {
            number,
            start_frame: lba as u64 * FRAMES_PER_SECTOR,
            end_frame: pair[1].2 as u64 * FRAMES_PER_SECTOR,
            pre_emphasis: control & 1 != 0,
            copy_permitted: control & 2 != 0,
        });
    }
    Ok(tracks)
}

/// Inspect a package without opening or relying on its split BIN files.
pub fn read_audio_layout(sidecars: &Sidecars) -> Result<Vec<RawAudioTrack>, AnalysisError> {
    let structure = validate_current_cd_raw(sidecars)?;
    if let Some(path) = &sidecars.fulltoc {
        if std::fs::metadata(path)?.len() > 65_537 {
            return Err(AnalysisError::invalid_format(
                "full TOC exceeds MMC response size",
            ));
        }
        let full = std::fs::read(path)?;
        if full.len() < 4
            || usize::from(u16::from_be_bytes([full[0], full[1]])) + 2 != full.len()
            || !(full.len() - 4).is_multiple_of(11)
        {
            return Err(AnalysisError::invalid_format("invalid redumper full TOC"));
        }
        if full[2] != 1 || full[3] != 1 || full[4..].chunks_exact(11).any(|d| d[0] != 1) {
            return Err(AnalysisError::unsupported("redumper multisession playback"));
        }
    }
    let tracks = parse_audio_toc(&std::fs::read(sidecars.toc.as_ref().unwrap())?)?;
    let end = tracks.last().unwrap().end_frame + ORIGIN_SECTORS * FRAMES_PER_SECTOR;
    if end > structure.sample_frames || end / FRAMES_PER_SECTOR > structure.subcode_frames {
        return Err(AnalysisError::invalid_format(
            "redumper lead-out exceeds raw timeline",
        ));
    }
    Ok(tracks)
}

/// How to handle samples which redumper did not successfully recover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidSamplePolicy {
    Reject,
    Silence,
    Preserve,
}

/// Quality counts for the returned block, with redumper's states kept distinct.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SampleQuality {
    pub skipped: usize,
    pub c2_errors: usize,
    pub c2_unchecked: usize,
    pub scsi_unchecked: usize,
    pub verified: usize,
}

/// Bounded-memory, exact-seek PCM reader. Reads at most 588 stereo frames per call.
/// Errors do not advance the logical cursor. Unknown quality states always fail.
pub struct RawAudioReader {
    pcm: Box<dyn ReadSeek>,
    state: Box<dyn ReadSeek>,
    start: u64,
    length: u64,
    position: u64,
    policy: InvalidSamplePolicy,
    pcm_at: Option<u64>,
    state_at: Option<u64>,
}
impl RawAudioReader {
    pub fn open(
        sidecars: &Sidecars,
        number: u8,
        policy: InvalidSamplePolicy,
    ) -> Result<Self, AnalysisError> {
        let tracks = read_audio_layout(sidecars)?;
        let track = tracks
            .iter()
            .find(|t| t.number == number)
            .ok_or_else(|| AnalysisError::invalid_format("audio track not found"))?;
        Self::from_sources(
            Box::new(BufReader::new(File::open(
                sidecars.scram.as_ref().unwrap(),
            )?)),
            Box::new(BufReader::new(File::open(
                sidecars.state.as_ref().unwrap(),
            )?)),
            track.start_frame,
            track.end_frame,
            policy,
        )
    }

    /// Construct a reader for an explicitly validated audio range on the disc timeline.
    /// The caller must ensure the range contains stereo audio, not CD-ROM data.
    /// No sample offset is inferred from source lengths.
    pub fn from_sources(
        pcm: Box<dyn ReadSeek>,
        state: Box<dyn ReadSeek>,
        start_frame: u64,
        end_frame: u64,
        policy: InvalidSamplePolicy,
    ) -> Result<Self, AnalysisError> {
        let start = i64::try_from(start_frame)
            .map_err(|_| AnalysisError::invalid_format("frame overflow"))?;
        let end = i64::try_from(end_frame)
            .map_err(|_| AnalysisError::invalid_format("frame overflow"))?;
        Self::from_signed_sources(pcm, state, start, end, policy)
    }

    /// Like `from_sources`, but permits negative disc-frame addresses for explicitly
    /// validated first-track pregap/HTOA ranges. The caller validates track type.
    pub fn from_signed_sources(
        mut pcm: Box<dyn ReadSeek>,
        mut state: Box<dyn ReadSeek>,
        start_frame: i64,
        end_frame: i64,
        policy: InvalidSamplePolicy,
    ) -> Result<Self, AnalysisError> {
        let origin = (ORIGIN_SECTORS * FRAMES_PER_SECTOR) as i64;
        let absolute = |frame: i64| {
            frame
                .checked_add(origin)
                .and_then(|f| u64::try_from(f).ok())
                .ok_or_else(|| AnalysisError::invalid_format("frame outside redumper timeline"))
        };
        let start = absolute(start_frame)?;
        let end = absolute(end_frame)?;
        let bytes = pcm.seek(SeekFrom::End(0))?;
        let states = state.seek(SeekFrom::End(0))?;
        if start_frame > end_frame || end > states || states.checked_mul(4) != Some(bytes) {
            return Err(AnalysisError::invalid_format(
                "audio range exceeds matching scram/state sources",
            ));
        }
        Ok(Self {
            pcm,
            state,
            start,
            length: end - start,
            position: 0,
            policy,
            pcm_at: None,
            state_at: None,
        })
    }
    pub fn total_frames(&self) -> u64 {
        self.length
    }
    pub fn position_frames(&self) -> u64 {
        self.position
    }
    pub fn seek_frame(&mut self, frame: u64) -> Result<(), AnalysisError> {
        if frame > self.length {
            return Err(AnalysisError::invalid_format("seek beyond audio range"));
        }
        self.position = frame;
        Ok(())
    }
    pub fn read_frames(
        &mut self,
        output: &mut [[i16; 2]],
    ) -> Result<(usize, SampleQuality), AnalysisError> {
        let count = output
            .len()
            .min(588)
            .min((self.length - self.position).min(588) as usize);
        let mut quality = SampleQuality::default();
        if count == 0 {
            return Ok((0, quality));
        }
        let offset = self.start + self.position;
        let mut states = [0u8; 588];
        let mut bytes = [0u8; 2352];
        read_at(
            self.state.as_mut(),
            &mut self.state_at,
            offset,
            &mut states[..count],
        )?;
        for (i, s) in states[..count].iter().enumerate() {
            match s {
                0 => quality.skipped += 1,
                1 => quality.c2_errors += 1,
                2 => quality.c2_unchecked += 1,
                3 => quality.scsi_unchecked += 1,
                4 => quality.verified += 1,
                _ => {
                    return Err(AnalysisError::unsupported(format!(
                        "unknown redumper state {s} at disc frame {}",
                        (offset + i as u64) as i64 - (ORIGIN_SECTORS * FRAMES_PER_SECTOR) as i64
                    )));
                }
            }
        }
        if self.policy == InvalidSamplePolicy::Reject && quality.skipped + quality.c2_errors != 0 {
            return Err(AnalysisError::invalid_format(format!(
                "unrecovered redumper audio at track frame {}: {} skipped, {} C2 errors",
                self.position, quality.skipped, quality.c2_errors
            )));
        }
        read_at(
            self.pcm.as_mut(),
            &mut self.pcm_at,
            offset * 4,
            &mut bytes[..count * 4],
        )?;
        for i in 0..count {
            let b = &bytes[i * 4..i * 4 + 4];
            output[i] = if states[i] < 2 && self.policy == InvalidSamplePolicy::Silence {
                [0, 0]
            } else {
                [
                    i16::from_le_bytes([b[0], b[1]]),
                    i16::from_le_bytes([b[2], b[3]]),
                ]
            };
        }
        self.position += count as u64;
        Ok((count, quality))
    }
}

fn read_at(
    source: &mut dyn ReadSeek,
    current: &mut Option<u64>,
    offset: u64,
    bytes: &mut [u8],
) -> Result<(), AnalysisError> {
    let positioned = *current == Some(offset);
    *current = None;
    if !positioned {
        source.seek(SeekFrom::Start(offset))?;
    }
    source.read_exact(bytes)?;
    *current = Some(offset + bytes.len() as u64);
    Ok(())
}
