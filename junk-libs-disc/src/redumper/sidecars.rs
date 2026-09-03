//! Filesystem discovery of redumper sidecars next to a CUE file.

use std::path::{Path, PathBuf};

use junk_libs_core::AnalysisError;

use super::Ripper;

/// Paths to redumper sidecar artifacts matching a given CUE's basename.
///
/// All fields are `None` when the corresponding file is absent — missing
/// sidecars are a normal state (users may delete logs, or the rip may
/// predate `.fulltoc` support). Callers should not treat absence as an
/// error.
#[derive(Debug, Clone, Default)]
pub struct Sidecars {
    /// `<name>.scram` — current CD main channel on redumper's sample timeline.
    pub scram: Option<PathBuf>,
    /// `<name>.state` — one quality-state byte per four-byte `.scram` sample.
    pub state: Option<PathBuf>,
    /// `<name>.scrap` — obsolete, mutually exclusive CD main-channel form.
    pub legacy_scrap: Option<PathBuf>,
    /// `<name>.sdram` — raw DVD recording-frame stream.
    pub sdram: Option<PathBuf>,
    /// `<name>.sbram` — raw Blu-ray data-frame stream.
    pub sbram: Option<PathBuf>,
    /// `<name>.log` — primary text log with drive/offset/MCN/ISRCs.
    pub log: Option<PathBuf>,
    /// `<name>.cdtext` — binary CD-TEXT dump.
    pub cdtext: Option<PathBuf>,
    /// `<name>.toc` — raw TOC (READ TOC command output).
    pub toc: Option<PathBuf>,
    /// `<name>.fulltoc` — full TOC (READ TOC format 2).
    pub fulltoc: Option<PathBuf>,
    /// `<name>.subcode` — raw subchannel (P-W) dump.
    pub subcode: Option<PathBuf>,
    /// `<name>.atip` — ATIP info (CD-R/RW only).
    pub atip: Option<PathBuf>,
    /// `<name>.pma` — optional Program Memory Area response.
    pub pma: Option<PathBuf>,
    /// `<name>.cache` — optional MediaTek-family drive cache evidence.
    pub cache: Option<PathBuf>,
}

impl Sidecars {
    /// True when no sidecars were found at all.
    pub fn is_empty(&self) -> bool {
        self.scram.is_none()
            && self.state.is_none()
            && self.legacy_scrap.is_none()
            && self.sdram.is_none()
            && self.sbram.is_none()
            && self.log.is_none()
            && self.cdtext.is_none()
            && self.toc.is_none()
            && self.fulltoc.is_none()
            && self.subcode.is_none()
            && self.atip.is_none()
            && self.pma.is_none()
            && self.cache.is_none()
    }
}

/// Look for redumper sidecars matching a CUE's basename in its directory.
///
/// Given `/path/to/foo.cue`, checks for `foo.log`, `foo.cdtext`, `foo.toc`,
/// `foo.fulltoc`, `foo.subcode`, `foo.atip` in `/path/to/` and populates
/// [`Sidecars`] with whichever exist.
///
/// No I/O happens on the sidecars themselves beyond `exists()` checks.
/// Call [`parse_log`](super::parse_log) / [`parse_cdtext`](super::parse_cdtext)
/// on the returned paths for actual content.
///
/// Returns an empty [`Sidecars`] (all `None`) if `cue_path` has no parent
/// directory or no file stem — both pathological cases, not errors.
pub fn find_sidecars(cue_path: &Path) -> Sidecars {
    let Some(parent) = cue_path.parent() else {
        return Sidecars::default();
    };
    let Some(stem) = cue_path.file_stem() else {
        return Sidecars::default();
    };

    // Build candidates as `"<stem>.<ext>"` manually. `PathBuf::set_extension`
    // would rewrite the portion *after the last dot in the stem*, which
    // corrupts basenames with embedded dots — e.g. a stem like
    // `foo..bar.BAZ-` becomes `foo..bar.log` instead of
    // `foo..bar.BAZ-.log`, so sibling sidecars go undiscovered.
    let candidate = |ext: &str| -> Option<PathBuf> {
        let mut name = stem.to_os_string();
        name.push(".");
        name.push(ext);
        let p = parent.join(name);
        p.exists().then_some(p)
    };

    Sidecars {
        scram: candidate("scram"),
        state: candidate("state"),
        legacy_scrap: candidate("scrap"),
        sdram: candidate("sdram"),
        sbram: candidate("sbram"),
        log: candidate("log"),
        cdtext: candidate("cdtext"),
        toc: candidate("toc"),
        fulltoc: candidate("fulltoc"),
        subcode: candidate("subcode"),
        atip: candidate("atip"),
        pma: candidate("pma"),
        cache: candidate("cache"),
    }
}

/// Structural facts checked for a current redumper CD raw package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdRawStructure {
    /// Four-byte stereo sample frames in both `.scram` and `.state`.
    pub sample_frames: u64,
    /// 96-byte raw multiplexed subchannel frames in `.subcode`.
    pub subcode_frames: u64,
    /// Main-channel sample frames minus `subcode_frames * 588`.
    ///
    /// This is intentionally signed. A Redumper package made with a drive
    /// read offset of `+6`, for example, has a delta of `-6`: the corrected
    /// main-channel timeline contains six fewer sample frames than the
    /// sector-addressed subchannel timeline. It is alignment evidence for
    /// later extraction code, not by itself a corruption indicator.
    pub sample_frame_delta: i64,
}

/// Validate the byte-level invariants required by current redumper CD split.
/// This is structural evidence only; it does not verify track boundaries or
/// hashes and therefore cannot authorize deletion of the source package.
pub fn validate_current_cd_raw(sidecars: &Sidecars) -> Result<CdRawStructure, AnalysisError> {
    if sidecars.legacy_scrap.is_some() {
        return Err(AnalysisError::unsupported(
            "legacy .scrap packages require a matching historical redumper build",
        ));
    }
    if sidecars.sdram.is_some() || sidecars.sbram.is_some() {
        return Err(AnalysisError::invalid_format(
            "DVD/Blu-ray raw streams cannot be validated as a CD .scram package",
        ));
    }
    fn required<'a>(
        path: &'a Option<PathBuf>,
        extension: &str,
    ) -> Result<&'a PathBuf, AnalysisError> {
        path.as_ref().ok_or_else(|| {
            AnalysisError::invalid_format(format!(
                "current redumper CD package is missing .{extension}"
            ))
        })
    }
    let scram = required(&sidecars.scram, "scram")?;
    let state = required(&sidecars.state, "state")?;
    let subcode = required(&sidecars.subcode, "subcode")?;
    let toc = required(&sidecars.toc, "toc")?;

    let scram_len = std::fs::metadata(scram)?.len();
    let state_len = std::fs::metadata(state)?.len();
    let subcode_len = std::fs::metadata(subcode)?.len();
    let expected_scram_len = state_len
        .checked_mul(4)
        .ok_or_else(|| AnalysisError::corrupted_header(".state sample count overflow"))?;
    if !scram_len.is_multiple_of(4) || scram_len != expected_scram_len {
        return Err(AnalysisError::corrupted_header(format!(
            ".scram/.state sample timelines disagree: {scram_len} bytes versus {state_len} states"
        )));
    }
    if subcode_len == 0 || !subcode_len.is_multiple_of(96) {
        return Err(AnalysisError::corrupted_header(format!(
            ".subcode length {subcode_len} is not a nonzero multiple of 96"
        )));
    }
    let subcode_frames = subcode_len / 96;
    let nominal_sample_frames = subcode_frames
        .checked_mul(588)
        .ok_or_else(|| AnalysisError::corrupted_header("CD sample timeline overflow"))?;
    let sample_frame_delta =
        i64::try_from(i128::from(state_len) - i128::from(nominal_sample_frames))
            .map_err(|_| AnalysisError::corrupted_header("CD sample timeline delta overflow"))?;
    validate_scsi_response_length(toc, ".toc")?;
    Ok(CdRawStructure {
        sample_frames: state_len,
        subcode_frames,
        sample_frame_delta,
    })
}

fn validate_scsi_response_length(path: &Path, label: &str) -> Result<(), AnalysisError> {
    use std::io::Read;

    let actual = std::fs::metadata(path)?.len();
    if actual < 4 {
        return Err(AnalysisError::corrupted_header(format!(
            "{label} is shorter than its four-byte SCSI response header"
        )));
    }
    let mut header = [0u8; 2];
    std::fs::File::open(path)?.read_exact(&mut header)?;
    let declared = u64::from(u16::from_be_bytes(header)) + 2;
    if actual != declared {
        return Err(AnalysisError::corrupted_header(format!(
            "{label} length disagrees with its SCSI response header: {actual} bytes present, {declared} declared"
        )));
    }
    Ok(())
}

/// Classify a `.log` file by reading its opening bytes.
///
/// Each major ripper writes a distinctive header in its log:
/// - **redumper**: an opening line begins with `redumper`, for example
///   `redumper (build: 709)`.
/// - **EAC**: first non-empty line begins with `Exact Audio Copy`.
/// - **XLD**: begins with `X Lossless Decoder version`.
/// - **dBpoweramp / CUERipper**: includes `dBpoweramp` or `CUERipper` in the
///   first few header lines.
///
/// Returns [`Ripper::Unknown`] when nothing matches, so callers can still
/// record "a log is here but we don't understand it" as a state distinct
/// from "no log at all."
///
/// Only the first ~2 KiB is read — enough for any known header without
/// loading a multi-megabyte log just to sniff it.
pub fn sniff_ripper(log_path: &Path) -> Result<Ripper, AnalysisError> {
    use std::io::Read;

    let mut file = std::fs::File::open(log_path)?;
    let mut buf = [0u8; 2048];
    let n = file.read(&mut buf)?;
    let head = String::from_utf8_lossy(&buf[..n]);

    for line in head.lines().take(20) {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("redumper") {
            return Ok(Ripper::Redumper);
        }
        if lower.starts_with("exact audio copy") || lower.contains("exact audio copy v") {
            return Ok(Ripper::Eac);
        }
        if lower.starts_with("x lossless decoder") {
            return Ok(Ripper::Xld);
        }
        if lower.contains("cueripper") {
            return Ok(Ripper::Cueripper);
        }
        if lower.contains("dbpoweramp") {
            return Ok(Ripper::DbPoweramp);
        }
    }

    Ok(Ripper::Unknown)
}
