use domain::{Album, AppSettings, Artist, ImageRef, QueueEntry, Track};

use crate::external_activity;

mod album_lookup;
mod release_type_lookup;

pub use album_lookup::fetch_album_cover;
pub use release_type_lookup::{
    AlbumReleaseMetadata, fetch_album_release_metadata, is_expected_release_type_lookup_miss,
};

const EXTERNAL_ALBUM_IMAGE_PREFIX: &str = "external:album:";
const EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX: &str = "external:mb-release:";
const EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX: &str = "external:mb-release-group:";
const EXTERNAL_ALBUM_IMAGE_TAG_VERSION: &str = "external-v1";
const EXTERNAL_ALBUM_IDENTITY_TAG_VERSION: &str = "external-v2";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalAlbumArt {
    pub artist: String,
    pub album: String,
    pub musicbrainz_release_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
}

pub fn enabled(settings: &AppSettings) -> bool {
    external_activity::external_metadata_lookup(settings)
}

pub fn cached_refs_enabled(settings: &AppSettings) -> bool {
    external_activity::cached_external_metadata_refs(settings)
}

pub fn is_external_image_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with(EXTERNAL_ALBUM_IMAGE_PREFIX)
        || image_ref
            .item_id
            .starts_with(EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX)
        || image_ref
            .item_id
            .starts_with(EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX)
}

pub fn album_art_from_image_ref(image_ref: &ImageRef) -> Option<ExternalAlbumArt> {
    if let Some(release_group_id) = image_ref
        .item_id
        .strip_prefix(EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX)
        .and_then(valid_mbid)
    {
        return Some(ExternalAlbumArt {
            artist: String::new(),
            album: String::new(),
            musicbrainz_release_id: None,
            musicbrainz_release_group_id: Some(release_group_id.to_string()),
        });
    }
    if let Some(release_id) = image_ref
        .item_id
        .strip_prefix(EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX)
        .and_then(valid_mbid)
    {
        return Some(ExternalAlbumArt {
            artist: String::new(),
            album: String::new(),
            musicbrainz_release_id: Some(release_id.to_string()),
            musicbrainz_release_group_id: None,
        });
    }
    let rest = image_ref
        .item_id
        .strip_prefix(EXTERNAL_ALBUM_IMAGE_PREFIX)?;
    let (artist, album) = rest.split_once(':')?;
    Some(ExternalAlbumArt {
        artist: percent_decode_component(artist)?,
        album: percent_decode_component(album)?,
        musicbrainz_release_id: None,
        musicbrainz_release_group_id: None,
    })
}

pub fn normalize_track_with_album_ref(
    track: &mut Track,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) {
    normalize_track_ref(track, album_image_ref, settings);
}

#[cfg(test)]
fn normalize_album(album: &mut Album, settings: &AppSettings) {
    normalize_image_ref(&mut album.image_ref, settings);
    if enabled(settings) && album.image_ref.is_none() {
        album.image_ref = external_album_identity_image_ref(album)
            .or_else(|| external_album_image_ref(&album.artist, &album.title));
    }
}

#[cfg(test)]
fn normalize_track(track: &mut Track, settings: &AppSettings) {
    normalize_track_ref(track, None, settings);
}

#[cfg(test)]
fn normalize_album_detail(album: &mut Album, tracks: &mut [Track], settings: &AppSettings) {
    normalize_album(album, settings);
    let album_image_ref = album.image_ref.as_ref();
    for track in tracks {
        normalize_track_ref(track, album_image_ref, settings);
    }
}

fn normalize_track_ref(
    track: &mut Track,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) {
    normalize_image_ref(&mut track.image_ref, settings);
    let weak_album_ref = has_untagged_jellyfin_album_ref(&track.image_ref, track.album_id.as_str())
        || replaceable_album_ref(&track.image_ref, album_image_ref);
    if (track.image_ref.is_none() || weak_album_ref)
        && let Some(image_ref) = album_image_ref
        && (!is_external_image_ref(image_ref) || cached_refs_enabled(settings))
    {
        track.image_ref = Some(image_ref.clone());
        return;
    }
    if enabled(settings) && weak_album_ref {
        track.image_ref = None;
    }
    if enabled(settings) && track.image_ref.is_none() {
        track.image_ref = external_album_image_ref(&track.artist, &track.album);
    }
}

pub fn normalize_artist(artist: &mut Artist, settings: &AppSettings) {
    if artist.image_ref.as_ref().is_some_and(stale_artist_ref) {
        artist.image_ref = None;
        return;
    }
    normalize_image_ref(&mut artist.image_ref, settings);
}

pub fn normalize_queue_entry_with_album_ref(
    entry: &mut QueueEntry,
    album_image_ref: Option<&ImageRef>,
    settings: &AppSettings,
) {
    normalize_image_ref(&mut entry.image_ref, settings);
    let weak_album_ref = entry.album_id.as_ref().is_some_and(|album_id| {
        has_untagged_jellyfin_album_ref(&entry.image_ref, album_id.as_str())
    }) || replaceable_album_ref(&entry.image_ref, album_image_ref);
    if (entry.image_ref.is_none() || weak_album_ref)
        && let Some(image_ref) = album_image_ref
        && (!is_external_image_ref(image_ref) || cached_refs_enabled(settings))
    {
        entry.image_ref = Some(image_ref.clone());
        return;
    }
    if enabled(settings) && weak_album_ref {
        entry.image_ref = None;
    }
    if enabled(settings) && entry.image_ref.is_none() {
        entry.image_ref = external_album_image_ref(&entry.artist, &entry.album);
    }
}

#[cfg(test)]
fn normalize_queue_entry(entry: &mut QueueEntry, settings: &AppSettings) {
    normalize_queue_entry_with_album_ref(entry, None, settings);
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
}

fn normalize_image_ref(image_ref: &mut Option<ImageRef>, settings: &AppSettings) {
    if image_ref
        .as_ref()
        .is_some_and(|image_ref| is_external_image_ref(image_ref) && !cached_refs_enabled(settings))
    {
        *image_ref = None;
    }
}

fn stale_artist_ref(image_ref: &ImageRef) -> bool {
    image_ref.item_id.starts_with("external:artist:")
}

fn has_untagged_jellyfin_album_ref(image_ref: &Option<ImageRef>, album_id: &str) -> bool {
    image_ref.as_ref().is_some_and(|image_ref| {
        image_ref.item_id == album_id
            && image_ref.item_id.starts_with("jellyfin:album:")
            && image_ref.tag.as_deref().is_none_or(str::is_empty)
    })
}

fn replaceable_album_ref(image_ref: &Option<ImageRef>, album_image_ref: Option<&ImageRef>) -> bool {
    let Some(image_ref) = image_ref else {
        return false;
    };
    album_image_ref.is_some_and(|album_image_ref| {
        image_ref != album_image_ref
            && (is_external_image_ref(image_ref) || image_ref.item_id == album_image_ref.item_id)
    })
}

pub fn external_album_image_ref(artist: &str, album: &str) -> Option<ImageRef> {
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

pub fn external_album_identity_image_ref(album: &Album) -> Option<ImageRef> {
    if let Some(group_id) = album
        .musicbrainz_release_group_id
        .as_deref()
        .and_then(valid_mbid)
    {
        return Some(musicbrainz_image_ref(
            EXTERNAL_MUSICBRAINZ_RELEASE_GROUP_PREFIX,
            group_id,
        ));
    }
    let release_id = album.musicbrainz_album_id.as_deref().and_then(valid_mbid)?;
    Some(musicbrainz_image_ref(
        EXTERNAL_MUSICBRAINZ_RELEASE_PREFIX,
        release_id,
    ))
}

fn musicbrainz_image_ref(prefix: &str, id: &str) -> ImageRef {
    let item_id = format!("{prefix}{id}");
    let tag = format!(
        "{EXTERNAL_ALBUM_IDENTITY_TAG_VERSION}-{:016x}",
        stable_album_hash(id, prefix)
    );
    ImageRef::new(item_id, Some(tag))
}

fn valid_mbid(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(value)
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
    use super::album_lookup::{
        cover_art_size_path, json_ids, lastfm_album_image_url, read_bounded,
    };
    use super::{
        album_art_from_image_ref, enabled, external_album_image_ref, is_expected_lookup_miss,
        is_external_image_ref, normalize_album, normalize_album_detail, normalize_artist,
        normalize_queue_entry, normalize_queue_entry_with_album_ref, normalize_track,
    };
    use domain::{
        Album, AlbumId, AppSettings, Artist, ArtistId, ImageRef, QueueEntry, QueueEntryId, Track,
        TrackId,
    };
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn metadata_require_mode() {
        let mut settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };

        assert!(enabled(&settings));

        settings.private_mode = true;
        assert!(!enabled(&settings));
    }

    #[test]
    fn metadata_trip_lookup() {
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
                musicbrainz_release_id: None,
                musicbrainz_release_group_id: None,
            })
        );
    }

    #[test]
    fn metadata_prefers_album_identity_ref() {
        let mut album = album_without_cover("Example Album", "Example Artist");
        album.musicbrainz_release_group_id =
            Some("441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string());
        normalize_album(
            &mut album,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        let image_ref = album.image_ref.expect("external image ref");
        assert!(image_ref.item_id.starts_with("external:mb-release-group:"));
        assert_eq!(
            album_art_from_image_ref(&image_ref),
            Some(super::ExternalAlbumArt {
                artist: String::new(),
                album: String::new(),
                musicbrainz_release_id: None,
                musicbrainz_release_group_id: Some(
                    "441f9fa7-4c22-4b0f-a363-ba6fa6b04ded".to_string()
                ),
            })
        );
    }

    #[test]
    fn metadata_strip_refs() {
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
    fn metadata_private_mode_keeps_refs() {
        let image_ref = ImageRef::new("external:album:Example%20Artist:Example%20Album", None);
        let mut track = track_without_cover("Example Track", "Example Artist", "Example Album");
        track.image_ref = Some(image_ref.clone());

        normalize_track(
            &mut track,
            &AppSettings {
                external_metadata_enabled: true,
                private_mode: true,
                ..AppSettings::default()
            },
        );

        assert_eq!(track.image_ref, Some(image_ref));
    }

    #[test]
    fn track_untagged_jellyfin() {
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
                musicbrainz_release_id: None,
                musicbrainz_release_group_id: None,
            })
        );
    }

    #[test]
    fn metadata_use_ref() {
        let mut album = album_without_cover("Example Album", "Example Artist");
        let album_image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));
        album.id = AlbumId::new("jellyfin:album:one");
        album.image_ref = Some(album_image_ref.clone());
        let mut missing = track_without_cover("Example Track", "Example Artist", "Example Album");
        missing.album_id = album.id.clone();
        let mut weak = track_without_cover("Example Track Two", "Example Artist", "Example Album");
        weak.album_id = album.id.clone();
        weak.image_ref = Some(ImageRef::new(album.id.as_str(), None));
        let dedicated_image_ref =
            ImageRef::new("jellyfin:track:three", Some("tag-three".to_string()));
        let mut dedicated =
            track_without_cover("Example Track Three", "Example Artist", "Example Album");
        dedicated.album_id = album.id.clone();
        dedicated.image_ref = Some(dedicated_image_ref.clone());
        let mut tracks = vec![missing, weak, dedicated];

        normalize_album_detail(&mut album, &mut tracks, &AppSettings::default());

        assert_eq!(tracks[0].image_ref, Some(album_image_ref.clone()));
        assert_eq!(tracks[1].image_ref, Some(album_image_ref));
        assert_eq!(tracks[2].image_ref, Some(dedicated_image_ref));
    }

    #[test]
    fn metadata_share_ref() {
        let mut album = album_without_cover("Example Album", "Example Artist");
        let mut tracks = vec![track_without_cover(
            "Example Track",
            "Different Performer",
            "Example Album",
        )];

        normalize_album_detail(
            &mut album,
            &mut tracks,
            &AppSettings {
                external_metadata_enabled: true,
                ..AppSettings::default()
            },
        );

        assert!(album.image_ref.as_ref().is_some_and(is_external_image_ref));
        assert_eq!(tracks[0].image_ref, album.image_ref);
    }

    #[test]
    fn metadata_track_kept() {
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
    fn queue_entry_untagged() {
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
                musicbrainz_release_id: None,
                musicbrainz_release_group_id: None,
            })
        );
    }

    #[test]
    fn queue_entry_uses_canonical_album_ref() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };
        let album_ref = external_album_image_ref("未来古代楽団", "忘れじの言の葉/エデンの揺り籃")
            .expect("album ref");
        let weak_ref = external_album_image_ref(
            "未来古代楽団, 安次嶺希和子",
            "忘れじの言の葉/エデンの揺り籃",
        )
        .expect("track artist ref");
        let mut first = queue_entry_without_cover(
            "忘れじの言の葉",
            "未来古代楽団, 安次嶺希和子",
            "忘れじの言の葉/エデンの揺り籃",
        );
        first.image_ref = Some(weak_ref);
        let mut next = queue_entry_without_cover(
            "エデンの揺り籃",
            "未来古代楽団",
            "忘れじの言の葉/エデンの揺り籃",
        );

        normalize_queue_entry_with_album_ref(&mut first, Some(&album_ref), &settings);
        normalize_queue_entry_with_album_ref(&mut next, Some(&album_ref), &settings);

        assert_eq!(first.image_ref, Some(album_ref.clone()));
        assert_eq!(next.image_ref, Some(album_ref));
    }

    #[test]
    fn queue_entry_preserves_direct_track_ref() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };
        let album_ref = ImageRef::new("jellyfin:album:one", Some("album-tag".to_string()));
        let track_ref = ImageRef::new("jellyfin:track:one", Some("track-tag".to_string()));
        let mut entry =
            queue_entry_without_cover("Example Track", "Example Artist", "Example Album");
        entry.image_ref = Some(track_ref.clone());

        normalize_queue_entry_with_album_ref(&mut entry, Some(&album_ref), &settings);

        assert_eq!(entry.image_ref, Some(track_ref));
    }

    #[test]
    fn metadata_create_refs() {
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
    fn metadata_stale_removed() {
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
    fn metadata_create_ref() {
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
    fn metadata_use_image() {
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
            lastfm_album_image_url(&value).expect("lastfm album image url"),
            Some("https://example.test/large.jpg".to_string())
        );
    }

    #[test]
    fn metadata_lastfm_miss() {
        let value = json!({
            "error": 6,
            "message": "Album not found"
        });

        assert_eq!(
            lastfm_album_image_url(&value).expect("lastfm album not found"),
            None
        );
    }

    #[test]
    fn metadata_hide_image() {
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
            lastfm_album_image_url(&value).expect("lastfm album placeholder image url"),
            Some("https://example.test/small.jpg".to_string())
        );
    }

    #[test]
    fn metadata_dedupe_id() {
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
    fn metadata_use_steps() {
        assert_eq!(cover_art_size_path(96), "front-250");
        assert_eq!(cover_art_size_path(250), "front-250");
        assert_eq!(cover_art_size_path(256), "front-500");
    }

    #[test]
    fn metadata_exclude_error() {
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

    #[test]
    fn metadata_read_limit() {
        let mut body = Cursor::new(vec![b'a'; 9]);

        let error = read_bounded(&mut body, 8, "metadata body").expect_err("oversized body");

        assert!(error.contains("metadata body exceeded"));
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
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
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
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
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
            local_path: None,
            source_format: None,
            origin: None,
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
            musicbrainz_artist_id: None,
            image_ref: None,
        }
    }
}
