use crate::CredentialSourceConfig;
use crate::config::{decode_provider_payload, encode_provider_payload, require_payload_version};
use crate::{
    ArtistCollections, FavoriteMutator, FolderBrowser, GeneratedTrackProvider, GeneratedTrackSeed,
    GeneratedTrackStrategy, GeneratedTracksRequest, ImageBytes, ImageProvider, LibraryChange,
    LibraryChangeFeed, LibraryChangeResolution, LibraryChangeResolver, LibraryObjectObservation,
    LyricsProvider, LyricsSearch, MusicFolderProvider, MusicSource, NativeLyricLine, NativeLyrics,
    NativeLyricsOrigin, PageState, PagedRequest, PlaybackReport, PlaybackReportKind,
    PlaybackReporter, PlayedFilter, PlaylistCreator, PlaylistDeleter, PlaylistEntryMover,
    PlaylistEntryRemover, PlaylistReader, PlaylistRenamer, PlaylistTrackAdder, RandomTrackProvider,
    RandomTrackRequest, SourceError, SourceIdentity, SourceObjectChanges, SourceObjectKeyProvider,
    SourceResult, StreamDescriptor, StreamRequest, StreamResolver,
};
use async_trait::async_trait;
pub use discovery::{DiscoveredJellyfinServer, discover_jellyfin_servers};
use item::{
    ALBUM_FIELDS, FOLDER_FIELDS, ItemQueryResult, JellyfinItem, MIXED_ITEM_FIELDS, PLAYLIST_FIELDS,
    TRACK_FIELDS, album_from_item, artist_from_item, folder_from_item, genre_from_item,
    is_audio_item, parent_folder_id, playlist_from_item, track_from_item,
};
use library::{
    Album, AlbumDetail, AlbumId, Artist, FavoriteItemId, Folder, FolderDetail, FolderId, Genre,
    GenreDetail, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection, HomeSectionKind, ImageRef,
    MusicFolder, MusicFolderId, PagedResponse, Playlist, PlaylistDetail, PlaylistEntry,
    PlaylistEntryKey, PlaylistId, PlaylistSnapshot, SourceEntityKind, SourceId,
    SourceObjectMapping, Track, TrackId,
};
#[cfg(test)]
use library::{ArtistCredit, ArtistId};
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;
use tracing::instrument;

mod client;
mod discovery;
mod item;
mod library_changes;
mod source_impl;
mod websocket;

use client::*;
pub(crate) use client::{jellyfin_id, normalize_base_url, stable_hash};
use source_impl::*;

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
const COLLECTION_PAGE_SIZE: usize = 500;

pub const JELLYFIN_SOURCE_ID: &str = "jellyfin";
const SOURCE_CONFIG_VERSION: u32 = 1;

async fn collect_pages<T, F, Fut>(mut fetch: F) -> SourceResult<Vec<T>>
where
    F: FnMut(PagedRequest) -> Fut,
    Fut: std::future::Future<Output = SourceResult<PagedResponse<T>>>,
{
    let mut pages = PageState::default();
    let mut items = Vec::new();
    loop {
        let page = fetch(pages.request(COLLECTION_PAGE_SIZE)).await?;
        let finished = pages
            .add(page.items.len(), (page.total > 0).then_some(page.total))
            .ok_or_else(|| SourceError::Other("Jellyfin returned an unstable page".to_string()))?;
        items.extend(page.items);
        if finished {
            return Ok(items);
        }
    }
}

#[derive(Deserialize)]
struct JellyfinSourcePayload {
    version: u32,
    base_url: String,
    user_id: String,
    username: String,
    trust_invalid_cert: bool,
    use_jellyfin_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinSourceConfig {
    pub credentials: CredentialSourceConfig,
    pub use_instant_mix: bool,
}

impl JellyfinSourceConfig {
    pub fn from_stored(stored: &library::StoredSource) -> SourceResult<Self> {
        if stored.kind != JELLYFIN_SOURCE_ID {
            return Err(SourceError::InvalidConfig(format!(
                "expected {JELLYFIN_SOURCE_ID}, found {}",
                stored.kind
            )));
        }
        let payload: JellyfinSourcePayload = decode_provider_payload(stored)?;
        require_payload_version(payload.version, SOURCE_CONFIG_VERSION)?;
        Ok(Self {
            credentials: CredentialSourceConfig::from_stored_fields(
                stored,
                payload.base_url,
                payload.user_id,
                payload.username,
                payload.trust_invalid_cert,
            ),
            use_instant_mix: payload.use_jellyfin_instant_mix,
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
                "use_jellyfin_instant_mix": self.use_instant_mix,
            }),
        )
    }
}
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
#[derive(Clone)]
pub struct JellyfinLoginRequest {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
    pub device_id: String,
}
#[derive(Clone)]
pub struct JellyfinLoginSession {
    pub source: SourceIdentity,
    pub user_id: String,
    pub username: String,
    pub access_token: String,
    pub device_id: String,
}
#[derive(Clone)]
pub struct JellyfinConfiguredSession {
    pub source: SourceIdentity,
    pub user_id: String,
    pub trust_invalid_cert: bool,
    pub access_token: String,
    pub device_id: String,
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
    pub async fn login(request: JellyfinLoginRequest) -> SourceResult<JellyfinLoginSession> {
        let config = JellyfinClientConfig::new(
            &request.base_url,
            request.trust_invalid_cert,
            Some(request.device_id),
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

        Ok(JellyfinLoginSession {
            source: SourceIdentity {
                id: SourceId::new(format!("jellyfin:server:{source_id}")),
                kind: "jellyfin".to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            user_id: response.user.id,
            username: response.user.name,
            access_token: response.access_token,
            device_id: config.device_id,
        })
    }

    pub fn from_configured_session(session: JellyfinConfiguredSession) -> SourceResult<Self> {
        let config = JellyfinClientConfig::new(
            &session.source.base_url,
            session.trust_invalid_cert,
            Some(session.device_id),
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
        let fields = match include_types {
            "MusicAlbum" => ALBUM_FIELDS,
            "Audio" => TRACK_FIELDS,
            "Playlist" => PLAYLIST_FIELDS,
            _ => MIXED_ITEM_FIELDS,
        };
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", include_types)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("Fields", fields)
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
            .append_pair("Fields", TRACK_FIELDS);
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
            .append_pair("Fields", TRACK_FIELDS);
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
            .append_pair("Fields", TRACK_FIELDS)
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

    async fn server_lyrics(&self, track_id: &TrackId) -> SourceResult<Option<NativeLyrics>> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let local_url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/Lyrics"))?;
        match self.send_json::<LyricDto>(self.client.get(local_url)).await {
            Ok(dto) => Ok(Some(lyrics_from_dto(NativeLyricsOrigin::Server, dto))),
            Err(SourceError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn remote_lyrics(&self, track_id: &TrackId) -> SourceResult<Option<NativeLyrics>> {
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
            Ok(dto) => Ok(Some(lyrics_from_dto(NativeLyricsOrigin::Remote, dto))),
            Err(SourceError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
