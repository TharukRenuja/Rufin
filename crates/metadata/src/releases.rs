use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use library::{AlbumId, AlbumIdentityCandidate, normalize_release_types};
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;

const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const EXTERNAL_METADATA_USER_AGENT: &str = concat!(
    "Rufin/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/screwys/Rufin)"
);
const EXTERNAL_METADATA_JSON_MAX_BYTES: usize = 4 * 1024 * 1024;
const MUSICBRAINZ_MIN_INTERVAL: Duration = Duration::from_millis(1100);
pub const ALBUM_IDENTITY_LOOKUP_LIMIT: usize = 500;

thread_local! {
    static MUSICBRAINZ_CLIENT: Result<Client, String> = build_musicbrainz_client();
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumIdentityChange {
    Updated(AlbumReleaseMetadata),
    Missing(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlbumIdentityEnrichment {
    pub updated: Vec<AlbumId>,
    pub misses: usize,
    pub errors: usize,
}

pub fn enrich_album_identities(
    candidates: &[AlbumIdentityCandidate],
    mut is_current: impl FnMut() -> bool,
    mut apply: impl FnMut(&AlbumIdentityCandidate, AlbumIdentityChange) -> Result<(), String>,
) -> Result<AlbumIdentityEnrichment, String> {
    let mut summary = AlbumIdentityEnrichment::default();
    for candidate in candidates {
        if !is_current() {
            break;
        }
        let lookup = fetch_album_release_metadata(
            candidate.musicbrainz_release_group_id.as_deref(),
            candidate.musicbrainz_album_id.as_deref(),
        );
        if !is_current() {
            break;
        }
        match lookup {
            Ok(metadata) => {
                apply(candidate, AlbumIdentityChange::Updated(metadata))?;
                summary.updated.push(candidate.album_id.clone());
            }
            Err(error) if is_expected_release_type_lookup_miss(&error) => {
                apply(candidate, AlbumIdentityChange::Missing(error))?;
                summary.misses += 1;
            }
            Err(error) => {
                summary.errors += 1;
                tracing::warn!(
                    %error,
                    album_id = %candidate.album_id,
                    "failed to look up album identity"
                );
            }
        }
    }
    Ok(summary)
}

fn fetch_musicbrainz_json(client: &Client, url: Url, context: &str) -> Result<Value, String> {
    wait_for_musicbrainz_slot();
    fetch_json(client, url, context)
}

fn fetch_json(client: &Client, url: Url, context: &str) -> Result<Value, String> {
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "{context} failed with status {}",
            response.status()
        ));
    }
    let bytes = read_response_bounded(response, EXTERNAL_METADATA_JSON_MAX_BYTES, context)?;
    serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())
}

fn wait_for_musicbrainz_slot() {
    static LAST_REQUEST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let lock = LAST_REQUEST.get_or_init(|| Mutex::new(None));
    let Ok(mut last_request) = lock.lock() else {
        return;
    };
    if let Some(last_request) = *last_request {
        let elapsed = last_request.elapsed();
        if elapsed < MUSICBRAINZ_MIN_INTERVAL {
            std::thread::sleep(MUSICBRAINZ_MIN_INTERVAL - elapsed);
        }
    }
    *last_request = Some(Instant::now());
}

fn read_response_bounded(
    response: reqwest::blocking::Response,
    limit: usize,
    context: &str,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!(
            "{context} exceeded {} MiB limit",
            bytes_to_mib(limit)
        ));
    }
    read_bounded(response, limit, context)
}

fn read_bounded<R: Read>(mut reader: R, limit: usize, context: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > limit)
        {
            return Err(format!(
                "{context} exceeded {} MiB limit",
                bytes_to_mib(limit)
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
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

const fn bytes_to_mib(bytes: usize) -> usize {
    bytes / 1024 / 1024
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
    MUSICBRAINZ_CLIENT.with(|client| {
        let client = client.as_ref().map_err(Clone::clone)?;
        if let Some(release_group_id) = usable_mbid(release_group_id) {
            return fetch_release_group_metadata(client, release_group_id);
        }
        if let Some(release_id) = usable_mbid(release_id) {
            return fetch_release_metadata(client, release_id);
        }
        Err("album has no MusicBrainz release or release-group id".to_string())
    })
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

fn build_musicbrainz_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())
}

pub fn search_album_release_group_ids(artist: &str, album: &str) -> Result<Vec<String>, String> {
    search_album_identity_ids(
        artist,
        album,
        "releasegroup",
        MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL,
        "/release-groups",
        "MusicBrainz release-group lookup",
    )
}

pub fn search_album_release_ids(artist: &str, album: &str) -> Result<Vec<String>, String> {
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
    MUSICBRAINZ_CLIENT.with(|client| {
        let client = client.as_ref().map_err(Clone::clone)?;
        let value = fetch_musicbrainz_json(client, url, context)?;
        Ok(json_ids(&value, collection_pointer))
    })
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

fn usable_mbid(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
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
