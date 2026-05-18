use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::Client;
use rufin_core::{
    Album, AppSettings, Artist, HomeSection, ImageRef, QueueEntry, QueueSnapshot, Track,
};
use rufin_provider::SearchResults;
use serde_json::Value;

const EXTERNAL_ALBUM_IMAGE_PREFIX: &str = "external:album:";
const EXTERNAL_ARTIST_IMAGE_PREFIX: &str = "external:artist:";
const EXTERNAL_ALBUM_IMAGE_TAG_VERSION: &str = "external-v1";
const EXTERNAL_ARTIST_IMAGE_TAG_VERSION: &str = "external-artist-v1";
const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_ARCHIVE_RELEASE_URL: &str = "https://coverartarchive.org/release";
const COVER_ART_ARCHIVE_RELEASE_GROUP_URL: &str = "https://coverartarchive.org/release-group";
const EXTERNAL_METADATA_USER_AGENT: &str = concat!(
    "Rufin/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/screwys/Rufin)"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalAlbumArt {
    pub artist: String,
    pub album: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalArtistImage {
    pub artist: String,
}

pub fn enabled(settings: &AppSettings) -> bool {
    settings.external_metadata_enabled && !settings.private_mode
}

pub fn is_external_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with(EXTERNAL_ALBUM_IMAGE_PREFIX)
        || image_ref.item_id.starts_with(EXTERNAL_ARTIST_IMAGE_PREFIX)
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

pub fn artist_image_from_image_ref(image_ref: &ImageRef) -> Option<ExternalArtistImage> {
    let artist = image_ref
        .item_id
        .strip_prefix(EXTERNAL_ARTIST_IMAGE_PREFIX)?;
    Some(ExternalArtistImage {
        artist: percent_decode_component(artist)?,
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
    if enabled(settings) && track.image_ref.is_none() {
        track.image_ref = external_album_image_ref(&track.artist, &track.album);
    }
}

pub fn normalize_artist(artist: &mut Artist, settings: &AppSettings) {
    normalize_image_ref(&mut artist.image_ref, settings);
    if enabled(settings) && !settings.lastfm_api_key.trim().is_empty() && artist.image_ref.is_none()
    {
        artist.image_ref = external_artist_image_ref(&artist.name);
    }
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
    if enabled(settings) && entry.image_ref.is_none() {
        entry.image_ref = external_album_image_ref(&entry.artist, &entry.album);
    }
}

pub fn fetch_album_cover(
    art: &ExternalAlbumArt,
    size: u32,
    lastfm_api_key: &str,
) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;

    let mut errors = Vec::new();
    if !lastfm_api_key.trim().is_empty() {
        match lastfm_album_cover_url(&client, art, lastfm_api_key) {
            Ok(Some(url)) => match download_image(&client, &url) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => errors.push(error),
            },
            Ok(None) => errors.push("Last.fm did not return album art".to_string()),
            Err(error) => errors.push(error),
        }
    }

    match cover_art_archive_release_group_urls(&client, art, size) {
        Ok(urls) => {
            for url in urls {
                match download_image(&client, &url) {
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => errors.push(error),
                }
            }
        }
        Err(error) => errors.push(error),
    }

    match cover_art_archive_release_urls(&client, art, size) {
        Ok(urls) => {
            for url in urls {
                match download_image(&client, &url) {
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => errors.push(error),
                }
            }
        }
        Err(error) => errors.push(error),
    }

    Err(format!(
        "external cover lookup found no usable image: {}",
        errors.join("; ")
    ))
}

pub fn fetch_artist_image(
    image: &ExternalArtistImage,
    lastfm_api_key: &str,
) -> Result<Vec<u8>, String> {
    let lastfm_api_key = lastfm_api_key.trim();
    if lastfm_api_key.is_empty() {
        return Err("Last.fm API key is required for external artist images".to_string());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;

    match lastfm_artist_image_url(&client, image, lastfm_api_key) {
        Ok(Some(url)) => download_image(&client, &url),
        Ok(None) => Err("Last.fm did not return artist image".to_string()),
        Err(error) => Err(error),
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
        || error.contains("did not return artist image")
        || error.contains("did not return matching")
        || error.contains("API key is required for external artist images")
}

fn download_image(client: &Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "external cover image failed with status {}",
            response.status()
        ));
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err("external cover response was empty".to_string());
    }
    Ok(bytes.to_vec())
}

fn lastfm_album_cover_url(
    client: &Client,
    art: &ExternalAlbumArt,
    api_key: &str,
) -> Result<Option<String>, String> {
    let url = Url::parse_with_params(
        LASTFM_API_URL,
        [
            ("method", "album.getinfo"),
            ("api_key", api_key.trim()),
            ("artist", art.artist.as_str()),
            ("album", art.album.as_str()),
            ("autocorrect", "1"),
            ("format", "json"),
        ],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_json(client, url, "Last.fm lookup")?;
    lastfm_album_image_url(&value)
}

fn lastfm_artist_image_url(
    client: &Client,
    image: &ExternalArtistImage,
    api_key: &str,
) -> Result<Option<String>, String> {
    let url = Url::parse_with_params(
        LASTFM_API_URL,
        [
            ("method", "artist.getinfo"),
            ("api_key", api_key.trim()),
            ("artist", image.artist.as_str()),
            ("autocorrect", "1"),
            ("format", "json"),
        ],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_json(client, url, "Last.fm artist lookup")?;
    lastfm_artist_image_url_from_value(&value)
}

fn lastfm_album_image_url(value: &Value) -> Result<Option<String>, String> {
    lastfm_image_url(value, "/album/image")
}

fn lastfm_artist_image_url_from_value(value: &Value) -> Result<Option<String>, String> {
    lastfm_image_url(value, "/artist/image")
}

fn lastfm_image_url(value: &Value, image_pointer: &str) -> Result<Option<String>, String> {
    if let Some(error_code) = value.get("error").and_then(Value::as_i64) {
        if error_code == 6 {
            return Ok(None);
        }
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!(
            "Last.fm lookup returned error {error_code}: {message}"
        ));
    }

    let Some(images) = value.pointer(image_pointer).and_then(Value::as_array) else {
        return Ok(None);
    };
    Ok(images
        .iter()
        .rev()
        .filter_map(|image| image.get("#text").and_then(Value::as_str))
        .map(str::trim)
        .find(|url| !url.is_empty())
        .map(str::to_string))
}

fn cover_art_archive_release_group_urls(
    client: &Client,
    art: &ExternalAlbumArt,
    size: u32,
) -> Result<Vec<String>, String> {
    let ids = musicbrainz_release_group_ids(client, art)?;
    let cover_path = cover_art_size_path(size);
    Ok(ids
        .into_iter()
        .map(|id| format!("{COVER_ART_ARCHIVE_RELEASE_GROUP_URL}/{id}/{cover_path}"))
        .collect())
}

fn cover_art_archive_release_urls(
    client: &Client,
    art: &ExternalAlbumArt,
    size: u32,
) -> Result<Vec<String>, String> {
    let ids = musicbrainz_release_ids(client, art)?;
    let cover_path = cover_art_size_path(size);
    Ok(ids
        .into_iter()
        .map(|id| format!("{COVER_ART_ARCHIVE_RELEASE_URL}/{id}/{cover_path}"))
        .collect())
}

fn musicbrainz_release_group_ids(
    client: &Client,
    art: &ExternalAlbumArt,
) -> Result<Vec<String>, String> {
    let query = format!(
        "artist:\"{}\" AND releasegroup:\"{}\"",
        musicbrainz_phrase(&art.artist),
        musicbrainz_phrase(&art.album)
    );
    let url = Url::parse_with_params(
        MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL,
        [("query", query.as_str()), ("fmt", "json"), ("limit", "5")],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_json(client, url, "MusicBrainz release-group lookup")?;
    let ids = json_ids(&value, "/release-groups");
    if ids.is_empty() {
        Err("MusicBrainz did not return matching release groups".to_string())
    } else {
        Ok(ids)
    }
}

fn musicbrainz_release_ids(client: &Client, art: &ExternalAlbumArt) -> Result<Vec<String>, String> {
    let query = format!(
        "artist:\"{}\" AND release:\"{}\"",
        musicbrainz_phrase(&art.artist),
        musicbrainz_phrase(&art.album)
    );
    let url = Url::parse_with_params(
        MUSICBRAINZ_RELEASE_SEARCH_URL,
        [("query", query.as_str()), ("fmt", "json"), ("limit", "5")],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_json(client, url, "MusicBrainz release lookup")?;
    let ids = json_ids(&value, "/releases");
    if ids.is_empty() {
        Err("MusicBrainz did not return matching releases".to_string())
    } else {
        Ok(ids)
    }
}

fn fetch_json(client: &Client, url: Url, context: &str) -> Result<Value, String> {
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "{context} failed with status {}",
            response.status()
        ));
    }
    response.json::<Value>().map_err(|error| error.to_string())
}

fn json_ids(value: &Value, collection_pointer: &str) -> Vec<String> {
    let Some(items) = value.pointer(collection_pointer).and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for id in items
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

fn normalize_image_ref(image_ref: &mut Option<ImageRef>, settings: &AppSettings) {
    if image_ref
        .as_ref()
        .is_some_and(|image_ref| is_external_image_ref(image_ref) && !enabled(settings))
    {
        *image_ref = None;
    }
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

fn external_artist_image_ref(artist: &str) -> Option<ImageRef> {
    let artist = normalized_lookup_value(artist)?;
    let item_id = format!(
        "{EXTERNAL_ARTIST_IMAGE_PREFIX}{}",
        percent_encode_component(&artist)
    );
    let tag = format!(
        "{EXTERNAL_ARTIST_IMAGE_TAG_VERSION}-{:016x}",
        stable_artist_hash(&artist)
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

fn cover_art_size_path(size: u32) -> &'static str {
    if size <= 250 {
        "front-250"
    } else {
        "front-500"
    }
}

fn musicbrainz_phrase(value: &str) -> String {
    value.replace('\\', " ").replace('"', "\\\"")
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

fn stable_artist_hash(artist: &str) -> u64 {
    stable_album_hash(artist, "")
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
    use super::{
        album_art_from_image_ref, artist_image_from_image_ref, cover_art_size_path, enabled,
        is_expected_lookup_miss, is_external_image_ref, json_ids, lastfm_album_image_url,
        lastfm_artist_image_url_from_value, normalize_album, normalize_artist, normalize_track,
    };
    use rufin_core::{Album, AlbumId, AppSettings, Artist, ArtistId, Track, TrackId};
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
    fn artist_external_image_refs_require_lastfm_key_and_round_trip_lookup_values() {
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

        let image_ref = artist.image_ref.expect("external artist image ref");
        assert!(is_external_image_ref(&image_ref));
        assert_eq!(
            artist_image_from_image_ref(&image_ref),
            Some(super::ExternalArtistImage {
                artist: "Slowdive".to_string(),
            })
        );
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
    fn lastfm_artist_image_url_uses_largest_available_image() {
        let value = json!({
            "artist": {
                "image": [
                    { "#text": "https://example.test/small.jpg", "size": "small" },
                    { "#text": "https://example.test/large.jpg", "size": "extralarge" }
                ]
            }
        });

        assert_eq!(
            lastfm_artist_image_url_from_value(&value).unwrap(),
            Some("https://example.test/large.jpg".to_string())
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
        assert!(is_expected_lookup_miss(
            "Last.fm did not return artist image"
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
