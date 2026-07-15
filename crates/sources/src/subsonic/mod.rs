use crate::CredentialSourceConfig;
use crate::config::{decode_provider_payload, encode_provider_payload, require_payload_version};
use crate::{
    ArtistCollections, FavoriteMutator, FolderBrowser, GeneratedTrackProvider, GeneratedTrackSeed,
    GeneratedTracksRequest, ImageBytes, ImageProvider, LibraryFreshnessProbe, LibraryProbeResult,
    LyricsProvider, LyricsSearch, MusicFolderProvider, MusicSource, NativeLyricLine, NativeLyrics,
    NativeLyricsOrigin, PagedRequest, PlaybackReport, PlaybackReportKind, PlaybackReporter,
    PlayedFilter, PlaylistCreator, PlaylistDeleter, PlaylistEntryMover, PlaylistEntryRemover,
    PlaylistReader, PlaylistRenamer, PlaylistTrackAdder, RandomTrackProvider, RandomTrackRequest,
    SourceError, SourceIdentity, SourceObjectKeyProvider, SourceResult, StreamDescriptor,
    StreamRequest, StreamResolver,
};
use async_trait::async_trait;
use library::{
    Album, AlbumDetail, AlbumId, Artist, ArtistId, FavoriteItemId, Folder, FolderDetail, FolderId,
    Genre, GenreDetail, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection, HomeSectionKind, ImageRef,
    MusicFolder, MusicFolderId, PagedResponse, Playlist, PlaylistDetail, PlaylistEntry,
    PlaylistEntryKey, PlaylistId, PlaylistSnapshot, SearchResults, SourceEntityKind, SourceId,
    Track, TrackId, normalize_release_types,
};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde::de::{self, DeserializeOwned, Visitor};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::instrument;

mod client;
mod source_impl;

use client::*;
use source_impl::*;

#[cfg(test)]
mod tests;

const CLIENT_NAME: &str = "Rufin";
const API_VERSION: &str = "1.16.1";
const SALT_BYTES: usize = 12;
const SUBSONIC_PAGE_SIZE: usize = 500;
const SOURCE_CONFIG_VERSION: u32 = 1;

#[derive(Deserialize)]
struct SubsonicSourcePayload {
    version: u32,
    base_url: String,
    user_id: String,
    username: String,
    trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubsonicSourceConfig {
    pub credentials: CredentialSourceConfig,
}

impl SubsonicSourceConfig {
    pub fn from_stored(stored: &library::StoredSource) -> SourceResult<Self> {
        if !matches!(stored.kind.as_str(), "navidrome" | "subsonic") {
            return Err(SourceError::InvalidConfig(format!(
                "expected a Subsonic source, found {}",
                stored.kind
            )));
        }
        let payload: SubsonicSourcePayload = decode_provider_payload(stored)?;
        require_payload_version(payload.version, SOURCE_CONFIG_VERSION)?;
        Ok(Self {
            credentials: CredentialSourceConfig::from_stored_fields(
                stored,
                payload.base_url,
                payload.user_id,
                payload.username,
                payload.trust_invalid_cert,
            ),
        })
    }

    pub fn into_stored(self) -> library::StoredSource {
        let CredentialSourceConfig {
            source,
            user_id,
            username,
            trust_invalid_cert,
        } = self.credentials;
        encode_provider_payload(
            source.clone(),
            serde_json::json!({
                "version": SOURCE_CONFIG_VERSION,
                "base_url": source.base_url,
                "user_id": user_id,
                "username": username,
                "trust_invalid_cert": trust_invalid_cert,
            }),
        )
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubsonicFlavor {
    Navidrome,
    Subsonic,
}
impl SubsonicFlavor {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Navidrome => "Navidrome",
            Self::Subsonic => "OpenSubsonic",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubsonicLoginRequest {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
    pub flavor: SubsonicFlavor,
}
#[derive(Clone)]
pub struct SubsonicLoginSession {
    pub source: SourceIdentity,
    pub username: String,
    pub credential: String,
}
#[derive(Clone)]
pub struct SubsonicConfiguredSession {
    pub source: SourceIdentity,
    pub username: String,
    pub trust_invalid_cert: bool,
    pub credential: String,
}
#[derive(Clone, Debug)]
pub struct SubsonicSource {
    client: Client,
    base_url: Url,
    username: String,
    credential: Arc<SubsonicCredential>,
    identity: SourceIdentity,
    scan_probe: Arc<Mutex<ScanProbeState>>,
}
#[derive(Clone, Copy, Debug, Default)]
struct ScanProbeState {
    last_idle_count: Option<i64>,
    saw_busy: bool,
}
impl SubsonicSource {
    #[instrument(skip(request), fields(base_url = %request.base_url, username = %request.username, source_kind = request.flavor.source_id(), trust_invalid_cert = request.trust_invalid_cert))]
    pub async fn login(request: SubsonicLoginRequest) -> SourceResult<SubsonicLoginSession> {
        let base_url = normalize_base_url(&request.base_url)?;
        let client = build_client(request.trust_invalid_cert)?;
        let credential = SubsonicCredential::from_password(&request.password);
        let mut auth_url = endpoint(&base_url, "getUser")?;
        auth_url.query_pairs_mut().extend_pairs(
            credential.common_query(&request.username, &[("username", &request.username)]),
        );
        let response = subsonic_json::<AuthenticateBody>(client.get(auth_url)).await?;
        let body = response.body;

        let source_kind = request.flavor.source_id();
        let server_name = response
            .server_type
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| request.flavor.display_name().to_string());
        let source_hash = stable_source_id(source_kind, base_url.as_str(), &request.username);

        Ok(SubsonicLoginSession {
            source: SourceIdentity {
                id: SourceId::new(format!("{source_kind}:server:{source_hash}")),
                kind: source_kind.to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            username: body.user.username,
            credential: credential.serialize(),
        })
    }

    pub fn from_configured_session(session: SubsonicConfiguredSession) -> SourceResult<Self> {
        let base_url = normalize_base_url(&session.source.base_url)?;
        let client = build_client(session.trust_invalid_cert)?;
        let credential = SubsonicCredential::parse(&session.credential)?;
        Ok(Self {
            client,
            base_url,
            username: session.username,
            credential: Arc::new(credential),
            identity: session.source,
            scan_probe: Arc::new(Mutex::new(ScanProbeState::default())),
        })
    }

    fn source_id(&self) -> &str {
        self.identity.kind.as_str()
    }

    fn id(&self, kind: &str, raw_id: &str) -> String {
        format!("{}:{kind}:{raw_id}", self.source_id())
    }

    fn authenticated_url(&self, method: &str, extra: &[(&str, String)]) -> SourceResult<Url> {
        let mut url = endpoint(&self.base_url, method)?;
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs(self.credential.common_query(&self.username, &[]));
            for (key, value) in extra {
                query.append_pair(key, value);
            }
        }
        Ok(url)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        method: &str,
        extra: &[(&str, String)],
    ) -> SourceResult<T> {
        let url = self.authenticated_url(method, extra)?;
        subsonic_json(self.client.get(url))
            .await
            .map(|response: SubsonicApiResponse<T>| response.body)
    }

    async fn get_unit(&self, method: &str, extra: &[(&str, String)]) -> SourceResult<()> {
        let url = self.authenticated_url(method, extra)?;
        subsonic_json::<SubsonicEmpty>(self.client.get(url))
            .await
            .map(|_| ())
    }

    async fn get_all_artists(&self) -> SourceResult<Vec<Artist>> {
        let body: ArtistsBody = self.get_json("getArtists", &[]).await?;
        let mut artists = body
            .artists
            .index
            .into_iter()
            .flat_map(|index| index.artist)
            .map(|artist| artist_from_dto(self, artist))
            .collect::<Vec<_>>();
        artists.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(artists)
    }

    async fn songs_by_genre(&self, genre_name: &str) -> SourceResult<Vec<Track>> {
        let mut offset = 0;
        let mut tracks = Vec::new();
        loop {
            let body: SongsByGenreBody = self
                .get_json(
                    "getSongsByGenre",
                    &[
                        ("genre", genre_name.to_string()),
                        ("count", SUBSONIC_PAGE_SIZE.to_string()),
                        ("offset", offset.to_string()),
                    ],
                )
                .await?;
            let songs = body
                .songs_by_genre
                .map(|songs| songs.song)
                .unwrap_or_default();
            let count = songs.len();
            tracks.extend(songs.into_iter().map(|song| track_from_dto(self, song)));
            if count < SUBSONIC_PAGE_SIZE {
                return Ok(tracks);
            }
            offset += count;
        }
    }

    async fn similar_songs(&self, raw_id: &str, count: usize) -> SourceResult<Vec<Track>> {
        let body: SimilarSongsBody = self
            .get_json(
                "getSimilarSongs",
                &[
                    ("id", raw_id.to_string()),
                    ("count", count.clamp(1, 500).to_string()),
                ],
            )
            .await?;
        Ok(body
            .similar_songs
            .map(|songs| songs.song)
            .unwrap_or_default()
            .into_iter()
            .map(|song| track_from_dto(self, song))
            .collect())
    }

    async fn similar_songs2(&self, raw_id: &str, count: usize) -> SourceResult<Vec<Track>> {
        let body: SimilarSongs2Body = self
            .get_json(
                "getSimilarSongs2",
                &[
                    ("id", raw_id.to_string()),
                    ("count", count.clamp(1, 500).to_string()),
                ],
            )
            .await?;
        Ok(body
            .similar_songs
            .map(|songs| songs.song)
            .unwrap_or_default()
            .into_iter()
            .map(|song| track_from_dto(self, song))
            .collect())
    }

    async fn replace_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()> {
        let mut extra = vec![("playlistId", raw_item_id(playlist_id.as_str()).to_string())];
        extra.extend(
            track_ids
                .iter()
                .map(|track_id| ("songId", raw_item_id(track_id.as_str()).to_string())),
        );
        self.get_unit("createPlaylist", &extra).await
    }

    async fn playlist_track_ids(&self, playlist_id: &PlaylistId) -> SourceResult<Vec<TrackId>> {
        Ok(self
            .playlist_detail(playlist_id)
            .await?
            .entries
            .into_iter()
            .map(|entry| entry.track.id)
            .collect())
    }
}
