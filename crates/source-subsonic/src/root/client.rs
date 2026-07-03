use super::*;

use source::remote_http::{self, BodyLimit, RemoteHttpPolicy, RemoteTimeouts};
use std::time::Duration;

const SUBSONIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SUBSONIC_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const SUBSONIC_JSON_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const SUBSONIC_IMAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const SUBSONIC_ERROR_BODY_MAX_BYTES: usize = 64 * 1024;
const SUBSONIC_HTTP: RemoteHttpPolicy = RemoteHttpPolicy {
    auth_context: "Subsonic server returned",
    error_body: BodyLimit {
        max_bytes: SUBSONIC_ERROR_BODY_MAX_BYTES,
        context: "Subsonic error response",
    },
    redact_error_url: Some(redact_subsonic_query),
};

#[async_trait(?Send)]
impl MusicSource for SubsonicSource {
    fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    async fn home_sections(&self) -> SourceResult<Vec<HomeSection>> {
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

    async fn home_section(&self, kind: HomeSectionKind) -> SourceResult<HomeSection> {
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

    async fn albums(&self, request: PagedRequest) -> SourceResult<PagedResponse<Album>> {
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

    async fn album_detail(&self, album_id: &AlbumId) -> SourceResult<AlbumDetail> {
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

    async fn tracks(&self, request: PagedRequest) -> SourceResult<PagedResponse<Track>> {
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

    async fn music_folders(&self) -> SourceResult<Vec<MusicFolder>> {
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
    ) -> SourceResult<PagedResponse<Track>> {
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

    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<FolderDetail> {
        let Some(folder_id) = folder_id else {
            let mut extra = Vec::new();
            if let Some(music_folder_id) = music_folder_id {
                extra.push((
                    "musicFolderId",
                    raw_item_id(music_folder_id.as_str()).to_string(),
                ));
            }
            let body: IndexesBody = self.get_json("getIndexes", &extra).await?;
            let mut folders = body
                .indexes
                .map(|indexes| indexes.index)
                .unwrap_or_default()
                .into_iter()
                .flat_map(|index| index.artist)
                .map(|artist| folder_from_artist(self, artist))
                .collect::<Vec<_>>();
            sort_folders_by_name(&mut folders);
            return Ok(FolderDetail {
                folder: Folder {
                    id: FolderId::new(self.id("folder", "root")),
                    name: "Folders".to_string(),
                },
                parent_id: None,
                folders,
                tracks: Vec::new(),
            });
        };

        let body: MusicDirectoryBody = self
            .get_json(
                "getMusicDirectory",
                &[("id", raw_item_id(folder_id.as_str()).to_string())],
            )
            .await?;
        let directory = body.directory;
        let folder = folder_from_directory(self, &directory);
        let parent_id = directory
            .parent
            .as_ref()
            .map(|id| FolderId::new(self.id("folder", id.0.as_str())));
        let mut folders = Vec::new();
        let mut tracks = Vec::new();
        for child in directory.child {
            if child.is_dir.unwrap_or(false) {
                folders.push(folder_from_child(self, child));
            } else {
                tracks.push(track_from_dto(self, child));
            }
        }
        sort_folders_by_name(&mut folders);
        Ok(FolderDetail {
            folder,
            parent_id,
            folders,
            tracks,
        })
    }

    async fn random_tracks(&self, request: RandomTrackRequest) -> SourceResult<Vec<Track>> {
        if request.played_filter != PlayedFilter::All {
            return Err(SourceError::Unsupported("random played filter"));
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

    async fn generated_tracks(&self, request: GeneratedTracksRequest) -> SourceResult<Vec<Track>> {
        match request.seed {
            GeneratedTrackSeed::Track(track_id) => {
                self.similar_songs(raw_item_id(track_id.as_str()), request.limit)
                    .await
            }
            GeneratedTrackSeed::Album(album_id) => {
                self.similar_songs(raw_item_id(album_id.as_str()), request.limit)
                    .await
            }
            GeneratedTrackSeed::Artist(artist_id) => {
                self.similar_songs2(raw_item_id(artist_id.as_str()), request.limit)
                    .await
            }
            GeneratedTrackSeed::Genre { id, name } => {
                self.random_tracks(RandomTrackRequest {
                    limit: request.limit,
                    min_year: None,
                    max_year: None,
                    genre_id: id,
                    genre_name: (!name.trim().is_empty()).then_some(name),
                    played_filter: PlayedFilter::All,
                })
                .await
            }
            GeneratedTrackSeed::Playlist(playlist_id) => {
                let detail = self.playlist_detail(&playlist_id).await?;
                let seed = detail.tracks.first().ok_or(SourceError::NotFound)?;
                self.similar_songs(raw_item_id(seed.id.as_str()), request.limit)
                    .await
            }
        }
    }

    async fn artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>> {
        let artists = self.get_all_artists().await?;
        Ok(page(artists, request))
    }

    async fn album_artists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Artist>> {
        self.artists(request).await
    }

    async fn genres(&self, request: PagedRequest) -> SourceResult<PagedResponse<Genre>> {
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

    async fn playlists(&self, request: PagedRequest) -> SourceResult<PagedResponse<Playlist>> {
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

    async fn playlist_detail(&self, playlist_id: &PlaylistId) -> SourceResult<PlaylistDetail> {
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

    async fn genre_detail(&self, genre_id: &GenreId) -> SourceResult<GenreDetail> {
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
                    release_types: Vec::new(),
                    is_compilation: None,
                    musicbrainz_album_id: None,
                    musicbrainz_release_group_id: None,
                });
        }
        let genre = Genre {
            id: genre_id.clone(),
            name: genre_name,
            album_count: albums.len() as u32,
            track_count: tracks.len() as u32,
            duration_seconds: tracks.iter().fold(0_u32, |total, track| {
                total.saturating_add(track.duration_seconds)
            }),
            image_refs: Vec::new(),
            image_ref: None,
        };
        Ok(GenreDetail {
            genre,
            albums: albums.into_values().collect(),
            tracks,
        })
    }

    async fn track(&self, track_id: &TrackId) -> SourceResult<Track> {
        let body: SongBody = self
            .get_json(
                "getSong",
                &[("id", raw_item_id(track_id.as_str()).to_string())],
            )
            .await?;
        Ok(track_from_dto(self, body.song))
    }

    async fn stream(&self, track_id: &TrackId) -> SourceResult<StreamDescriptor> {
        self.stream_with_request(&StreamRequest::original(track_id.clone()))
            .await
    }

    async fn stream_with_request(&self, request: &StreamRequest) -> SourceResult<StreamDescriptor> {
        let mut extra = vec![("id", raw_item_id(request.track_id.as_str()).to_string())];
        if let Some(kbps) = request.quality.max_bitrate_kbps() {
            extra.push(("maxBitRate", kbps.to_string()));
        }
        let url = self.authenticated_url("stream", &extra)?;
        let redacted = redacted_subsonic_url(&url);
        Ok(StreamDescriptor::with_redacted(url.to_string(), redacted))
    }

    async fn search(&self, query: &str) -> SourceResult<SearchResults> {
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

    async fn image_metadata(&self, item_id: &str, kind: ImageKind) -> SourceResult<ImageMetadata> {
        let url =
            self.authenticated_url("getCoverArt", &[("id", raw_item_id(item_id).to_string())])?;
        Ok(ImageMetadata {
            item_id: item_id.to_string(),
            kind,
            tag: None,
            url: url.to_string(),
        })
    }

    async fn image_bytes(&self, request: ImageRequest) -> SourceResult<ImageBytes> {
        let mut extra = vec![("id", raw_item_id(&request.item_id).to_string())];
        if request.size > 0 {
            extra.push(("size", request.size.to_string()));
        }
        let url = self.authenticated_url("getCoverArt", &extra)?;
        subsonic_bytes(self.client.get(url)).await
    }

    async fn set_favorite(&self, item_id: FavoriteItemId, favorite: bool) -> SourceResult<()> {
        let method = if favorite { "star" } else { "unstar" };
        let key = match &item_id {
            FavoriteItemId::Album(_) => "albumId",
            FavoriteItemId::Track(_) => "id",
            FavoriteItemId::Artist(_) => "artistId",
        };
        self.get_unit(method, &[(key, raw_item_id(item_id.as_str()).to_string())])
            .await
    }

    async fn create_playlist(&self, name: &str, track_ids: &[TrackId]) -> SourceResult<PlaylistId> {
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

    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> SourceResult<()> {
        self.get_unit(
            "updatePlaylist",
            &[
                ("playlistId", raw_item_id(playlist_id.as_str()).to_string()),
                ("name", name.trim().to_string()),
            ],
        )
        .await
    }

    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> SourceResult<()> {
        self.get_unit(
            "deletePlaylist",
            &[("id", raw_item_id(playlist_id.as_str()).to_string())],
        )
        .await
    }

    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()> {
        let mut ids = self.playlist_track_ids(playlist_id).await?;
        ids.extend_from_slice(track_ids);
        self.replace_playlist_tracks(playlist_id, &ids).await
    }

    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> SourceResult<()> {
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
    ) -> SourceResult<()> {
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
    ) -> SourceResult<Option<Lyrics>> {
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
            external_provider: None,
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

    async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()> {
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
pub(super) struct SubsonicCredential {
    pub(super) salt: String,
    pub(super) token: String,
}
impl SubsonicCredential {
    pub(super) fn from_password(password: &str) -> Self {
        let salt = random_salt();
        let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
        Self { salt, token }
    }

    pub(super) fn parse(raw: &str) -> SourceResult<Self> {
        let Some((salt, token)) = raw.split_once(':') else {
            return Err(SourceError::Other(
                "saved Subsonic credential is invalid".to_string(),
            ));
        };
        if salt.is_empty() || token.is_empty() {
            return Err(SourceError::Other(
                "saved Subsonic credential is invalid".to_string(),
            ));
        }
        Ok(Self {
            salt: salt.to_string(),
            token: token.to_string(),
        })
    }

    pub(super) fn serialize(&self) -> String {
        format!("{}:{}", self.salt, self.token)
    }

    pub(super) fn common_query<'a>(
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
pub(super) struct SubsonicApiResponse<T> {
    pub(super) body: T,
    pub(super) server_type: Option<String>,
}
pub(super) async fn subsonic_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> SourceResult<SubsonicApiResponse<T>> {
    let envelope = remote_http::json::<SubsonicEnvelope<T>>(
        request,
        SUBSONIC_HTTP,
        BodyLimit {
            max_bytes: SUBSONIC_JSON_MAX_BYTES,
            context: "Subsonic JSON response",
        },
    )
    .await?;
    if envelope.response.status != "ok" {
        let message = envelope
            .response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| format!("Subsonic returned {}", envelope.response.status));
        return Err(SourceError::Server {
            status: 200,
            message,
        });
    }
    Ok(SubsonicApiResponse {
        body: envelope.response.body,
        server_type: envelope.response.server_type,
    })
}
pub(super) async fn subsonic_bytes(request: reqwest::RequestBuilder) -> SourceResult<ImageBytes> {
    remote_http::bytes(
        request,
        SUBSONIC_HTTP,
        BodyLimit {
            max_bytes: SUBSONIC_IMAGE_MAX_BYTES,
            context: "Subsonic image response",
        },
    )
    .await
}
pub(super) fn build_client(trust_invalid_cert: bool) -> SourceResult<Client> {
    build_client_with_timeouts(
        trust_invalid_cert,
        SUBSONIC_CONNECT_TIMEOUT,
        SUBSONIC_REQUEST_TIMEOUT,
    )
}

pub(super) fn build_client_with_timeouts(
    trust_invalid_cert: bool,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> SourceResult<Client> {
    remote_http::build_client(
        trust_invalid_cert,
        RemoteTimeouts {
            connect: connect_timeout,
            request: request_timeout,
        },
        SUBSONIC_HTTP,
    )
}
pub(super) fn normalize_base_url(raw: &str) -> SourceResult<Url> {
    let trimmed = raw.trim().trim_end_matches('/');
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut url = Url::parse(&candidate).map_err(|error| SourceError::Other(error.to_string()))?;
    let path = url.path().trim_end_matches('/').to_string();
    let normalized_path = if path.is_empty() {
        "/".to_string()
    } else {
        format!("{path}/")
    };
    url.set_path(&normalized_path);
    Ok(url)
}
pub(super) fn endpoint(base_url: &Url, method: &str) -> SourceResult<Url> {
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
