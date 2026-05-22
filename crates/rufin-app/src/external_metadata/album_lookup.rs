use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;

use super::ExternalAlbumArt;

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LASTFM_PLACEHOLDER_IMAGE_IDS: [&str; 1] = ["2a96cbd8b46e442fc41c2b86b821562f"];
const MUSICBRAINZ_RELEASE_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release/";
const MUSICBRAINZ_RELEASE_GROUP_SEARCH_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_ARCHIVE_RELEASE_URL: &str = "https://coverartarchive.org/release";
const COVER_ART_ARCHIVE_RELEASE_GROUP_URL: &str = "https://coverartarchive.org/release-group";
const EXTERNAL_METADATA_USER_AGENT: &str = concat!(
    "Rufin/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/screwys/Rufin)"
);

pub fn fetch_album_cover(
    art: &ExternalAlbumArt,
    size: u32,
    lastfm_api_key: &str,
) -> Result<Vec<u8>, String> {
    thread_local! {
        static ALBUM_COVER_CLIENT: Result<Client, String> = build_album_cover_client();
    }

    ALBUM_COVER_CLIENT.with(|client| {
        let client = client.as_ref().map_err(Clone::clone)?;
        fetch_album_cover_with_client(client, art, size, lastfm_api_key)
    })
}

fn build_album_cover_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(EXTERNAL_METADATA_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())
}

fn fetch_album_cover_with_client(
    client: &Client,
    art: &ExternalAlbumArt,
    size: u32,
    lastfm_api_key: &str,
) -> Result<Vec<u8>, String> {
    let mut errors = Vec::new();
    if !lastfm_api_key.trim().is_empty() {
        match lastfm_album_cover_url(client, art, lastfm_api_key) {
            Ok(Some(url)) => match download_image(client, &url) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => errors.push(error),
            },
            Ok(None) => errors.push("Last.fm did not return album art".to_string()),
            Err(error) => errors.push(error),
        }
    }

    match cover_art_archive_release_group_urls(client, art, size) {
        Ok(urls) => {
            for url in urls {
                match download_image(client, &url) {
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => errors.push(error),
                }
            }
        }
        Err(error) => errors.push(error),
    }

    match cover_art_archive_release_urls(client, art, size) {
        Ok(urls) => {
            for url in urls {
                match download_image(client, &url) {
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

pub(super) fn lastfm_album_image_url(value: &Value) -> Result<Option<String>, String> {
    lastfm_image_url(value, "/album/image")
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
        .find(|url| !url.is_empty() && !is_lastfm_placeholder_image_url(url))
        .map(str::to_string))
}

fn is_lastfm_placeholder_image_url(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    LASTFM_PLACEHOLDER_IMAGE_IDS
        .iter()
        .any(|placeholder| url.contains(placeholder))
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

pub(super) fn json_ids(value: &Value, collection_pointer: &str) -> Vec<String> {
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

pub(super) fn cover_art_size_path(size: u32) -> &'static str {
    if size <= 250 {
        "front-250"
    } else {
        "front-500"
    }
}

fn musicbrainz_phrase(value: &str) -> String {
    value.replace('\\', " ").replace('"', "\\\"")
}
