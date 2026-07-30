use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::http::{client, fetch_json};

const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const MUSICBRAINZ_MIN_INTERVAL: Duration = Duration::from_millis(1100);

pub fn lookup_album_release(
    release_group_id: Option<&str>,
    release_id: Option<&str>,
) -> Result<Option<AlbumReleaseMetadata>, String> {
    match fetch_album_release_metadata(release_group_id, release_id) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if is_expected_release_type_lookup_miss(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn fetch_musicbrainz_json(client: &Client, url: Url, context: &str) -> Result<Value, String> {
    wait_for_musicbrainz_slot();
    fetch_json(client, url, context)
}

fn wait_for_musicbrainz_slot() {
    static NEXT_REQUEST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let lock = NEXT_REQUEST.get_or_init(|| Mutex::new(None));
    let Ok(mut next_request) = lock.lock() else {
        return;
    };
    let now = Instant::now();
    let slot = next_request.map_or(now, |next| next.max(now));
    *next_request = Some(slot + MUSICBRAINZ_MIN_INTERVAL);
    drop(next_request);
    let delay = slot.saturating_duration_since(now);
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumReleaseMetadata {
    pub release_types: Vec<String>,
    pub is_compilation: Option<bool>,
}

fn fetch_album_release_metadata(
    release_group_id: Option<&str>,
    release_id: Option<&str>,
) -> Result<AlbumReleaseMetadata, String> {
    let client = client()?;
    if let Some(release_group_id) = release_group_id.and_then(usable_mbid) {
        return fetch_release_group_metadata(client, release_group_id);
    }
    if let Some(release_id) = release_id.and_then(usable_mbid) {
        return fetch_release_metadata(client, release_id);
    }
    Err("album has no MusicBrainz release or release-group id".to_string())
}

fn is_expected_release_type_lookup_miss(error: &str) -> bool {
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

    error.contains("404 Not Found") || error.contains("did not return release group type")
}

pub(crate) fn search_album_release_group_ids(
    artist: &str,
    album: &str,
) -> Result<Vec<String>, String> {
    search_album_identity_ids(
        artist,
        album,
        "releasegroup",
        MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL,
        "/release-groups",
        "MusicBrainz release-group lookup",
    )
}

pub(crate) fn search_album_release_ids(artist: &str, album: &str) -> Result<Vec<String>, String> {
    search_album_identity_ids(
        artist,
        album,
        "release",
        MUSICBRAINZ_RELEASE_SEARCH_URL,
        "/releases",
        "MusicBrainz release lookup",
    )
}

fn search_album_identity_ids(
    artist: &str,
    album: &str,
    album_field: &str,
    endpoint: &str,
    collection_pointer: &str,
    context: &str,
) -> Result<Vec<String>, String> {
    let query = format!(
        "artist:\"{}\" AND {album_field}:\"{}\"",
        musicbrainz_phrase(artist),
        musicbrainz_phrase(album)
    );
    let url = Url::parse_with_params(
        endpoint,
        [("query", query.as_str()), ("fmt", "json"), ("limit", "5")],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_musicbrainz_json(client()?, url, context)?;
    Ok(json_ids(&value, collection_pointer))
}

fn musicbrainz_phrase(value: &str) -> String {
    value.replace('\\', " ").replace('"', "\\\"")
}

fn fetch_release_group_metadata(
    client: &Client,
    release_group_id: &str,
) -> Result<AlbumReleaseMetadata, String> {
    let url = Url::parse_with_params(
        &format!("{MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL}{release_group_id}"),
        [("fmt", "json")],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_musicbrainz_json(client, url, "MusicBrainz release-group lookup")?;
    release_metadata_from_group(&value)
        .ok_or_else(|| "MusicBrainz did not return release group type".to_string())
}

fn fetch_release_metadata(
    client: &Client,
    release_id: &str,
) -> Result<AlbumReleaseMetadata, String> {
    let url = Url::parse_with_params(
        &format!("{MUSICBRAINZ_RELEASE_SEARCH_URL}{release_id}"),
        [("fmt", "json"), ("inc", "release-groups")],
    )
    .map_err(|error| error.to_string())?;
    let value = fetch_musicbrainz_json(client, url, "MusicBrainz release lookup")?;
    let Some(group) = value.get("release-group") else {
        return Err("MusicBrainz did not return release group type".to_string());
    };
    release_metadata_from_group(group)
        .ok_or_else(|| "MusicBrainz did not return release group type".to_string())
}

fn release_metadata_from_group(group: &Value) -> Option<AlbumReleaseMetadata> {
    let mut raw_types = Vec::new();
    if let Some(primary_type) = group.get("primary-type").and_then(Value::as_str) {
        raw_types.push(primary_type.to_string());
    }
    if let Some(secondary_types) = group.get("secondary-types").and_then(Value::as_array) {
        raw_types.extend(
            secondary_types
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string),
        );
    }
    let release_types = normalize_release_types(raw_types);
    if release_types.is_empty() {
        return None;
    }
    let is_compilation = Some(release_types.iter().any(|kind| kind == "compilation"));
    Some(AlbumReleaseMetadata {
        release_types,
        is_compilation,
    })
}

fn normalize_release_types(types: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut values = Vec::new();
    for release_type in types {
        let value = release_type.as_ref().trim().to_ascii_lowercase();
        if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
            values.push(value);
        }
    }
    values
}

pub(crate) fn usable_mbid(value: &str) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_primary_and_secondary_release_group_types() {
        let value = json!({
            "primary-type": "Album",
            "secondary-types": ["Compilation", "Live"]
        });

        assert_eq!(
            release_metadata_from_group(&value),
            Some(AlbumReleaseMetadata {
                release_types: vec![
                    "album".to_string(),
                    "compilation".to_string(),
                    "live".to_string(),
                ],
                is_compilation: Some(true),
            })
        );
    }

    #[test]
    fn parses_single_without_compilation() {
        let value = json!({
            "primary-type": "Single",
            "secondary-types": []
        });

        assert_eq!(
            release_metadata_from_group(&value),
            Some(AlbumReleaseMetadata {
                release_types: vec!["single".to_string()],
                is_compilation: Some(false),
            })
        );
    }

    #[test]
    fn identity_results_ignore_empty_and_duplicate_ids() {
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
}
