use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url, header};
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection,
    HomeSectionKind, ImageRef, Playlist, PlaylistId, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, FavoriteItemId, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest,
    LoginRequest, LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest, PagedResponse,
    PlaybackReport, PlaybackReportKind, PlaylistDetail, PlaylistEntry, ProviderCapabilities,
    ProviderError, ProviderIdentity, ProviderResult, ProviderSession, SavedProviderSession,
    SearchResults, StreamDescriptor,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::instrument;

const CLIENT_NAME: &str = "Rufin";
const DEVICE_NAME: &str = "Linux Desktop";
const DEVICE_ID: &str = "rufin-native";
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
    pub fn new(base_url: impl Into<String>, trust_invalid_cert: bool) -> Self {
        Self {
            base_url: base_url.into(),
            trust_invalid_cert,
            device_id: DEVICE_ID.to_string(),
            device_name: DEVICE_NAME.to_string(),
            client_name: CLIENT_NAME.to_string(),
            client_version: CLIENT_VERSION.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JellyfinProvider {
    client: Client,
    base_url: Url,
    user_id: String,
    access_token: Arc<str>,
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
}

impl JellyfinProvider {
    #[instrument(skip(request), fields(base_url = %request.base_url, username = %request.username, trust_invalid_cert = request.trust_invalid_cert))]
    pub async fn login(request: LoginRequest) -> ProviderResult<ProviderSession> {
        let config = JellyfinClientConfig::new(&request.base_url, request.trust_invalid_cert);
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
        let server_id = response
            .server_id
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| stable_server_id(base_url.as_str()));

        Ok(ProviderSession {
            server: ServerIdentity {
                id: ServerId::new(format!("jellyfin:server:{server_id}")),
                provider: "jellyfin".to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            user_id: response.user.id,
            username: response.user.name,
            access_token: response.access_token,
        })
    }

    pub fn from_saved_session(session: SavedProviderSession) -> ProviderResult<Self> {
        let config =
            JellyfinClientConfig::new(&session.server.base_url, session.trust_invalid_cert);
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;
        Ok(Self {
            client,
            base_url,
            user_id: session.user_id,
            access_token: Arc::from(session.access_token),
            identity: ProviderIdentity {
                server: session.server,
            },
            capabilities: jellyfin_capabilities(),
        })
    }

    pub fn image_url(
        &self,
        item_id: &str,
        kind: ImageKind,
        tag: Option<&str>,
    ) -> ProviderResult<String> {
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

    async fn item_page(
        &self,
        include_types: &str,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
        self.item_page_sorted(include_types, request, "SortName", "Ascending")
            .await
    }

    async fn item_page_sorted(
        &self,
        include_types: &str,
        request: PagedRequest,
        sort_by: &str,
        sort_order: &str,
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", include_types)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("Fields", "Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,ParentId,AlbumId,ArtistItems,UserData,ImageTags,ChildCount,AlbumCount,SongCount")
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
    ) -> ProviderResult<HomeSection> {
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
    ) -> ProviderResult<HomeSection> {
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
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, path)?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags",
            );

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn music_genre_page(
        &self,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
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

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> ProviderResult<T> {
        let config = JellyfinClientConfig::new(self.identity.server.base_url.clone(), false);
        send_json(self.client.get(url).header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> ProviderResult<T> {
        let config = JellyfinClientConfig::new(self.identity.server.base_url.clone(), false);
        send_json(request.header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn send_unit(&self, request: reqwest::RequestBuilder) -> ProviderResult<()> {
        let config = JellyfinClientConfig::new(self.identity.server.base_url.clone(), false);
        send_unit(request.header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }
}

#[async_trait(?Send)]
impl MusicProvider for JellyfinProvider {
    fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn home_sections(&self) -> ProviderResult<Vec<HomeSection>> {
        let sections = [
            self.home_section(HomeSectionKind::Explore).await?,
            self.home_section(HomeSectionKind::MostPlayed).await?,
            self.home_section(HomeSectionKind::NewlyAdded).await?,
            self.home_section(HomeSectionKind::RecentlyPlayed).await?,
            self.home_section(HomeSectionKind::RecentlyReleased).await?,
        ]
        .into_iter()
        .filter(|section| !section.albums.is_empty() || !section.tracks.is_empty())
        .collect();
        Ok(sections)
    }

    async fn home_section(&self, kind: HomeSectionKind) -> ProviderResult<HomeSection> {
        match kind {
            HomeSectionKind::Explore => {
                self.home_album_section(kind, "Random,SortName", "Ascending")
                    .await
            }
            HomeSectionKind::MostPlayed => {
                self.home_track_section(kind, "PlayCount,SortName", "Descending")
                    .await
            }
            HomeSectionKind::NewlyAdded => {
                self.home_album_section(kind, "DateCreated,SortName", "Descending")
                    .await
            }
            HomeSectionKind::RecentlyPlayed => {
                self.home_track_section(kind, "DatePlayed,SortName", "Descending")
                    .await
            }
            HomeSectionKind::RecentlyReleased => {
                self.home_album_section(kind, "ProductionYear,PremiereDate,SortName", "Descending")
                    .await
            }
        }
    }

    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>> {
        let response = self.item_page("MusicAlbum", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(album_from_item).collect(),
            response.total,
        ))
    }

    async fn album_detail(&self, album_id: &AlbumId) -> ProviderResult<AlbumDetail> {
        let raw_album_id = raw_item_id(album_id.as_str());
        let mut album_url = endpoint(&self.base_url, &format!("Items/{raw_album_id}"))?;
        album_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id);
        let album = album_from_item(self.get_json::<JellyfinItem>(album_url).await?);

        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("ParentId", raw_album_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("SortBy", "ParentIndexNumber,IndexNumber,SortName")
            .append_pair(
                "Fields",
                "Genres,ProductionYear,RunTimeTicks,ParentId,UserData,ImageTags",
            )
            .append_pair("StartIndex", "0")
            .append_pair("Limit", "500");
        let response = self.get_json::<ItemQueryResult>(url).await?;
        let tracks = response.items.into_iter().map(track_from_item).collect();

        Ok(AlbumDetail { album, tracks })
    }

    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>> {
        let response = self.item_page("Audio", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(track_from_item).collect(),
            response.total,
        ))
    }

    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        let response = self.people_page("Artists", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(artist_from_item).collect(),
            response.total,
        ))
    }

    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        let response = self.people_page("Artists/AlbumArtists", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(artist_from_item).collect(),
            response.total,
        ))
    }

    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>> {
        let response = self.music_genre_page(request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(genre_from_item).collect(),
            response.total,
        ))
    }

    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>> {
        let response = self.item_page("Playlist", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(playlist_from_item).collect(),
            response.total,
        ))
    }

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> ProviderResult<PlaylistDetail> {
        let raw_playlist_id = raw_item_id(playlist_id.as_str());
        let mut playlist_url = endpoint(&self.base_url, &format!("Items/{raw_playlist_id}"))?;
        playlist_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id);
        let playlist = playlist_from_item(self.get_json::<JellyfinItem>(playlist_url).await?);

        let mut entries = Vec::new();
        let mut offset = 0;
        loop {
            let mut url = endpoint(
                &self.base_url,
                &format!("Playlists/{raw_playlist_id}/Items"),
            )?;
            url.query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("StartIndex", &offset.to_string())
                .append_pair("Limit", "500")
                .append_pair(
                    "Fields",
                    "Genres,ProductionYear,RunTimeTicks,ParentId,AlbumId,ArtistItems,UserData,ImageTags",
                );
            let response = self.get_json::<ItemQueryResult>(url).await?;
            let item_count = response.items.len();
            entries.extend(response.items.into_iter().map(|item| {
                let entry_id = item
                    .playlist_item_id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| item.id.clone());
                PlaylistEntry {
                    entry_id,
                    track: track_from_item(item),
                }
            }));
            offset += item_count;
            let total = response.total_record_count.unwrap_or(0);
            if item_count == 0 || (total > 0 && offset >= total) || (total == 0 && item_count < 500)
            {
                break;
            }
        }

        let tracks = entries.iter().map(|entry| entry.track.clone()).collect();
        Ok(PlaylistDetail {
            playlist,
            tracks,
            entries,
        })
    }

    async fn genre_detail(&self, genre_id: &GenreId) -> ProviderResult<GenreDetail> {
        let raw_genre_id = raw_item_id(genre_id.as_str());
        let mut genre_url = endpoint(&self.base_url, &format!("Items/{raw_genre_id}"))?;
        genre_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id);
        let genre = genre_from_item(self.get_json::<JellyfinItem>(genre_url).await?);

        let mut albums_url = endpoint(&self.base_url, "Items")?;
        albums_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "MusicAlbum")
            .append_pair("Genres", &genre.name)
            .append_pair("Limit", "500")
            .append_pair(
                "Fields",
                "Genres,ProductionYear,RunTimeTicks,ParentId,UserData,ImageTags",
            );
        let albums = self
            .get_json::<ItemQueryResult>(albums_url)
            .await?
            .items
            .into_iter()
            .map(album_from_item)
            .collect();

        let mut tracks_url = endpoint(&self.base_url, "Items")?;
        tracks_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("Genres", &genre.name)
            .append_pair("Limit", "500")
            .append_pair("Fields", "Genres,ProductionYear,RunTimeTicks,ParentId,AlbumId,ArtistItems,UserData,ImageTags");
        let tracks = self
            .get_json::<ItemQueryResult>(tracks_url)
            .await?
            .items
            .into_iter()
            .map(track_from_item)
            .collect();

        Ok(GenreDetail {
            genre,
            albums,
            tracks,
        })
    }

    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track> {
        let mut url = endpoint(
            &self.base_url,
            &format!("Items/{}", raw_item_id(track_id.as_str())),
        )?;
        url.query_pairs_mut().append_pair("UserId", &self.user_id);
        self.get_json::<JellyfinItem>(url)
            .await
            .map(track_from_item)
    }

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let mut url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/stream"))?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("DeviceId", DEVICE_ID)
            .append_pair("Static", "true")
            .append_pair("api_key", &self.access_token);
        let mut redacted_url = url.clone();
        redacted_url
            .query_pairs_mut()
            .clear()
            .append_pair("UserId", &self.user_id)
            .append_pair("DeviceId", DEVICE_ID)
            .append_pair("Static", "true")
            .append_pair("api_key", "<redacted>");
        Ok(StreamDescriptor::with_redacted(
            url.to_string(),
            redacted_url.to_string(),
        ))
    }

    async fn search(&self, query: &str) -> ProviderResult<SearchResults> {
        if query.trim().is_empty() {
            return Ok(SearchResults::default());
        }

        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("SearchTerm", query)
            .append_pair("IncludeItemTypes", "Audio,MusicAlbum,MusicArtist,Playlist")
            .append_pair("Limit", "100")
            .append_pair(
                "Fields",
                "Genres,ProductionYear,RunTimeTicks,ParentId,UserData,ImageTags",
            );
        let response = self.get_json::<ItemQueryResult>(url).await?;
        let mut results = SearchResults::default();
        for item in response.items {
            match item.item_type.as_deref() {
                Some("Audio") => results.tracks.push(track_from_item(item)),
                Some("MusicAlbum") => results.albums.push(album_from_item(item)),
                Some("MusicArtist") | Some("Artist") => {
                    results.artists.push(artist_from_item(item))
                }
                Some("Playlist") => results.playlists.push(playlist_from_item(item)),
                _ => {}
            }
        }
        Ok(results)
    }

    async fn image_metadata(
        &self,
        item_id: &str,
        kind: ImageKind,
    ) -> ProviderResult<ImageMetadata> {
        Ok(ImageMetadata {
            item_id: raw_item_id(item_id).to_string(),
            kind,
            tag: None,
            url: self.image_url(item_id, kind, None)?,
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> ProviderResult<ImageBytes> {
        let mut url = endpoint(
            &self.base_url,
            &format!(
                "Items/{}/Images/{}",
                raw_item_id(&request.item_id),
                image_kind_path(request.kind)
            ),
        )?;
        url.query_pairs_mut()
            .append_pair("fillWidth", &request.size.max(1).to_string())
            .append_pair("fillHeight", &request.size.max(1).to_string())
            .append_pair("quality", "90");
        if let Some(tag) = request.tag.as_deref().filter(|tag| !tag.is_empty()) {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        let config = JellyfinClientConfig::new(self.identity.server.base_url.clone(), false);
        send_bytes(self.client.get(url).header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }

    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> ProviderResult<()> {
        let mut url = endpoint(
            &self.base_url,
            &format!("UserFavoriteItems/{}", raw_item_id(item_id.as_str())),
        )?;
        url.query_pairs_mut().append_pair("userId", &self.user_id);
        if favorite {
            self.send_unit(self.client.post(url)).await
        } else {
            self.send_unit(self.client.delete(url)).await
        }
    }

    async fn create_playlist(
        &self,
        name: &str,
        track_ids: &[TrackId],
    ) -> ProviderResult<PlaylistId> {
        let url = endpoint(&self.base_url, "Playlists")?;
        let body = CreatePlaylistDto {
            name: name.to_string(),
            ids: raw_track_ids(track_ids),
            user_id: Some(self.user_id.clone()),
            media_type: Some("Audio".to_string()),
            is_public: false,
        };
        let result = self
            .send_json::<PlaylistCreationResult>(self.client.post(url).json(&body))
            .await?;
        Ok(PlaylistId::new(jellyfin_id("playlist", &result.id)))
    }

    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> ProviderResult<()> {
        let url = endpoint(
            &self.base_url,
            &format!("Playlists/{}", raw_item_id(playlist_id.as_str())),
        )?;
        let body = UpdatePlaylistDto {
            name: Some(name.to_string()),
        };
        self.send_unit(self.client.post(url).json(&body)).await
    }

    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> ProviderResult<()> {
        let mut url = endpoint(
            &self.base_url,
            &format!("Playlists/{}/Items", raw_item_id(playlist_id.as_str())),
        )?;
        url.query_pairs_mut()
            .append_pair("userId", &self.user_id)
            .append_pair("ids", &raw_track_ids(track_ids).join(","));
        self.send_unit(self.client.post(url)).await
    }

    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> ProviderResult<()> {
        let mut url = endpoint(
            &self.base_url,
            &format!("Playlists/{}/Items", raw_item_id(playlist_id.as_str())),
        )?;
        url.query_pairs_mut()
            .append_pair("entryIds", &entry_ids.join(","));
        self.send_unit(self.client.delete(url)).await
    }

    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> ProviderResult<()> {
        let url = endpoint(
            &self.base_url,
            &format!(
                "Playlists/{}/Items/{}/Move/{}",
                raw_item_id(playlist_id.as_str()),
                raw_item_id(entry_id),
                new_index
            ),
        )?;
        self.send_unit(self.client.post(url)).await
    }

    async fn lyrics(
        &self,
        track_id: &TrackId,
        allow_remote: bool,
    ) -> ProviderResult<Option<Lyrics>> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let local_url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/Lyrics"))?;
        match self.send_json::<LyricDto>(self.client.get(local_url)).await {
            Ok(dto) => {
                return Ok(Some(lyrics_from_dto(
                    track_id.clone(),
                    LyricsSource::Server,
                    dto,
                )));
            }
            Err(ProviderError::NotFound) if allow_remote => {}
            Err(ProviderError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        }

        let remote_url = endpoint(
            &self.base_url,
            &format!("Audio/{raw_track_id}/RemoteSearch/Lyrics"),
        )?;
        let results = self
            .send_json::<Vec<RemoteLyricInfoDto>>(self.client.get(remote_url))
            .await?;
        let Some(first) = results.into_iter().find(|result| !result.id.is_empty()) else {
            return Ok(None);
        };
        let lyric_url = endpoint(&self.base_url, &format!("Providers/Lyrics/{}", first.id))?;
        let dto = self
            .send_json::<LyricDto>(self.client.get(lyric_url))
            .await?;
        Ok(Some(lyrics_from_dto(
            track_id.clone(),
            LyricsSource::Remote,
            dto,
        )))
    }

    async fn report_playback(&self, report: PlaybackReport) -> ProviderResult<()> {
        let path = match report.kind {
            PlaybackReportKind::Started => "Sessions/Playing",
            PlaybackReportKind::Progress => "Sessions/Playing/Progress",
            PlaybackReportKind::Stopped => "Sessions/Playing/Stopped",
        };
        let url = endpoint(&self.base_url, path)?;
        let body = PlaybackReportDto::from_report(report);
        self.send_unit(self.client.post(url).json(&body)).await
    }
}

async fn public_server_name(
    client: &Client,
    base_url: &Url,
    config: &JellyfinClientConfig,
) -> Option<String> {
    let url = endpoint(base_url, "System/Info/Public").ok()?;
    let response = send_json::<PublicSystemInfo>(
        client
            .get(url)
            .header(header::AUTHORIZATION, auth_header(config, None)),
    )
    .await
    .ok()?;
    response.server_name.or(response.local_address)
}

async fn send_json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> ProviderResult<T> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Jellyfin returned {}",
            status.as_u16()
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProviderError::NotFound);
    }
    if status.is_client_error() || status.is_server_error() {
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(ProviderError::Server {
            status: status.as_u16(),
            message,
        });
    }

    response.json::<T>().await.map_err(map_reqwest_error)
}

async fn send_unit(request: reqwest::RequestBuilder) -> ProviderResult<()> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Jellyfin returned {}",
            status.as_u16()
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProviderError::NotFound);
    }
    if status.is_client_error() || status.is_server_error() {
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(ProviderError::Server {
            status: status.as_u16(),
            message,
        });
    }
    Ok(())
}

async fn send_bytes(request: reqwest::RequestBuilder) -> ProviderResult<ImageBytes> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Jellyfin returned {}",
            status.as_u16()
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProviderError::NotFound);
    }
    if status.is_client_error() || status.is_server_error() {
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(ProviderError::Server {
            status: status.as_u16(),
            message,
        });
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await.map_err(map_reqwest_error)?.to_vec();
    Ok(ImageBytes {
        bytes,
        content_type,
    })
}

fn build_client(trust_invalid_cert: bool) -> ProviderResult<Client> {
    Client::builder()
        .danger_accept_invalid_certs(trust_invalid_cert)
        .build()
        .map_err(map_reqwest_error)
}

fn normalize_base_url(raw: &str) -> ProviderResult<Url> {
    let trimmed = raw.trim().trim_end_matches('/');
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut url =
        Url::parse(&candidate).map_err(|error| ProviderError::Other(error.to_string()))?;
    let path = url.path().trim_end_matches('/').to_string();
    let normalized_path = if path.is_empty() {
        "/".to_string()
    } else {
        format!("{path}/")
    };
    url.set_path(&normalized_path);
    Ok(url)
}

fn endpoint(base_url: &Url, path: &str) -> ProviderResult<Url> {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let full_path = if base_path.is_empty() {
        format!("/{path}")
    } else {
        format!("{base_path}/{path}")
    };
    url.set_path(&full_path);
    url.set_query(None);
    Ok(url)
}

fn auth_header(config: &JellyfinClientConfig, token: Option<&str>) -> String {
    let mut value = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        config.client_name, config.device_name, config.device_id, config.client_version
    );
    if let Some(token) = token {
        value.push_str(&format!(", Token=\"{token}\""));
    }
    value
}

fn map_reqwest_error(error: reqwest::Error) -> ProviderError {
    let message = error.to_string();
    if message.to_lowercase().contains("certificate") || message.to_lowercase().contains("tls") {
        ProviderError::Tls(message)
    } else if error.is_connect() || error.is_request() || error.is_timeout() {
        ProviderError::Network(message)
    } else if let Some(status) = error.status() {
        ProviderError::Server {
            status: status.as_u16(),
            message,
        }
    } else {
        ProviderError::Other(message)
    }
}

fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

fn image_kind_path(kind: ImageKind) -> &'static str {
    match kind {
        ImageKind::Primary => "Primary",
        ImageKind::Backdrop => "Backdrop",
    }
}

fn jellyfin_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        lyrics: true,
        playback_reporting: true,
        playlist_mutations: true,
        favorite_mutations: true,
        ..ProviderCapabilities::default()
    }
}

fn jellyfin_id(kind: &str, id: &str) -> String {
    format!("jellyfin:{kind}:{id}")
}

fn raw_track_ids(track_ids: &[TrackId]) -> Vec<String> {
    track_ids
        .iter()
        .map(|id| raw_item_id(id.as_str()).to_string())
        .collect()
}

fn stable_server_id(input: &str) -> String {
    format!("{:016x}", stable_hash(input))
}

fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}

fn duration_seconds(ticks: Option<i64>) -> u32 {
    ticks
        .map(|value| (value.max(0) / 10_000_000) as u32)
        .unwrap_or(0)
}

fn ticks_to_millis(ticks: Option<i64>) -> Option<u64> {
    ticks.map(|value| (value.max(0) / 10_000) as u64)
}

fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}

fn u32_from_option(value: Option<i32>) -> u32 {
    value.unwrap_or_default().max(0) as u32
}

fn favorite(user_data: &Option<UserData>) -> bool {
    user_data
        .as_ref()
        .and_then(|data| data.is_favorite)
        .unwrap_or(false)
}

fn album_from_item(item: JellyfinItem) -> Album {
    let item_id = item.id.clone();
    let artist_id = item
        .artist_items
        .as_ref()
        .and_then(|items| items.first())
        .map(|artist| ArtistId::new(jellyfin_id("artist", &artist.id)));
    let artist = item
        .album_artist
        .clone()
        .or_else(|| {
            item.artists
                .as_ref()
                .and_then(|artists| artists.first().cloned())
        })
        .unwrap_or_else(|| "Unknown Artist".to_string());
    Album {
        id: AlbumId::new(jellyfin_id("album", &item.id)),
        title: item.name.unwrap_or_else(|| "Untitled Album".to_string()),
        artist,
        artist_id,
        year: u16_from_option(item.production_year),
        track_count: u16_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        color_seed: color_seed(&item_id),
        image_ref: primary_image_ref("album", &item.id, &item.image_tags),
        genres: item.genres.unwrap_or_default(),
    }
}

fn track_from_item(item: JellyfinItem) -> Track {
    let image_ref = primary_image_ref("track", &item.id, &item.image_tags).or_else(|| {
        item.album_id.as_ref().map(|album_id| ImageRef {
            item_id: jellyfin_id("album", album_id),
            tag: None,
        })
    });
    let artist_id = item
        .artist_items
        .as_ref()
        .and_then(|items| items.first())
        .map(|artist| ArtistId::new(jellyfin_id("artist", &artist.id)));
    let album_id = item
        .album_id
        .as_deref()
        .or(item.parent_id.as_deref())
        .unwrap_or(&item.id);
    Track {
        id: TrackId::new(jellyfin_id("track", &item.id)),
        album_id: AlbumId::new(jellyfin_id("album", album_id)),
        title: item.name.unwrap_or_else(|| "Untitled Track".to_string()),
        artist: item
            .artists
            .as_ref()
            .and_then(|artists| artists.first().cloned())
            .unwrap_or_else(|| {
                item.album_artist
                    .unwrap_or_else(|| "Unknown Artist".to_string())
            }),
        artist_id,
        album: item.album.unwrap_or_else(|| "Unknown Album".to_string()),
        year: u16_from_option(item.production_year),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        disc_number: u16_from_option(item.parent_index_number),
        track_number: u16_from_option(item.index_number),
        image_ref,
        genres: item.genres.unwrap_or_default(),
    }
}

fn artist_from_item(item: JellyfinItem) -> Artist {
    Artist {
        id: ArtistId::new(jellyfin_id("artist", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
        favorite: favorite(&item.user_data),
        image_ref: primary_image_ref("artist", &item.id, &item.image_tags),
    }
}

fn genre_from_item(item: JellyfinItem) -> Genre {
    Genre {
        id: GenreId::new(jellyfin_id("genre", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Genre".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
        image_ref: primary_image_ref("genre", &item.id, &item.image_tags),
    }
}

fn playlist_from_item(item: JellyfinItem) -> Playlist {
    Playlist {
        id: PlaylistId::new(jellyfin_id("playlist", &item.id)),
        name: item.name.unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: u32_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
        image_ref: primary_image_ref("playlist", &item.id, &item.image_tags),
    }
}

fn lyrics_from_dto(track_id: TrackId, source: LyricsSource, dto: LyricDto) -> Lyrics {
    Lyrics {
        track_id,
        source,
        lines: dto
            .lyrics
            .unwrap_or_default()
            .into_iter()
            .filter_map(|line| {
                let text = line.text.unwrap_or_default();
                (!text.trim().is_empty()).then_some(LyricLine {
                    text,
                    start_millis: ticks_to_millis(line.start),
                })
            })
            .collect(),
    }
}

fn primary_image_ref(
    kind: &str,
    item_id: &str,
    image_tags: &Option<HashMap<String, String>>,
) -> Option<ImageRef> {
    image_tags
        .as_ref()
        .and_then(|tags| tags.get("Primary"))
        .filter(|tag| !tag.is_empty())
        .map(|tag| ImageRef {
            item_id: jellyfin_id(kind, item_id),
            tag: Some(tag.clone()),
        })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    server_name: Option<String>,
    local_address: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticateByNameRequest {
    username: String,
    #[serde(rename = "Pw")]
    password: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResult {
    access_token: String,
    server_id: Option<String>,
    user: JellyfinUser,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinUser {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemQueryResult {
    #[serde(default)]
    items: Vec<JellyfinItem>,
    total_record_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinItem {
    id: String,
    name: Option<String>,
    #[serde(rename = "Type")]
    item_type: Option<String>,
    album_artist: Option<String>,
    artists: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    artist_items: Option<Vec<NameIdPair>>,
    album: Option<String>,
    album_id: Option<String>,
    parent_id: Option<String>,
    production_year: Option<i32>,
    run_time_ticks: Option<i64>,
    child_count: Option<i32>,
    album_count: Option<i32>,
    song_count: Option<i32>,
    item_counts: Option<JellyfinItemCounts>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    user_data: Option<UserData>,
    image_tags: Option<HashMap<String, String>>,
    playlist_item_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinItemCounts {
    album_count: Option<i32>,
    song_count: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NameIdPair {
    #[allow(dead_code)]
    name: Option<String>,
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserData {
    is_favorite: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CreatePlaylistDto {
    name: String,
    ids: Vec<String>,
    user_id: Option<String>,
    media_type: Option<String>,
    is_public: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct UpdatePlaylistDto {
    name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlaylistCreationResult {
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LyricDto {
    lyrics: Option<Vec<LyricLineDto>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LyricLineDto {
    text: Option<String>,
    start: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteLyricInfoDto {
    id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackReportDto {
    can_seek: bool,
    item_id: String,
    is_paused: bool,
    is_muted: bool,
    position_ticks: i64,
    volume_level: i32,
    play_method: &'static str,
    repeat_mode: &'static str,
    playback_order: &'static str,
    failed: bool,
}

impl PlaybackReportDto {
    fn from_report(report: PlaybackReport) -> Self {
        Self {
            can_seek: true,
            item_id: raw_item_id(report.track_id.as_str()).to_string(),
            is_paused: report.paused,
            is_muted: report.muted,
            position_ticks: i64::from(report.position_seconds) * 10_000_000,
            volume_level: i32::from(report.volume_percent.min(100)),
            play_method: "DirectPlay",
            repeat_mode: if report.repeat_one {
                "RepeatOne"
            } else if report.repeat_all {
                "RepeatAll"
            } else {
                "RepeatNone"
            },
            playback_order: if report.shuffle { "Shuffle" } else { "Default" },
            failed: report.failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_provider::MusicProvider;
    use wiremock::matchers::{
        body_json, body_partial_json, header_regex, method, path, query_param,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn login_posts_credentials_and_maps_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Users/AuthenticateByName"))
            .and(header_regex(
                "authorization",
                "MediaBrowser Client=\"Rufin\"",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "AccessToken": "secret-token",
                "ServerId": "server-one",
                "User": { "Id": "user-one", "Name": "demo" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ServerName": "Music Box"
            })))
            .mount(&server)
            .await;

        let session = JellyfinProvider::login(LoginRequest {
            base_url: server.uri(),
            username: "demo".to_string(),
            password: "pw".to_string(),
            trust_invalid_cert: false,
        })
        .await
        .expect("login");

        assert_eq!(session.server.id.as_str(), "jellyfin:server:server-one");
        assert_eq!(session.server.name, "Music Box");
        assert_eq!(session.username, "demo");
        assert_eq!(session.access_token, "secret-token");
    }

    #[test]
    fn bare_server_addresses_default_to_http() {
        let url = normalize_base_url("music.local:8096").expect("normalized url");

        assert_eq!(url.as_str(), "http://music.local:8096/");
    }

    #[tokio::test]
    async fn album_reads_send_auth_header_and_map_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("IncludeItemTypes", "MusicAlbum"))
            .and(query_param("StartIndex", "5"))
            .and(query_param("Limit", "2"))
            .and(header_regex("authorization", "Token=\"token-one\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "TotalRecordCount": 20,
                    "Items": [{
                        "Id": "album-one",
                        "Name": "Blue Rooms",
                    "Type": "MusicAlbum",
                    "AlbumArtist": "Astral Kin",
                    "Genres": ["Ambient", "Electronic"],
                    "ProductionYear": 2024,
                    "ChildCount": 9,
                    "RunTimeTicks": 1800000000i64,
                    "UserData": { "IsFavorite": true },
                    "ImageTags": { "Primary": "album-tag-one" }
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let page = provider
            .albums(PagedRequest::new(5, 2))
            .await
            .expect("albums");

        assert_eq!(page.total, 20);
        assert_eq!(page.items[0].id.as_str(), "jellyfin:album:album-one");
        assert_eq!(page.items[0].title, "Blue Rooms");
        assert_eq!(page.items[0].genres, vec!["Ambient", "Electronic"]);
        assert_eq!(
            page.items[0].image_ref,
            Some(ImageRef {
                item_id: "jellyfin:album:album-one".to_string(),
                tag: Some("album-tag-one".to_string()),
            })
        );
        assert!(page.items[0].favorite);
    }

    #[tokio::test]
    async fn image_bytes_send_auth_header_and_size_params() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/album-one/Images/Primary"))
            .and(query_param("fillWidth", "256"))
            .and(query_param("fillHeight", "256"))
            .and(query_param("quality", "90"))
            .and(query_param("tag", "album-tag-one"))
            .and(header_regex("authorization", "Token=\"secret-token\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(vec![1_u8, 2, 3]),
            )
            .mount(&server)
            .await;
        let provider = provider(&server, "secret-token");

        let image = provider
            .image_bytes(ImageRequest {
                item_id: "jellyfin:album:album-one".to_string(),
                kind: ImageKind::Primary,
                tag: Some("album-tag-one".to_string()),
                size: 256,
            })
            .await
            .expect("image bytes");

        assert_eq!(image.bytes, vec![1, 2, 3]);
        assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
    }

    #[tokio::test]
    async fn image_errors_do_not_expose_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/album-one/Images/Primary"))
            .respond_with(ResponseTemplate::new(500).set_body_string("broken"))
            .mount(&server)
            .await;
        let provider = provider(&server, "secret-token");

        let error = provider
            .image_bytes(ImageRequest {
                item_id: "jellyfin:album:album-one".to_string(),
                kind: ImageKind::Primary,
                tag: None,
                size: 256,
            })
            .await
            .expect_err("image error");

        assert!(!format!("{error:?}").contains("secret-token"));
        assert!(!error.to_string().contains("secret-token"));
    }

    #[tokio::test]
    async fn album_detail_loads_album_and_matching_tracks() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/album-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin",
                "ChildCount": 1
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("ParentId", "album-one"))
            .and(query_param("IncludeItemTypes", "Audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{
                    "Id": "track-one",
                    "Name": "First Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "IndexNumber": 1,
                    "RunTimeTicks": 2100000000i64
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let detail = provider
            .album_detail(&AlbumId::new("jellyfin:album:album-one"))
            .await
            .expect("detail");

        assert_eq!(detail.album.id.as_str(), "jellyfin:album:album-one");
        assert_eq!(
            detail.tracks[0].album_id.as_str(),
            "jellyfin:album:album-one"
        );
        assert_eq!(detail.tracks[0].duration_seconds, 210);
    }

    #[tokio::test]
    async fn artists_playlists_and_favorites_map_common_counts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Artists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{
                    "Id": "artist-one",
                    "Name": "Astral Kin",
                    "ItemCounts": {
                        "AlbumCount": 4,
                        "SongCount": 30
                    },
                    "UserData": { "IsFavorite": true }
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let artists = provider
            .artists(PagedRequest::new(0, 50))
            .await
            .expect("artists");

        assert_eq!(artists.items[0].id.as_str(), "jellyfin:artist:artist-one");
        assert_eq!(artists.items[0].album_count, 4);
        assert_eq!(artists.items[0].track_count, 30);
        assert!(artists.items[0].favorite);
    }

    #[tokio::test]
    async fn genres_use_music_genres_endpoint_and_scope_to_music_items() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/MusicGenres"))
            .and(query_param("IncludeItemTypes", "Audio,MusicAlbum"))
            .and(query_param("StartIndex", "3"))
            .and(query_param("Limit", "7"))
            .and(header_regex("authorization", "Token=\"token-one\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{
                    "Id": "genre-one",
                    "Name": "Dream Pop",
                    "Type": "MusicGenre",
                    "ItemCounts": {
                        "AlbumCount": 4,
                        "SongCount": 31
                    },
                    "ImageTags": { "Primary": "genre-tag" }
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let genres = provider
            .genres(PagedRequest::new(3, 7))
            .await
            .expect("genres");

        assert_eq!(genres.total, 1);
        assert_eq!(genres.items[0].id.as_str(), "jellyfin:genre:genre-one");
        assert_eq!(genres.items[0].name, "Dream Pop");
        assert_eq!(genres.items[0].album_count, 4);
        assert_eq!(genres.items[0].track_count, 31);
        assert_eq!(
            genres.items[0].image_ref,
            Some(ImageRef {
                item_id: "jellyfin:genre:genre-one".to_string(),
                tag: Some("genre-tag".to_string()),
            })
        );
    }

    #[tokio::test]
    async fn playlist_detail_paginates_and_maps_ordered_tracks() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/playlist-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "playlist-one",
                "Name": "Late Set",
                "Type": "Playlist",
                "ChildCount": 501,
                "RunTimeTicks": 9000000000i64,
                "ImageTags": { "Primary": "playlist-tag" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Playlists/playlist-one/Items"))
            .and(query_param("StartIndex", "0"))
            .and(query_param("Limit", "500"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 501,
                "Items": [{
                    "Id": "track-one",
                    "Name": "First Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "Genres": ["Ambient"],
                    "IndexNumber": 1,
                    "RunTimeTicks": 2100000000i64,
                    "PlaylistItemId": "entry-one",
                    "ImageTags": { "Primary": "track-tag-one" }
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Playlists/playlist-one/Items"))
            .and(query_param("StartIndex", "1"))
            .and(query_param("Limit", "500"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 501,
                "Items": [{
                    "Id": "track-two",
                    "Name": "Second Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "Genres": ["Ambient"],
                    "IndexNumber": 2,
                    "RunTimeTicks": 2200000000i64,
                    "PlaylistItemId": "entry-two"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Playlists/playlist-one/Items"))
            .and(query_param("StartIndex", "2"))
            .and(query_param("Limit", "500"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 2,
                "Items": []
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let detail = provider
            .playlist_detail(&PlaylistId::new("jellyfin:playlist:playlist-one"))
            .await
            .expect("playlist detail");

        assert_eq!(detail.playlist.name, "Late Set");
        assert_eq!(
            detail.playlist.image_ref,
            Some(ImageRef {
                item_id: "jellyfin:playlist:playlist-one".to_string(),
                tag: Some("playlist-tag".to_string()),
            })
        );
        assert_eq!(detail.tracks.len(), 2);
        assert_eq!(detail.entries.len(), 2);
        assert_eq!(detail.entries[0].entry_id, "entry-one");
        assert_eq!(detail.entries[1].entry_id, "entry-two");
        assert_eq!(detail.tracks[0].title, "First Motion");
        assert_eq!(detail.tracks[0].genres, vec!["Ambient"]);
        assert_eq!(
            detail.tracks[0].image_ref,
            Some(ImageRef {
                item_id: "jellyfin:track:track-one".to_string(),
                tag: Some("track-tag-one".to_string()),
            })
        );
        assert_eq!(detail.tracks[1].title, "Second Motion");
    }

    #[tokio::test]
    async fn favorite_mutations_use_jellyfin_item_favorite_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/UserFavoriteItems/track-one"))
            .and(query_param("userId", "user-one"))
            .and(header_regex("authorization", "Token=\"token-one\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/UserFavoriteItems/album-one"))
            .and(query_param("userId", "user-one"))
            .and(header_regex("authorization", "Token=\"token-one\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        provider
            .set_favorite(
                FavoriteItemId::Track(TrackId::new("jellyfin:track:track-one")),
                true,
            )
            .await
            .expect("favorite track");
        provider
            .set_favorite(
                FavoriteItemId::Album(AlbumId::new("jellyfin:album:album-one")),
                false,
            )
            .await
            .expect("unfavorite album");
    }

    #[tokio::test]
    async fn playlist_write_mutations_use_jellyfin_playlist_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Playlists"))
            .and(body_json(serde_json::json!({
                "Name": "Road",
                "Ids": ["track-one", "track-two"],
                "UserId": "user-one",
                "MediaType": "Audio",
                "IsPublic": false
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "playlist-one"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Playlists/playlist-one"))
            .and(body_json(serde_json::json!({ "Name": "Road Mix" })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Playlists/playlist-one/Items"))
            .and(query_param("userId", "user-one"))
            .and(query_param("ids", "track-three"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/Playlists/playlist-one/Items"))
            .and(query_param("entryIds", "entry-one,entry-two"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Playlists/playlist-one/Items/entry-three/Move/0"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");
        let playlist_id = PlaylistId::new("jellyfin:playlist:playlist-one");

        assert_eq!(
            provider
                .create_playlist(
                    "Road",
                    &[
                        TrackId::new("jellyfin:track:track-one"),
                        TrackId::new("jellyfin:track:track-two")
                    ]
                )
                .await
                .expect("create playlist"),
            playlist_id
        );
        provider
            .rename_playlist(&playlist_id, "Road Mix")
            .await
            .expect("rename playlist");
        provider
            .add_playlist_tracks(&playlist_id, &[TrackId::new("jellyfin:track:track-three")])
            .await
            .expect("add playlist tracks");
        provider
            .remove_playlist_entries(
                &playlist_id,
                &["entry-one".to_string(), "entry-two".to_string()],
            )
            .await
            .expect("remove playlist entries");
        provider
            .move_playlist_entry(&playlist_id, "entry-three", 0)
            .await
            .expect("move playlist entry");
    }

    #[tokio::test]
    async fn lyrics_use_local_first_and_remote_fallback_when_enabled() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Audio/track-local/Lyrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Lyrics": [
                    { "Text": "local line", "Start": 120000000i64 }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Audio/track-remote/Lyrics"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Audio/track-remote/RemoteSearch/Lyrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "Id": "remote-lyric-one" }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Providers/Lyrics/remote-lyric-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Lyrics": [
                    { "Text": "remote line", "Start": 340000000i64 }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let local = provider
            .lyrics(&TrackId::new("jellyfin:track:track-local"), true)
            .await
            .expect("local lyrics")
            .expect("local lyrics");
        assert_eq!(local.source, LyricsSource::Server);
        assert_eq!(local.lines[0].text, "local line");
        assert_eq!(local.lines[0].start_millis, Some(12_000));

        let remote = provider
            .lyrics(&TrackId::new("jellyfin:track:track-remote"), true)
            .await
            .expect("remote lyrics")
            .expect("remote lyrics");
        assert_eq!(remote.source, LyricsSource::Remote);
        assert_eq!(remote.lines[0].text, "remote line");
        assert_eq!(remote.lines[0].start_millis, Some(34_000));
    }

    #[tokio::test]
    async fn playback_reporting_posts_expected_payloads() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing"))
            .and(body_partial_json(serde_json::json!({
                "ItemId": "track-one",
                "PositionTicks": 420000000i64,
                "VolumeLevel": 67,
                "RepeatMode": "RepeatAll",
                "PlaybackOrder": "Shuffle",
                "Failed": false
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Progress"))
            .and(body_partial_json(serde_json::json!({
                "ItemId": "track-one",
                "IsPaused": true,
                "IsMuted": true
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Playing/Stopped"))
            .and(body_partial_json(serde_json::json!({
                "ItemId": "track-one",
                "Failed": true
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");
        let base_report = PlaybackReport {
            kind: PlaybackReportKind::Started,
            track_id: TrackId::new("jellyfin:track:track-one"),
            position_seconds: 42,
            paused: false,
            muted: false,
            volume_percent: 67,
            shuffle: true,
            repeat_one: false,
            repeat_all: true,
            failed: false,
        };

        provider
            .report_playback(base_report.clone())
            .await
            .expect("started report");
        provider
            .report_playback(PlaybackReport {
                kind: PlaybackReportKind::Progress,
                paused: true,
                muted: true,
                ..base_report.clone()
            })
            .await
            .expect("progress report");
        provider
            .report_playback(PlaybackReport {
                kind: PlaybackReportKind::Stopped,
                failed: true,
                ..base_report
            })
            .await
            .expect("stopped report");
    }

    #[tokio::test]
    async fn auth_and_server_errors_are_distinct() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
            .mount(&server)
            .await;
        let provider = provider(&server, "bad-token");

        let error = provider
            .albums(PagedRequest::new(0, 1))
            .await
            .expect_err("auth error");

        assert!(matches!(error, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn stream_url_uses_direct_audio_endpoint_and_redacts_token() {
        let server = MockServer::start().await;
        let provider = provider(&server, "secret-token");

        let stream = provider
            .stream(&TrackId::new("jellyfin:track:track-one"))
            .await
            .expect("stream");

        assert!(
            stream
                .uri()
                .starts_with(&format!("{}/Audio/track-one/stream?", server.uri()))
        );
        assert!(stream.uri().contains("api_key=secret-token"));
        assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
        assert!(!format!("{stream:?}").contains("secret-token"));
    }

    fn provider(server: &MockServer, token: &str) -> JellyfinProvider {
        JellyfinProvider::from_saved_session(SavedProviderSession {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:test"),
                provider: "jellyfin".to_string(),
                name: "Test".to_string(),
                base_url: server.uri(),
            },
            user_id: "user-one".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
            access_token: token.to_string(),
        })
        .expect("provider")
    }
}
