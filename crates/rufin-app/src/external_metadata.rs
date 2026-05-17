use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::Client;
use rufin_core::{Album, AppSettings, HomeSection, ImageRef, QueueEntry, QueueSnapshot, Track};
use rufin_provider::SearchResults;
use serde_json::Value;

const EXTERNAL_ALBUM_IMAGE_PREFIX: &str = "external:album:";
const EXTERNAL_ALBUM_IMAGE_TAG_VERSION: &str = "external-v1";
const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const COVER_ART_ARCHIVE_RELEASE_URL: &str = "https://coverartarchive.org/release";
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

pub fn enabled(settings: &AppSettings) -> bool {
    settings.external_metadata_enabled && !settings.private_mode
}

pub fn is_external_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with(EXTERNAL_ALBUM_IMAGE_PREFIX)
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
    if enabled(settings) && track.image_ref.is_none() {
        track.image_ref = external_album_image_ref(&track.artist, &track.album);
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

pub fn fetch_album_cover(art: &ExternalAlbumArt, size: u32) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let release_id = musicbrainz_release_id(&client, art)?;
    let url = format!(
        "{}/{}/{}",
        COVER_ART_ARCHIVE_RELEASE_URL,
        release_id,
        cover_art_size_path(size)
    );
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "external cover lookup failed with status {}",
            response.status()
        ));
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err("external cover response was empty".to_string());
    }
    Ok(bytes.to_vec())
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

fn musicbrainz_release_id(client: &Client, art: &ExternalAlbumArt) -> Result<String, String> {
    let query = format!(
        "artist:\"{}\" AND release:\"{}\"",
        musicbrainz_phrase(&art.artist),
        musicbrainz_phrase(&art.album)
    );
    let url = Url::parse_with_params(
        MUSICBRAINZ_RELEASE_SEARCH_URL,
        [("query", query.as_str()), ("fmt", "json"), ("limit", "1")],
    )
    .map_err(|error| error.to_string())?;
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "MusicBrainz lookup failed with status {}",
            response.status()
        ));
    }
    let value = response
        .json::<Value>()
        .map_err(|error| error.to_string())?;
    value
        .pointer("/releases/0/id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|release_id| !release_id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "MusicBrainz did not return a matching release".to_string())
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
        album_art_from_image_ref, enabled, is_external_image_ref, normalize_album, normalize_track,
    };
    use rufin_core::{Album, AlbumId, AppSettings, Track, TrackId};

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
        }
    }
}
