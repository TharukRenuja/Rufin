use super::*;

use crate::remote_http::{self, BodyLimit, RemoteHttpPolicy, RemoteTimeouts};
use crate::source::RemotePlaylistSource;
use serde::{
    Deserialize,
    de::{self, DeserializeOwned, Visitor},
};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SUBSONIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SUBSONIC_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const SUBSONIC_JSON_MAX_BYTES: usize = 64 * 1024 * 1024;
pub(super) const SUBSONIC_IMAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const SUBSONIC_ERROR_BODY_MAX_BYTES: usize = 64 * 1024;
const SUBSONIC_HTTP: RemoteHttpPolicy = RemoteHttpPolicy {
    service: "opensubsonic",
    auth_context: "Subsonic server returned",
    error_body: BodyLimit {
        max_bytes: SUBSONIC_ERROR_BODY_MAX_BYTES,
        context: "Subsonic error response",
    },
    redact_error_url: Some(redact_subsonic_query),
};

impl SubsonicSource {
    pub(crate) async fn search(
        &self,
        request: &library::SearchRequest,
    ) -> SourceResult<library::SearchResults> {
        if request.query().trim().is_empty() {
            return Ok(library::SearchResults::default());
        }
        let count = request.limit().to_string();
        let body: SearchBody = self
            .get_json(
                "search3",
                &[
                    ("query", request.query().to_string()),
                    ("artistCount", count.clone()),
                    ("artistOffset", "0".to_string()),
                    ("albumCount", count.clone()),
                    ("albumOffset", "0".to_string()),
                    ("songCount", count),
                    ("songOffset", "0".to_string()),
                ],
            )
            .await?;
        let results = body.search_result.unwrap_or_default();
        Ok(library::SearchResults {
            artists: results
                .artist
                .unwrap_or_default()
                .into_iter()
                .map(|artist| artist_from_dto(self, artist))
                .collect(),
            albums: results
                .album
                .unwrap_or_default()
                .into_iter()
                .map(|album| album_from_dto(self, album))
                .collect(),
            tracks: results
                .song
                .unwrap_or_default()
                .into_iter()
                .map(|song| track_from_dto(self, song))
                .collect(),
        })
    }

    pub(crate) async fn read_track(&self, track_id: &TrackId) -> SourceResult<Track> {
        let body: SongBody = self
            .get_json(
                "getSong",
                &[("id", raw_item_id(track_id.as_str()).to_string())],
            )
            .await?;
        Ok(track_from_dto(self, body.song))
    }
}

impl SubsonicSource {
    pub(crate) async fn read_folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<library::FolderContents> {
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
            return Ok(folder_contents(folders, Vec::new()));
        };

        let body: MusicDirectoryBody = self
            .get_json(
                "getMusicDirectory",
                &[("id", raw_item_id(folder_id.as_str()).to_string())],
            )
            .await?;
        let directory = body.directory;
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
        Ok(folder_contents(folders, tracks))
    }
}

fn folder_contents(folders: Vec<Folder>, tracks: Vec<Track>) -> library::FolderContents {
    library::FolderContents {
        folders: folders.into(),
        tracks: tracks.into(),
    }
}

impl SubsonicSource {
    pub(crate) async fn random_tracks(
        &self,
        request: RandomTrackRequest,
    ) -> SourceResult<Vec<Track>> {
        if request.played_filter != PlayedFilter::All {
            return Err(SourceError::InvalidRequest("random played filter"));
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
}

impl SubsonicSource {
    pub(crate) async fn generated_tracks(
        &self,
        request: GeneratedTracksRequest,
    ) -> SourceResult<Vec<Track>> {
        match request.seed {
            library::RadioSeed::Track(track_id) => {
                self.similar_songs(raw_item_id(track_id.as_str()), request.limit)
                    .await
            }
            library::RadioSeed::Album(album_id) => {
                self.similar_songs(raw_item_id(album_id.as_str()), request.limit)
                    .await
            }
            library::RadioSeed::Artist(artist_id) => {
                self.similar_songs2(raw_item_id(artist_id.as_str()), request.limit)
                    .await
            }
            library::RadioSeed::Genre { id, name } => {
                self.random_tracks(RandomTrackRequest {
                    limit: request.limit,
                    min_year: None,
                    max_year: None,
                    genre_id: Some(id),
                    genre_name: (!name.trim().is_empty()).then_some(name),
                    played_filter: PlayedFilter::All,
                })
                .await
            }
            library::RadioSeed::Playlist(playlist_id) => {
                let snapshot = self.read_playlist(&playlist_id).await?;
                let seed = snapshot.entries.first().ok_or(SourceError::NotFound)?;
                self.similar_songs(raw_item_id(seed.track_id.as_str()), request.limit)
                    .await
            }
        }
    }
}

impl SubsonicSource {
    pub(crate) async fn read_playlist(
        &self,
        playlist_id: &PlaylistId,
    ) -> SourceResult<PlaylistSnapshot> {
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
            .map(|(index, song)| {
                let raw_track_id = raw_id_string(&song.id);
                PlaylistEntry {
                    occurrence_id: playlist_entry_id(&playlist.id, index, &raw_track_id),
                    track_id: TrackId::new(self.id("track", &raw_track_id)),
                }
            })
            .collect::<Vec<_>>();
        Ok(PlaylistSnapshot { playlist, entries })
    }
}

impl SubsonicSource {
    pub(crate) async fn resolve_stream(
        &self,
        request: &StreamRequest,
    ) -> SourceResult<StreamDescriptor> {
        let mut extra = vec![("id", raw_item_id(request.track_id.as_str()).to_string())];
        if let Some(kbps) = request.quality.max_bitrate_kbps() {
            extra.push(("maxBitRate", kbps.to_string()));
        }
        let url = self.authenticated_url("stream", &extra)?;
        let redacted = redacted_subsonic_url(&url);
        Ok(StreamDescriptor::with_redacted(url.to_string(), redacted)
            .with_trust_invalid_certificate(self.trust_invalid_cert))
    }
}

impl SubsonicSource {
    pub(crate) async fn image_bytes(
        &self,
        image_ref: &ImageRef,
        size: u32,
    ) -> SourceResult<ImageBytes> {
        let mut extra = vec![("id", raw_item_id(&image_ref.item_id).to_string())];
        if size > 0 {
            extra.push(("size", size.to_string()));
        }
        let url = self.authenticated_url("getCoverArt", &extra)?;
        subsonic_bytes(self.client.get(url)).await
    }
}

impl SubsonicSource {
    pub(crate) async fn set_favorite(
        &self,
        item_id: FavoriteItemId,
        favorite: bool,
    ) -> SourceResult<()> {
        let method = if favorite { "star" } else { "unstar" };
        let key = match &item_id {
            FavoriteItemId::Album(_) => "albumId",
            FavoriteItemId::Track(_) => "id",
            FavoriteItemId::Artist(_) => "artistId",
        };
        self.get_unit(method, &[(key, raw_item_id(item_id.as_str()).to_string())])
            .await
    }
}

impl RemotePlaylistSource for SubsonicSource {
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
        let mut extra = vec![("playlistId", raw_item_id(playlist_id.as_str()).to_string())];
        extra.extend(
            track_ids
                .iter()
                .map(|track_id| ("songIdToAdd", raw_item_id(track_id.as_str()).to_string())),
        );
        self.get_unit("updatePlaylist", &extra).await
    }
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> SourceResult<()> {
        let prefix = format!("{}:", playlist_id.as_str());
        let mut extra = vec![("playlistId", raw_item_id(playlist_id.as_str()).to_string())];
        for entry_id in entry_ids {
            let index = entry_id
                .strip_prefix(&prefix)
                .and_then(|value| value.split_once(':'))
                .and_then(|(index, _)| index.parse::<usize>().ok())
                .ok_or(SourceError::InvalidRequest(
                    "playlist entry does not belong to this playlist",
                ))?;
            extra.push(("songIndexToRemove", index.to_string()));
        }
        self.get_unit("updatePlaylist", &extra).await
    }
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> SourceResult<()> {
        let mut entries = self.read_playlist(playlist_id).await?.entries;
        if let Some(old_index) = entries
            .iter()
            .position(|entry| entry.occurrence_id == entry_id)
        {
            let entry = entries.remove(old_index);
            entries.insert(new_index.min(entries.len()), entry);
        }
        let ids = entries
            .into_iter()
            .map(|entry| entry.track_id)
            .collect::<Vec<_>>();
        self.replace_playlist_tracks(playlist_id, &ids).await
    }

    async fn read_playlist_snapshot(
        &self,
        playlist_id: &PlaylistId,
    ) -> SourceResult<PlaylistSnapshot> {
        SubsonicSource::read_playlist(self, playlist_id).await
    }
}

impl SubsonicSource {
    pub(crate) async fn lyrics(
        &self,
        track_id: &TrackId,
        _search: LyricsSearch,
    ) -> SourceResult<Option<NativeLyrics>> {
        let extensions: OpenSubsonicExtensionsBody = self
            .get_json("getOpenSubsonicExtensions", &[])
            .await
            .unwrap_or_default();
        let song_lyrics_version = extensions
            .open_subsonic_extensions
            .iter()
            .find(|extension| extension.name == "songLyrics")
            .and_then(|extension| extension.versions.iter().max())
            .copied()
            .unwrap_or_default();
        if song_lyrics_version >= 1 {
            let mut extra = vec![("id", raw_item_id(track_id.as_str()).to_string())];
            if song_lyrics_version >= 2 {
                extra.push(("enhanced", "true".to_string()));
            }
            let body: StructuredLyricsBody = self.get_json("getLyricsBySongId", &extra).await?;
            let lyrics = native_lyrics_from_structured(body.lyrics_list.structured_lyrics);
            return Ok((!lyrics.documents.is_empty()).then_some(lyrics));
        }

        let track = self.read_track(track_id).await?;
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
        Ok(Some(NativeLyrics {
            origin: NativeLyricsOrigin::Server,
            documents: vec![NativeLyricsDocument {
                role: NativeLyricsRole::Original,
                language: None,
                offset_millis: 0,
                lines: value
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| NativeLyricLine {
                        text: line.trim().to_string(),
                        start_millis: None,
                        end_millis: None,
                        cue_lines: Vec::new(),
                    })
                    .collect(),
                agents: Vec::new(),
            }],
        }))
    }
}

pub(super) fn native_lyrics_from_structured(entries: Vec<StructuredLyricsDto>) -> NativeLyrics {
    let documents = entries
        .into_iter()
        .filter_map(|entry| {
            let role = match entry.kind.as_deref().unwrap_or("main") {
                "main" => NativeLyricsRole::Original,
                "translation" => NativeLyricsRole::Translation,
                "pronunciation" => NativeLyricsRole::Pronunciation,
                _ => return None,
            };
            let agents = entry
                .agents
                .into_iter()
                .filter_map(|agent| {
                    let role = match agent.role.as_str() {
                        "main" => NativeLyricAgentRole::Main,
                        "voice" => NativeLyricAgentRole::Voice,
                        "bg" => NativeLyricAgentRole::Background,
                        "group" => NativeLyricAgentRole::Group,
                        _ => return None,
                    };
                    Some(NativeLyricAgent {
                        id: agent.id,
                        role,
                        name: agent.name,
                    })
                })
                .collect::<Vec<_>>();
            let mut cue_lines_by_index = vec![Vec::new(); entry.line.len()];
            for cue_line in entry.cue_line {
                let Some(lines) = cue_lines_by_index.get_mut(cue_line.index) else {
                    continue;
                };
                let cues = cue_line
                    .cue
                    .into_iter()
                    .filter_map(|cue| {
                        let byte_end_exclusive = cue.byte_end.checked_add(1)?;
                        (cue.byte_start <= cue.byte_end
                            && byte_end_exclusive <= cue_line.value.len()
                            && cue_line.value.is_char_boundary(cue.byte_start)
                            && cue_line.value.is_char_boundary(byte_end_exclusive))
                        .then_some(NativeLyricCue {
                            text: cue.value,
                            start_millis: cue.start,
                            end_millis: cue.end,
                            byte_start: cue.byte_start,
                            byte_end_exclusive,
                        })
                    })
                    .collect();
                lines.push(NativeLyricCueLine {
                    text: cue_line.value,
                    start_millis: cue_line.start,
                    end_millis: cue_line.end,
                    agent_id: cue_line.agent_id,
                    cues,
                });
            }
            let lines = entry
                .line
                .into_iter()
                .zip(cue_lines_by_index)
                .filter_map(|(line, cue_lines)| {
                    (!line.value.trim().is_empty()).then_some(NativeLyricLine {
                        text: line.value,
                        start_millis: line.start,
                        end_millis: cue_lines.iter().filter_map(|line| line.end_millis).max(),
                        cue_lines,
                    })
                })
                .collect::<Vec<_>>();
            (!lines.is_empty()).then_some(NativeLyricsDocument {
                role,
                language: normalize_native_language(entry.lang),
                offset_millis: entry.offset.unwrap_or_default(),
                lines,
                agents,
            })
        })
        .collect();
    NativeLyrics {
        origin: NativeLyricsOrigin::Server,
        documents,
    }
}

fn normalize_native_language(language: String) -> Option<String> {
    let language = language.trim();
    (!language.is_empty()
        && !language.eq_ignore_ascii_case("und")
        && !language.eq_ignore_ascii_case("xxx"))
    .then(|| language.to_string())
}

impl SubsonicSource {
    pub(crate) async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()> {
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
            PlaybackReportKind::QualifiedPlay => {
                let started_at_millis = u64::try_from(report.started_at_unix_seconds)
                    .ok()
                    .and_then(|seconds| seconds.checked_mul(1_000))
                    .ok_or(SourceError::InvalidRequest(
                        "playback start time is outside the OpenSubsonic range",
                    ))?;
                self.get_unit(
                    "scrobble",
                    &[
                        ("id", raw_item_id(report.track_id.as_str()).to_string()),
                        ("submission", "true".to_string()),
                        ("time", started_at_millis.to_string()),
                    ],
                )
                .await
            }
            PlaybackReportKind::Progress | PlaybackReportKind::Stopped => Ok(()),
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
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/rest").unwrap_or(path);
    let normalized_path = if path.is_empty() {
        "/".to_string()
    } else {
        format!("{path}/")
    };
    url.set_path(&normalized_path);
    Ok(url)
}

pub(super) fn rest_endpoint_identity(base_url: &Url) -> String {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let path = if base_path.is_empty() {
        "/rest/".to_string()
    } else {
        format!("{base_path}/rest/")
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
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
const CLIENT_NAME: &str = "Rufin";
const API_VERSION: &str = "1.16.1";
const SALT_BYTES: usize = 12;

pub(super) fn redact_subsonic_query(url: &mut Url) {
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
pub(super) fn redacted_subsonic_url(url: &Url) -> String {
    let mut redacted = url.clone();
    redact_subsonic_query(&mut redacted);
    redacted.to_string()
}
pub(super) fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
pub(super) fn raw_id_string(id: &SubsonicId) -> String {
    id.0.clone()
}
pub(super) fn playlist_entry_id(playlist_id: &PlaylistId, index: usize, track_id: &str) -> String {
    format!("{}:{index}:{track_id}", playlist_id.as_str())
}
pub(super) fn current_year() -> u16 {
    let days_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    year_from_unix_days(days_since_epoch)
}
pub(super) fn year_from_unix_days(mut days: u64) -> u16 {
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
pub(super) fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}
pub(super) fn random_salt() -> String {
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
pub(super) fn stable_source_id(source_id: &str, base_url: &str, username: &str) -> String {
    format!(
        "{:016x}",
        stable_hash(&format!("{source_id}:{base_url}:{username}"))
    )
}
pub(super) fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
pub(super) fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}
pub(super) fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}
pub(super) fn favorite(value: &Option<serde_json::Value>) -> bool {
    value.as_ref().is_some_and(|value| match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Null
        | serde_json::Value::Number(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => false,
    })
}

impl SubsonicSource {
    fn source_id(&self) -> &str {
        self.flavor.source_id()
    }

    pub(super) fn id(&self, kind: &str, raw_id: &str) -> String {
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

    pub(super) async fn get_json<T: DeserializeOwned>(
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

    pub(super) async fn get_all_artists(&self) -> SourceResult<Vec<Artist>> {
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
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SubsonicEmpty {}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicEnvelope<T> {
    #[serde(rename = "subsonic-response")]
    pub(super) response: SubsonicResponse<T>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicResponse<T> {
    pub(super) status: String,
    #[serde(default, rename = "type")]
    pub(super) server_type: Option<String>,
    #[serde(default)]
    pub(super) error: Option<SubsonicError>,
    #[serde(flatten)]
    pub(super) body: T,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicError {
    pub(super) message: String,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct AuthenticateBody {
    pub(super) user: SubsonicUser,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicUser {
    pub(super) username: String,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ScanStatusBody {
    #[serde(rename = "scanStatus")]
    pub(super) scan_status: ScanStatus,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ScanStatus {
    pub(super) scanning: bool,
    #[serde(default)]
    pub(super) count: i64,
    #[serde(default, rename = "folderCount")]
    pub(super) folder_count: Option<i64>,
    #[serde(default, rename = "lastScan")]
    pub(super) last_scan: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct AlbumListBody {
    #[serde(default, rename = "albumList2")]
    pub(super) album_list: AlbumList,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct AlbumList {
    #[serde(default)]
    pub(super) album: Vec<SubsonicAlbum>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SearchBody {
    #[serde(default, rename = "searchResult3")]
    pub(super) search_result: Option<SearchResult>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SearchResult {
    #[serde(default)]
    pub(super) artist: Option<Vec<SubsonicArtist>>,
    #[serde(default)]
    pub(super) album: Option<Vec<SubsonicAlbum>>,
    #[serde(default)]
    pub(super) song: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct MusicFoldersBody {
    #[serde(default, rename = "musicFolders")]
    pub(super) music_folders: MusicFolders,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct MusicFolders {
    #[serde(default, rename = "musicFolder")]
    pub(super) music_folder: Vec<SubsonicMusicFolder>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicMusicFolder {
    pub(super) id: SubsonicId,
    pub(super) name: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct IndexesBody {
    #[serde(default)]
    pub(super) indexes: Option<ArtistsIndex>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct MusicDirectoryBody {
    pub(super) directory: SubsonicDirectory,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicDirectory {
    #[serde(default)]
    pub(super) child: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistsBody {
    pub(super) artists: ArtistsIndex,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistsIndex {
    #[serde(default)]
    pub(super) index: Vec<ArtistIndex>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArtistIndex {
    #[serde(default)]
    pub(super) artist: Vec<SubsonicArtist>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenresBody {
    pub(super) genres: GenresList,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenresList {
    #[serde(default)]
    pub(super) genre: Vec<SubsonicGenre>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct PlaylistsBody {
    #[serde(default)]
    pub(super) playlists: Option<PlaylistsList>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct PlaylistsList {
    #[serde(default)]
    pub(super) playlist: Vec<SubsonicPlaylist>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct PlaylistBody {
    pub(super) playlist: SubsonicPlaylist,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SongBody {
    pub(super) song: SubsonicSong,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct RandomSongsBody {
    #[serde(default, rename = "randomSongs")]
    pub(super) random_songs: Option<SongsList>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SimilarSongsBody {
    #[serde(default, rename = "similarSongs")]
    pub(super) similar_songs: Option<SongsList>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct SimilarSongs2Body {
    #[serde(default, rename = "similarSongs2")]
    pub(super) similar_songs: Option<SongsList>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SongsList {
    #[serde(default)]
    pub(super) song: Vec<SubsonicSong>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct LyricsBody {
    #[serde(default)]
    pub(super) lyrics: Option<SubsonicLyrics>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct OpenSubsonicExtensionsBody {
    #[serde(default, rename = "openSubsonicExtensions")]
    pub(super) open_subsonic_extensions: Vec<OpenSubsonicExtensionDto>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct OpenSubsonicExtensionDto {
    pub(super) name: String,
    #[serde(default)]
    pub(super) versions: Vec<u32>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct StructuredLyricsBody {
    #[serde(default, rename = "lyricsList")]
    pub(super) lyrics_list: StructuredLyricsListDto,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct StructuredLyricsListDto {
    #[serde(default, rename = "structuredLyrics")]
    pub(super) structured_lyrics: Vec<StructuredLyricsDto>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct StructuredLyricsDto {
    pub(super) lang: String,
    #[serde(default)]
    pub(super) line: Vec<StructuredLyricLineDto>,
    #[serde(default)]
    pub(super) offset: Option<i64>,
    #[serde(default)]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) agents: Vec<StructuredLyricAgentDto>,
    #[serde(default, rename = "cueLine")]
    pub(super) cue_line: Vec<StructuredLyricCueLineDto>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct StructuredLyricLineDto {
    pub(super) value: String,
    #[serde(default)]
    pub(super) start: Option<u64>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct StructuredLyricAgentDto {
    pub(super) id: String,
    pub(super) role: String,
    #[serde(default)]
    pub(super) name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct StructuredLyricCueLineDto {
    pub(super) index: usize,
    pub(super) value: String,
    #[serde(default)]
    pub(super) start: Option<u64>,
    #[serde(default)]
    pub(super) end: Option<u64>,
    #[serde(default, rename = "agentId")]
    pub(super) agent_id: Option<String>,
    #[serde(default)]
    pub(super) cue: Vec<StructuredLyricCueDto>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct StructuredLyricCueDto {
    pub(super) value: String,
    pub(super) start: u64,
    #[serde(default)]
    pub(super) end: Option<u64>,
    #[serde(rename = "byteStart")]
    pub(super) byte_start: usize,
    #[serde(rename = "byteEnd")]
    pub(super) byte_end: usize,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicLyrics {
    #[serde(default)]
    pub(super) value: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicAlbum {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) album: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default, rename = "displayArtist")]
    pub(super) display_artist: Option<String>,
    #[serde(default, rename = "artistId")]
    pub(super) artist_id: Option<SubsonicId>,
    #[serde(default)]
    pub(super) artists: Vec<SubsonicArtistRef>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default)]
    pub(super) year: Option<i32>,
    #[serde(default, rename = "releaseDate")]
    pub(super) release_date: Option<SubsonicItemDate>,
    #[serde(default)]
    pub(super) created: Option<String>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) genre: Option<String>,
    #[serde(default)]
    pub(super) genres: Vec<GenreName>,
    #[serde(default, rename = "releaseTypes")]
    pub(super) release_types: Vec<String>,
    #[serde(default, rename = "isCompilation")]
    pub(super) is_compilation: Option<bool>,
    #[serde(default, rename = "musicBrainzId")]
    pub(super) musicbrainz_album_id: Option<String>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicSong {
    pub(super) id: SubsonicId,
    #[serde(default, rename = "isDir")]
    pub(super) is_dir: Option<bool>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) album: Option<String>,
    #[serde(default, rename = "albumId")]
    pub(super) album_id: Option<SubsonicId>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default, rename = "displayArtist")]
    pub(super) display_artist: Option<String>,
    #[serde(default, rename = "artistId")]
    pub(super) artist_id: Option<SubsonicId>,
    #[serde(default)]
    pub(super) artists: Vec<SubsonicArtistRef>,
    #[serde(default, rename = "albumArtists")]
    pub(super) album_artists: Vec<SubsonicArtistRef>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default)]
    pub(super) duration: Option<u32>,
    #[serde(default)]
    pub(super) track: Option<i32>,
    #[serde(default)]
    pub(super) year: Option<i32>,
    #[serde(default)]
    pub(super) created: Option<String>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) genre: Option<String>,
    #[serde(default)]
    pub(super) comment: Option<String>,
    #[serde(default)]
    pub(super) genres: Vec<GenreName>,
    #[serde(default)]
    pub(super) moods: Vec<String>,
    #[serde(default)]
    pub(super) bpm: Option<u32>,
    #[serde(default, rename = "discNumber")]
    pub(super) disc_number: Option<i32>,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) suffix: Option<String>,
    #[serde(default, rename = "contentType")]
    pub(super) content_type: Option<String>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
    #[serde(default, rename = "musicBrainzId")]
    pub(super) musicbrainz_recording_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicArtist {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default)]
    pub(super) played: Option<String>,
    #[serde(default, rename = "playCount")]
    pub(super) play_count: Option<u64>,
    #[serde(default, rename = "userRating")]
    pub(super) user_rating: Option<u32>,
    #[serde(default)]
    pub(super) starred: Option<serde_json::Value>,
    #[serde(default, rename = "musicBrainzId")]
    pub(super) musicbrainz_artist_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicGenre {
    #[serde(default, alias = "name")]
    pub(super) value: String,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicPlaylist {
    pub(super) id: SubsonicId,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default, rename = "coverArt")]
    pub(super) cover_art: Option<SubsonicId>,
    #[serde(default)]
    pub(super) entry: Option<Vec<SubsonicSong>>,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GenreName {
    pub(super) name: String,
}
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SubsonicArtistRef {
    pub(super) id: SubsonicId,
    pub(super) name: String,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub(super) struct SubsonicItemDate {
    #[serde(default)]
    pub(super) year: i32,
    #[serde(default)]
    pub(super) month: i32,
    #[serde(default)]
    pub(super) day: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SubsonicId(pub(super) String);
impl<'de> Deserialize<'de> for SubsonicId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SubsonicIdVisitor)
    }
}
pub(super) struct SubsonicIdVisitor;
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
