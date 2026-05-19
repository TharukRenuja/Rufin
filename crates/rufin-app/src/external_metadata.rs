use rufin_core::{
    Album, AppSettings, Artist, HomeSection, ImageRef, QueueEntry, QueueSnapshot, Track,
};
use rufin_provider::SearchResults;

mod album_lookup;

pub use album_lookup::fetch_album_cover;

const EXTERNAL_ALBUM_IMAGE_PREFIX: &str = "external:album:";
const EXTERNAL_ARTIST_IMAGE_PREFIX: &str = "external:artist:";
const EXTERNAL_ALBUM_IMAGE_TAG_VERSION: &str = "external-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalAlbumArt {
    pub artist: String,
    pub album: String,
}

pub fn enabled(settings: &AppSettings) -> bool {
    settings.external_metadata_enabled && !settings.private_mode
}

pub fn is_external_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with(EXTERNAL_ALBUM_IMAGE_PREFIX)
        || image_ref.item_id.starts_with(EXTERNAL_ARTIST_IMAGE_PREFIX)
}

pub fn is_external_artist_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with(EXTERNAL_ARTIST_IMAGE_PREFIX)
}

pub fn album_art_from_image_ref(image_ref: &ImageRef) -> Option<ExternalAlbumArt> {
    let rest = image_ref
        .item_id
        .strip_prefix(EXTERNAL_ALBUM_IMAGE_PREFIX)?;
    let (artist, album) = rest.split_once(':')?;
    Some(ExternalAlbumArt {
        artist: percent_decode_component(artist)?,
        album: percent_decode_component(album)?,
    })
}

pub fn normalize_album(album: &mut Album, settings: &AppSettings) {
    normalize_image_ref(&mut album.image_ref, settings);
    if enabled(settings) && album.image_ref.is_none() {
        album.image_ref = external_album_image_ref(&album.artist, &album.title);
    }
}

pub fn normalize_track(track: &mut Track, settings: &AppSettings) {
    normalize_image_ref(&mut track.image_ref, settings);
    if enabled(settings)
        && has_untagged_jellyfin_album_ref(&track.image_ref, track.album_id.as_str())
    {
        track.image_ref = None;
    }
    if enabled(settings) && track.image_ref.is_none() {
        track.image_ref = external_album_image_ref(&track.artist, &track.album);
    }
}

pub fn normalize_artist(artist: &mut Artist, settings: &AppSettings) {
    if artist
        .image_ref
        .as_ref()
        .is_some_and(is_external_artist_image_ref)
    {
        artist.image_ref = None;
        return;
    }
    normalize_image_ref(&mut artist.image_ref, settings);
}

pub fn normalize_albums(albums: &mut [Album], settings: &AppSettings) {
    for album in albums {
        normalize_album(album, settings);
    }
}

pub fn normalize_tracks(tracks: &mut [Track], settings: &AppSettings) {
    for track in tracks {
        normalize_track(track, settings);
    }
}

pub fn normalize_artists(artists: &mut [Artist], settings: &AppSettings) {
    for artist in artists {
        normalize_artist(artist, settings);
    }
}

pub fn normalize_home_sections(sections: &mut [HomeSection], settings: &AppSettings) {
    for section in sections {
        normalize_home_section(section, settings);
    }
}

pub fn normalize_home_section(section: &mut HomeSection, settings: &AppSettings) {
    normalize_albums(&mut section.albums, settings);
    normalize_tracks(&mut section.tracks, settings);
}

pub fn normalize_search_results(results: &mut SearchResults, settings: &AppSettings) {
    normalize_albums(&mut results.albums, settings);
    normalize_tracks(&mut results.tracks, settings);
    normalize_artists(&mut results.artists, settings);
}

pub fn normalize_queue_snapshot(snapshot: &mut QueueSnapshot, settings: &AppSettings) {
    for entry in &mut snapshot.entries {
        normalize_queue_entry(entry, settings);
    }
}

pub fn normalize_queue_entry(entry: &mut QueueEntry, settings: &AppSettings) {
    normalize_image_ref(&mut entry.image_ref, settings);
    if enabled(settings)
        && entry.album_id.as_ref().is_some_and(|album_id| {
            has_untagged_jellyfin_album_ref(&entry.image_ref, album_id.as_str())
        })
    {
        entry.image_ref = None;
    }
    if enabled(settings) && entry.image_ref.is_none() {
        entry.image_ref = external_album_image_ref(&entry.artist, &entry.album);
    }
}

pub fn is_expected_lookup_miss(error: &str) -> bool {
    if error.contains("error sending request")
        || error.contains("timed out")
        || error.contains("status 401")
        || error.contains("status 403")
        || error.contains("status 429")
        || error.contains("status 500")
        || error.contains("status 502")
        || error.contains("status 503")
        || error.contains("status 504")
    {
        return false;
    }

    error.contains("404 Not Found")
        || error.contains("did not return album art")
        || error.contains("did not return matching")
        || error.contains("external artist image lookup is disabled")
}

fn normalize_image_ref(image_ref: &mut Option<ImageRef>, settings: &AppSettings) {
    if image_ref
        .as_ref()
        .is_some_and(|image_ref| is_external_image_ref(image_ref) && !enabled(settings))
    {
        *image_ref = None;
    }
}

fn has_untagged_jellyfin_album_ref(image_ref: &Option<ImageRef>, album_id: &str) -> bool {
    image_ref.as_ref().is_some_and(|image_ref| {
        image_ref.item_id == album_id
            && image_ref.item_id.starts_with("jellyfin:album:")
            && image_ref.tag.as_deref().is_none_or(str::is_empty)
    })
}

fn external_album_image_ref(artist: &str, album: &str) -> Option<ImageRef> {
    let artist = normalized_lookup_value(artist)?;
    let album = normalized_lookup_value(album)?;
    let item_id = format!(
        "{EXTERNAL_ALBUM_IMAGE_PREFIX}{}:{}",
        percent_encode_component(&artist),
        percent_encode_component(&album)
    );
    let tag = format!(
        "{EXTERNAL_ALBUM_IMAGE_TAG_VERSION}-{:016x}",
        stable_album_hash(&artist, &album)
    );
    Some(ImageRef::new(item_id, Some(tag)))
}

fn normalized_lookup_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_lowercase();
    if matches!(
        lower.as_str(),
        "unknown" | "unknown album" | "unknown artist" | "untitled album" | "untitled track"
    ) {
        return None;
    }
    Some(value.to_string())
}

fn stable_album_hash(artist: &str, album: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in artist
        .as_bytes()
        .iter()
        .copied()
        .chain([0])
        .chain(album.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn percent_encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            byte => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn percent_decode_component(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let high = input.next()?;
        let low = input.next()?;
        bytes.push(hex_value(high)? * 16 + hex_value(low)?);
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::album_lookup::{cover_art_size_path, json_ids, lastfm_album_image_url};
    use super::{
        album_art_from_image_ref, enabled, is_expected_lookup_miss, is_external_image_ref,
        normalize_album, normalize_artist, normalize_queue_entry, normalize_track,
    };
    use rufin_core::{
        Album, AlbumId, AppSettings, Artist, ArtistId, ImageRef, QueueEntry, QueueEntryId, Track,
        TrackId,
    };
    use serde_json::json;

    #[test]
    fn external_metadata_requires_setting_and_non_private_mode() {
        let mut settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };

        assert!(enabled(&settings));

        settings.private_mode = true;
        assert!(!enabled(&settings));
    }

    #[test]
    fn album_external_cover_refs_round_trip_lookup_values() {
        let mut album = album_without_cover("Hurry Up, We're Dreaming", "M83");
        normalize_album(
            &mut album,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        let image_ref = album.image_ref.expect("external image ref");
        assert!(is_external_image_ref(&image_ref));
        assert_eq!(
            album_art_from_image_ref(&image_ref),
            Some(super::ExternalAlbumArt {
                artist: "M83".to_string(),
                album: "Hurry Up, We're Dreaming".to_string(),
            })
        );
    }

    #[test]
    fn disabled_external_metadata_strips_synthetic_refs() {
        let enabled_settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };
        let mut track = track_without_cover("Midnight City", "M83", "Hurry Up, We're Dreaming");
        normalize_track(&mut track, &enabled_settings);
        assert!(track.image_ref.is_some());

        normalize_track(
            &mut track,
            &AppSettings {
                external_metadata_enabled: false,
                ..AppSettings::default()
            },
        );

        assert_eq!(track.image_ref, None);
    }

    #[test]
    fn tracks_with_untagged_jellyfin_album_refs_use_external_album_fallback() {
        let mut track = track_without_cover("Example Track", "Example Artist", "Example Album");
        track.album_id = AlbumId::new("jellyfin:album:one");
        track.image_ref = Some(ImageRef::new("jellyfin:album:one", None));

        normalize_track(
            &mut track,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        let image_ref = track.image_ref.expect("external album image ref");
        assert!(is_external_image_ref(&image_ref));
        assert_eq!(
            album_art_from_image_ref(&image_ref),
            Some(super::ExternalAlbumArt {
                artist: "Example Artist".to_string(),
                album: "Example Album".to_string(),
            })
        );
    }

    #[test]
    fn tagged_or_non_jellyfin_track_refs_are_kept() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };
        let tagged_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));
        let local_ref = ImageRef::new("local:cover:one", None);
        let mut tagged_track =
            track_without_cover("Midnight City", "M83", "Hurry Up, We're Dreaming");
        tagged_track.album_id = AlbumId::new("jellyfin:album:one");
        tagged_track.image_ref = Some(tagged_ref.clone());
        let mut local_track =
            track_without_cover("Midnight City", "M83", "Hurry Up, We're Dreaming");
        local_track.album_id = AlbumId::new("jellyfin:album:one");
        local_track.image_ref = Some(local_ref.clone());

        normalize_track(&mut tagged_track, &settings);
        normalize_track(&mut local_track, &settings);

        assert_eq!(tagged_track.image_ref, Some(tagged_ref));
        assert_eq!(local_track.image_ref, Some(local_ref));
    }

    #[test]
    fn queue_entries_with_untagged_jellyfin_album_refs_use_external_album_fallback() {
        let mut entry =
            queue_entry_without_cover("Example Track", "Example Artist", "Example Album");
        entry.album_id = Some(AlbumId::new("jellyfin:album:one"));
        entry.image_ref = Some(ImageRef::new("jellyfin:album:one", None));

        normalize_queue_entry(
            &mut entry,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        let image_ref = entry.image_ref.expect("external album image ref");
        assert!(is_external_image_ref(&image_ref));
        assert_eq!(
            album_art_from_image_ref(&image_ref),
            Some(super::ExternalAlbumArt {
                artist: "Example Artist".to_string(),
                album: "Example Album".to_string(),
            })
        );
    }

    #[test]
    fn artists_do_not_create_external_image_refs() {
        let mut artist = artist_without_cover("Slowdive");
        normalize_artist(
            &mut artist,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );
        assert_eq!(artist.image_ref, None);

        normalize_artist(
            &mut artist,
            &AppSettings {
                external_metadata_enabled: true,
                lastfm_api_key: "key".to_string(),
                ..AppSettings::default()
            },
        );

        assert_eq!(artist.image_ref, None);
    }

    #[test]
    fn stale_artist_external_image_refs_are_removed() {
        let mut artist = artist_without_cover("Slowdive");
        artist.image_ref = Some(ImageRef::new(
            "external:artist:Slowdive",
            Some("external-artist-v1-old".to_string()),
        ));

        normalize_artist(
            &mut artist,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        assert_eq!(artist.image_ref, None);
    }

    #[test]
    fn unknown_album_metadata_does_not_create_external_cover_ref() {
        let mut album = album_without_cover("Unknown Album", "Unknown Artist");
        normalize_album(
            &mut album,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        assert_eq!(album.image_ref, None);
    }

    #[test]
    fn lastfm_album_image_url_uses_largest_available_image() {
        let value = json!({
            "album": {
                "image": [
                    { "#text": "https://example.test/small.jpg", "size": "small" },
                    { "#text": "", "size": "medium" },
                    { "#text": "https://example.test/large.jpg", "size": "extralarge" }
                ]
            }
        });

        assert_eq!(
            lastfm_album_image_url(&value).unwrap(),
            Some("https://example.test/large.jpg".to_string())
        );
    }

    #[test]
    fn lastfm_album_not_found_is_a_lookup_miss() {
        let value = json!({
            "error": 6,
            "message": "Album not found"
        });

        assert_eq!(lastfm_album_image_url(&value).unwrap(), None);
    }

    #[test]
    fn lastfm_placeholder_image_url_does_not_hide_real_fallback_image() {
        let value = json!({
            "album": {
                "image": [
                    { "#text": "https://example.test/small.jpg", "size": "small" },
                    {
                        "#text": "https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png",
                        "size": "extralarge"
                    }
                ]
            }
        });

        assert_eq!(
            lastfm_album_image_url(&value).unwrap(),
            Some("https://example.test/small.jpg".to_string())
        );
    }

    #[test]
    fn musicbrainz_id_extraction_deduplicates_empty_and_repeated_ids() {
        let value = json!({
            "release-groups": [
                { "id": "first" },
                { "id": "" },
                { "id": "first" },
                { "id": "second" }
            ]
        });

        assert_eq!(json_ids(&value, "/release-groups"), vec!["first", "second"]);
    }

    #[test]
    fn cover_art_archive_thumbnail_size_uses_supported_steps() {
        assert_eq!(cover_art_size_path(96), "front-250");
        assert_eq!(cover_art_size_path(250), "front-250");
        assert_eq!(cover_art_size_path(256), "front-500");
    }

    #[test]
    fn expected_lookup_misses_exclude_network_and_service_errors() {
        assert!(is_expected_lookup_miss(
            "external cover image failed with status 404 Not Found"
        ));
        assert!(is_expected_lookup_miss(
            "MusicBrainz did not return matching release groups"
        ));
        assert!(!is_expected_lookup_miss(
            "error sending request for url (https://coverartarchive.org/release/id/front-500)"
        ));
        assert!(!is_expected_lookup_miss(
            "MusicBrainz release lookup failed with status 503 Service Unavailable"
        ));
    }

    fn album_without_cover(title: &str, artist: &str) -> Album {
        Album {
            id: AlbumId::new("album-one"),
            title: title.to_string(),
            artist: artist.to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2011,
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
        }
    }

    fn track_without_cover(title: &str, artist: &str, album: &str) -> Track {
        Track {
            id: TrackId::new("track-one"),
            album_id: AlbumId::new("album-one"),
            title: title.to_string(),
            artist: artist.to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: album.to_string(),
            year: 2011,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 60,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            genres: Vec::new(),
            local_path: None,
        }
    }

    fn queue_entry_without_cover(title: &str, artist: &str, album: &str) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new("entry-one"),
            track_id: TrackId::new("track-one"),
            album_id: Some(AlbumId::new("album-one")),
            title: title.to_string(),
            artist: artist.to_string(),
            artist_id: None,
            album: album.to_string(),
            year: 2011,
            duration_seconds: 60,
            favorite: false,
            image_ref: None,
        }
    }

    fn artist_without_cover(name: &str) -> Artist {
        Artist {
            id: ArtistId::new(format!("artist-{name}")),
            name: name.to_string(),
            album_count: 1,
            track_count: 1,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            image_ref: None,
        }
    }
}
