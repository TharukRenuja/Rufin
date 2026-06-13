use rufin_core::{
    Album, AppSettings, Artist, Genre, HomeSection, ImageRef, Playlist, QueueEntry, QueueSnapshot,
    SmartPlaylist, Track,
};
use rufin_provider::{PlaylistDetail, SearchResults};

use crate::external_metadata;

const GROUP_ARTWORK_REF_LIMIT: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedArtwork {
    pub image_ref: Option<ImageRef>,
    pub image_refs: Vec<ImageRef>,
    pub selection: ArtworkSelection,
    pub provenance: ArtworkProvenance,
    pub fetch_policy: ArtworkFetchPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkSelection {
    ImageRefs,
    FinalMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkProvenance {
    Source,
    ExternalMbid,
    ExternalTextFallback,
    RepresentativeFallback,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkFetchPolicy {
    LocalSource,
    ProviderSource,
    #[allow(dead_code)]
    CachedExternal,
    OptionalExternalNetwork,
    FinalMissing,
}

pub fn selected_album_artwork(album: &Album, settings: &AppSettings) -> SelectedArtwork {
    if let Some(selected) = selected_existing_ref(album.image_ref.as_ref(), settings) {
        return selected;
    }
    if external_metadata::enabled(settings) {
        if let Some(image_ref) = external_metadata::external_album_identity_image_ref(album) {
            return selected(
                image_ref,
                ArtworkProvenance::ExternalMbid,
                ArtworkFetchPolicy::OptionalExternalNetwork,
            );
        }
        if let Some(image_ref) =
            external_metadata::external_album_image_ref(&album.artist, &album.title)
        {
            return selected(
                image_ref,
                ArtworkProvenance::ExternalTextFallback,
                ArtworkFetchPolicy::OptionalExternalNetwork,
            );
        }
    }
    missing_artwork()
}

pub fn selected_track_artwork(
    track: &Track,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) -> SelectedArtwork {
    let original_ref = track.image_ref.clone();
    let mut track = track.clone();
    external_metadata::normalize_track_with_album_ref(&mut track, album_image_ref, settings);
    selected_track_ref(track.image_ref, original_ref.as_ref(), album_image_ref)
}

pub fn selected_queue_artwork(
    entry: &QueueEntry,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) -> SelectedArtwork {
    let original_ref = entry.image_ref.clone();
    let mut entry = entry.clone();
    external_metadata::normalize_queue_entry_with_album_ref(&mut entry, album_image_ref, settings);
    selected_track_ref(entry.image_ref, original_ref.as_ref(), album_image_ref)
}

pub fn selected_artist_artwork(artist: &Artist, settings: &AppSettings) -> SelectedArtwork {
    let mut artist = artist.clone();
    external_metadata::normalize_artist(&mut artist, settings);
    selected_existing_ref(artist.image_ref.as_ref(), settings).unwrap_or_else(missing_artwork)
}

pub fn selected_genre_artwork(genre: &Genre) -> SelectedArtwork {
    selected_collection_artwork(&genre.image_refs, genre.image_ref.as_ref(), false)
}

pub fn selected_playlist_artwork(playlist: &Playlist, settings: &AppSettings) -> SelectedArtwork {
    selected_collection_artwork(
        &playlist.image_refs,
        playlist.image_ref.as_ref(),
        settings.prefer_server_playlist_covers,
    )
}

pub fn selected_smart_playlist_artwork(playlist: &SmartPlaylist) -> SelectedArtwork {
    selected_collection_artwork(&playlist.image_refs, playlist.image_ref.as_ref(), false)
}

pub fn bind_album(album: &mut Album, settings: &AppSettings) -> SelectedArtwork {
    let artwork = selected_album_artwork(album, settings);
    album.image_ref = artwork.image_ref.clone();
    artwork
}

pub fn bind_albums(albums: &mut [Album], settings: &AppSettings) {
    for album in albums {
        bind_album(album, settings);
    }
}

pub fn bind_track(track: &mut Track, settings: &AppSettings) -> SelectedArtwork {
    bind_track_with_album_ref(track, None, settings)
}

pub fn bind_track_with_album_ref(
    track: &mut Track,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) -> SelectedArtwork {
    let artwork = selected_track_artwork(track, album_image_ref, settings);
    track.image_ref = artwork.image_ref.clone();
    artwork
}

pub fn bind_tracks(tracks: &mut [Track], settings: &AppSettings) {
    for track in tracks {
        bind_track(track, settings);
    }
}

pub fn bind_artist(artist: &mut Artist, settings: &AppSettings) -> SelectedArtwork {
    let artwork = selected_artist_artwork(artist, settings);
    artist.image_ref = artwork.image_ref.clone();
    artwork
}

pub fn bind_artists(artists: &mut [Artist], settings: &AppSettings) {
    for artist in artists {
        bind_artist(artist, settings);
    }
}

pub fn bind_playlist(playlist: &mut Playlist, settings: &AppSettings) {
    playlist.image_refs = selected_playlist_artwork(playlist, settings).image_refs;
}

pub fn bind_playlists(playlists: &mut [Playlist], settings: &AppSettings) {
    for playlist in playlists {
        bind_playlist(playlist, settings);
    }
}

pub fn bind_home_sections(sections: &mut [HomeSection], settings: &AppSettings) {
    for section in sections {
        bind_home_section(section, settings);
    }
}

pub fn bind_home_section(section: &mut HomeSection, settings: &AppSettings) {
    bind_albums(&mut section.albums, settings);
    bind_tracks(&mut section.tracks, settings);
}

pub fn bind_search_results(results: &mut SearchResults, settings: &AppSettings) {
    bind_albums(&mut results.albums, settings);
    bind_tracks(&mut results.tracks, settings);
    bind_artists(&mut results.artists, settings);
    bind_playlists(&mut results.playlists, settings);
}

pub fn bind_queue_snapshot(snapshot: &mut QueueSnapshot, settings: &AppSettings) {
    for entry in &mut snapshot.entries {
        bind_queue_entry(entry, settings);
    }
}

pub fn bind_queue_entry(entry: &mut QueueEntry, settings: &AppSettings) -> SelectedArtwork {
    bind_queue_entry_with_album_ref(entry, None, settings)
}

pub fn bind_queue_entry_with_album_ref(
    entry: &mut QueueEntry,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) -> SelectedArtwork {
    let artwork = selected_queue_artwork(entry, album_image_ref, settings);
    entry.image_ref = artwork.image_ref.clone();
    artwork
}

pub fn bind_album_detail(album: &mut Album, tracks: &mut [Track], settings: &AppSettings) {
    bind_album(album, settings);
    let album_image_ref = album.image_ref.clone();
    for track in tracks {
        bind_track_with_album_ref(track, album_image_ref.as_ref(), settings);
    }
}

pub fn bind_playlist_detail(detail: &mut PlaylistDetail, settings: &AppSettings) {
    bind_playlist(&mut detail.playlist, settings);
}

pub fn is_external_image_ref(image_ref: &ImageRef) -> bool {
    external_metadata::is_external_image_ref(image_ref)
}

fn selected_existing_ref(
    image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) -> Option<SelectedArtwork> {
    let image_ref = image_ref?;
    if external_metadata::is_external_image_ref(image_ref) {
        if !external_metadata::enabled(settings) {
            return None;
        }
        return Some(selected(
            image_ref.clone(),
            external_ref_provenance(image_ref),
            ArtworkFetchPolicy::OptionalExternalNetwork,
        ));
    }
    Some(selected(
        image_ref.clone(),
        ArtworkProvenance::Source,
        source_fetch_policy(image_ref),
    ))
}

fn selected_track_ref(
    image_ref: Option<ImageRef>,
    original_ref: Option<&ImageRef>,
    album_image_ref: Option<&ImageRef>,
) -> SelectedArtwork {
    let Some(image_ref) = image_ref else {
        return missing_artwork();
    };
    let provenance = if original_ref.is_some_and(|original| original == &image_ref) {
        if external_metadata::is_external_image_ref(&image_ref) {
            external_ref_provenance(&image_ref)
        } else {
            ArtworkProvenance::Source
        }
    } else if album_image_ref.is_some_and(|album_ref| album_ref == &image_ref) {
        ArtworkProvenance::RepresentativeFallback
    } else {
        external_ref_provenance(&image_ref)
    };
    let fetch_policy = if external_metadata::is_external_image_ref(&image_ref) {
        ArtworkFetchPolicy::OptionalExternalNetwork
    } else {
        source_fetch_policy(&image_ref)
    };
    selected(image_ref, provenance, fetch_policy)
}

fn selected(
    image_ref: ImageRef,
    provenance: ArtworkProvenance,
    fetch_policy: ArtworkFetchPolicy,
) -> SelectedArtwork {
    SelectedArtwork {
        image_ref: Some(image_ref.clone()),
        image_refs: vec![image_ref],
        selection: ArtworkSelection::ImageRefs,
        provenance,
        fetch_policy,
    }
}

fn missing_artwork() -> SelectedArtwork {
    SelectedArtwork {
        image_ref: None,
        image_refs: Vec::new(),
        selection: ArtworkSelection::FinalMissing,
        provenance: ArtworkProvenance::None,
        fetch_policy: ArtworkFetchPolicy::FinalMissing,
    }
}

fn selected_collection_artwork(
    image_refs: &[ImageRef],
    image_ref: Option<&ImageRef>,
    prefer_single_ref: bool,
) -> SelectedArtwork {
    let selected_refs = selected_collection_refs(image_refs, image_ref, prefer_single_ref);
    let Some(selected_ref) = selected_refs.first().cloned() else {
        return missing_artwork();
    };
    let direct_selected = image_ref.is_some_and(|direct| direct == &selected_ref)
        && (prefer_single_ref || image_refs.is_empty());
    let provenance = if direct_selected {
        if external_metadata::is_external_image_ref(&selected_ref) {
            external_ref_provenance(&selected_ref)
        } else {
            ArtworkProvenance::Source
        }
    } else {
        ArtworkProvenance::RepresentativeFallback
    };
    let fetch_policy = if external_metadata::is_external_image_ref(&selected_ref) {
        ArtworkFetchPolicy::OptionalExternalNetwork
    } else {
        source_fetch_policy(&selected_ref)
    };
    SelectedArtwork {
        image_ref: Some(selected_ref.clone()),
        image_refs: selected_refs,
        selection: ArtworkSelection::ImageRefs,
        provenance,
        fetch_policy,
    }
}

pub fn selected_collection_refs(
    image_refs: &[ImageRef],
    image_ref: Option<&ImageRef>,
    prefer_single_ref: bool,
) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    if prefer_single_ref {
        push_selected_ref(&mut refs, image_ref);
        if !refs.is_empty() {
            return refs;
        }
    }
    for image_ref in image_refs {
        push_selected_ref(&mut refs, Some(image_ref));
    }
    if refs.is_empty() {
        push_selected_ref(&mut refs, image_ref);
    }
    refs
}

pub fn selected_collection_slots(image_refs: &[ImageRef]) -> Vec<ImageRef> {
    let Some(first) = image_refs.first() else {
        return Vec::new();
    };
    if image_refs.len() == 1 {
        return vec![first.clone()];
    }
    (0..GROUP_ARTWORK_REF_LIMIT)
        .filter_map(|index| image_refs.get(index % image_refs.len()).cloned())
        .collect()
}

fn push_selected_ref(refs: &mut Vec<ImageRef>, image_ref: Option<&ImageRef>) {
    if refs.len() >= GROUP_ARTWORK_REF_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    if refs.iter().any(|existing| existing == image_ref) {
        return;
    }
    refs.push(image_ref.clone());
}

fn source_fetch_policy(image_ref: &ImageRef) -> ArtworkFetchPolicy {
    if image_ref.item_id.starts_with("local:cover:") {
        ArtworkFetchPolicy::LocalSource
    } else {
        ArtworkFetchPolicy::ProviderSource
    }
}

fn external_ref_provenance(image_ref: &ImageRef) -> ArtworkProvenance {
    if image_ref.item_id.starts_with("external:mb-release") {
        ArtworkProvenance::ExternalMbid
    } else {
        ArtworkProvenance::ExternalTextFallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_core::{AlbumId, ArtistId, PlaylistId, TrackId};

    #[test]
    fn policy_source_art_wins() {
        let album = Album {
            image_ref: Some(ImageRef::new(
                "local:cover:file%3A%2F%2Fcover.jpg",
                Some("tag-one".to_string()),
            )),
            musicbrainz_release_group_id: Some("group-one".to_string()),
            ..album_without_cover()
        };

        let artwork = selected_album_artwork(
            &album,
            &AppSettings {
                external_metadata_enabled: true,
                ..Default::default()
            },
        );

        assert_eq!(
            artwork
                .image_ref
                .as_ref()
                .map(|image_ref| image_ref.item_id.as_str()),
            Some("local:cover:file%3A%2F%2Fcover.jpg")
        );
        assert_eq!(artwork.provenance, ArtworkProvenance::Source);
        assert_eq!(artwork.fetch_policy, ArtworkFetchPolicy::LocalSource);
    }

    #[test]
    fn policy_mbid_before_text() {
        let album = Album {
            musicbrainz_release_group_id: Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string()),
            musicbrainz_album_id: Some("3d92a863-0a57-4b49-b2a9-87c1446b53e0".to_string()),
            ..album_without_cover()
        };

        let artwork = selected_album_artwork(
            &album,
            &AppSettings {
                external_metadata_enabled: true,
                ..Default::default()
            },
        );

        assert_eq!(artwork.provenance, ArtworkProvenance::ExternalMbid);
        assert!(
            artwork.image_ref.as_ref().is_some_and(|image_ref| image_ref
                .item_id
                .starts_with("external:mb-release-group:"))
        );
    }

    #[test]
    fn policy_private_mode_is_final_missing() {
        let album = album_without_cover();

        let artwork = selected_album_artwork(
            &album,
            &AppSettings {
                external_metadata_enabled: true,
                private_mode: true,
                ..Default::default()
            },
        );

        assert_eq!(artwork.provenance, ArtworkProvenance::None);
        assert_eq!(artwork.fetch_policy, ArtworkFetchPolicy::FinalMissing);
        assert_eq!(artwork.selection, ArtworkSelection::FinalMissing);
        assert!(artwork.image_ref.is_none());
    }

    #[test]
    fn policy_missing_mbid_keeps_text_fallback() {
        let album = Album {
            musicbrainz_release_group_id: Some("".to_string()),
            musicbrainz_album_id: Some("not a mbid".to_string()),
            ..album_without_cover()
        };

        let artwork = selected_album_artwork(
            &album,
            &AppSettings {
                external_metadata_enabled: true,
                ..Default::default()
            },
        );

        assert_eq!(artwork.provenance, ArtworkProvenance::ExternalTextFallback);
        assert!(
            artwork
                .image_ref
                .as_ref()
                .is_some_and(|image_ref| image_ref.item_id.starts_with("external:album:"))
        );
    }

    #[test]
    fn policy_track_uses_selected_album_ref() {
        let album_ref = ImageRef::new(
            "external:mb-release-group:group-one",
            Some("tag-one".to_string()),
        );
        let track = track_without_cover();

        let artwork = selected_track_artwork(
            &track,
            Some(&album_ref),
            &AppSettings {
                external_metadata_enabled: true,
                ..Default::default()
            },
        );

        assert_eq!(artwork.image_ref, Some(album_ref));
        assert_eq!(
            artwork.provenance,
            ArtworkProvenance::RepresentativeFallback
        );
    }

    #[test]
    fn playlist_policy_keeps_group_art() {
        let server_ref = image_ref("server-cover");
        let group_ref = image_ref("group-cover");
        let mut playlist = playlist_with_refs(Some(server_ref), vec![group_ref.clone()]);

        bind_playlist(&mut playlist, &AppSettings::default());

        assert_eq!(playlist.image_refs, vec![group_ref]);
    }

    #[test]
    fn collection_policy_dedupes_and_caps_group_art() {
        let first = image_ref("first-cover");
        let second = image_ref("second-cover");
        let third = image_ref("third-cover");
        let fourth = image_ref("fourth-cover");
        let fifth = image_ref("fifth-cover");
        let fallback = image_ref("fallback-cover");

        let selected = selected_collection_refs(
            &[
                first.clone(),
                second.clone(),
                first.clone(),
                third.clone(),
                fourth.clone(),
                fifth,
            ],
            Some(&fallback),
            false,
        );

        assert_eq!(selected, vec![first, second, third, fourth]);
    }

    #[test]
    fn collection_policy_uses_direct_ref_only_when_group_empty() {
        let fallback = image_ref("fallback-cover");

        assert_eq!(
            selected_collection_refs(&[], Some(&fallback), false),
            vec![fallback]
        );
    }

    #[test]
    fn collection_policy_binds_no_art_explicitly() {
        let genre = Genre {
            id: rufin_core::GenreId::new("genre:empty"),
            name: "Empty Genre".to_string(),
            album_count: 1,
            track_count: 3,
            image_ref: None,
            image_refs: Vec::new(),
        };

        let artwork = selected_genre_artwork(&genre);

        assert_eq!(artwork.selection, ArtworkSelection::FinalMissing);
        assert_eq!(artwork.provenance, ArtworkProvenance::None);
        assert_eq!(artwork.fetch_policy, ArtworkFetchPolicy::FinalMissing);
        assert!(artwork.image_ref.is_none());
        assert!(artwork.image_refs.is_empty());
    }

    #[test]
    fn collection_policy_binds_representative_refs_explicitly() {
        let first = image_ref("first-cover");
        let second = image_ref("second-cover");
        let genre = Genre {
            id: rufin_core::GenreId::new("genre:covered"),
            name: "Covered Genre".to_string(),
            album_count: 2,
            track_count: 4,
            image_ref: None,
            image_refs: vec![first.clone(), second.clone()],
        };

        let artwork = selected_genre_artwork(&genre);

        assert_eq!(artwork.selection, ArtworkSelection::ImageRefs);
        assert_eq!(
            artwork.provenance,
            ArtworkProvenance::RepresentativeFallback
        );
        assert_eq!(artwork.image_ref, Some(first.clone()));
        assert_eq!(artwork.image_refs, vec![first, second]);
    }

    #[test]
    fn collection_policy_slots_stable_without_decode_state() {
        let first = image_ref("first-cover");
        let second = image_ref("second-cover");
        let slots = selected_collection_slots(&[first.clone(), second.clone()]);

        assert_eq!(slots, vec![first.clone(), second.clone(), first, second]);
    }

    #[test]
    fn playlist_policy_prefers_server_art() {
        let server_ref = image_ref("server-cover");
        let group_ref = image_ref("group-cover");
        let mut playlist = playlist_with_refs(Some(server_ref.clone()), vec![group_ref]);
        let settings = AppSettings {
            prefer_server_playlist_covers: true,
            ..AppSettings::default()
        };

        let artwork = selected_playlist_artwork(&playlist, &settings);
        bind_playlist(&mut playlist, &settings);

        assert_eq!(artwork.selection, ArtworkSelection::ImageRefs);
        assert_eq!(artwork.provenance, ArtworkProvenance::Source);
        assert_eq!(playlist.image_refs, vec![server_ref]);
    }

    #[test]
    fn playlist_policy_uses_server_art_when_group_empty() {
        let server_ref = image_ref("server-cover");
        let mut playlist = playlist_with_refs(Some(server_ref.clone()), Vec::new());

        let artwork = selected_playlist_artwork(&playlist, &AppSettings::default());
        bind_playlist(&mut playlist, &AppSettings::default());

        assert_eq!(artwork.provenance, ArtworkProvenance::Source);
        assert_eq!(playlist.image_refs, vec![server_ref]);
    }

    #[test]
    fn playlist_policy_keeps_group_art_without_server_art() {
        let group_ref = image_ref("group-cover");
        let mut playlist = playlist_with_refs(None, vec![group_ref.clone()]);

        bind_playlist(
            &mut playlist,
            &AppSettings {
                prefer_server_playlist_covers: true,
                ..AppSettings::default()
            },
        );

        assert_eq!(playlist.image_refs, vec![group_ref]);
    }

    fn album_without_cover() -> Album {
        Album {
            id: AlbumId::fake(1),
            title: "Example Album".to_string(),
            artist: "Example Artist".to_string(),
            artist_id: Some(ArtistId::fake(1)),
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 1991,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 60,
            favorite: false,
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }

    fn track_without_cover() -> Track {
        Track {
            id: TrackId::fake(1),
            album_id: AlbumId::fake(1),
            title: "Example Track".to_string(),
            artist: "Example Artist".to_string(),
            artist_id: Some(ArtistId::fake(1)),
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Example Album".to_string(),
            year: 1991,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            disc_number: 1,
            track_number: 1,
            duration_seconds: 60,
            favorite: false,
            image_ref: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
        }
    }

    fn playlist_with_refs(image_ref: Option<ImageRef>, image_refs: Vec<ImageRef>) -> Playlist {
        Playlist {
            id: PlaylistId::fake(1),
            name: "Example Playlist".to_string(),
            track_count: 2,
            duration_seconds: 120,
            image_refs,
            image_ref,
        }
    }

    fn image_ref(item_id: &str) -> ImageRef {
        ImageRef::new(item_id, Some(format!("{item_id}-tag")))
    }
}
