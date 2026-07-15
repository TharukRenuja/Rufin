use std::io::Read;
use std::time::Duration;

use metadata::{search_album_release_group_ids, search_album_release_ids};
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::Value;
use sources::SourceError;
use tokio::runtime::Runtime;

use crate::selection::{Candidate, valid_mbid};
use crate::{ExternalPolicy, SourceImages};

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LASTFM_IMAGE_HOST: &str = "lastfm.freetls.fastly.net";
const LASTFM_PLACEHOLDER_IMAGE_ID: &str = "2a96cbd8b46e442fc41c2b86b821562f";
const RELEASE_URL: &str = "https://coverartarchive.org/release";
const RELEASE_GROUP_URL: &str = "https://coverartarchive.org/release-group";
const IMAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const JSON_MAX_BYTES: usize = 4 * 1024 * 1024;
const USER_AGENT: &str = concat!(
    "Rufin/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/screwys/Rufin)"
);

#[derive(Debug)]
pub(crate) enum FetchOutcome {
    Ready(Vec<u8>),
    Missing,
}

#[derive(Clone)]
pub(crate) struct FetchContext {
    client: Client,
}

impl FetchContext {
    pub(crate) fn new() -> Result<Self, String> {
        Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent(USER_AGENT)
            .build()
            .map(|client| Self { client })
            .map_err(|error| error.to_string())
    }

    pub(crate) fn fetch(
        &self,
        runtime: &Runtime,
        source: &SourceImages,
        candidate: &Candidate,
        size: u32,
        policy: &ExternalPolicy,
    ) -> Result<FetchOutcome, String> {
        match candidate {
            Candidate::Native(image_ref) => {
                let Some(provider) = source.provider.as_ref() else {
                    return Ok(FetchOutcome::Missing);
                };
                runtime
                    .block_on(provider.image_bytes(image_ref, size))
                    .map(|image| {
                        if image.bytes.is_empty() {
                            FetchOutcome::Missing
                        } else {
                            FetchOutcome::Ready(image.bytes)
                        }
                    })
                    .or_else(|error| match error {
                        SourceError::NotFound => Ok(FetchOutcome::Missing),
                        error => Err(error.to_string()),
                    })
            }
            Candidate::MusicBrainzReleaseGroup(id) => {
                if !policy.allow_musicbrainz {
                    return Ok(FetchOutcome::Missing);
                }
                self.download(&cover_art_url(RELEASE_GROUP_URL, id, size))
            }
            Candidate::MusicBrainzRelease(id) => {
                if !policy.allow_musicbrainz {
                    return Ok(FetchOutcome::Missing);
                }
                self.download(&cover_art_url(RELEASE_URL, id, size))
            }
            Candidate::AlbumText { artist, album } => {
                self.fetch_album_text(artist, album, size, policy)
            }
        }
    }

    pub(crate) fn public_url(
        &self,
        candidates: &crate::ArtworkBinding,
        size: u32,
        policy: &ExternalPolicy,
    ) -> Result<Option<String>, String> {
        if !policy.allow_network {
            return Ok(None);
        }
        let mut failures = Vec::new();
        let album_text = candidates.candidates().iter().filter_map(|candidate| {
            if let Candidate::AlbumText { artist, album } = candidate {
                Some((artist.as_str(), album.as_str()))
            } else {
                None
            }
        });
        if !policy.lastfm_api_key.trim().is_empty() {
            for (artist, album) in album_text.clone() {
                match self.lastfm_url(artist, album, &policy.lastfm_api_key) {
                    Ok(Some(url)) => return Ok(Some(url)),
                    Ok(None) => {}
                    Err(error) => failures.push(error),
                }
            }
        }
        if policy.allow_musicbrainz {
            for candidate in candidates.candidates() {
                if let Candidate::MusicBrainzRelease(id) = candidate
                    && valid_mbid(id)
                {
                    return Ok(Some(cover_art_url(RELEASE_URL, id, size)));
                }
            }
            for candidate in candidates.candidates() {
                if let Candidate::MusicBrainzReleaseGroup(id) = candidate
                    && valid_mbid(id)
                {
                    return Ok(Some(cover_art_url(RELEASE_GROUP_URL, id, size)));
                }
            }
            for (artist, album) in album_text {
                match self.musicbrainz_album_text_url(artist, album, size) {
                    Ok(Some(url)) => return Ok(Some(url)),
                    Ok(None) => {}
                    Err(error) => failures.push(error),
                }
            }
        }
        if failures.is_empty() {
            Ok(None)
        } else {
            Err(failures.join("; "))
        }
    }

    fn fetch_album_text(
        &self,
        artist: &str,
        album: &str,
        size: u32,
        policy: &ExternalPolicy,
    ) -> Result<FetchOutcome, String> {
        let mut failures = Vec::new();
        if !policy.lastfm_api_key.trim().is_empty() {
            match self.lastfm_url(artist, album, &policy.lastfm_api_key) {
                Ok(Some(url)) => match self.download(&url) {
                    Ok(FetchOutcome::Ready(bytes)) => return Ok(FetchOutcome::Ready(bytes)),
                    Ok(FetchOutcome::Missing) => {}
                    Err(error) => failures.push(error),
                },
                Ok(None) => {}
                Err(error) => failures.push(error),
            }
        }
        if !policy.allow_musicbrainz {
            return if failures.is_empty() {
                Ok(FetchOutcome::Missing)
            } else {
                Err(failures.join("; "))
            };
        }
        match search_album_release_group_ids(artist, album) {
            Ok(ids) => {
                if let Some(bytes) =
                    self.download_identities(RELEASE_GROUP_URL, ids, size, &mut failures)
                {
                    return Ok(FetchOutcome::Ready(bytes));
                }
            }
            Err(error) => failures.push(error),
        }
        match search_album_release_ids(artist, album) {
            Ok(ids) => {
                if let Some(bytes) = self.download_identities(RELEASE_URL, ids, size, &mut failures)
                {
                    return Ok(FetchOutcome::Ready(bytes));
                }
            }
            Err(error) => failures.push(error),
        }
        if failures.is_empty() {
            Ok(FetchOutcome::Missing)
        } else {
            Err(failures.join("; "))
        }
    }

    fn musicbrainz_album_text_url(
        &self,
        artist: &str,
        album: &str,
        size: u32,
    ) -> Result<Option<String>, String> {
        match search_album_release_ids(artist, album) {
            Ok(ids) => {
                if let Some(id) = ids.into_iter().find(|id| valid_mbid(id)) {
                    return Ok(Some(cover_art_url(RELEASE_URL, &id, size)));
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn download_identities(
        &self,
        root: &str,
        ids: Vec<String>,
        size: u32,
        failures: &mut Vec<String>,
    ) -> Option<Vec<u8>> {
        for id in ids.into_iter().filter(|id| valid_mbid(id)) {
            match self.download(&cover_art_url(root, &id, size)) {
                Ok(FetchOutcome::Ready(bytes)) => return Some(bytes),
                Ok(FetchOutcome::Missing) => {}
                Err(error) => failures.push(error),
            }
        }
        None
    }

    fn lastfm_url(
        &self,
        artist: &str,
        album: &str,
        api_key: &str,
    ) -> Result<Option<String>, String> {
        let url = Url::parse_with_params(
            LASTFM_API_URL,
            &[
                ("method", "album.getinfo"),
                ("api_key", api_key),
                ("artist", artist),
                ("album", album),
                ("format", "json"),
                ("autocorrect", "1"),
            ],
        )
        .map_err(|error| error.to_string())?;
        let value = self.fetch_json(url, "Last.fm album lookup")?;
        let images = value
            .pointer("/album/image")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for wanted in ["mega", "extralarge", "large", "medium"] {
            for image in &images {
                if image.get("size").and_then(Value::as_str) != Some(wanted) {
                    continue;
                }
                let Some(url) = image.get("#text").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(url) = public_lastfm_url(url) {
                    return Ok(Some(url));
                }
            }
        }
        Ok(None)
    }

    fn download(&self, url: &str) -> Result<FetchOutcome, String> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| error.to_string())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(FetchOutcome::Missing);
        }
        if !response.status().is_success() {
            return Err(format!(
                "artwork request failed with status {}",
                response.status()
            ));
        }
        let bytes = read_response_bounded(response, IMAGE_MAX_BYTES, "artwork image")?;
        if bytes.is_empty() {
            Ok(FetchOutcome::Missing)
        } else {
            Ok(FetchOutcome::Ready(bytes))
        }
    }

    fn fetch_json(&self, url: Url, context: &str) -> Result<Value, String> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "{context} failed with status {}",
                response.status()
            ));
        }
        let bytes = read_response_bounded(response, JSON_MAX_BYTES, context)?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }
}

fn cover_art_url(root: &str, id: &str, size: u32) -> String {
    let suffix = if size <= 250 {
        "front-250"
    } else {
        "front-500"
    };
    format!("{root}/{id}/{suffix}")
}

fn public_lastfm_url(raw: &str) -> Option<String> {
    if raw.contains(LASTFM_PLACEHOLDER_IMAGE_ID) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some(LASTFM_IMAGE_HOST)
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
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
            limit / 1024 / 1024
        ));
    }
    read_bounded(response, limit, context)
}

fn read_bounded(mut reader: impl Read, limit: usize, context: &str) -> Result<Vec<u8>, String> {
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
                limit / 1024 / 1024
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lastfm_urls_are_limited_to_the_public_image_host() {
        assert!(
            public_lastfm_url("https://lastfm.freetls.fastly.net/i/u/300x300/cover.jpg").is_some()
        );
        assert!(public_lastfm_url("https://example.com/cover.jpg").is_none());
        assert!(public_lastfm_url("http://lastfm.freetls.fastly.net/cover.jpg").is_none());
        assert!(
            public_lastfm_url(
                "https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png"
            )
            .is_none()
        );
    }

    #[test]
    fn public_album_url_uses_the_accepted_release_without_text_lookup() {
        let candidates = crate::ArtworkBinding::album_facts(
            "Artist",
            "Album",
            Some("11111111-1111-1111-1111-111111111111"),
            Some("22222222-2222-2222-2222-222222222222"),
        );
        let context = FetchContext::new().unwrap_or_else(|error| panic!("fetch context: {error}"));
        let url = context
            .public_url(&candidates, 250, &ExternalPolicy::new(false, true, ""))
            .unwrap_or_else(|error| panic!("public URL: {error}"));
        assert_eq!(
            url.as_deref(),
            Some(
                "https://coverartarchive.org/release/22222222-2222-2222-2222-222222222222/front-250"
            )
        );
    }
}
