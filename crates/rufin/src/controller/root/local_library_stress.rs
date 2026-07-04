use super::*;
use domain::ArtistCredit;
use std::collections::{HashMap, HashSet};

const LOCAL_STRESS_MULTIPLIER_ENV: &str = "RUFIN_LOCAL_STRESS_MULTIPLIER";
const LOCAL_STRESS_MULTIPLIER_MAX: usize = 100;
const LOCAL_STRESS_ALBUM_ID_PREFIX: &str = "local:stress-album:";
const LOCAL_STRESS_ARTIST_ID_PREFIX: &str = "local:stress-artist:";
const LOCAL_STRESS_GENRE_ID_PREFIX: &str = "local:stress-genre:";
pub(super) const LOCAL_STRESS_TRACK_ID_PREFIX: &str = "local:stress-track:";

pub(super) struct LocalStressSnapshot<'a> {
    pub(super) store: &'a StoreHandle,
    pub(super) source_id: &'a SourceId,
    pub(super) scan: &'a LocalManifestScan,
    pub(super) tracks: &'a mut Vec<Track>,
    pub(super) albums: &'a mut Vec<Album>,
    pub(super) artists: &'a mut Vec<Artist>,
    pub(super) album_artists: &'a mut Vec<Artist>,
    pub(super) genres: &'a mut Vec<Genre>,
    pub(super) home_sections: &'a mut Vec<HomeSection>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LocalStressDelta {
    pub(super) changed_track_ids: HashSet<TrackId>,
    pub(super) metadata_track_ids: HashSet<TrackId>,
    pub(super) artwork_track_ids: HashSet<TrackId>,
    pub(super) deleted_track_ids: Vec<TrackId>,
    pub(super) dirty_album_ids: HashSet<AlbumId>,
    pub(super) dirty_artist_ids: HashSet<ArtistId>,
    pub(super) dirty_album_artist_ids: HashSet<ArtistId>,
    pub(super) dirty_genre_names: HashSet<String>,
}

impl LocalStressDelta {
    pub(super) fn is_empty(&self) -> bool {
        self.changed_track_ids.is_empty()
            && self.metadata_track_ids.is_empty()
            && self.artwork_track_ids.is_empty()
            && self.deleted_track_ids.is_empty()
            && self.dirty_album_ids.is_empty()
            && self.dirty_artist_ids.is_empty()
            && self.dirty_album_artist_ids.is_empty()
            && self.dirty_genre_names.is_empty()
    }
}

pub(super) fn local_library_stress_multiplier() -> usize {
    #[cfg(debug_assertions)]
    {
        std::env::var(LOCAL_STRESS_MULTIPLIER_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 1)
            .map(|value| value.min(LOCAL_STRESS_MULTIPLIER_MAX))
            .unwrap_or(1)
    }
    #[cfg(not(debug_assertions))]
    {
        1
    }
}

pub(super) fn apply_local_library_stress_multiplier(
    snapshot: LocalStressSnapshot<'_>,
    stress_multiplier: usize,
) -> Result<LocalStressDelta, String> {
    let stress_multiplier = stress_multiplier.clamp(1, LOCAL_STRESS_MULTIPLIER_MAX);
    let existing_stress_ids = snapshot.store.with_store(|store| {
        store.load_track_ids_with_prefix(snapshot.source_id, LOCAL_STRESS_TRACK_ID_PREFIX)
    })?;
    let existing_graph_needs_rewrite = snapshot.store.with_store(|store| {
        store.tracks_with_prefix_have_album_prefix_mismatch(
            snapshot.source_id,
            LOCAL_STRESS_TRACK_ID_PREFIX,
            LOCAL_STRESS_ALBUM_ID_PREFIX,
        )
    })?;
    let existing_stress_ids = existing_stress_ids.into_iter().collect::<HashSet<_>>();
    let mut delta = LocalStressDelta::default();
    if stress_multiplier <= 1 {
        delta.deleted_track_ids = sorted_track_ids(existing_stress_ids);
        if !delta.deleted_track_ids.is_empty() {
            mark_all_snapshot_aggregates_dirty(&snapshot, &mut delta);
            info!(
                source_id = %snapshot.source_id,
                removed_tracks = delta.deleted_track_ids.len(),
                "removing local stress library tracks"
            );
        }
        return Ok(delta);
    }

    let base_tracks = snapshot.tracks.clone();
    let base_albums = snapshot.albums.clone();
    let base_artists = snapshot.artists.clone();
    let base_album_artists = snapshot.album_artists.clone();
    let base_genres = snapshot.genres.clone();
    let base_home_sections = snapshot.home_sections.clone();
    if base_tracks.is_empty() {
        delta.deleted_track_ids = sorted_track_ids(existing_stress_ids);
        if !delta.deleted_track_ids.is_empty() {
            mark_all_snapshot_aggregates_dirty(&snapshot, &mut delta);
        }
        return Ok(delta);
    }

    let mut dirty_track_ids = snapshot
        .scan
        .changed_track_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    dirty_track_ids.extend(snapshot.scan.metadata_track_ids.iter().cloned());
    dirty_track_ids.extend(snapshot.scan.artwork_track_ids.iter().cloned());
    let dirty_album_ids = snapshot
        .scan
        .dirty_album_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let dirty_artist_ids = snapshot
        .scan
        .dirty_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let dirty_album_artist_ids = snapshot
        .scan
        .dirty_album_artist_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let dirty_genre_names = snapshot
        .scan
        .dirty_genre_names
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut current_stress_ids = HashSet::new();

    for copy_index in 1..stress_multiplier {
        let copy_has_track_writes = base_tracks.iter().any(|base| {
            let track_id = stress_track_id(copy_index, &base.id);
            existing_graph_needs_rewrite
                || !existing_stress_ids.contains(&track_id)
                || dirty_track_ids.contains(&base.id)
        });
        append_stress_albums(
            snapshot.albums,
            &base_albums,
            copy_index,
            copy_has_track_writes,
            &dirty_album_ids,
            &mut delta,
        );
        append_stress_artists(
            snapshot.artists,
            &base_artists,
            copy_index,
            false,
            copy_has_track_writes,
            &dirty_artist_ids,
            &mut delta,
        );
        append_stress_artists(
            snapshot.album_artists,
            &base_album_artists,
            copy_index,
            true,
            copy_has_track_writes,
            &dirty_album_artist_ids,
            &mut delta,
        );
        append_stress_genres(
            snapshot.genres,
            &base_genres,
            copy_index,
            copy_has_track_writes,
            &dirty_genre_names,
            &mut delta,
        );
        append_stress_tracks(
            snapshot.tracks,
            &base_tracks,
            copy_index,
            &existing_stress_ids,
            &dirty_track_ids,
            existing_graph_needs_rewrite,
            &mut current_stress_ids,
            &mut delta,
        );
    }

    delta.deleted_track_ids = sorted_track_ids(
        existing_stress_ids
            .difference(&current_stress_ids)
            .cloned()
            .collect(),
    );
    append_stress_home_sections(
        snapshot.home_sections,
        &base_home_sections,
        stress_multiplier,
    );
    if !delta.is_empty() {
        info!(
            source_id = %snapshot.source_id,
            multiplier = stress_multiplier,
            base_albums = base_albums.len(),
            total_albums = snapshot.albums.len(),
            base_artists = base_artists.len(),
            total_artists = snapshot.artists.len(),
            base_album_artists = base_album_artists.len(),
            total_album_artists = snapshot.album_artists.len(),
            base_genres = base_genres.len(),
            total_genres = snapshot.genres.len(),
            base_tracks = base_tracks.len(),
            total_tracks = snapshot.tracks.len(),
            changed_tracks = delta.changed_track_ids.len(),
            metadata_tracks = delta.metadata_track_ids.len(),
            artwork_tracks = delta.artwork_track_ids.len(),
            deleted_tracks = delta.deleted_track_ids.len(),
            env = LOCAL_STRESS_MULTIPLIER_ENV,
            "applied local stress library multiplier"
        );
    }
    Ok(delta)
}

fn append_stress_albums(
    albums: &mut Vec<Album>,
    base_albums: &[Album],
    copy_index: usize,
    copy_has_track_writes: bool,
    dirty_base_album_ids: &HashSet<AlbumId>,
    delta: &mut LocalStressDelta,
) {
    for base in base_albums {
        let mut album = base.clone();
        album.id = stress_album_id(copy_index, &base.id);
        album.artist_id = base
            .artist_id
            .as_ref()
            .map(|artist_id| stress_artist_id(copy_index, artist_id));
        album.artist_credits = stress_artist_credits(copy_index, &base.artist_credits);
        album.album_artist_credits = stress_artist_credits(copy_index, &base.album_artist_credits);
        album.genres = stress_genre_names(copy_index, &base.genres);
        album.musicbrainz_album_id = None;
        album.musicbrainz_release_group_id = None;
        if copy_has_track_writes || dirty_base_album_ids.contains(&base.id) {
            delta.dirty_album_ids.insert(album.id.clone());
        }
        albums.push(album);
    }
}

fn append_stress_artists(
    artists: &mut Vec<Artist>,
    base_artists: &[Artist],
    copy_index: usize,
    album_artist: bool,
    copy_has_track_writes: bool,
    dirty_base_artist_ids: &HashSet<ArtistId>,
    delta: &mut LocalStressDelta,
) {
    for base in base_artists {
        let mut artist = base.clone();
        artist.id = stress_artist_id(copy_index, &base.id);
        artist.musicbrainz_artist_id = None;
        if copy_has_track_writes || dirty_base_artist_ids.contains(&base.id) {
            if album_artist {
                delta.dirty_album_artist_ids.insert(artist.id.clone());
            } else {
                delta.dirty_artist_ids.insert(artist.id.clone());
            }
        }
        artists.push(artist);
    }
}

fn append_stress_genres(
    genres: &mut Vec<Genre>,
    base_genres: &[Genre],
    copy_index: usize,
    copy_has_track_writes: bool,
    dirty_base_genre_names: &HashSet<String>,
    delta: &mut LocalStressDelta,
) {
    for base in base_genres {
        let mut genre = base.clone();
        genre.id = stress_genre_id(copy_index, &base.id);
        genre.name = stress_genre_name(copy_index, &base.name);
        if copy_has_track_writes || dirty_base_genre_names.contains(&base.name) {
            delta.dirty_genre_names.insert(genre.name.clone());
        }
        genres.push(genre);
    }
}

fn append_stress_tracks(
    tracks: &mut Vec<Track>,
    base_tracks: &[Track],
    copy_index: usize,
    existing_stress_ids: &HashSet<TrackId>,
    dirty_track_ids: &HashSet<TrackId>,
    rewrite_existing_tracks: bool,
    current_stress_ids: &mut HashSet<TrackId>,
    delta: &mut LocalStressDelta,
) {
    for base in base_tracks {
        let mut track = base.clone();
        track.id = stress_track_id(copy_index, &base.id);
        track.album_id = stress_album_id(copy_index, &base.album_id);
        track.artist_id = base
            .artist_id
            .as_ref()
            .map(|artist_id| stress_artist_id(copy_index, artist_id));
        track.artist_credits = stress_artist_credits(copy_index, &base.artist_credits);
        track.album_artist_credits = stress_artist_credits(copy_index, &base.album_artist_credits);
        track.genres = stress_genre_names(copy_index, &base.genres);
        track.musicbrainz_recording_id = None;
        track.musicbrainz_release_track_id = None;
        let track_id = track.id.clone();
        current_stress_ids.insert(track_id.clone());
        if rewrite_existing_tracks
            || !existing_stress_ids.contains(&track_id)
            || dirty_track_ids.contains(&base.id)
        {
            delta.changed_track_ids.insert(track_id);
        }
        tracks.push(track);
    }
}

fn append_stress_home_sections(
    home_sections: &mut [HomeSection],
    base_home_sections: &[HomeSection],
    stress_multiplier: usize,
) {
    let base_by_kind = base_home_sections
        .iter()
        .map(|section| (section.kind, section.clone()))
        .collect::<HashMap<_, _>>();
    for section in home_sections {
        let Some(base) = base_by_kind.get(&section.kind) else {
            continue;
        };
        for copy_index in 1..stress_multiplier {
            section.albums.extend(
                base.albums
                    .iter()
                    .cloned()
                    .map(|album| stress_home_album(copy_index, album)),
            );
            section.tracks.extend(
                base.tracks
                    .iter()
                    .cloned()
                    .map(|track| stress_home_track(copy_index, track)),
            );
        }
    }
}

fn stress_home_album(copy_index: usize, mut album: Album) -> Album {
    let base_id = album.id.clone();
    album.id = stress_album_id(copy_index, &base_id);
    album.artist_id = album
        .artist_id
        .as_ref()
        .map(|artist_id| stress_artist_id(copy_index, artist_id));
    album.artist_credits = stress_artist_credits(copy_index, &album.artist_credits);
    album.album_artist_credits = stress_artist_credits(copy_index, &album.album_artist_credits);
    album.genres = stress_genre_names(copy_index, &album.genres);
    album.musicbrainz_album_id = None;
    album.musicbrainz_release_group_id = None;
    album
}

fn stress_home_track(copy_index: usize, mut track: Track) -> Track {
    let base_id = track.id.clone();
    let base_album_id = track.album_id.clone();
    track.id = stress_track_id(copy_index, &base_id);
    track.album_id = stress_album_id(copy_index, &base_album_id);
    track.artist_id = track
        .artist_id
        .as_ref()
        .map(|artist_id| stress_artist_id(copy_index, artist_id));
    track.artist_credits = stress_artist_credits(copy_index, &track.artist_credits);
    track.album_artist_credits = stress_artist_credits(copy_index, &track.album_artist_credits);
    track.genres = stress_genre_names(copy_index, &track.genres);
    track.musicbrainz_recording_id = None;
    track.musicbrainz_release_track_id = None;
    track
}

fn mark_all_snapshot_aggregates_dirty(
    snapshot: &LocalStressSnapshot<'_>,
    delta: &mut LocalStressDelta,
) {
    delta
        .dirty_album_ids
        .extend(snapshot.albums.iter().map(|album| album.id.clone()));
    delta
        .dirty_artist_ids
        .extend(snapshot.artists.iter().map(|artist| artist.id.clone()));
    delta.dirty_album_artist_ids.extend(
        snapshot
            .album_artists
            .iter()
            .map(|artist| artist.id.clone()),
    );
    delta
        .dirty_genre_names
        .extend(snapshot.genres.iter().map(|genre| genre.name.clone()));
}

fn stress_artist_credits(copy_index: usize, credits: &[ArtistCredit]) -> Vec<ArtistCredit> {
    credits
        .iter()
        .map(|credit| ArtistCredit {
            id: stress_artist_id(copy_index, &credit.id),
            name: credit.name.clone(),
            musicbrainz_artist_id: None,
        })
        .collect()
}

fn stress_genre_names(copy_index: usize, names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|name| stress_genre_name(copy_index, name))
        .collect()
}

fn stress_album_id(copy_index: usize, base_id: &AlbumId) -> AlbumId {
    AlbumId::new(format!(
        "{LOCAL_STRESS_ALBUM_ID_PREFIX}{copy_index}:{}",
        base_id.as_str()
    ))
}

fn stress_artist_id(copy_index: usize, base_id: &ArtistId) -> ArtistId {
    ArtistId::new(format!(
        "{LOCAL_STRESS_ARTIST_ID_PREFIX}{copy_index}:{}",
        base_id.as_str()
    ))
}

fn stress_genre_id(copy_index: usize, base_id: &GenreId) -> GenreId {
    GenreId::new(format!(
        "{LOCAL_STRESS_GENRE_ID_PREFIX}{copy_index}:{}",
        base_id.as_str()
    ))
}

fn stress_track_id(copy_index: usize, base_id: &TrackId) -> TrackId {
    TrackId::new(format!(
        "{LOCAL_STRESS_TRACK_ID_PREFIX}{copy_index}:{}",
        base_id.as_str()
    ))
}

fn stress_genre_name(copy_index: usize, name: &str) -> String {
    format!("{name} [stress {copy_index}]")
}

fn sorted_track_ids(ids: HashSet<TrackId>) -> Vec<TrackId> {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort();
    ids
}
