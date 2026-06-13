use rufin_core::{
    Album, AppSettings, Artist, HomeSection, ImageRef, Playlist, QueueEntry, QueueSnapshot, Track,
};
use rufin_provider::{PlaylistDetail, SearchResults};

use crate::external_metadata;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedArtwork {
    pub image_ref: Option<ImageRef>,
    pub provenance: ArtworkProvenance,
    pub fetch_policy: ArtworkFetchPolicy,
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
    if settings.prefer_server_playlist_covers {
        if let Some(image_ref) = playlist.image_ref.clone() {
            playlist.image_refs = vec![image_ref];
        }
        return;
    }
    if playlist.image_refs.is_empty()
        && let Some(image_ref) = playlist.image_ref.clone()
    {
        playlist.image_refs = vec![image_ref];
    }
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
        image_ref: Some(image_ref),
        provenance,
        fetch_policy,
    }
}

fn missing_artwork() -> SelectedArtwork {
    SelectedArtwork {
        image_ref: None,
        provenance: ArtworkProvenance::None,
        fetch_policy: ArtworkFetchPolicy::FinalMissing,
    }
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
    fn playlist_policy_prefers_server_art() {
        let server_ref = image_ref("server-cover");
        let group_ref = image_ref("group-cover");
        let mut playlist = playlist_with_refs(Some(server_ref.clone()), vec![group_ref]);

        bind_playlist(
            &mut playlist,
            &AppSettings {
                prefer_server_playlist_covers: true,
                ..AppSettings::default()
            },
        );

        assert_eq!(playlist.image_refs, vec![server_ref]);
    }

    #[test]
    fn playlist_policy_uses_server_art_when_group_empty() {
        let server_ref = image_ref("server-cover");
        let mut playlist = playlist_with_refs(Some(server_ref.clone()), Vec::new());

        bind_playlist(&mut playlist, &AppSettings::default());

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
