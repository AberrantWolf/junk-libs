//! Neutral audio-CD TOC coordinates and deterministic external identifiers.

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use thiserror::Error;

const LEAD_IN_FRAMES: u32 = 150;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TocError {
    #[error("invalid audio-CD TOC: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toc {
    pub first_track: u8,
    pub last_track: u8,
    pub leadout_sector: u32,
    pub track_offsets: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackSpan {
    pub position: u8,
    pub start_sector: u32,
    pub length_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalculatedDiscIds {
    pub musicbrainz: String,
    pub freedb: String,
    pub accuraterip_id1: String,
    pub accuraterip_id2: String,
}

impl Toc {
    pub fn validate(&self) -> Result<(), TocError> {
        if self.first_track == 0 || self.first_track > self.last_track || self.last_track > 99 {
            return Err(TocError::Invalid("track range must be within 1..99".into()));
        }
        let expected = usize::from(self.last_track - self.first_track + 1);
        if self.track_offsets.len() != expected {
            return Err(TocError::Invalid(format!(
                "track range declares {expected} tracks but {} offsets were supplied",
                self.track_offsets.len()
            )));
        }
        if self.track_offsets.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(TocError::Invalid(
                "track offsets must be strictly increasing".into(),
            ));
        }
        if self
            .track_offsets
            .last()
            .is_none_or(|last| self.leadout_sector <= *last)
        {
            return Err(TocError::Invalid(
                "lead-out must follow the final track".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn track_count(&self) -> usize {
        self.track_offsets.len()
    }

    #[must_use]
    pub fn track_length_frames(&self, index: usize) -> Option<u64> {
        let start = *self.track_offsets.get(index)?;
        let end = self
            .track_offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.leadout_sector);
        end.checked_sub(start).map(u64::from)
    }

    pub fn iter_track_spans(&self) -> impl Iterator<Item = TrackSpan> + '_ {
        self.track_offsets
            .iter()
            .enumerate()
            .filter_map(|(index, &start)| {
                Some(TrackSpan {
                    position: self.first_track.checked_add(u8::try_from(index).ok()?)?,
                    start_sector: start,
                    length_frames: self.track_length_frames(index)?,
                })
            })
    }

    #[must_use]
    pub fn total_length_frames(&self) -> u64 {
        self.track_offsets
            .first()
            .and_then(|first| self.leadout_sector.checked_sub(*first))
            .map_or(0, u64::from)
    }
}

pub fn calculate_disc_ids(toc: &Toc) -> Result<CalculatedDiscIds, TocError> {
    toc.validate()?;
    let (accuraterip_id1, accuraterip_id2) = accuraterip_disc_ids_unchecked(toc);
    Ok(CalculatedDiscIds {
        musicbrainz: musicbrainz_disc_id_unchecked(toc),
        freedb: freedb_disc_id_unchecked(toc),
        accuraterip_id1,
        accuraterip_id2,
    })
}

pub fn musicbrainz_disc_id(toc: &Toc) -> Result<String, TocError> {
    toc.validate()?;
    Ok(musicbrainz_disc_id_unchecked(toc))
}

pub fn freedb_disc_id(toc: &Toc) -> Result<String, TocError> {
    toc.validate()?;
    Ok(freedb_disc_id_unchecked(toc))
}

pub fn accuraterip_disc_ids(toc: &Toc) -> Result<(String, String), TocError> {
    toc.validate()?;
    Ok(accuraterip_disc_ids_unchecked(toc))
}

fn musicbrainz_disc_id_unchecked(toc: &Toc) -> String {
    use base64::Engine;

    let mut input = String::with_capacity(804);
    input.push_str(&format!(
        "{:02X}{:02X}{:08X}",
        toc.first_track, toc.last_track, toc.leadout_sector
    ));
    for number in 1..=99_u8 {
        let offset = number
            .checked_sub(toc.first_track)
            .map(usize::from)
            .filter(|_| number <= toc.last_track)
            .and_then(|index| toc.track_offsets.get(index))
            .copied()
            .unwrap_or(0);
        input.push_str(&format!("{offset:08X}"));
    }
    let digest = Sha1::new().chain_update(input.as_bytes()).finalize();
    base64::engine::general_purpose::STANDARD
        .encode(digest)
        .replace('+', ".")
        .replace('/', "_")
        .replace('=', "-")
}

fn digit_sum(mut value: u32) -> u32 {
    let mut sum = 0_u32;
    while value > 0 {
        sum += value % 10;
        value /= 10;
    }
    sum
}

fn freedb_disc_id_unchecked(toc: &Toc) -> String {
    let checksum = toc.track_offsets.iter().fold(0_u32, |sum, offset| {
        sum.wrapping_add(digit_sum(offset / 75))
    });
    let seconds = (toc.leadout_sector - toc.track_offsets[0]) / 75;
    let id = ((checksum % 0xff) << 24) | (seconds << 8) | toc.track_count() as u32;
    format!("{id:08x}")
}

fn accuraterip_disc_ids_unchecked(toc: &Toc) -> (String, String) {
    let leadout = toc.leadout_sector.saturating_sub(LEAD_IN_FRAMES);
    let mut id1 = leadout;
    let mut id2 = leadout.wrapping_mul(toc.track_count() as u32 + 1);
    for (index, offset) in toc.track_offsets.iter().enumerate() {
        let lsn = offset.saturating_sub(LEAD_IN_FRAMES);
        id1 = id1.wrapping_add(lsn);
        let nonzero_lsn = if lsn == 0 { 1 } else { lsn };
        let number = u32::from(toc.first_track) + index as u32;
        id2 = id2.wrapping_add(nonzero_lsn.wrapping_mul(number));
    }
    (format!("{id1:08x}"), format!("{id2:08x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arver_toc() -> Toc {
        Toc {
            first_track: 1,
            last_track: 3,
            leadout_sector: 336_103,
            track_offsets: vec![150, 75_408, 130_223],
        }
    }

    #[test]
    fn published_arver_vector_produces_all_ids() {
        let ids = calculate_disc_ids(&arver_toc()).unwrap();
        assert_eq!(ids.musicbrainz, "dUmct3Sk4dAt1a98qUKYKC0ZjYU-");
        assert_eq!(ids.freedb, "19117f03");
        assert_eq!(ids.accuraterip_id1, "00084264");
        assert_eq!(ids.accuraterip_id2, "001cc184");
    }

    #[test]
    fn malformed_toc_fails_instead_of_wrapping() {
        let mut toc = arver_toc();
        toc.last_track = 4;
        assert!(calculate_disc_ids(&toc).is_err());
        toc.last_track = 3;
        toc.track_offsets[1] = toc.track_offsets[0];
        assert!(calculate_disc_ids(&toc).is_err());
    }

    #[test]
    fn published_libdiscid_22_track_vector_matches() {
        let toc = Toc {
            first_track: 1,
            last_track: 22,
            leadout_sector: 303_602,
            track_offsets: vec![
                150, 9_700, 25_887, 39_297, 53_795, 63_735, 77_517, 94_877, 107_270, 123_552,
                135_522, 148_422, 161_197, 174_790, 192_022, 205_545, 218_010, 228_700, 239_590,
                255_470, 266_932, 288_750,
            ],
        };
        let ids = calculate_disc_ids(&toc).unwrap();
        assert_eq!(ids.musicbrainz, "xUp1F2NkfP8s8jaeFn_Av3jNEI4-");
        assert_eq!(ids.freedb, "370fce16");
    }
}
