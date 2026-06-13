use std::time::Duration;

use domain::normalize_release_types;
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;

use super::album_lookup::{
    EXTERNAL_METADATA_USER_AGENT, MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL,
    MUSICBRAINZ_RELEASE_SEARCH_URL, fetch_musicbrainz_json,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumReleaseMetadata {
    pub release_types: Vec<String>,
    pub is_compilation: Option<bool>,
}

pub fn fetch_album_release_metadata(
    release_group_id: Option<&str>,
    release_id: Option<&str>,
) -> Result<AlbumReleaseMetadata, String> {
    thread_local! {
        static RELEASE_TYPE_CLIENT: Result<Client, String> = build_release_type_client();
    }

    RELEASE_TYPE_CLIENT.with(|client| {
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

pub fn is_expected_release_type_lookup_miss(error: &str) -> bool {
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

fn build_release_type_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())
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
}
