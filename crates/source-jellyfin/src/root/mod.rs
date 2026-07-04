pub use crate::discovery::{DiscoveredJellyfinServer, discover_jellyfin_servers};
use crate::item::{
    ITEM_FIELDS, ItemQueryResult, JellyfinItem, album_from_item, artist_from_item,
    folder_from_item, genre_from_item, is_audio_item, parent_folder_id, playlist_from_item,
    track_from_item,
};
use async_trait::async_trait;
use domain::{
    Album, AlbumId, Artist, Folder, FolderId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection,
    HomeSectionKind, MusicFolder, MusicFolderId, Playlist, PlaylistId, SourceId, Track, TrackId,
};
#[cfg(test)]
use domain::{ArtistCredit, ArtistId, ImageRef};
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use source::{
    AlbumDetail, FavoriteItemId, FolderDetail, GeneratedTrackSeed, GeneratedTrackStrategy,
    GeneratedTracksRequest, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest,
    LoginRequest, LyricLine, Lyrics, LyricsSource, MusicSource, PagedRequest, PagedResponse,
    PlaybackReport, PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry,
    RandomTrackRequest, SavedSourceSession, SearchResults, SourceError, SourceIdentity,
    SourceResult, SourceSession, StreamDescriptor, StreamRequest,
};
use std::sync::Arc;
use tracing::instrument;

mod client;
mod source_impl;
mod websocket;

use client::*;
pub(crate) use client::{jellyfin_id, normalize_base_url, stable_hash};
use source_impl::*;
pub use websocket::JellyfinLibraryChange;

#[cfg(test)]
mod library_api_tests;
#[cfg(test)]
mod lyrics_playback_tests;
#[cfg(test)]
use lyrics_playback_tests::provider;

const CLIENT_NAME: &str = "Rufin";
const DEVICE_NAME: &str = "Rufin";
const DEFAULT_DEVICE_ID: &str = "rufin-native";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinClientConfig {
    pub base_url: String,
    pub trust_invalid_cert: bool,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
    pub client_version: String,
}
impl JellyfinClientConfig {
    pub fn new(
        base_url: impl Into<String>,
        trust_invalid_cert: bool,
        device_id: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            trust_invalid_cert,
            device_id: device_id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_DEVICE_ID.to_string()),
            device_name: DEVICE_NAME.to_string(),
            client_name: CLIENT_NAME.to_string(),
            client_version: CLIENT_VERSION.to_string(),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JellyfinLyricsSearch {
    ServerOnly,
    ServerThenRemote,
    RemoteThenServer,
}
#[derive(Clone, Debug)]
pub struct JellyfinSource {
    client: Client,
    base_url: Url,
    user_id: String,
    access_token: Arc<str>,
    device_id: Arc<str>,
    identity: SourceIdentity,
}
impl JellyfinSource {
    #[instrument(skip(request), fields(base_url = %request.base_url, username = %request.username, trust_invalid_cert = request.trust_invalid_cert))]
    pub async fn login(request: LoginRequest) -> SourceResult<SourceSession> {
        let config = JellyfinClientConfig::new(
            &request.base_url,
            request.trust_invalid_cert,
            request.device_id,
        );
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;

        let body = AuthenticateByNameRequest {
            username: request.username.clone(),
            password: request.password,
        };
        let auth_url = endpoint(&base_url, "Users/AuthenticateByName")?;
        let response = send_json::<AuthenticationResult>(
            client
                .post(auth_url)
                .header(header::AUTHORIZATION, auth_header(&config, None))
                .json(&body),
        )
        .await?;

        let server_name = public_server_name(&client, &base_url, &config)
            .await
            .unwrap_or_else(|| "Jellyfin".to_string());
        let source_id = response
            .source_id
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| stable_source_id(base_url.as_str()));

        Ok(SourceSession {
            source: SourceIdentity {
                id: SourceId::new(format!("jellyfin:server:{source_id}")),
                kind: "jellyfin".to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            user_id: response.user.id,
            username: response.user.name,
            access_token: response.access_token,
            device_id: Some(config.device_id),
        })
    }

    pub fn from_saved_session(session: SavedSourceSession) -> SourceResult<Self> {
        let config = JellyfinClientConfig::new(
            &session.source.base_url,
            session.trust_invalid_cert,
            session.device_id,
        );
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;
        Ok(Self {
            client,
            base_url,
            user_id: session.user_id,
            access_token: Arc::from(session.access_token),
            device_id: Arc::from(config.device_id),
            identity: session.source,
        })
    }

    pub fn stream_descriptor_from_saved_session(
        session: &SavedSourceSession,
        request: &StreamRequest,
    ) -> SourceResult<StreamDescriptor> {
        let config = JellyfinClientConfig::new(
            &session.source.base_url,
            session.trust_invalid_cert,
            session.device_id.clone(),
        );
        let base_url = normalize_base_url(&config.base_url)?;
        stream_descriptor(
            &base_url,
            &session.user_id,
            &config.device_id,
            &session.access_token,
            request,
        )
    }

    pub fn image_url(
        &self,
        item_id: &str,
        kind: ImageKind,
        tag: Option<&str>,
    ) -> SourceResult<String> {
        let mut url = endpoint(
            &self.base_url,
            &format!(
                "Items/{}/Images/{}",
                raw_item_id(item_id),
                image_kind_path(kind)
            ),
        )?;
        if let Some(tag) = tag.filter(|tag| !tag.is_empty()) {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        Ok(url.to_string())
    }

    pub async fn tracks_by_raw_item_ids(&self, raw_ids: &[String]) -> SourceResult<Vec<Track>> {
        let mut tracks = Vec::new();
        for chunk in raw_ids.chunks(100).filter(|chunk| !chunk.is_empty()) {
            let mut url = endpoint(&self.base_url, "Items")?;
            url.query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("Recursive", "true")
                .append_pair("IncludeItemTypes", "Audio")
                .append_pair("Ids", &chunk.join(","))
                .append_pair("Fields", ITEM_FIELDS)
                .append_pair("EnableTotalRecordCount", "false");
            tracks.extend(
                self.get_json::<ItemQueryResult>(url)
                    .await?
                    .items
                    .into_iter()
                    .filter(is_audio_item)
                    .map(track_from_item),
            );
        }
        Ok(tracks)
    }

    pub async fn recently_added_tracks(&self, limit: usize) -> SourceResult<Vec<Track>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let response = self
            .item_page_sorted(
                "Audio",
                PagedRequest::new(0, limit.clamp(1, 500)),
                "DateCreated,SortName",
                "Descending",
            )
            .await?;
        Ok(response.items.into_iter().map(track_from_item).collect())
    }

    pub async fn lyrics_with_search(
        &self,
        track_id: &TrackId,
        search: JellyfinLyricsSearch,
    ) -> SourceResult<Option<Lyrics>> {
        match search {
            JellyfinLyricsSearch::ServerOnly => self.server_lyrics(track_id).await,
            JellyfinLyricsSearch::ServerThenRemote => {
                if let Some(lyrics) = self.server_lyrics(track_id).await? {
                    return Ok(Some(lyrics));
                }
                self.remote_lyrics(track_id).await
            }
            JellyfinLyricsSearch::RemoteThenServer => {
                if let Some(lyrics) = self.remote_lyrics(track_id).await? {
                    return Ok(Some(lyrics));
                }
                self.server_lyrics(track_id).await
            }
        }
    }

    async fn item_page(
        &self,
        include_types: &str,
        request: PagedRequest,
    ) -> SourceResult<PagedResponse<JellyfinItem>> {
        self.item_page_sorted(include_types, request, "SortName", "Ascending")
            .await
    }

    async fn item_page_sorted(
        &self,
        include_types: &str,
        request: PagedRequest,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", include_types)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("Fields", ITEM_FIELDS)
            .append_pair("SortBy", sort_by)
            .append_pair("SortOrder", sort_order);

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn home_album_section(
        &self,
        kind: HomeSectionKind,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<HomeSection> {
        let page = self
            .item_page_sorted(
                "MusicAlbum",
                PagedRequest::new(0, HOME_SECTION_ITEM_LIMIT),
                sort_by,
                sort_order,
            )
            .await?;
        Ok(HomeSection {
            kind,
            albums: page.items.into_iter().map(album_from_item).collect(),
            tracks: Vec::new(),
        })
    }

    async fn home_track_section(
        &self,
        kind: HomeSectionKind,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<HomeSection> {
        let page = self
            .item_page_sorted(
                "Audio",
                PagedRequest::new(0, HOME_SECTION_ITEM_LIMIT),
                sort_by,
                sort_order,
            )
            .await?;
        Ok(HomeSection {
            kind,
            albums: Vec::new(),
            tracks: page.items.into_iter().map(track_from_item).collect(),
        })
    }

    async fn people_page(
        &self,
        path: &str,
        request: PagedRequest,
    ) -> SourceResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, path)?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags,ProviderIds",
            );

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn similar_tracks(&self, track_id: &TrackId, limit: usize) -> SourceResult<Vec<Track>> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let mut url = endpoint(&self.base_url, &format!("Items/{raw_track_id}/Similar"))?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Limit", &limit.clamp(1, 500).to_string())
            .append_pair("Fields", ITEM_FIELDS);
        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(response
            .items
            .into_iter()
            .filter(is_audio_item)
            .map(track_from_item)
            .collect())
    }

    async fn instant_mix_tracks(
        &self,
        seed: &GeneratedTrackSeed,
        limit: usize,
    ) -> SourceResult<Vec<Track>> {
        let mut url = self.instant_mix_url(seed)?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Limit", &limit.clamp(1, 500).to_string())
            .append_pair("Fields", ITEM_FIELDS);
        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(response
            .items
            .into_iter()
            .filter(is_audio_item)
            .map(track_from_item)
            .collect())
    }

    fn instant_mix_url(&self, seed: &GeneratedTrackSeed) -> SourceResult<Url> {
        match seed {
            GeneratedTrackSeed::Track(track_id) => endpoint(
                &self.base_url,
                &format!("Songs/{}/InstantMix", raw_item_id(track_id.as_str())),
            ),
            GeneratedTrackSeed::Album(album_id) => endpoint(
                &self.base_url,
                &format!("Albums/{}/InstantMix", raw_item_id(album_id.as_str())),
            ),
            GeneratedTrackSeed::Artist(artist_id) => endpoint(
                &self.base_url,
                &format!("Artists/{}/InstantMix", raw_item_id(artist_id.as_str())),
            ),
            GeneratedTrackSeed::Playlist(playlist_id) => endpoint(
                &self.base_url,
                &format!("Playlists/{}/InstantMix", raw_item_id(playlist_id.as_str())),
            ),
            GeneratedTrackSeed::Genre { id: Some(id), .. } => {
                let mut url = endpoint(&self.base_url, "MusicGenres/InstantMix")?;
                url.query_pairs_mut()
                    .append_pair("Id", raw_item_id(id.as_str()));
                Ok(url)
            }
            GeneratedTrackSeed::Genre { id: None, name } => {
                let mut url = endpoint(&self.base_url, "MusicGenres")?;
                url.path_segments_mut()
                    .map_err(|_| SourceError::Other("invalid Jellyfin base URL".to_string()))?
                    .push(name)
                    .push("InstantMix");
                Ok(url)
            }
        }
    }

    async fn music_genre_page(
        &self,
        request: PagedRequest,
    ) -> SourceResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, "MusicGenres")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("IncludeItemTypes", "Audio,MusicAlbum")
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags",
            )
            .append_pair("SortBy", "SortName");

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn folder_children(
        &self,
        raw_parent_id: &str,
    ) -> SourceResult<(Vec<Folder>, Vec<Track>)> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("ParentId", raw_parent_id)
            .append_pair("Recursive", "false")
            .append_pair("Fields", ITEM_FIELDS)
            .append_pair("SortBy", "SortName")
            .append_pair("SortOrder", "Ascending");
        let response = self.get_json::<ItemQueryResult>(url).await?;
        let mut folders = Vec::new();
        let mut tracks = Vec::new();
        for item in response.items {
            if is_audio_item(&item) {
                tracks.push(track_from_item(item));
            } else {
                folders.push(folder_from_item(item));
            }
        }
        folders.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok((folders, tracks))
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> SourceResult<T> {
        let config = JellyfinClientConfig::new(
            self.identity.base_url.clone(),
            false,
            Some(self.device_id.to_string()),
        );
        send_json(self.client.get(url).header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> SourceResult<T> {
        let config = JellyfinClientConfig::new(
            self.identity.base_url.clone(),
            false,
            Some(self.device_id.to_string()),
        );
        send_json(request.header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn send_unit(&self, request: reqwest::RequestBuilder) -> SourceResult<()> {
        let config = JellyfinClientConfig::new(
            self.identity.base_url.clone(),
            false,
            Some(self.device_id.to_string()),
        );
        send_unit(request.header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn server_lyrics(&self, track_id: &TrackId) -> SourceResult<Option<Lyrics>> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let local_url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/Lyrics"))?;
        match self.send_json::<LyricDto>(self.client.get(local_url)).await {
            Ok(dto) => Ok(Some(lyrics_from_dto(
                track_id.clone(),
                LyricsSource::Server,
                dto,
            ))),
            Err(SourceError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn remote_lyrics(&self, track_id: &TrackId) -> SourceResult<Option<Lyrics>> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let remote_url = endpoint(
            &self.base_url,
            &format!("Audio/{raw_track_id}/RemoteSearch/Lyrics"),
        )?;
        let results = match self
            .send_json::<Vec<RemoteLyricInfoDto>>(self.client.get(remote_url))
            .await
        {
            Ok(results) => results,
            Err(SourceError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(first) = results.into_iter().find(|result| !result.id.is_empty()) else {
            return Ok(None);
        };
        let lyric_url = endpoint(&self.base_url, &format!("Providers/Lyrics/{}", first.id))?;
        match self.send_json::<LyricDto>(self.client.get(lyric_url)).await {
            Ok(dto) => Ok(Some(lyrics_from_dto(
                track_id.clone(),
                LyricsSource::Remote,
                dto,
            ))),
            Err(SourceError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
