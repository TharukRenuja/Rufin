use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url, header};
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, HOME_SECTION_ITEM_LIMIT, HomeSection,
    HomeSectionKind, ImageRef, MusicFolder, MusicFolderId, Playlist, PlaylistId, ServerId,
    ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, FavoriteItemId, GenreDetail, ImageBytes, ImageKind, ImageMetadata, ImageRequest,
    LyricLine, Lyrics, LyricsSource, MusicProvider, PagedRequest, PagedResponse, PlaybackReport,
    PlaybackReportKind, PlayedFilter, PlaylistDetail, PlaylistEntry, ProviderCapabilities,
    ProviderError, ProviderIdentity, ProviderResult, ProviderSession, RandomTrackRequest,
    SavedProviderSession, SearchResults, StreamDescriptor, StreamRequest,
};
use serde::Deserialize;
use serde::de::{self, DeserializeOwned, Visitor};
use tracing::instrument;

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

#[async_trait(?Send)]
impl MusicProvider for SubsonicProvider {
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
                let body: RandomSongsBody = self
                    .get_json(
                        "getRandomSongs",
                        &[("size", HOME_SECTION_ITEM_LIMIT.to_string())],
                    )
                    .await?;
                Ok(HomeSection {
                    kind,
                    albums: Vec::new(),
                    tracks: body
                        .random_songs
                        .map(|songs| songs.song)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|song| track_from_dto(self, song))
                        .collect(),
                })
            }
            HomeSectionKind::MostPlayed => {
                let body: AlbumListBody = self
                    .get_json(
                        "getAlbumList2",
                        &[
                            ("type", "frequent".to_string()),
                            ("size", HOME_SECTION_ITEM_LIMIT.to_string()),
                        ],
                    )
                    .await?;
                Ok(HomeSection {
                    kind,
                    albums: body
                        .album_list
                        .album
                        .into_iter()
                        .map(|album| album_from_dto(self, album))
                        .collect(),
                    tracks: Vec::new(),
                })
            }
            HomeSectionKind::NewlyAdded => {
                let body: AlbumListBody = self
                    .get_json(
                        "getAlbumList2",
                        &[
                            ("type", "newest".to_string()),
                            ("size", HOME_SECTION_ITEM_LIMIT.to_string()),
                        ],
                    )
                    .await?;
                Ok(HomeSection {
                    kind,
                    albums: body
                        .album_list
                        .album
                        .into_iter()
                        .map(|album| album_from_dto(self, album))
                        .collect(),
                    tracks: Vec::new(),
                })
            }
            HomeSectionKind::RecentlyPlayed => {
                let body: AlbumListBody = self
                    .get_json(
                        "getAlbumList2",
                        &[
                            ("type", "recent".to_string()),
                            ("size", HOME_SECTION_ITEM_LIMIT.to_string()),
                        ],
                    )
                    .await?;
                Ok(HomeSection {
                    kind,
                    albums: body
                        .album_list
                        .album
                        .into_iter()
                        .map(|album| album_from_dto(self, album))
                        .collect(),
                    tracks: Vec::new(),
                })
            }
            HomeSectionKind::RecentlyReleased => {
                let body: AlbumListBody = self
                    .get_json(
                        "getAlbumList2",
                        &[
                            ("type", "byYear".to_string()),
                            ("fromYear", current_year().to_string()),
                            ("toYear", "0".to_string()),
                            ("size", HOME_SECTION_ITEM_LIMIT.to_string()),
                        ],
                    )
                    .await?;
                Ok(HomeSection {
                    kind,
                    albums: body
                        .album_list
                        .album
                        .into_iter()
                        .map(|album| album_from_dto(self, album))
                        .collect(),
                    tracks: Vec::new(),
                })
            }
        }
    }

    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>> {
        let body: AlbumListBody = self
            .get_json(
                "getAlbumList2",
                &[
                    ("type", "alphabeticalByName".to_string()),
                    ("size", request.limit.to_string()),
                    ("offset", request.offset.to_string()),
                ],
            )
            .await?;
        Ok(PagedResponse::new(
            body.album_list
                .album
                .into_iter()
                .map(|album| album_from_dto(self, album))
                .collect(),
            0,
        ))
    }

    async fn album_detail(&self, album_id: &AlbumId) -> ProviderResult<AlbumDetail> {
        let body: AlbumBody = self
            .get_json(
                "getAlbum",
                &[("id", raw_item_id(album_id.as_str()).to_string())],
            )
            .await?;
        let album = album_from_dto(self, body.album.clone());
        let tracks = body
            .album
            .song
            .into_iter()
            .map(|song| track_from_dto(self, song))
            .collect();
        Ok(AlbumDetail { album, tracks })
    }

    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>> {
        let body: SearchBody = self
            .get_json(
                "search3",
                &[
                    ("query", String::new()),
                    ("artistCount", "0".to_string()),
                    ("artistOffset", "0".to_string()),
                    ("albumCount", "0".to_string()),
                    ("albumOffset", "0".to_string()),
                    ("songCount", request.limit.to_string()),
                    ("songOffset", request.offset.to_string()),
                ],
            )
            .await?;
        Ok(PagedResponse::new(
            body.search_result
                .and_then(|result| result.song)
                .unwrap_or_default()
                .into_iter()
                .map(|song| track_from_dto(self, song))
                .collect(),
            0,
        ))
    }

    async fn music_folders(&self) -> ProviderResult<Vec<MusicFolder>> {
        let body: MusicFoldersBody = self.get_json("getMusicFolders", &[]).await?;
        Ok(body
            .music_folders
            .music_folder
            .into_iter()
            .map(|folder| MusicFolder {
                id: MusicFolderId::new(self.id("music-folder", folder.id.0.as_str())),
                name: folder.name,
            })
            .collect())
    }

    async fn tracks_in_music_folder(
        &self,
        folder_id: &MusicFolderId,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<Track>> {
        let body: SearchBody = self
            .get_json(
                "search3",
                &[
                    ("query", String::new()),
                    ("artistCount", "0".to_string()),
                    ("artistOffset", "0".to_string()),
                    ("albumCount", "0".to_string()),
                    ("albumOffset", "0".to_string()),
                    ("songCount", request.limit.to_string()),
                    ("songOffset", request.offset.to_string()),
                    ("musicFolderId", raw_item_id(folder_id.as_str()).to_string()),
                ],
            )
            .await?;
        Ok(PagedResponse::new(
            body.search_result
                .and_then(|result| result.song)
                .unwrap_or_default()
                .into_iter()
                .map(|song| track_from_dto(self, song))
                .collect(),
            0,
        ))
    }

    async fn random_tracks(&self, request: RandomTrackRequest) -> ProviderResult<Vec<Track>> {
        if request.played_filter != PlayedFilter::All {
            return Err(ProviderError::Unsupported("random played filter"));
        }

        let mut extra = vec![("size", request.limit.clamp(1, 500).to_string())];
        if let Some(min_year) = request.min_year {
            extra.push(("fromYear", min_year.to_string()));
        }
        if let Some(max_year) = request.max_year {
            extra.push(("toYear", max_year.to_string()));
        }
        if let Some(genre) = request
            .genre_name
            .as_deref()
            .filter(|genre| !genre.trim().is_empty())
        {
            extra.push(("genre", genre.to_string()));
        } else if let Some(genre_id) = request.genre_id.as_ref() {
            extra.push(("genre", raw_item_id(genre_id.as_str()).to_string()));
        }

        let body: RandomSongsBody = self.get_json("getRandomSongs", &extra).await?;
        Ok(body
            .random_songs
            .map(|songs| songs.song)
            .unwrap_or_default()
            .into_iter()
            .map(|song| track_from_dto(self, song))
            .collect())
    }

    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        let artists = self.get_all_artists().await?;
        Ok(page(artists, request))
    }

    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        self.artists(request).await
    }

    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>> {
        let body: GenresBody = self.get_json("getGenres", &[]).await?;
        let mut genres = body
            .genres
            .genre
            .into_iter()
            .map(|genre| genre_from_dto(self, genre))
            .collect::<Vec<_>>();
        genres.sort_by_key(|genre| genre.name.to_lowercase());
        Ok(page(genres, request))
    }

    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>> {
        let body: PlaylistsBody = self.get_json("getPlaylists", &[]).await?;
        let mut playlists = body
            .playlists
            .map(|playlists| playlists.playlist)
            .unwrap_or_default()
            .into_iter()
            .map(|playlist| playlist_from_dto(self, playlist))
            .collect::<Vec<_>>();
        playlists.sort_by_key(|playlist| playlist.name.to_lowercase());
        Ok(page(playlists, request))
    }

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> ProviderResult<PlaylistDetail> {
        let body: PlaylistBody = self
            .get_json(
                "getPlaylist",
                &[("id", raw_item_id(playlist_id.as_str()).to_string())],
            )
            .await?;
        let playlist = playlist_from_dto(self, body.playlist.clone());
        let entries = body
            .playlist
            .entry
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, song)| PlaylistEntry {
                entry_id: playlist_entry_id(&playlist.id, index, raw_id_string(&song.id).as_str()),
                track: track_from_dto(self, song),
            })
            .collect::<Vec<_>>();
        let tracks = entries.iter().map(|entry| entry.track.clone()).collect();
        Ok(PlaylistDetail {
            playlist,
            tracks,
            entries,
        })
    }

    async fn genre_detail(&self, genre_id: &GenreId) -> ProviderResult<GenreDetail> {
        let genre_name = raw_item_id(genre_id.as_str()).to_string();
        let tracks = self.songs_by_genre(&genre_name).await?;
        let mut albums = HashMap::<AlbumId, Album>::new();
        for track in &tracks {
            albums
                .entry(track.album_id.clone())
                .or_insert_with(|| Album {
                    id: track.album_id.clone(),
                    title: track.album.clone(),
                    artist: track.artist.clone(),
                    artist_id: track.artist_id.clone(),
                    album_artist_credits: Vec::new(),
                    artist_credits: Vec::new(),
                    year: track.year,
                    release_date: track.release_date.clone(),
                    date_added: track.date_added.clone(),
                    last_played: track.last_played.clone(),
                    play_count: track.play_count,
                    user_rating: track.user_rating,
                    track_count: 0,
                    duration_seconds: 0,
                    favorite: false,
                    color_seed: color_seed(track.album_id.as_str()),
                    image_ref: track.image_ref.clone(),
                    genres: track.genres.clone(),
                });
        }
        let genre = Genre {
            id: genre_id.clone(),
            name: genre_name,
            album_count: albums.len() as u32,
            track_count: tracks.len() as u32,
            image_ref: None,
        };
        Ok(GenreDetail {
            genre,
            albums: albums.into_values().collect(),
            tracks,
        })
    }

    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track> {
        let body: SongBody = self
            .get_json(
                "getSong",
                &[("id", raw_item_id(track_id.as_str()).to_string())],
            )
            .await?;
        Ok(track_from_dto(self, body.song))
    }

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor> {
        self.stream_with_request(&StreamRequest::original(track_id.clone()))
            .await
    }

    async fn stream_with_request(
        &self,
        request: &StreamRequest,
    ) -> ProviderResult<StreamDescriptor> {
        let mut extra = vec![("id", raw_item_id(request.track_id.as_str()).to_string())];
        if let Some(kbps) = request.quality.max_bitrate_kbps() {
            extra.push(("maxBitRate", kbps.to_string()));
        }
        let url = self.authenticated_url("stream", &extra)?;
        let redacted = redacted_subsonic_url(&url);
        Ok(StreamDescriptor::with_redacted(url.to_string(), redacted))
    }

    async fn search(&self, query: &str) -> ProviderResult<SearchResults> {
        let body: SearchBody = self
            .get_json(
                "search3",
                &[
                    ("query", query.to_string()),
                    ("artistCount", "25".to_string()),
                    ("artistOffset", "0".to_string()),
                    ("albumCount", "25".to_string()),
                    ("albumOffset", "0".to_string()),
                    ("songCount", "50".to_string()),
                    ("songOffset", "0".to_string()),
                ],
            )
            .await?;
        let result = body.search_result.unwrap_or_default();
        Ok(SearchResults {
            albums: result
                .album
                .unwrap_or_default()
                .into_iter()
                .map(|album| album_from_dto(self, album))
                .collect(),
            tracks: result
                .song
                .unwrap_or_default()
                .into_iter()
                .map(|song| track_from_dto(self, song))
                .collect(),
            artists: result
                .artist
                .unwrap_or_default()
                .into_iter()
                .map(|artist| artist_from_dto(self, artist))
                .collect(),
            playlists: Vec::new(),
        })
    }

    async fn image_metadata(
        &self,
        item_id: &str,
        kind: ImageKind,
    ) -> ProviderResult<ImageMetadata> {
        let url =
            self.authenticated_url("getCoverArt", &[("id", raw_item_id(item_id).to_string())])?;
        Ok(ImageMetadata {
            item_id: item_id.to_string(),
            kind,
            tag: None,
            url: url.to_string(),
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> ProviderResult<ImageBytes> {
        let mut extra = vec![("id", raw_item_id(&request.item_id).to_string())];
        if request.size > 0 {
            extra.push(("size", request.size.to_string()));
        }
        let url = self.authenticated_url("getCoverArt", &extra)?;
        subsonic_bytes(self.client.get(url)).await
    }

    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> ProviderResult<()> {
        let method = if favorite { "star" } else { "unstar" };
        let key = match &item_id {
            FavoriteItemId::Album(_) => "albumId",
            FavoriteItemId::Track(_) => "id",
            FavoriteItemId::Artist(_) => "artistId",
        };
        self.get_unit(method, &[(key, raw_item_id(item_id.as_str()).to_string())])
            .await
    }

    async fn create_playlist(
        &self,
        name: &str,
        track_ids: &[TrackId],
    ) -> ProviderResult<PlaylistId> {
        let mut extra = vec![("name", name.trim().to_string())];
        extra.extend(
            track_ids
                .iter()
                .map(|track_id| ("songId", raw_item_id(track_id.as_str()).to_string())),
        );
        let body: PlaylistBody = self.get_json("createPlaylist", &extra).await?;
        Ok(PlaylistId::new(
            self.id("playlist", &raw_id_string(&body.playlist.id)),
        ))
    }

    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> ProviderResult<()> {
        self.get_unit(
            "updatePlaylist",
            &[
                ("playlistId", raw_item_id(playlist_id.as_str()).to_string()),
                ("name", name.trim().to_string()),
            ],
        )
        .await
    }

    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> ProviderResult<()> {
        let mut ids = self.playlist_track_ids(playlist_id).await?;
        ids.extend_from_slice(track_ids);
        self.replace_playlist_tracks(playlist_id, &ids).await
    }

    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> ProviderResult<()> {
        let remove = entry_ids.iter().cloned().collect::<HashSet<_>>();
        let ids = self
            .playlist_detail(playlist_id)
            .await?
            .entries
            .into_iter()
            .filter(|entry| !remove.contains(&entry.entry_id))
            .map(|entry| entry.track.id)
            .collect::<Vec<_>>();
        self.replace_playlist_tracks(playlist_id, &ids).await
    }

    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> ProviderResult<()> {
        let mut entries = self.playlist_detail(playlist_id).await?.entries;
        if let Some(old_index) = entries.iter().position(|entry| entry.entry_id == entry_id) {
            let entry = entries.remove(old_index);
            entries.insert(new_index.min(entries.len()), entry);
        }
        let ids = entries
            .into_iter()
            .map(|entry| entry.track.id)
            .collect::<Vec<_>>();
        self.replace_playlist_tracks(playlist_id, &ids).await
    }

    async fn lyrics(
        &self,
        track_id: &TrackId,
        _allow_remote: bool,
    ) -> ProviderResult<Option<Lyrics>> {
        let track = self.track(track_id).await?;
        let body: LyricsBody = self
            .get_json(
                "getLyrics",
                &[
                    ("artist", track.artist.clone()),
                    ("title", track.title.clone()),
                ],
            )
            .await?;
        let Some(lyrics) = body.lyrics else {
            return Ok(None);
        };
        let Some(value) = lyrics.value.filter(|value| !value.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some(Lyrics {
            track_id: track_id.clone(),
            source: LyricsSource::Server,
            lines: value
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| LyricLine {
                    text: line.trim().to_string(),
                    start_millis: None,
                })
                .collect(),
        }))
    }

    async fn report_playback(&self, report: PlaybackReport) -> ProviderResult<()> {
        match report.kind {
            PlaybackReportKind::Started => {
                self.get_unit(
                    "scrobble",
                    &[
                        ("id", raw_item_id(report.track_id.as_str()).to_string()),
                        ("submission", "false".to_string()),
                    ],
                )
                .await
            }
            PlaybackReportKind::Stopped => {
                self.get_unit(
                    "scrobble",
                    &[
                        ("id", raw_item_id(report.track_id.as_str()).to_string()),
                        ("submission", (!report.failed).to_string()),
                    ],
                )
                .await
            }
            PlaybackReportKind::Progress => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
struct SubsonicCredential {
    salt: String,
    token: String,
}

impl SubsonicCredential {
    fn from_password(password: &str) -> Self {
        let salt = random_salt();
        let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
        Self { salt, token }
    }

    fn parse(raw: &str) -> ProviderResult<Self> {
        let Some((salt, token)) = raw.split_once(':') else {
            return Err(ProviderError::Other(
                "saved Subsonic credential is invalid".to_string(),
            ));
        };
        if salt.is_empty() || token.is_empty() {
            return Err(ProviderError::Other(
                "saved Subsonic credential is invalid".to_string(),
            ));
        }
        Ok(Self {
            salt: salt.to_string(),
            token: token.to_string(),
        })
    }

    fn serialize(&self) -> String {
        format!("{}:{}", self.salt, self.token)
    }

    fn common_query<'a>(
        &'a self,
        username: &'a str,
        extra: &'a [(&'a str, &'a str)],
    ) -> Vec<(&'a str, &'a str)> {
        let mut query = vec![
            ("u", username),
            ("s", self.salt.as_str()),
            ("t", self.token.as_str()),
            ("v", API_VERSION),
            ("c", CLIENT_NAME),
            ("f", "json"),
        ];
        query.extend_from_slice(extra);
        query
    }
}

#[derive(Debug)]
struct SubsonicApiResponse<T> {
    body: T,
    server_type: Option<String>,
}

async fn subsonic_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> ProviderResult<SubsonicApiResponse<T>> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Subsonic server returned {}",
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

    let envelope = response
        .json::<SubsonicEnvelope<T>>()
        .await
        .map_err(map_reqwest_error)?;
    if envelope.response.status != "ok" {
        let message = envelope
            .response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| format!("Subsonic returned {}", envelope.response.status));
        return Err(ProviderError::Server {
            status: 200,
            message,
        });
    }
    Ok(SubsonicApiResponse {
        body: envelope.response.body,
        server_type: envelope.response.server_type,
    })
}

async fn subsonic_bytes(request: reqwest::RequestBuilder) -> ProviderResult<ImageBytes> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Subsonic server returned {}",
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

fn endpoint(base_url: &Url, method: &str) -> ProviderResult<Url> {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let method = method.trim_end_matches(".view");
    let full_path = if base_path.is_empty() {
        format!("/rest/{method}.view")
    } else {
        format!("{base_path}/rest/{method}.view")
    };
    url.set_path(&full_path);
    url.set_query(None);
    Ok(url)
}

fn map_reqwest_error(mut error: reqwest::Error) -> ProviderError {
    if let Some(url) = error.url_mut() {
        redact_subsonic_query(url);
    }
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

fn redact_subsonic_query(url: &mut Url) {
    let pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let value = if matches!(key.as_ref(), "p" | "s" | "t") {
                "<redacted>".into()
            } else {
                value
            };
            (key.into_owned(), value.into_owned())
        })
        .collect::<Vec<_>>();
    url.query_pairs_mut().clear().extend_pairs(pairs);
}

fn redacted_subsonic_url(url: &Url) -> String {
    let mut redacted = url.clone();
    redact_subsonic_query(&mut redacted);
    redacted.to_string()
}

fn subsonic_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        lyrics: true,
        playback_reporting: true,
        playlist_mutations: true,
        favorite_mutations: true,
        random_tracks: true,
        music_folders: true,
        ..ProviderCapabilities::default()
    }
}

fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

fn raw_id_string(id: &SubsonicId) -> String {
    id.0.clone()
}

fn playlist_entry_id(playlist_id: &PlaylistId, index: usize, track_id: &str) -> String {
    format!("{}:{index}:{track_id}", playlist_id.as_str())
}

fn page<T>(items: Vec<T>, request: PagedRequest) -> PagedResponse<T> {
    let total = items.len();
    PagedResponse::new(
        items
            .into_iter()
            .skip(request.offset)
            .take(request.limit)
            .collect(),
        total,
    )
}

fn current_year() -> u16 {
    let days_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    year_from_unix_days(days_since_epoch)
}

fn year_from_unix_days(mut days: u64) -> u16 {
    let mut year = 1970_u16;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            return year;
        }
        days -= days_in_year;
        year = year.saturating_add(1);
    }
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn random_salt() -> String {
    let mut bytes = [0_u8; SALT_BYTES];
    if getrandom::fill(&mut bytes).is_err() {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (seed.rotate_left(index as u32) & 0xff) as u8;
        }
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stable_server_id(provider_id: &str, base_url: &str, username: &str) -> String {
    format!(
        "{:016x}",
        stable_hash(&format!("{provider_id}:{base_url}:{username}"))
    )
}

fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}

fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}

fn u16_from_u32(value: Option<u32>) -> u16 {
    value.unwrap_or_default().min(u32::from(u16::MAX)) as u16
}

fn favorite(value: &Option<serde_json::Value>) -> bool {
    value.as_ref().is_some_and(|value| match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => false,
    })
}

fn image_ref(provider: &SubsonicProvider, cover_art: Option<SubsonicId>) -> Option<ImageRef> {
    cover_art.map(|id| ImageRef::new(provider.id("cover", &id.0), None))
}

fn genres_from_item(genre: Option<String>, genres: Vec<GenreName>) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(genre) = genre.filter(|genre| !genre.trim().is_empty()) {
        values.push(genre);
    }
    for genre in genres {
        if !genre.name.trim().is_empty() && !values.iter().any(|value| value == &genre.name) {
            values.push(genre.name);
        }
    }
    values
}

fn album_from_dto(provider: &SubsonicProvider, album: SubsonicAlbum) -> Album {
    let raw_id = raw_id_string(&album.id);
    Album {
        id: AlbumId::new(provider.id("album", &raw_id)),
        title: album
            .title
            .or(album.name)
            .or(album.album)
            .unwrap_or_else(|| "Untitled Album".to_string()),
        artist: album.artist.unwrap_or_else(|| "Unknown Artist".to_string()),
        artist_id: album
            .artist_id
            .map(|id| ArtistId::new(provider.id("artist", &id.0))),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: u16_from_option(album.year),
        release_date: album
            .year
            .map(|year| format!("{}-01-01", year.clamp(0, i32::from(u16::MAX)))),
        date_added: normalized_date(album.created),
        last_played: normalized_date(album.played),
        play_count: album
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: album
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        track_count: u16_from_u32(album.song_count),
        duration_seconds: album.duration.unwrap_or_default(),
        favorite: favorite(&album.starred),
        color_seed: color_seed(&raw_id),
        image_ref: image_ref(provider, album.cover_art),
        genres: genres_from_item(album.genre, album.genres),
    }
}

fn track_from_dto(provider: &SubsonicProvider, song: SubsonicSong) -> Track {
    let raw_id = raw_id_string(&song.id);
    let album_id = song
        .album_id
        .as_ref()
        .or(song.parent.as_ref())
        .map(raw_id_string)
        .unwrap_or_else(|| raw_id.clone());
    Track {
        id: TrackId::new(provider.id("track", &raw_id)),
        album_id: AlbumId::new(provider.id("album", &album_id)),
        title: song.title.unwrap_or_else(|| "Untitled Track".to_string()),
        artist: song.artist.unwrap_or_else(|| "Unknown Artist".to_string()),
        artist_id: song
            .artist_id
            .map(|id| ArtistId::new(provider.id("artist", &id.0))),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: song.album.unwrap_or_else(|| "Unknown Album".to_string()),
        year: u16_from_option(song.year),
        release_date: song
            .year
            .map(|year| format!("{}-01-01", year.clamp(0, i32::from(u16::MAX)))),
        date_added: normalized_date(song.created),
        last_played: normalized_date(song.played),
        play_count: song
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: song
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        duration_seconds: song.duration.unwrap_or_default(),
        favorite: favorite(&song.starred),
        disc_number: u16_from_option(song.disc_number).max(1),
        track_number: u16_from_option(song.track).max(1),
        image_ref: image_ref(provider, song.cover_art),
        genres: genres_from_item(song.genre, song.genres),
        local_path: song.path,
    }
}

fn artist_from_dto(provider: &SubsonicProvider, artist: SubsonicArtist) -> Artist {
    let raw_id = raw_id_string(&artist.id);
    Artist {
        id: ArtistId::new(provider.id("artist", &raw_id)),
        name: artist.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_count: artist.album_count.unwrap_or_default(),
        track_count: artist.song_count.unwrap_or_default(),
        favorite: favorite(&artist.starred),
        last_played: normalized_date(artist.played),
        play_count: artist
            .play_count
            .map(|value| value.min(u64::from(u32::MAX)) as u32),
        user_rating: artist
            .user_rating
            .map(|value| value.min(u32::from(u8::MAX)) as u8),
        image_ref: image_ref(provider, artist.cover_art),
    }
}

fn genre_from_dto(provider: &SubsonicProvider, genre: SubsonicGenre) -> Genre {
    Genre {
        id: GenreId::new(provider.id("genre", &genre.value)),
        name: genre.value,
        album_count: genre.album_count.unwrap_or_default(),
        track_count: genre.song_count.unwrap_or_default(),
        image_ref: None,
    }
}

fn normalized_date(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.len() >= 10 {
        let prefix = &value[..10];
        if prefix.as_bytes().get(4) == Some(&b'-') && prefix.as_bytes().get(7) == Some(&b'-') {
            return Some(prefix.to_string());
        }
    }
    Some(value)
}

fn playlist_from_dto(provider: &SubsonicProvider, playlist: SubsonicPlaylist) -> Playlist {
    let raw_id = raw_id_string(&playlist.id);
    Playlist {
        id: PlaylistId::new(provider.id("playlist", &raw_id)),
        name: playlist
            .name
            .unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: playlist.song_count.unwrap_or_default(),
        duration_seconds: playlist.duration.unwrap_or_default(),
        image_ref: image_ref(provider, playlist.cover_art),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SubsonicEmpty {}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicEnvelope<T> {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse<T>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicResponse<T> {
    status: String,
    #[serde(default, rename = "type")]
    server_type: Option<String>,
    #[serde(default)]
    error: Option<SubsonicError>,
    #[serde(flatten)]
    body: T,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicError {
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AuthenticateBody {
    user: SubsonicUser,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicUser {
    username: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AlbumListBody {
    #[serde(default, rename = "albumList2")]
    album_list: AlbumList,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AlbumList {
    #[serde(default)]
    album: Vec<SubsonicAlbum>,
}

#[derive(Clone, Debug, Deserialize)]
struct AlbumBody {
    album: SubsonicAlbum,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SearchBody {
    #[serde(default, rename = "searchResult3")]
    search_result: Option<SearchResult>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SearchResult {
    #[serde(default)]
    album: Option<Vec<SubsonicAlbum>>,
    #[serde(default)]
    artist: Option<Vec<SubsonicArtist>>,
    #[serde(default)]
    song: Option<Vec<SubsonicSong>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MusicFoldersBody {
    #[serde(default, rename = "musicFolders")]
    music_folders: MusicFolders,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MusicFolders {
    #[serde(default, rename = "musicFolder")]
    music_folder: Vec<SubsonicMusicFolder>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicMusicFolder {
    id: SubsonicId,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ArtistsBody {
    artists: ArtistsIndex,
}

#[derive(Clone, Debug, Deserialize)]
struct ArtistsIndex {
    #[serde(default)]
    index: Vec<ArtistIndex>,
}

#[derive(Clone, Debug, Deserialize)]
struct ArtistIndex {
    #[serde(default)]
    artist: Vec<SubsonicArtist>,
}

#[derive(Clone, Debug, Deserialize)]
struct GenresBody {
    genres: GenresList,
}

#[derive(Clone, Debug, Deserialize)]
struct GenresList {
    #[serde(default)]
    genre: Vec<SubsonicGenre>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PlaylistsBody {
    #[serde(default)]
    playlists: Option<PlaylistsList>,
}

#[derive(Clone, Debug, Deserialize)]
struct PlaylistsList {
    #[serde(default)]
    playlist: Vec<SubsonicPlaylist>,
}

#[derive(Clone, Debug, Deserialize)]
struct PlaylistBody {
    playlist: SubsonicPlaylist,
}

#[derive(Clone, Debug, Deserialize)]
struct SongBody {
    song: SubsonicSong,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RandomSongsBody {
    #[serde(default, rename = "randomSongs")]
    random_songs: Option<SongsList>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SongsByGenreBody {
    #[serde(default, rename = "songsByGenre")]
    songs_by_genre: Option<SongsList>,
}

#[derive(Clone, Debug, Deserialize)]
struct SongsList {
    #[serde(default)]
    song: Vec<SubsonicSong>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LyricsBody {
    #[serde(default)]
    lyrics: Option<SubsonicLyrics>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicLyrics {
    #[serde(default)]
    value: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicAlbum {
    id: SubsonicId,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default, rename = "artistId")]
    artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    genres: Vec<GenreName>,
    #[serde(default)]
    song: Vec<SubsonicSong>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicSong {
    id: SubsonicId,
    #[serde(default)]
    parent: Option<SubsonicId>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default, rename = "albumId")]
    album_id: Option<SubsonicId>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default, rename = "artistId")]
    artist_id: Option<SubsonicId>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    track: Option<i32>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    genres: Vec<GenreName>,
    #[serde(default, rename = "discNumber")]
    disc_number: Option<i32>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicArtist {
    id: SubsonicId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "albumCount")]
    album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    played: Option<String>,
    #[serde(default, rename = "playCount")]
    play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    user_rating: Option<u32>,
    #[serde(default)]
    starred: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicGenre {
    #[serde(default, alias = "name")]
    value: String,
    #[serde(default, rename = "albumCount")]
    album_count: Option<u32>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct SubsonicPlaylist {
    id: SubsonicId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<SubsonicId>,
    #[serde(default, rename = "songCount")]
    song_count: Option<u32>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    entry: Option<Vec<SubsonicSong>>,
}

#[derive(Clone, Debug, Deserialize)]
struct GenreName {
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SubsonicId(String);

impl<'de> Deserialize<'de> for SubsonicId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SubsonicIdVisitor)
    }
}

struct SubsonicIdVisitor;

impl Visitor<'_> for SubsonicIdVisitor {
    type Value = SubsonicId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string or numeric Subsonic id")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(SubsonicId(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_provider::MusicProvider;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn login_uses_salted_token_auth_and_maps_session() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getUser.view"))
            .and(query_param("u", "demo"))
            .and(query_param("username", "demo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "type": "Navidrome",
                    "user": { "username": "demo" }
                }
            })))
            .mount(&server)
            .await;

        let session = SubsonicProvider::login(SubsonicLoginRequest {
            base_url: server.uri(),
            username: "demo".to_string(),
            password: "pw".to_string(),
            trust_invalid_cert: false,
            flavor: SubsonicFlavor::Navidrome,
        })
        .await
        .expect("login");

        assert!(session.server.id.as_str().starts_with("navidrome:server:"));
        assert_eq!(session.server.provider, "navidrome");
        assert_eq!(session.server.name, "Navidrome");
        assert_eq!(session.username, "demo");
        assert!(session.access_token.contains(':'));
        assert!(!session.access_token.contains("pw"));
    }

    #[tokio::test]
    async fn albums_map_subsonic_album_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getAlbumList2.view"))
            .and(query_param("type", "alphabeticalByName"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "albumList2": {
                        "album": [{
                            "id": "album-one",
                            "name": "Blue Rooms",
                            "artist": "Astral Kin",
                            "artistId": "artist-one",
                            "songCount": 8,
                            "duration": 1800,
                            "year": 2024,
                            "genre": "Ambient",
                            "coverArt": "cover-one",
                            "created": "2024-03-02T09:10:11Z",
                            "played": "2024-04-02T09:10:11Z",
                            "playCount": 12,
                            "userRating": 5,
                            "starred": "2024-01-01T00:00:00Z"
                        }]
                    }
                }
            })))
            .mount(&server)
            .await;
        let provider = provider(&server);

        let page = provider
            .albums(PagedRequest::new(0, 50))
            .await
            .expect("albums");

        assert_eq!(page.items[0].id.as_str(), "subsonic:album:album-one");
        assert_eq!(page.items[0].title, "Blue Rooms");
        assert_eq!(
            page.items[0].artist_id.as_ref().map(ArtistId::as_str),
            Some("subsonic:artist:artist-one")
        );
        assert_eq!(
            page.items[0]
                .image_ref
                .as_ref()
                .map(|image| image.item_id.as_str()),
            Some("subsonic:cover:cover-one")
        );
        assert_eq!(page.items[0].release_date.as_deref(), Some("2024-01-01"));
        assert_eq!(page.items[0].date_added.as_deref(), Some("2024-03-02"));
        assert_eq!(page.items[0].last_played.as_deref(), Some("2024-04-02"));
        assert_eq!(page.items[0].play_count, Some(12));
        assert_eq!(page.items[0].user_rating, Some(5));
        assert!(page.items[0].favorite);
    }

    #[tokio::test]
    async fn album_detail_maps_subsonic_song_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getAlbum.view"))
            .and(query_param("id", "album-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "album": {
                        "id": "album-one",
                        "name": "Blue Rooms",
                        "artist": "Astral Kin",
                        "artistId": "artist-one",
                        "songCount": 1,
                        "duration": 210,
                        "year": 2024,
                        "song": [{
                            "id": "track-one",
                            "albumId": "album-one",
                            "title": "First Motion",
                            "artist": "Astral Kin",
                            "artistId": "artist-one",
                            "album": "Blue Rooms",
                            "year": 2024,
                            "duration": 210,
                            "discNumber": 1,
                            "track": 1,
                            "created": "2024-03-03T09:10:11Z",
                            "played": "2024-04-03T09:10:11Z",
                            "playCount": 7,
                            "userRating": 4
                        }]
                    }
                }
            })))
            .mount(&server)
            .await;
        let provider = provider(&server);

        let detail = provider
            .album_detail(&AlbumId::new("subsonic:album:album-one"))
            .await
            .expect("detail");

        assert_eq!(detail.tracks[0].id.as_str(), "subsonic:track:track-one");
        assert_eq!(detail.tracks[0].release_date.as_deref(), Some("2024-01-01"));
        assert_eq!(detail.tracks[0].date_added.as_deref(), Some("2024-03-03"));
        assert_eq!(detail.tracks[0].last_played.as_deref(), Some("2024-04-03"));
        assert_eq!(detail.tracks[0].play_count, Some(7));
        assert_eq!(detail.tracks[0].user_rating, Some(4));
    }

    #[tokio::test]
    async fn random_tracks_use_subsonic_random_song_filters() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getRandomSongs.view"))
            .and(query_param("size", "37"))
            .and(query_param("fromYear", "1999"))
            .and(query_param("toYear", "2001"))
            .and(query_param("genre", "Ambient"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "randomSongs": {
                        "song": [{
                            "id": "track-one",
                            "albumId": "album-one",
                            "title": "First Motion",
                            "artist": "Astral Kin",
                            "album": "Blue Rooms",
                            "year": 2000,
                            "duration": 210,
                            "genre": "Ambient"
                        }]
                    }
                }
            })))
            .mount(&server)
            .await;
        let provider = provider(&server);

        let tracks = provider
            .random_tracks(RandomTrackRequest {
                limit: 37,
                min_year: Some(1999),
                max_year: Some(2001),
                genre_id: Some(GenreId::new("subsonic:genre:ambient")),
                genre_name: Some("Ambient".to_string()),
                played_filter: PlayedFilter::All,
            })
            .await
            .expect("random tracks");

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id.as_str(), "subsonic:track:track-one");
        assert_eq!(tracks[0].genres, vec!["Ambient".to_string()]);
    }

    #[tokio::test]
    async fn random_tracks_reject_played_filter_for_subsonic() {
        let server = MockServer::start().await;
        let provider = provider(&server);

        let error = provider
            .random_tracks(RandomTrackRequest {
                limit: 10,
                min_year: None,
                max_year: None,
                genre_id: None,
                genre_name: None,
                played_filter: PlayedFilter::Played,
            })
            .await
            .expect_err("unsupported played filter");

        assert!(matches!(error, ProviderError::Unsupported(_)));
    }

    #[tokio::test]
    async fn stream_url_redacts_subsonic_credentials() {
        let server = MockServer::start().await;
        let provider = provider(&server);

        let stream = provider
            .stream(&TrackId::new("subsonic:track:track-one"))
            .await
            .expect("stream");

        assert!(stream.uri().contains("t=token"));
        assert!(stream.redacted_uri().contains("t=%3Credacted%3E"));
        assert!(!stream.redacted_uri().contains("token"));
    }

    #[tokio::test]
    async fn stream_url_includes_max_bitrate_when_limited() {
        let server = MockServer::start().await;
        let provider = provider(&server);

        let stream = provider
            .stream_with_request(&rufin_provider::StreamRequest::new(
                TrackId::new("subsonic:track:track-one"),
                rufin_core::StreamQuality::MaxBitrateKbps(192),
            ))
            .await
            .expect("stream");

        assert!(stream.uri().contains("maxBitRate=192"));
        assert!(stream.redacted_uri().contains("maxBitRate=192"));
        assert!(!stream.redacted_uri().contains("token"));
    }

    #[tokio::test]
    async fn image_bytes_fetch_cover_art() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getCoverArt.view"))
            .and(query_param("id", "cover-one"))
            .and(query_param("size", "256"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(vec![1_u8, 2, 3]),
            )
            .mount(&server)
            .await;
        let provider = provider(&server);

        let image = provider
            .image_bytes(ImageRequest {
                item_id: "subsonic:cover:cover-one".to_string(),
                kind: ImageKind::Primary,
                tag: None,
                size: 256,
            })
            .await
            .expect("image bytes");

        assert_eq!(image.bytes, vec![1, 2, 3]);
        assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
    }

    #[tokio::test]
    async fn music_folders_load_subsonic_music_folders() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/getMusicFolders.view"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "musicFolders": {
                        "musicFolder": [
                            { "id": 1, "name": "Music" }
                        ]
                    }
                }
            })))
            .mount(&server)
            .await;
        let provider = provider(&server);

        let folders = provider.music_folders().await.expect("folders");

        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id.as_str(), "subsonic:music-folder:1");
        assert_eq!(folders[0].name, "Music");
    }

    #[tokio::test]
    async fn tracks_in_music_folder_passes_music_folder_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/search3.view"))
            .and(query_param("musicFolderId", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "subsonic-response": {
                    "status": "ok",
                    "version": "1.16.1",
                    "searchResult3": {
                        "song": [{
                            "id": "track-one",
                            "title": "First Motion",
                            "album": "Blue Rooms",
                            "albumId": "album-one",
                            "artist": "Astral Kin",
                            "artistId": "artist-one",
                            "duration": 210
                        }]
                    }
                }
            })))
            .mount(&server)
            .await;
        let provider = provider(&server);

        let page = provider
            .tracks_in_music_folder(
                &MusicFolderId::new("subsonic:music-folder:1"),
                PagedRequest::new(0, 50),
            )
            .await
            .expect("tracks");

        assert_eq!(page.items[0].id.as_str(), "subsonic:track:track-one");
    }

    fn provider(server: &MockServer) -> SubsonicProvider {
        SubsonicProvider::from_saved_session(SavedProviderSession {
            server: ServerIdentity {
                id: ServerId::new("subsonic:server:test"),
                provider: "subsonic".to_string(),
                name: "Subsonic".to_string(),
                base_url: server.uri(),
            },
            user_id: "demo".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
            access_token: "salt:token".to_string(),
        })
        .expect("provider")
    }
}
