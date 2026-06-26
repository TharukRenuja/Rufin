use std::collections::{HashMap, HashSet};

use domain::{AlbumId, ArtistId, GeneratedTrackSeed, ServerId, Track, TrackId};

use super::AppController;

const RADIO_RELEVANCE_GENRE: u8 = 0;
const RADIO_RELEVANCE_ARTIST: u8 = 1;
const RADIO_RELEVANCE_RANDOM: u8 = 2;

struct RadioCandidate {
    relevance: u8,
    track: Track,
}

#[derive(Clone, Copy)]
struct CandidateContext<'a> {
    server_id: &'a ServerId,
    candidate_limit: usize,
    exclude_track_id: Option<&'a TrackId>,
    exclude_album_id: Option<&'a AlbumId>,
}

impl AppController {
    pub(super) fn local_generated_tracks_from_cache(
        &self,
        server_id: &ServerId,
        seed: GeneratedTrackSeed,
        limit: usize,
    ) -> Result<Vec<Track>, String> {
        let limit = limit.clamp(1, 500);
        let candidate_limit = limit.saturating_mul(8).clamp(limit, 500);
        let seed_key = local_generated_seed_key(&seed);
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let (genres, artist_ids, exclude_track_id, exclude_album_id) = match seed {
            GeneratedTrackSeed::Track(track_id) => {
                let seed = self
                    .store
                    .with_store(|store| store.load_track(server_id, &track_id))?
                    .ok_or_else(|| "The selected track is no longer available.".to_string())?;
                (
                    seed.genres,
                    fallback_artist_ids(
                        seed.artist_id,
                        seed.artist_credits,
                        seed.album_artist_credits,
                    ),
                    Some(track_id),
                    None,
                )
            }
            GeneratedTrackSeed::Album(album_id) => {
                let (album, _tracks) = self
                    .store
                    .with_store(|store| store.load_album_detail(server_id, &album_id))?
                    .ok_or_else(|| "The selected album is no longer available.".to_string())?;
                (
                    album.genres,
                    fallback_artist_ids(
                        album.artist_id,
                        album.artist_credits,
                        album.album_artist_credits,
                    ),
                    None,
                    Some(album_id),
                )
            }
            GeneratedTrackSeed::Artist(artist_id) => (Vec::new(), vec![artist_id], None, None),
            GeneratedTrackSeed::Genre { id: _, name } => (vec![name], Vec::new(), None, None),
            GeneratedTrackSeed::Playlist(_) => {
                return Err("Playlist radio is not supported for this source.".to_string());
            }
        };
        let context = CandidateContext {
            server_id,
            candidate_limit,
            exclude_track_id: exclude_track_id.as_ref(),
            exclude_album_id: exclude_album_id.as_ref(),
        };
        self.append_cached_fallback_candidates(
            context,
            &genres,
            &artist_ids,
            &mut seen,
            &mut candidates,
        )?;
        Ok(select_radio_tracks(&seed_key, candidates, limit))
    }

    fn append_cached_fallback_candidates(
        &self,
        context: CandidateContext<'_>,
        genres: &[String],
        artist_ids: &[ArtistId],
        seen: &mut HashSet<TrackId>,
        candidates: &mut Vec<RadioCandidate>,
    ) -> Result<(), String> {
        self.append_cached_genre_candidates(context, genres, seen, candidates)?;
        self.append_cached_artist_candidates(context, artist_ids, seen, candidates)?;
        self.append_cached_random_candidates(context, seen, candidates)
    }

    fn append_cached_genre_candidates(
        &self,
        context: CandidateContext<'_>,
        genres: &[String],
        seen: &mut HashSet<TrackId>,
        candidates: &mut Vec<RadioCandidate>,
    ) -> Result<(), String> {
        for genre in genres.iter().filter(|genre| !genre.trim().is_empty()) {
            let tracks = self.store.with_store(|store| {
                store.load_tracks_by_genre_name(context.server_id, genre, context.candidate_limit)
            })?;
            append_radio_candidates(
                tracks,
                RADIO_RELEVANCE_GENRE,
                context.exclude_track_id,
                context.exclude_album_id,
                seen,
                candidates,
            );
        }
        Ok(())
    }

    fn append_cached_artist_candidates(
        &self,
        context: CandidateContext<'_>,
        artist_ids: &[ArtistId],
        seen: &mut HashSet<TrackId>,
        candidates: &mut Vec<RadioCandidate>,
    ) -> Result<(), String> {
        for artist_id in artist_ids {
            let detail = self
                .store
                .with_store(|store| store.load_artist_detail(context.server_id, artist_id))?;
            if let Some(detail) = detail {
                append_radio_candidates(
                    detail.tracks,
                    RADIO_RELEVANCE_ARTIST,
                    context.exclude_track_id,
                    context.exclude_album_id,
                    seen,
                    candidates,
                );
            }
        }
        Ok(())
    }

    fn append_cached_random_candidates(
        &self,
        context: CandidateContext<'_>,
        seen: &mut HashSet<TrackId>,
        candidates: &mut Vec<RadioCandidate>,
    ) -> Result<(), String> {
        let tracks = self
            .store
            .with_store(|store| store.load_tracks(context.server_id, 0, context.candidate_limit))?
            .items;
        append_radio_candidates(
            tracks,
            RADIO_RELEVANCE_RANDOM,
            context.exclude_track_id,
            context.exclude_album_id,
            seen,
            candidates,
        );
        Ok(())
    }
}

fn append_radio_candidates(
    tracks: impl IntoIterator<Item = Track>,
    relevance: u8,
    exclude_track_id: Option<&TrackId>,
    exclude_album_id: Option<&AlbumId>,
    seen: &mut HashSet<TrackId>,
    candidates: &mut Vec<RadioCandidate>,
) {
    for track in tracks {
        if exclude_track_id.is_some_and(|track_id| &track.id == track_id)
            || exclude_album_id.is_some_and(|album_id| &track.album_id == album_id)
        {
            continue;
        }
        if seen.insert(track.id.clone()) {
            candidates.push(RadioCandidate { relevance, track });
        }
    }
}

pub(in crate::controller) fn spread_radio_tracks(seed_key: &str, tracks: Vec<Track>) -> Vec<Track> {
    final_shuffle_tracks(
        seed_key,
        tracks
            .into_iter()
            .enumerate()
            .map(|(index, track)| (RADIO_RELEVANCE_GENRE, index, track))
            .collect(),
    )
}

fn select_radio_tracks(
    seed_key: &str,
    candidates: Vec<RadioCandidate>,
    limit: usize,
) -> Vec<Track> {
    let mut stages = HashMap::<u8, Vec<RadioCandidate>>::new();
    for candidate in candidates {
        stages
            .entry(candidate.relevance)
            .or_default()
            .push(candidate);
    }
    let mut stage_order = stages.keys().copied().collect::<Vec<_>>();
    stage_order.sort_unstable();

    let mut selected = Vec::new();
    for relevance in stage_order {
        if selected.len() >= limit {
            break;
        }
        if let Some(candidates) = stages.remove(&relevance) {
            select_radio_stage(seed_key, relevance, candidates, limit, &mut selected);
        }
    }
    final_shuffle_tracks(seed_key, selected)
}

fn select_radio_stage(
    seed_key: &str,
    relevance: u8,
    candidates: Vec<RadioCandidate>,
    limit: usize,
    selected: &mut Vec<(u8, usize, Track)>,
) {
    let mut album_order = Vec::<AlbumId>::new();
    let mut albums = HashMap::<AlbumId, Vec<Track>>::new();
    for candidate in candidates {
        let album_id = candidate.track.album_id.clone();
        if !albums.contains_key(&album_id) {
            album_order.push(album_id.clone());
        }
        albums.entry(album_id).or_default().push(candidate.track);
    }

    album_order.sort_by_key(|album_id| radio_hash(seed_key, album_id.as_str()));
    let distinct_album_count = album_order.len();
    let remaining = limit.saturating_sub(selected.len());
    let first_pass_cap = if distinct_album_count <= 1 {
        remaining
    } else {
        remaining
            .div_ceil(distinct_album_count)
            .saturating_add(1)
            .clamp(2, 3)
    };
    let mut deferred = Vec::new();

    for album_id in &album_order {
        if let Some(tracks) = albums.get_mut(album_id) {
            tracks.sort_by_key(|track| radio_hash(seed_key, track.id.as_str()));
        }
    }

    for index in 0..first_pass_cap {
        for album_id in &album_order {
            if selected.len() >= limit {
                return;
            }
            let Some(tracks) = albums.get(album_id) else {
                continue;
            };
            if let Some(track) = tracks.get(index) {
                selected.push((relevance, selected.len(), track.clone()));
            }
        }
    }

    for album_id in album_order {
        if let Some(tracks) = albums.remove(&album_id) {
            deferred.extend(tracks.into_iter().skip(first_pass_cap));
        }
    }
    deferred.sort_by_key(|track| radio_hash(seed_key, track.id.as_str()));
    for track in deferred {
        if selected.len() >= limit {
            break;
        }
        selected.push((relevance, selected.len(), track));
    }
}

fn final_shuffle_tracks(seed_key: &str, mut tracks: Vec<(u8, usize, Track)>) -> Vec<Track> {
    tracks.sort_by_key(|(relevance, index, track)| {
        (*relevance, radio_hash(seed_key, track.id.as_str()), *index)
    });
    tracks
        .into_iter()
        .map(|(_relevance, _index, track)| track)
        .collect()
}

fn radio_hash(seed_key: &str, value: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;

    let mut hash = FNV_OFFSET;
    for byte in seed_key.bytes().chain([0xff]).chain(value.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn local_generated_seed_key(seed: &GeneratedTrackSeed) -> String {
    match seed {
        GeneratedTrackSeed::Track(track_id) => format!("track:{}", track_id.as_str()),
        GeneratedTrackSeed::Album(album_id) => format!("album:{}", album_id.as_str()),
        GeneratedTrackSeed::Artist(artist_id) => format!("artist:{}", artist_id.as_str()),
        GeneratedTrackSeed::Genre { id, name } => id
            .as_ref()
            .map(|id| format!("genre:{}:{name}", id.as_str()))
            .unwrap_or_else(|| format!("genre:{name}")),
        GeneratedTrackSeed::Playlist(playlist_id) => format!("playlist:{}", playlist_id.as_str()),
    }
}

fn fallback_artist_ids(
    primary_artist_id: Option<ArtistId>,
    artist_credits: Vec<domain::ArtistCredit>,
    album_artist_credits: Vec<domain::ArtistCredit>,
) -> Vec<ArtistId> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    if let Some(artist_id) = primary_artist_id
        && seen.insert(artist_id.clone())
    {
        ids.push(artist_id);
    }
    for credit in artist_credits {
        if seen.insert(credit.id.clone()) {
            ids.push(credit.id);
        }
    }
    for credit in album_artist_credits {
        if seen.insert(credit.id.clone()) {
            ids.push(credit.id);
        }
    }
    ids
}
