//! Bounded maximum-cardinality bipartite matching. Metadata hints never create
//! recording matches and one candidate slot cannot satisfy multiple files.
use crate::*;
use std::collections::BTreeSet;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mapping {
    pub key: String,
    pub medium: u16,
    pub position: u16,
    pub recording_id: String,
    pub score: f64,
    pub duration_delta_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlbumMatch {
    pub release: Release,
    pub mappings: Vec<Mapping>,
    pub local_tracks: usize,
    pub release_tracks: usize,
    pub complete_tracklist: bool,
    pub ambiguous_mapping: bool,
    pub duration_conflicts: usize,
    pub unmatched: Vec<String>,
}
fn augment(
    local: usize,
    edges: &[Vec<usize>],
    slots: &mut [Option<usize>],
    seen: &mut [bool],
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<bool> {
    checkpoint()?;
    for &slot in &edges[local] {
        if seen[slot] {
            continue;
        }
        seen[slot] = true;
        if slots[slot].is_none() || augment(slots[slot].unwrap(), edges, slots, seen, checkpoint)? {
            slots[slot] = Some(local);
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn compare(
    local: &[LocalTrack],
    release: &Release,
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<AlbumMatch> {
    checkpoint()?;
    if local.is_empty()
        || local.len() > MAX_TRACKS
        || release.media.len() > 99
        || !valid_mbid(&release.id)
    {
        return Err(Error::Invalid("album size/identity".into()));
    }
    let mut keys = BTreeSet::new();
    for t in local {
        if t.key.is_empty()
            || !text(&t.key)
            || !keys.insert(&t.key)
            || t.duration_ms == 0
            || t.duration_ms > 86_400_000
            || t.recordings.len() > MAX_RECORDINGS_PER_TRACK
            || t.recordings.iter().any(|r| {
                !valid_mbid(&r.recording_id)
                    || !r.score.is_finite()
                    || !(0.0..=1.0).contains(&r.score)
            })
        {
            return Err(Error::Invalid("local track envelope".into()));
        }
    }
    let mut slots = Vec::new();
    let mut positions = BTreeSet::new();
    let mut complete = true;
    let mut declared = 0usize;
    for m in &release.media {
        declared = declared
            .checked_add(m.track_count as usize)
            .ok_or_else(|| Error::Invalid("track count".into()))?;
        complete &= m.position.is_some() && m.track_count as usize == m.tracks.len();
        for t in &m.tracks {
            if slots.len() >= MAX_TRACKS
                || t.position == 0
                || m.position == Some(0)
                || !positions.insert((m.position, t.position))
                || t.recording_id.as_deref().is_some_and(|r| !valid_mbid(r))
                || !text(&t.title)
                || !text(&t.artist)
            {
                return Err(Error::Invalid("release track envelope".into()));
            }
            slots.push((m.position.unwrap_or(0), t));
        }
    }
    let mut edges = Vec::new();
    for t in local {
        checkpoint()?;
        let mut candidates: Vec<_> = slots
            .iter()
            .enumerate()
            .filter(|(_, (_, s))| {
                s.recording_id.as_ref().is_some_and(|id| {
                    t.recordings
                        .iter()
                        .any(|r| &r.recording_id == id && r.score >= 0.5)
                })
            })
            .map(|(i, _)| i)
            .collect();
        candidates.sort_by_key(|&i| {
            let (disc, s) = slots[i];
            (
                t.disc.is_some_and(|v| v != disc),
                t.position.is_some_and(|v| v != s.position),
                s.duration_ms
                    .map_or(u64::MAX, |d| d.abs_diff(t.duration_ms)),
                i,
            )
        });
        edges.push(candidates);
    }
    let mut occupied = vec![None; slots.len()];
    // Place the most constrained files first; augmenting paths still find the
    // maximum matching when two recordings compete for one position.
    let mut order: Vec<_> = (0..local.len()).collect();
    order.sort_by_key(|&i| edges[i].len());
    for i in order {
        augment(
            i,
            &edges,
            &mut occupied,
            &mut vec![false; slots.len()],
            checkpoint,
        )?;
    }
    // Maximum assignments are ambiguous only when an alternating path can
    // exchange an unmatched endpoint or when an alternating cycle exists.
    let mut owner_slot = vec![None; local.len()];
    for (slot, owner) in occupied.iter().enumerate() {
        if let Some(owner) = owner {
            owner_slot[*owner] = Some(slot);
        }
    }
    let mut graph = vec![Vec::new(); local.len()];
    let mut ambiguous = false;
    for (i, links) in edges.iter().enumerate() {
        if owner_slot[i].is_none() {
            ambiguous |= !links.is_empty();
            continue;
        }
        for &slot in links {
            if Some(slot) == owner_slot[i] {
                continue;
            }
            if let Some(other) = occupied[slot] {
                graph[i].push(other);
            } else {
                ambiguous = true;
            }
        }
    }
    fn cycle(
        i: usize,
        graph: &[Vec<usize>],
        color: &mut [u8],
        checkpoint: &mut dyn FnMut() -> Result<()>,
    ) -> Result<bool> {
        checkpoint()?;
        if color[i] == 1 {
            return Ok(true);
        }
        if color[i] == 2 {
            return Ok(false);
        }
        color[i] = 1;
        for &next in &graph[i] {
            if cycle(next, graph, color, checkpoint)? {
                return Ok(true);
            }
        }
        color[i] = 2;
        Ok(false)
    }
    let mut color = vec![0; local.len()];
    for i in 0..local.len() {
        ambiguous |= cycle(i, &graph, &mut color, checkpoint)?;
    }
    let mut mappings = Vec::new();
    let mut assigned = BTreeSet::new();
    let mut conflicts = 0;
    for (slot, owner) in occupied.into_iter().enumerate() {
        let Some(i) = owner else {
            continue;
        };
        let (disc, t) = slots[slot];
        let id = t.recording_id.as_ref().expect("edge recording");
        let score = local[i]
            .recordings
            .iter()
            .filter(|r| &r.recording_id == id)
            .map(|r| r.score)
            .max_by(f64::total_cmp)
            .unwrap();
        let delta = t.duration_ms.map(|d| d.abs_diff(local[i].duration_ms));
        conflicts += usize::from(delta.is_some_and(|d| d > 2000));
        assigned.insert(i);
        mappings.push(Mapping {
            key: local[i].key.clone(),
            medium: disc,
            position: t.position,
            recording_id: id.clone(),
            score,
            duration_delta_ms: delta,
        });
    }
    mappings.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(AlbumMatch {
        release: release.clone(),
        mappings,
        local_tracks: local.len(),
        release_tracks: declared,
        complete_tracklist: complete && !slots.is_empty(),
        ambiguous_mapping: ambiguous,
        duration_conflicts: conflicts,
        unmatched: local
            .iter()
            .enumerate()
            .filter(|(i, _)| !assigned.contains(i))
            .map(|(_, t)| t.key.clone())
            .collect(),
    })
}
pub fn rank(
    local: &[LocalTrack],
    releases: &[Release],
    checkpoint: &mut dyn FnMut() -> Result<()>,
) -> Result<Vec<AlbumMatch>> {
    if releases.len() > MAX_RELEASES {
        return Err(Error::Invalid("too many candidate releases".into()));
    }
    let mut ids = BTreeSet::new();
    let mut result = Vec::new();
    for r in releases {
        checkpoint()?;
        if !ids.insert(&r.id) {
            return Err(Error::Invalid("duplicate release identity".into()));
        }
        let m = compare(local, r, checkpoint)?;
        if !m.mappings.is_empty() {
            result.push(m);
        }
    }
    result.sort_by_key(|m| {
        (
            std::cmp::Reverse(m.mappings.len()),
            !m.complete_tracklist,
            m.duration_conflicts,
            m.release_tracks.saturating_sub(m.mappings.len()),
            m.ambiguous_mapping,
            !m.release
                .media
                .iter()
                .all(|d| d.format.as_deref() == Some("Digital Media")),
            m.release.id.clone(),
        )
    });
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u8) -> String {
        format!("00000000-0000-4000-8000-{n:012}")
    }
    fn local(key: &str, ids: &[u8]) -> LocalTrack {
        LocalTrack {
            key: key.into(),
            duration_ms: 180000,
            disc: None,
            position: None,
            recordings: ids
                .iter()
                .map(|&n| RecordingMatch {
                    recording_id: id(n),
                    score: 0.9,
                })
                .collect(),
        }
    }
    fn release() -> Release {
        Release {
            id: id(9),
            title: "Album".into(),
            artist: "Artist".into(),
            artist_ids: vec![],
            release_group_id: None,
            disambiguation: None,
            language: None,
            script: None,
            date: None,
            country: None,
            barcode: None,
            media: vec![Medium {
                position: Some(1),
                format: Some("Digital Media".into()),
                track_count: 2,
                disc_ids: vec![],
                tracks: vec![
                    Track {
                        position: 1,
                        title: "One".into(),
                        artist: String::new(),
                        recording_id: Some(id(1)),
                        duration_ms: Some(180000),
                    },
                    Track {
                        position: 2,
                        title: "Two".into(),
                        artist: String::new(),
                        recording_id: Some(id(2)),
                        duration_ms: Some(180000),
                    },
                ],
            }],
        }
    }
    #[test]
    fn augmenting_path_avoids_greedy_loss() {
        let m = compare(
            &[local("a", &[1, 2]), local("b", &[1])],
            &release(),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(m.mappings.len(), 2);
        assert!(!m.ambiguous_mapping);
        assert_eq!(
            m.mappings.iter().find(|m| m.key == "a").unwrap().position,
            2
        );
    }
    #[test]
    fn duplicate_files_do_not_reuse_a_position() {
        let m = compare(
            &[local("a", &[1]), local("b", &[1])],
            &release(),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(m.mappings.len(), 1);
        assert_eq!(m.unmatched.len(), 1);
        assert!(m.ambiguous_mapping);
    }
    #[test]
    fn incomplete_tracklist_never_claims_complete() {
        let mut r = release();
        r.media[0].track_count = 3;
        let m = compare(&[local("a", &[1])], &r, &mut || Ok(())).unwrap();
        assert!(!m.complete_tracklist);
        assert_eq!(m.release_tracks, 3);
    }
    #[test]
    fn cancellation_interrupts_matching() {
        assert!(matches!(
            compare(&[local("a", &[1])], &release(), &mut || Err(
                Error::Cancelled
            )),
            Err(Error::Cancelled)
        ));
    }
    #[test]
    fn duplicate_slots_fail_closed() {
        let mut r = release();
        r.media[0].tracks[1].position = 1;
        assert!(compare(&[local("a", &[1])], &r, &mut || Ok(())).is_err());
    }
}
