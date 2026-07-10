use super::*;

pub(in crate::controller) fn scrub_source_album_image_refs(
    saved: &SavedSource,
    albums: &mut [Album],
) {
    for album in albums {
        scrub_source_image_ref(saved, &mut album.image_ref);
    }
}

pub(in crate::controller) fn scrub_selected_album_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    albums: &mut [Album],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_album_image_refs(saved, albums, external_ref_policy);
}

pub(in crate::controller) fn scrub_source_track_image_refs(
    saved: &SavedSource,
    tracks: &mut [Track],
) {
    for track in tracks {
        scrub_source_image_ref(saved, &mut track.image_ref);
    }
}

pub(in crate::controller) fn scrub_selected_track_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    tracks: &mut [Track],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_track_image_refs(saved, tracks, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_artist_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    artists: &mut [Artist],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_artist_image_refs(saved, artists, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_genre_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    genres: &mut [Genre],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_genre_image_refs(saved, genres, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_mood_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    moods: &mut [Mood],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_mood_image_refs(saved, moods, external_ref_policy);
}

pub(in crate::controller) fn scrub_selected_playlist_image_refs(
    saved: &SavedSource,
    settings: &AppSettings,
    playlists: &mut [Playlist],
) {
    let external_ref_policy = snapshot_external_ref_policy(settings);
    scrub_snapshot_playlist_image_refs(saved, playlists, external_ref_policy);
}

pub(in crate::controller) fn scrub_smart_refs(
    saved: &SavedSource,
    playlists: &mut [SmartPlaylist],
) {
    for playlist in playlists {
        scrub_source_image_ref(saved, &mut playlist.image_ref);
        scrub_source_image_ref_vec(saved, &mut playlist.image_refs);
    }
}

pub(in crate::controller) fn scrub_home_refs(saved: &SavedSource, section: &mut HomeSection) {
    scrub_source_album_image_refs(saved, &mut section.albums);
    scrub_source_track_image_refs(saved, &mut section.tracks);
}

#[derive(Clone, Copy)]
pub(in crate::controller) struct SnapshotExternalRefPolicy {
    allow_identity_refs: bool,
    allow_cached_refs: bool,
}

pub(in crate::controller) fn snapshot_external_ref_policy(
    settings: &AppSettings,
) -> SnapshotExternalRefPolicy {
    let cached_refs_enabled = external_metadata::cached_refs_enabled(settings);
    SnapshotExternalRefPolicy {
        allow_identity_refs: cached_refs_enabled,
        allow_cached_refs: cached_refs_enabled && !external_metadata::enabled(settings),
    }
}

pub(in crate::controller::root) fn scrub_snapshot_home_refs(
    saved: &SavedSource,
    section: &mut HomeSection,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    scrub_snapshot_album_image_refs(saved, &mut section.albums, external_ref_policy);
    scrub_snapshot_track_image_refs(saved, &mut section.tracks, external_ref_policy);
}

pub(in crate::controller::root) fn scrub_snapshot_album_image_refs(
    saved: &SavedSource,
    albums: &mut [Album],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for album in albums {
        scrub_snapshot_image_ref(saved, &mut album.image_ref, external_ref_policy);
    }
}

pub(in crate::controller::root) fn scrub_snapshot_track_image_refs(
    saved: &SavedSource,
    tracks: &mut [Track],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for track in tracks {
        scrub_snapshot_image_ref(saved, &mut track.image_ref, external_ref_policy);
    }
}

pub(in crate::controller::root) fn scrub_snapshot_artist_image_refs(
    saved: &SavedSource,
    artists: &mut [Artist],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for artist in artists {
        scrub_snapshot_image_ref(saved, &mut artist.image_ref, external_ref_policy);
    }
}

pub(in crate::controller::root) fn scrub_snapshot_genre_image_refs(
    saved: &SavedSource,
    genres: &mut [Genre],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for genre in genres {
        scrub_snapshot_image_ref(saved, &mut genre.image_ref, external_ref_policy);
        scrub_snapshot_image_ref_vec(saved, &mut genre.image_refs, external_ref_policy);
    }
}

pub(in crate::controller::root) fn scrub_snapshot_mood_image_refs(
    saved: &SavedSource,
    moods: &mut [Mood],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for mood in moods {
        scrub_snapshot_image_ref(saved, &mut mood.image_ref, external_ref_policy);
        scrub_snapshot_image_ref_vec(saved, &mut mood.image_refs, external_ref_policy);
    }
}

pub(in crate::controller::root) fn scrub_snapshot_playlist_image_refs(
    saved: &SavedSource,
    playlists: &mut [Playlist],
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    for playlist in playlists {
        scrub_snapshot_image_ref(saved, &mut playlist.image_ref, external_ref_policy);
        scrub_snapshot_image_ref_vec(saved, &mut playlist.image_refs, external_ref_policy);
    }
}

fn scrub_snapshot_image_ref(
    saved: &SavedSource,
    image_ref: &mut Option<ImageRef>,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    let Some(ref_value) = image_ref else {
        return;
    };
    if snapshot_image_ref_allowed(saved, ref_value, external_ref_policy) {
        return;
    }
    *image_ref = None;
}

fn scrub_snapshot_image_ref_vec(
    saved: &SavedSource,
    image_refs: &mut Vec<ImageRef>,
    external_ref_policy: SnapshotExternalRefPolicy,
) {
    image_refs
        .retain(|image_ref| snapshot_image_ref_allowed(saved, image_ref, external_ref_policy));
}

fn snapshot_image_ref_allowed(
    saved: &SavedSource,
    image_ref: &ImageRef,
    external_ref_policy: SnapshotExternalRefPolicy,
) -> bool {
    source_image_ref_allowed(saved, image_ref)
        || (external_ref_policy.allow_cached_refs
            && external_metadata::is_external_image_ref(image_ref))
        || (external_ref_policy.allow_identity_refs && external_identity_image_ref(image_ref))
}

fn external_identity_image_ref(image_ref: &ImageRef) -> bool {
    external_metadata::album_art_from_image_ref(image_ref).is_some_and(|art| {
        art.musicbrainz_release_id.is_some() || art.musicbrainz_release_group_id.is_some()
    })
}

pub(in crate::controller) fn scrub_source_image_ref(
    saved: &SavedSource,
    image_ref: &mut Option<ImageRef>,
) {
    let Some(ref_value) = image_ref else {
        return;
    };
    if source_image_ref_allowed(saved, ref_value) {
        return;
    }
    *image_ref = None;
}

fn scrub_source_image_ref_vec(saved: &SavedSource, image_refs: &mut Vec<ImageRef>) {
    image_refs.retain(|image_ref| source_image_ref_allowed(saved, image_ref));
}

pub(in crate::controller) fn source_image_ref_allowed(
    saved: &SavedSource,
    image_ref: &ImageRef,
) -> bool {
    image_ref_allowed(&saved.source, image_ref)
}

pub(in crate::controller) fn image_ref_allowed(
    server: &SourceIdentity,
    image_ref: &ImageRef,
) -> bool {
    if server.kind == LOCAL_SOURCE_ID {
        return is_local_source_image_ref(image_ref);
    }
    !is_local_source_image_ref(image_ref)
}

pub(in crate::controller) fn is_local_source_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with("local:cover:")
}

pub(in crate::controller) fn is_local_album_id(album_id: &AlbumId) -> bool {
    album_id.as_str().starts_with("local:album:")
}

pub(in crate::controller) fn is_local_track_id(track_id: &TrackId) -> bool {
    track_id.as_str().starts_with("local:track:")
}

pub(in crate::controller) fn is_local_artist_id(artist_id: &ArtistId) -> bool {
    artist_id.as_str().starts_with("local:artist:")
}
