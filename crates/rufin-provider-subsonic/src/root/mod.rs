use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url, header};
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, Folder, FolderId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT,
    HomeSection, HomeSectionKind, ImageRef, MusicFolder, MusicFolderId, Playlist, PlaylistId,
    ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, FavoriteItemId, FolderDetail, GenreDetail, ImageBytes, ImageKind, ImageMetadata,
    ImageRequest, LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest, PagedResponse,
    PlaybackReport, PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry,
    ProviderCapabilities, ProviderError, ProviderIdentity, ProviderResult, ProviderSession,
    RandomTrackRequest, SavedProviderSession, SearchResults, StreamDescriptor, StreamRequest,
};
use serde::Deserialize;
use serde::de::{self, DeserializeOwned, Visitor};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::instrument;

mod client;
mod provider_impl;

use client::*;
use provider_impl::*;

#[cfg(test)]
mod tests;

const CLIENT_NAME: &str = "Rufin";
const API_VERSION: &str = "1.16.1";
const SALT_BYTES: usize = 12;
const SUBSONIC_PAGE_SIZE: usize = 500;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubsonicFlavor {
    Navidrome,
    Subsonic,
}
impl SubsonicFlavor {
    pub fn from_provider_id(provider: &str) -> Option<Self> {
        match provider {
            "navidrome" => Some(Self::Navidrome),
            "subsonic" | "opensubsonic" => Some(Self::Subsonic),
            _ => None,
        }
    }

    pub fn provider_id(self) -> &'static str {
        match self {
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Navidrome => "Navidrome",
            Self::Subsonic => "Subsonic",
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
#[derive(Clone, Debug)]
pub struct SubsonicProvider {
    client: Client,
    base_url: Url,
    username: String,
    credential: Arc<SubsonicCredential>,
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
}
impl SubsonicProvider {
    #[instrument(skip(request), fields(base_url = %request.base_url, username = %request.username, provider = request.flavor.provider_id(), trust_invalid_cert = request.trust_invalid_cert))]
    pub async fn login(request: SubsonicLoginRequest) -> ProviderResult<ProviderSession> {
        let base_url = normalize_base_url(&request.base_url)?;
        let client = build_client(request.trust_invalid_cert)?;
        let credential = SubsonicCredential::from_password(&request.password);
        let mut auth_url = endpoint(&base_url, "getUser")?;
        auth_url.query_pairs_mut().extend_pairs(
            credential.common_query(&request.username, &[("username", &request.username)]),
        );
        let response = subsonic_json::<AuthenticateBody>(client.get(auth_url)).await?;
        let body = response.body;

        let provider_id = request.flavor.provider_id();
        let server_name = response
            .server_type
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| request.flavor.display_name().to_string());
        let server_id = stable_server_id(provider_id, base_url.as_str(), &request.username);

        Ok(ProviderSession {
            server: ServerIdentity {
                id: ServerId::new(format!("{provider_id}:server:{server_id}")),
                provider: provider_id.to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            user_id: body.user.username.clone(),
            username: body.user.username,
            access_token: credential.serialize(),
            device_id: None,
        })
    }

    pub fn from_saved_session(session: SavedProviderSession) -> ProviderResult<Self> {
        let base_url = normalize_base_url(&session.server.base_url)?;
        let client = build_client(session.trust_invalid_cert)?;
        let credential = SubsonicCredential::parse(&session.access_token)?;
        Ok(Self {
            client,
            base_url,
            username: session.username,
            credential: Arc::new(credential),
            identity: ProviderIdentity {
                server: session.server,
            },
            capabilities: subsonic_capabilities(),
        })
    }

    fn provider_id(&self) -> &str {
        self.identity.server.provider.as_str()
    }

    fn id(&self, kind: &str, raw_id: &str) -> String {
        format!("{}:{kind}:{raw_id}", self.provider_id())
    }

    fn authenticated_url(&self, method: &str, extra: &[(&str, String)]) -> ProviderResult<Url> {
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
    ) -> ProviderResult<T> {
        let url = self.authenticated_url(method, extra)?;
        subsonic_json(self.client.get(url))
            .await
            .map(|response: SubsonicApiResponse<T>| response.body)
    }

    async fn get_unit(&self, method: &str, extra: &[(&str, String)]) -> ProviderResult<()> {
        let url = self.authenticated_url(method, extra)?;
        subsonic_json::<SubsonicEmpty>(self.client.get(url))
            .await
            .map(|_| ())
    }

    async fn get_all_artists(&self) -> ProviderResult<Vec<Artist>> {
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

    async fn songs_by_genre(&self, genre_name: &str) -> ProviderResult<Vec<Track>> {
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

    async fn replace_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> ProviderResult<()> {
        let mut extra = vec![("playlistId", raw_item_id(playlist_id.as_str()).to_string())];
        extra.extend(
            track_ids
                .iter()
                .map(|track_id| ("songId", raw_item_id(track_id.as_str()).to_string())),
        );
        self.get_unit("createPlaylist", &extra).await
    }

    async fn playlist_track_ids(&self, playlist_id: &PlaylistId) -> ProviderResult<Vec<TrackId>> {
        Ok(self
            .playlist_detail(playlist_id)
            .await?
            .entries
            .into_iter()
            .map(|entry| entry.track.id)
            .collect())
    }
}
