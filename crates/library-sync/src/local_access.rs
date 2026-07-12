use std::collections::{HashMap, HashSet};

use library::{LocalManifestDelta, Track, TrackId};
use sources::local::LocalManifestScan;

#[derive(Hash, Eq, PartialEq)]
struct MatchKey {
    title: String,
    album: String,
    artist: String,
    disc_number: u16,
    track_number: u16,
}

struct MatchCandidate {
    path: String,
    duration_seconds: u32,
}

#[derive(Default)]
struct LocalTrackIndex {
    tracks: HashMap<MatchKey, Vec<MatchCandidate>>,
}

impl LocalTrackIndex {
    fn from_tracks<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> Self {
        let mut index = Self::default();
        for track in tracks {
            let Some(path) = track.local_path.clone() else {
                continue;
            };
            index
                .tracks
                .entry(match_key(track))
                .or_default()
                .push(MatchCandidate {
                    path,
                    duration_seconds: track.duration_seconds,
                });
        }
        index
    }

    fn matches(&self, remote_tracks: &[Track]) -> Vec<(TrackId, String, String)> {
        remote_tracks
            .iter()
            .filter_map(|remote| {
                let candidates = self.tracks.get(&match_key(remote))?;
                let mut matched = candidates.iter().filter(|candidate| {
                    durations_close(remote.duration_seconds, candidate.duration_seconds)
                });
                let candidate = matched.next()?;
                matched.next().is_none().then(|| {
                    (
                        remote.id.clone(),
                        candidate.path.clone(),
                        "metadata".to_string(),
                    )
                })
            })
            .collect()
    }
}

pub struct LocalAccessObservation {
    index: LocalTrackIndex,
    manifest: LocalManifestDelta,
}

impl LocalAccessObservation {
    pub fn from_manifest_scan(scan: LocalManifestScan) -> Self {
        let changed_paths = scan
            .changed_manifest_paths
            .into_iter()
            .collect::<HashSet<_>>();
        let index = LocalTrackIndex::from_tracks(scan.entries.iter().map(|entry| &entry.track));
        let upserted_entries = scan
            .entries
            .into_iter()
            .filter(|entry| changed_paths.contains(&entry.facts.path))
            .collect();
        Self {
            index,
            manifest: LocalManifestDelta {
                upserted_entries,
                deleted_paths: scan.deleted_paths,
            },
        }
    }

    pub(crate) fn matches(&self, tracks: &[Track]) -> Vec<(TrackId, String, String)> {
        self.index.matches(tracks)
    }

    pub(crate) fn manifest(&self) -> &LocalManifestDelta {
        &self.manifest
    }
}

#[cfg(test)]
fn match_local_tracks(
    remote_tracks: &[Track],
    local_tracks: &[Track],
) -> Vec<(TrackId, String, String)> {
    LocalTrackIndex::from_tracks(local_tracks).matches(remote_tracks)
}

fn match_key(track: &Track) -> MatchKey {
    MatchKey {
        title: normalize(&track.title),
        album: normalize(&track.album),
        artist: normalize(&track.artist),
        disc_number: track.disc_number,
        track_number: track.track_number,
    }
}

fn durations_close(left: u32, right: u32) -> bool {
    left == 0 || right == 0 || left.abs_diff(right) <= 3
}

fn normalize(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, TrackId};

    use super::*;

    fn track(id: &str, path: Option<&str>, duration_seconds: u32) -> Track {
        Track {
            id: TrackId::new(id),
            album_id: AlbumId::new("album"),
            title: "First Motion".to_string(),
            artist: "Astral Kin".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Blue Rooms".to_string(),
            year: 0,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds,
            favorite: false,
            disc_number: 1,
            track_number: 7,
            image_ref: None,
            album_artwork: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: path.map(str::to_string),
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }

    #[test]
    fn metadata_match_requires_one_close_candidate() {
        let remote = track("remote", None, 210);
        let first = track("local-one", Some("/music/first.flac"), 212);
        assert_eq!(
            match_local_tracks(std::slice::from_ref(&remote), std::slice::from_ref(&first)),
            vec![(
                remote.id.clone(),
                "/music/first.flac".to_string(),
                "metadata".to_string()
            )]
        );

        let second = track("local-two", Some("/music/second.flac"), 209);
        assert!(match_local_tracks(&[remote], &[first, second]).is_empty());
    }
}
