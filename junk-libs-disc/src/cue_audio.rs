//! Playback-oriented CUE layout for standard BINARY CUE sheets.
//!
//! Positions are stereo frames on a playback timeline. Stored INDEX 00 gaps and
//! synthetic PREGAP/POSTGAP silence are separate extents. See PLAYBACK.md.
use crate::{
    cue,
    layout::{TrackKind, classify_mode},
};
use junk_libs_core::AnalysisError;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, Read, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct CueAudioTrack {
    pub number: u8,
    pub kind: TrackKind,
    /// Index positions on the playback timeline in stereo frames, including INDEX 00.
    pub indexes: Vec<(u8, u64)>,
    pub start_frame: u64,
    pub end_frame: u64,
    pub pre_emphasis: bool,
    pub copy_permitted: bool,
    pub synthetic_pregap_frames: u64,
    pub synthetic_postgap_frames: u64,
}
#[derive(Debug, Clone)]
struct Extent {
    start: u64,
    end: u64,
    /// CUE FILE name and byte offset of the extent's first frame.
    file: Option<(String, u64)>,
    audio: bool,
}
#[derive(Debug, Clone)]
pub struct CueAudioDisc {
    pub tracks: Vec<CueAudioTrack>,
    /// First-track audio before INDEX 01, including synthetic silence. Not queued implicitly.
    pub hidden_audio: Option<(u64, u64)>,
    extents: Vec<Extent>,
    /// Byte length of each CUE FILE, as recorded when the layout was built.
    source_lengths: BTreeMap<String, u64>,
    /// Directory that CUE FILE names resolve against, when opened from disk.
    base: Option<PathBuf>,
}
/// A seekable byte source for one CUE FILE.
pub type CueSource = Box<dyn junk_libs_core::ReadSeek + Send>;
#[derive(Default)]
struct Extras {
    pre: Option<u64>,
    post: Option<u64>,
    emphasis: bool,
    copy: bool,
}
fn invalid(s: &str) -> AnalysisError {
    AnalysisError::invalid_format(s)
}
fn timestamp(s: &str) -> Result<u64, AnalysisError> {
    let p = s
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid("invalid gap timestamp"))?;
    if p.len() != 3 || p[1] >= 60 || p[2] >= 75 {
        return Err(invalid("invalid gap timestamp"));
    }
    p[0].checked_mul(60 * 75)
        .and_then(|f| f.checked_add(p[1] * 75 + p[2]))
        .and_then(|f| f.checked_mul(588))
        .ok_or_else(|| invalid("invalid gap timestamp"))
}
impl CueAudioDisc {
    pub fn open(path: &Path) -> Result<Self, AnalysisError> {
        let text = std::fs::read_to_string(path)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut disc = Self::from_text(&text, |name| {
            Ok(std::fs::metadata(cue::resolve_local_file(dir, name)?)?.len())
        })?;
        disc.base = Some(dir.to_owned());
        Ok(disc)
    }

    /// Build the layout from CUE text and each FILE's byte length, without
    /// touching any payload. Read it later with [`Self::range_reader_with`].
    pub fn from_text(
        text: &str,
        mut file_length: impl FnMut(&str) -> Result<u64, AnalysisError>,
    ) -> Result<Self, AnalysisError> {
        // Nonstandard (CDRWin) directives can affect audio placement; reject rather
        // than silently converting them away.
        if !cue::check_cue_compat(text).is_standard() {
            return Err(AnalysisError::unsupported(
                "playback layout requires a standard BIN/CUE sheet",
            ));
        }
        let sheet = cue::parse_cue(text)?;

        let mut source_lengths = BTreeMap::new();
        for file in &sheet.files {
            if !source_lengths.contains_key(&file.filename) {
                source_lengths.insert(file.filename.clone(), file_length(&file.filename)?);
            }
        }
        let resolved = cue::compute_cue_resolved_layout(&sheet, |name| Ok(source_lengths[name]))?;
        let mut extras = BTreeMap::<u8, Extras>::new();
        let mut current = None;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(word) = words.next() else { continue };
            match word.to_ascii_uppercase().as_str() {
                "TRACK" => {
                    let n = words
                        .next()
                        .and_then(|s| s.parse::<u8>().ok())
                        .ok_or_else(|| invalid("invalid track number"))?;
                    if n == 0 || n > 99 || extras.insert(n, Extras::default()).is_some() {
                        return Err(invalid("duplicate or invalid track number"));
                    }
                    current = Some(n);
                }
                "PREGAP" | "POSTGAP" | "FLAGS" => {
                    let e = extras
                        .get_mut(&current.ok_or_else(|| invalid("gap/flags without track"))?)
                        .unwrap();
                    if word.eq_ignore_ascii_case("FLAGS") {
                        for flag in words {
                            match flag.to_ascii_uppercase().as_str() {
                                "PRE" => e.emphasis = true,
                                "DCP" => e.copy = true,
                                "SCMS" => {}
                                _ => {
                                    return Err(AnalysisError::unsupported(format!(
                                        "CUE audio flag {flag}"
                                    )));
                                }
                            }
                        }
                    } else {
                        let frames =
                            timestamp(words.next().ok_or_else(|| invalid("missing gap length"))?)?;
                        if words.next().is_some() {
                            return Err(invalid("extra gap fields"));
                        }
                        let target = if word.eq_ignore_ascii_case("PREGAP") {
                            &mut e.pre
                        } else {
                            &mut e.post
                        };
                        if target.replace(frames).is_some() {
                            return Err(invalid("duplicate gap directive"));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut result = Self {
            tracks: vec![],
            hidden_audio: None,
            extents: vec![],
            source_lengths,
            base: None,
        };
        let mut position = 0;
        let mut beginnings = Vec::new();
        for (file, source) in sheet.files.iter().zip(&resolved.files) {
            for (i, track) in file.tracks.iter().enumerate() {
                let kind = classify_mode(&track.mode);
                let audio = kind == TrackKind::Audio;
                let idx1 = track
                    .indexes
                    .iter()
                    .find(|idx| idx.number == 1)
                    .unwrap()
                    .to_sector_offset()
                    * 588;
                let idx0 = track
                    .indexes
                    .iter()
                    .find(|idx| idx.number == 0)
                    .map(|idx| idx.to_sector_offset() * 588);
                // Retain the complete first file prefix for hidden-track access.
                let first = if i == 0 { 0 } else { idx0.unwrap_or(idx1) };
                let end = file
                    .tracks
                    .get(i + 1)
                    .map(|t| {
                        t.indexes
                            .iter()
                            .find(|idx| idx.number == 0)
                            .or_else(|| t.indexes.iter().find(|idx| idx.number == 1))
                            .unwrap()
                            .to_sector_offset()
                            * 588
                    })
                    .unwrap_or(u64::from(source.file_sectors) * 588);
                if idx0.is_some_and(|v| v > idx1 || v < first) || idx1 < first || end <= idx1 {
                    return Err(invalid("overlapping CUE indexes"));
                }
                let e = &extras[&track.number];
                beginnings.push(position);
                if let Some(n) = e.pre {
                    result.push(&mut position, n, None, audio)?;
                }
                let storage_start = position;
                let mut indexes = Vec::new();
                let mut last = None;
                for idx in &track.indexes {
                    let offset = idx.to_sector_offset() * 588;
                    if offset < first
                        || offset >= end
                        || last.is_some_and(|(n, p)| idx.number <= n || offset < p)
                    {
                        return Err(invalid("invalid CUE index ordering"));
                    }
                    indexes.push((idx.number, storage_start + offset - first));
                    last = Some((idx.number, offset));
                }
                let start_frame = storage_start + idx1 - first;
                result.push(
                    &mut position,
                    end - first,
                    Some((file.filename.clone(), first / 588 * source.frame_bytes)),
                    audio,
                )?;
                if let Some(n) = e.post {
                    result.push(&mut position, n, None, audio)?;
                }
                result.tracks.push(CueAudioTrack {
                    number: track.number,
                    kind,
                    indexes,
                    start_frame,
                    end_frame: position,
                    pre_emphasis: e.emphasis,
                    copy_permitted: e.copy,
                    synthetic_pregap_frames: e.pre.unwrap_or(0),
                    synthetic_postgap_frames: e.post.unwrap_or(0),
                });
            }
        }
        for i in 0..result.tracks.len() {
            if i + 1 < result.tracks.len() {
                result.tracks[i].end_frame = if result.tracks[i + 1].kind == TrackKind::Audio {
                    result.tracks[i + 1].start_frame
                } else {
                    beginnings[i + 1]
                };
            }
        }
        if let Some(t) = result.tracks.first()
            && t.kind == TrackKind::Audio
            && t.start_frame > 0
        {
            result.hidden_audio = Some((0, t.start_frame));
        }
        Ok(result)
    }
    fn push(
        &mut self,
        position: &mut u64,
        n: u64,
        file: Option<(String, u64)>,
        audio: bool,
    ) -> Result<(), AnalysisError> {
        let end = position
            .checked_add(n)
            .ok_or_else(|| invalid("CUE playback timeline overflow"))?;
        if n != 0 {
            self.extents.push(Extent {
                start: *position,
                end,
                file,
                audio,
            });
        }
        *position = end;
        Ok(())
    }
    pub fn track_reader(&self, number: u8) -> Result<CueAudioReader, AnalysisError> {
        let t = self
            .tracks
            .iter()
            .find(|t| t.number == number)
            .ok_or_else(|| invalid("audio track absent"))?;
        if t.kind != TrackKind::Audio {
            return Err(AnalysisError::unsupported("non-audio CUE track"));
        }
        self.range_reader(t.start_frame, t.end_frame)
    }
    /// Read a range from the files next to the CUE sheet. Only for discs built
    /// with [`Self::open`]; use [`Self::range_reader_with`] otherwise.
    pub fn range_reader(&self, start: u64, end: u64) -> Result<CueAudioReader, AnalysisError> {
        let base = self.base.as_deref().ok_or_else(|| {
            AnalysisError::unsupported("CUE layout has no directory; use range_reader_with")
        })?;
        self.range_reader_with(start, end, |name| {
            let file = File::open(cue::resolve_local_file(base, name)?)?;
            Ok(Box::new(BufReader::new(file)))
        })
    }

    /// Read an audio range from caller-supplied sources, opened by CUE FILE
    /// name. Only files overlapping the range are opened, and each must still
    /// have the length recorded when the layout was built.
    pub fn range_reader_with(
        &self,
        start: u64,
        end: u64,
        mut open: impl FnMut(&str) -> Result<CueSource, AnalysisError>,
    ) -> Result<CueAudioReader, AnalysisError> {
        if start > end
            || end > self.extents.last().map(|e| e.end).unwrap_or(0)
            || self
                .extents
                .iter()
                .any(|e| e.start < end && e.end > start && !e.audio)
        {
            return Err(invalid("range includes non-audio or lies outside CUE"));
        }
        let mut sources = BTreeMap::new();
        for extent in self
            .extents
            .iter()
            .filter(|e| e.start < end && e.end > start)
        {
            if let Some((name, _)) = &extent.file
                && !sources.contains_key(name)
            {
                let mut source = open(name)?;
                let length = source.seek(SeekFrom::End(0))?;
                if length != self.source_lengths[name] {
                    return Err(invalid("CUE source length changed"));
                }
                sources.insert(name.clone(), (source, length));
            }
        }
        Ok(CueAudioReader {
            extents: self.extents.clone(),
            start,
            length: end - start,
            position: 0,
            sources,
        })
    }
}
pub struct CueAudioReader {
    extents: Vec<Extent>,
    start: u64,
    length: u64,
    position: u64,
    /// Open sources by CUE FILE name, with each one's current byte position.
    sources: BTreeMap<String, (CueSource, u64)>,
}
impl CueAudioReader {
    pub fn total_frames(&self) -> u64 {
        self.length
    }
    pub fn position_frames(&self) -> u64 {
        self.position
    }
    pub fn seek_frame(&mut self, frame: u64) -> Result<(), AnalysisError> {
        if frame > self.length {
            return Err(invalid("seek beyond CUE range"));
        }
        self.position = frame;
        Ok(())
    }
    pub fn read_frames(&mut self, out: &mut [[i16; 2]]) -> Result<usize, AnalysisError> {
        if out.is_empty() || self.position == self.length {
            return Ok(0);
        }
        let absolute = self.start + self.position;
        let e = self
            .extents
            .iter()
            .find(|e| e.start <= absolute && absolute < e.end)
            .ok_or_else(|| invalid("hole in CUE timeline"))?;
        let n = out
            .len()
            .min(588)
            .min((e.end - absolute).min(self.length - self.position).min(588) as usize);
        if let Some((name, offset)) = &e.file {
            let (r, at) = self
                .sources
                .get_mut(name)
                .ok_or_else(|| invalid("CUE source not opened"))?;
            let target = offset + (absolute - e.start) * 4;
            // Skip the seek for sequential reads so buffered sources stay buffered.
            if *at != target {
                r.seek(SeekFrom::Start(target))?;
                *at = target;
            }
            let mut bytes = [0; 2352];
            if let Err(error) = r.read_exact(&mut bytes[..n * 4]) {
                *at = u64::MAX; // Position unknown after a partial read.
                return Err(error.into());
            }
            *at += n as u64 * 4;
            for (o, b) in out[..n].iter_mut().zip(bytes.as_chunks::<4>().0) {
                *o = [
                    i16::from_le_bytes([b[0], b[1]]),
                    i16::from_le_bytes([b[2], b[3]]),
                ];
            }
        } else {
            out[..n].fill([0, 0]);
        }
        self.position += n as u64;
        Ok(n)
    }
}

impl crate::pcm::CdPcmReader for CueAudioReader {
    fn total_frames(&self) -> u64 {
        Self::total_frames(self)
    }

    fn seek_frame(&mut self, frame: u64) -> Result<(), AnalysisError> {
        Self::seek_frame(self, frame)
    }

    fn read_frames(&mut self, output: &mut [[i16; 2]]) -> Result<usize, AnalysisError> {
        Self::read_frames(self, output)
    }
}
