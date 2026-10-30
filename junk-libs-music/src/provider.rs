//! Community-owned MusicBrainz entity schemas, small tolerant result envelopes,
//! parameter-safe URLs and bounded AcoustID interpretation. No network I/O.
use crate::*;
use musicbrainz_rs::entity::release::Release as MbRelease;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

pub fn release_url(id: &str) -> Result<url::Url> {
    if !valid_mbid(id) {
        return Err(Error::Invalid("release MBID".into()));
    }
    let mut url = url::Url::parse(&format!("https://musicbrainz.org/ws/2/release/{id}"))
        .expect("fixed origin");
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair(
            "inc",
            "artist-credits+recordings+release-groups+discids+labels",
        );
    Ok(url)
}
pub fn recording_releases_url(recording: &str, offset: u32) -> Result<url::Url> {
    if !valid_mbid(recording) || offset > 10000 {
        return Err(Error::Invalid("recording browse input".into()));
    }
    let mut url = url::Url::parse("https://musicbrainz.org/ws/2/release").expect("fixed origin");
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair("recording", recording)
        .append_pair("limit", "100")
        .append_pair("offset", &offset.to_string());
    Ok(url)
}
pub fn search_url(query: &str, offset: u32) -> Result<url::Url> {
    if query.trim().is_empty() || query.len() > 1024 || !text(query) || offset > 10000 {
        return Err(Error::Invalid("search input".into()));
    }
    let mut url = url::Url::parse("https://musicbrainz.org/ws/2/release/").expect("fixed origin");
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair("query", query)
        .append_pair("limit", "25")
        .append_pair("offset", &offset.to_string());
    Ok(url)
}
#[derive(Deserialize)]
struct Envelope {
    releases: Vec<MbRelease>,
    #[serde(default, rename = "release-count", alias = "count")]
    count: Option<u32>,
    #[serde(default, rename = "release-offset", alias = "offset")]
    offset: u32,
}
#[derive(Clone, Debug)]
pub struct ReleasePage {
    pub releases: Vec<Release>,
    pub total: Option<u32>,
    pub offset: u32,
}
pub fn parse_releases(bytes: &[u8], single: bool) -> Result<ReleasePage> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(Error::Invalid("MusicBrainz response exceeds 8 MiB".into()));
    }
    let (values, total, offset) = if single {
        (
            vec![serde_json::from_slice::<MbRelease>(bytes).map_err(json_error)?],
            Some(1),
            0,
        )
    } else {
        let e: Envelope = serde_json::from_slice(bytes).map_err(json_error)?;
        (e.releases, e.count, e.offset)
    };
    if values.len() > MAX_RELEASES {
        return Err(Error::Invalid("too many releases".into()));
    }
    Ok(ReleasePage {
        releases: values.into_iter().map(convert).collect::<Result<_>>()?,
        total,
        offset,
    })
}
fn json_error(e: serde_json::Error) -> Error {
    Error::Invalid(format!("music provider JSON: {e}"))
}
fn credit(values: Option<&[musicbrainz_rs::entity::artist_credit::ArtistCredit]>) -> String {
    values
        .unwrap_or_default()
        .iter()
        .map(|c| format!("{}{}", c.name, c.joinphrase.as_deref().unwrap_or("")))
        .collect()
}
fn convert(r: MbRelease) -> Result<Release> {
    let language = r
        .text_representation
        .as_ref()
        .and_then(|t| t.language.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .map_err(json_error)?
        .and_then(|v| v.as_str().map(str::to_owned));
    let script = r
        .text_representation
        .as_ref()
        .and_then(|t| t.script.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .map_err(json_error)?
        .and_then(|v| v.as_str().map(str::to_owned));
    let mut count = 0usize;
    let mut media = Vec::new();
    let mut media_positions = BTreeSet::new();
    for m in r.media.unwrap_or_default() {
        if m.position
            .is_some_and(|position| position == 0 || !media_positions.insert(position))
        {
            return Err(Error::Invalid("duplicate/zero medium position".into()));
        }
        let mut track_positions = BTreeSet::new();
        let mut tracks = Vec::new();
        for t in m.tracks.unwrap_or_default() {
            count += 1;
            if count > 32_768 {
                return Err(Error::Invalid(
                    "provider release exceeds 32768 tracks".into(),
                ));
            }
            let title = if t.title.trim().is_empty() {
                t.recording
                    .as_ref()
                    .map(|r| r.title.clone())
                    .unwrap_or_default()
            } else {
                t.title
            };
            if t.position == 0
                || !track_positions.insert(t.position)
                || !text(&title)
                || t.recording
                    .as_ref()
                    .is_some_and(|recording| !valid_mbid(&recording.id))
            {
                return Err(Error::Invalid("track identity/position/text".into()));
            }
            let artist = credit(
                t.artist_credit
                    .as_deref()
                    .filter(|credits| !credits.is_empty())
                    .or_else(|| {
                        t.recording
                            .as_ref()
                            .and_then(|r| r.artist_credit.as_deref())
                    }),
            );
            if !text(&artist) {
                return Err(Error::Invalid("track artist text".into()));
            }
            tracks.push(Track {
                position: u16::try_from(t.position)
                    .map_err(|_| Error::Invalid("track position".into()))?,
                title,
                artist,
                recording_id: t.recording.as_ref().map(|r| r.id.clone()),
                duration_ms: t
                    .length
                    .or_else(|| t.recording.as_ref().and_then(|r| r.length))
                    .map(u64::from),
            });
        }
        media.push(Medium {
            position: m
                .position
                .map(u16::try_from)
                .transpose()
                .map_err(|_| Error::Invalid("medium position".into()))?,
            format: m.format,
            track_count: m.track_count,
            disc_ids: m
                .discs
                .unwrap_or_default()
                .into_iter()
                .map(|d| d.id)
                .collect(),
            tracks,
        });
        if media.len() > 128 {
            return Err(Error::Invalid("too many media".into()));
        }
    }
    let value = Release {
        id: r.id,
        title: r.title,
        artist: credit(r.artist_credit.as_deref()),
        artist_ids: r
            .artist_credit
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.artist.id)
            .collect(),
        release_group_id: r.release_group.map(|g| g.id),
        disambiguation: r.disambiguation,
        language,
        script,
        date: r.date.map(|v| v.to_string()),
        country: r.country,
        barcode: r.barcode,
        media,
    };
    if !valid_mbid(&value.id) || !text(&value.title) || !text(&value.artist) {
        return Err(Error::Invalid("release identity/text".into()));
    }
    Ok(value)
}
#[derive(Deserialize)]
struct AcoustId {
    status: String,
    results: Vec<AcoustResult>,
}
#[derive(Deserialize)]
struct AcoustResult {
    score: f64,
    #[serde(default)]
    recordings: Vec<AcoustRecording>,
}
#[derive(Deserialize)]
struct AcoustRecording {
    id: String,
}
pub fn parse_acoustid(bytes: &[u8]) -> Result<Vec<RecordingMatch>> {
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Invalid("AcoustID response exceeds 1 MiB".into()));
    }
    let response: AcoustId = serde_json::from_slice(bytes).map_err(json_error)?;
    if response.status != "ok" || response.results.len() > 128 {
        return Err(Error::Invalid(
            "AcoustID refused or oversized response".into(),
        ));
    }
    let mut values = BTreeMap::<String, f64>::new();
    for result in response.results {
        if !result.score.is_finite()
            || !(0.0..=1.0).contains(&result.score)
            || result.recordings.len() > 256
        {
            return Err(Error::Invalid("AcoustID result envelope".into()));
        }
        for recording in result.recordings {
            if !valid_mbid(&recording.id) {
                return Err(Error::Invalid("AcoustID recording ID".into()));
            }
            values
                .entry(recording.id)
                .and_modify(|s| *s = s.max(result.score))
                .or_insert(result.score);
        }
    }
    let mut values: Vec<_> = values
        .into_iter()
        .map(|(recording_id, score)| RecordingMatch {
            recording_id,
            score,
        })
        .collect();
    values.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.recording_id.cmp(&b.recording_id))
    });
    values.truncate(MAX_RECORDINGS_PER_TRACK);
    Ok(values)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_text_is_one_parameter() {
        let query = "release:\"A & B + C\" AND artist:\"X/Y\"";
        let u = search_url(query, 25).unwrap();
        assert_eq!(
            u.query_pairs().find(|(k, _)| k == "query").unwrap().1,
            query
        );
    }
    #[test]
    fn refused_and_inconsistent_acoustid_fail() {
        for b in [
            br#"{"status":"error","results":[]}"#.as_slice(),
            br#"{"status":"ok","results":[{"score":2,"recordings":[]}]}"#,
        ] {
            assert!(parse_acoustid(b).is_err());
        }
    }
    #[test]
    fn recordings_without_links_are_inconclusive() {
        assert!(
            parse_acoustid(br#"{"status":"ok","results":[{"score":0.99}]}"#)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn full_release_keeps_recording_artist_fallback_disc_identity_and_lengths() {
        let page = parse_releases(br#"{"id":"00000000-0000-4000-8000-000000000001","title":"A & B","artist-credit":[{"name":"Album artist","artist":{"id":"00000000-0000-4000-8000-000000000002","name":"Album artist"}}],"media":[{"id":"00000000-0000-4000-8000-000000000006","position":1,"track-count":1,"discs":[{"id":"disc","offset-count":1,"offsets":[150],"sectors":1050}],"tracks":[{"id":"00000000-0000-4000-8000-000000000003","position":1,"number":"1","title":"Track","artist-credit":[],"recording":{"id":"00000000-0000-4000-8000-000000000004","title":"Track","length":12000,"artist-credit":[{"name":"Track artist","artist":{"id":"00000000-0000-4000-8000-000000000005","name":"Track artist"}}]}}]}]}"#, true).unwrap();
        let medium = &page.releases[0].media[0];
        assert_eq!(medium.disc_ids, ["disc"]);
        assert_eq!(medium.tracks[0].artist, "Track artist");
        assert_eq!(medium.tracks[0].duration_ms, Some(12000));
    }
    #[test]
    fn malformed_success_and_track_entities_fail_closed() {
        assert!(parse_acoustid(br#"{"status":"ok"}"#).is_err());
        let base = serde_json::json!({"id":"00000000-0000-4000-8000-000000000001","title":"Album","media":[{"id":"00000000-0000-4000-8000-000000000002","position":1,"track-count":1,"tracks":[{"id":"00000000-0000-4000-8000-000000000003","number":"1","position":1,"title":"Track","recording":{"id":"bad","title":"Track"}}]}]});
        assert!(parse_releases(&serde_json::to_vec(&base).unwrap(), true).is_err());
        let mut value = base.clone();
        value["media"][0]["tracks"][0]["recording"]["id"] =
            serde_json::json!("00000000-0000-4000-8000-000000000004");
        value["media"][0]["tracks"][0]["title"] = serde_json::json!("x".repeat(4097));
        assert!(parse_releases(&serde_json::to_vec(&value).unwrap(), true).is_err());
        value["media"][0]["tracks"][0]["title"] = serde_json::json!("Track");
        let duplicate = value["media"][0].clone();
        value["media"].as_array_mut().unwrap().push(duplicate);
        assert!(parse_releases(&serde_json::to_vec(&value).unwrap(), true).is_err());
    }
    #[test]
    fn unknown_envelope_fields_survive_entity_parsing() {
        let page=parse_releases(br#"{"releases":[{"id":"00000000-0000-4000-8000-000000000001","title":"Album"}],"release-count":1,"release-offset":0,"future":true}"#,false).unwrap();
        assert_eq!(page.total, Some(1));
        assert_eq!(page.releases.len(), 1);
    }
}
