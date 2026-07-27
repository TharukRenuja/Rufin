use super::*;

use crate::remote_http::{self, BodyLimit, RemoteHttpPolicy, RemoteTimeouts};
use crate::source::RemotePlaylistSource;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::time::Duration;

use super::refresh::PageState;

const JELLYFIN_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const JELLYFIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const JELLYFIN_JSON_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const JELLYFIN_IMAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const JELLYFIN_ERROR_BODY_MAX_BYTES: usize = 64 * 1024;
const JELLYFIN_HTTP: RemoteHttpPolicy = RemoteHttpPolicy {
    service: "jellyfin",
    auth_context: "Jellyfin returned",
    error_body: BodyLimit {
        max_bytes: JELLYFIN_ERROR_BODY_MAX_BYTES,
        context: "Jellyfin error response",
    },
    redact_error_url: None,
};

impl JellyfinSource {
    pub(crate) async fn search(
        &self,
        request: &library::SearchRequest,
    ) -> SourceResult<library::SearchResults> {
        if request.query().trim().is_empty() {
            return Ok(library::SearchResults::default());
        }
        let (artists, albums, tracks) = tokio::try_join!(
            self.search_people(request.query(), request.limit()),
            self.search_items("MusicAlbum", ALBUM_FIELDS, request.query(), request.limit()),
            self.search_items("Audio", TRACK_FIELDS, request.query(), request.limit()),
        )?;
        Ok(library::SearchResults {
            artists: artists.items.into_iter().map(artist_from_item).collect(),
            albums: albums.items.into_iter().map(album_from_item).collect(),
            tracks: tracks
                .items
                .into_iter()
                .filter(is_audio_item)
                .map(track_from_item)
                .collect(),
        })
    }

    async fn search_items(
        &self,
        item_types: &str,
        fields: &str,
        query: &str,
        limit: usize,
    ) -> SourceResult<ItemQueryResult> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", item_types)
            .append_pair("SearchTerm", query)
            .append_pair("StartIndex", "0")
            .append_pair("Limit", &limit.clamp(1, 100).to_string())
            .append_pair("Fields", fields);
        self.get_json::<ItemQueryResult>(url).await
    }

    async fn search_people(&self, query: &str, limit: usize) -> SourceResult<ItemQueryResult> {
        let mut url = endpoint(&self.base_url, "Artists")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("SearchTerm", query)
            .append_pair("StartIndex", "0")
            .append_pair("Limit", &limit.clamp(1, 100).to_string())
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags,ProviderIds",
            );
        self.get_json::<ItemQueryResult>(url).await
    }

    async fn read_playlist_rows(
        &self,
        raw_playlist_id: &str,
        fields: Option<&str>,
    ) -> SourceResult<Option<Vec<(String, JellyfinItem)>>> {
        let mut pages = PageState::default();
        let mut entry_ids = BTreeSet::new();
        let mut items = Vec::new();
        loop {
            let mut url = endpoint(
                &self.base_url,
                &format!("Playlists/{raw_playlist_id}/Items"),
            )?;
            {
                let mut query = url.query_pairs_mut();
                query
                    .append_pair("UserId", &self.user_id)
                    .append_pair("StartIndex", &pages.offset().to_string())
                    .append_pair("Limit", &COLLECTION_PAGE_SIZE.to_string());
                if let Some(fields) = fields {
                    query.append_pair("Fields", fields);
                }
            }
            let response = self.get_json::<ItemQueryResult>(url).await?;
            let count = response.items.len();
            let Ok(finished) = pages.advance(count, response.total_record_count) else {
                return Ok(None);
            };
            for item in response.items {
                let Some(entry_id) = item.playlist_item_id.as_deref().filter(|id| !id.is_empty())
                else {
                    return Ok(None);
                };
                if !entry_ids.insert(entry_id.to_string()) {
                    return Ok(None);
                }
                items.push((entry_id.to_string(), item));
            }
            if finished {
                return Ok(Some(items));
            }
        }
    }

    pub(super) async fn read_playlist_entries(
        &self,
        raw_playlist_id: &str,
    ) -> SourceResult<Option<Vec<PlaylistEntry>>> {
        Ok(self
            .read_playlist_rows(raw_playlist_id, None)
            .await?
            .map(|items| {
                items
                    .into_iter()
                    .map(|(entry_id, item)| PlaylistEntry {
                        occurrence_id: entry_id,
                        track_id: TrackId::new(jellyfin_id("track", &item.id)),
                    })
                    .collect()
            }))
    }

    pub(super) async fn read_playlist_snapshot(
        &self,
        playlist: Playlist,
    ) -> SourceResult<Option<PlaylistSnapshot>> {
        let Some(entries) = self
            .read_playlist_entries(raw_item_id(playlist.id.as_str()))
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(PlaylistSnapshot { playlist, entries }))
    }
}

impl JellyfinSource {
    pub(crate) async fn read_folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> SourceResult<library::FolderContents> {
        if let Some(folder_id) = folder_id {
            let raw_folder_id = raw_item_id(folder_id.as_str());
            let (folders, tracks) = self.folder_children(raw_folder_id).await?;
            return Ok(folder_contents(folders, tracks));
        }

        if let Some(music_folder_id) = music_folder_id {
            let raw_folder_id = raw_item_id(music_folder_id.as_str());
            let (folders, tracks) = self.folder_children(raw_folder_id).await?;
            return Ok(folder_contents(folders, tracks));
        }

        let mut folders = self
            .read_music_folders()
            .await?
            .into_iter()
            .map(|folder| Folder {
                id: FolderId::new(jellyfin_id("folder", raw_item_id(folder.id.as_str()))),
                name: folder.name,
            })
            .collect::<Vec<_>>();
        folders.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(folder_contents(folders, Vec::new()))
    }
}

fn folder_contents(folders: Vec<Folder>, tracks: Vec<Track>) -> library::FolderContents {
    library::FolderContents {
        folders: folders.into(),
        tracks: tracks.into(),
    }
}

impl JellyfinSource {
    pub(crate) async fn random_tracks(
        &self,
        request: RandomTrackRequest,
    ) -> SourceResult<Vec<Track>> {
        let mut url = endpoint(&self.base_url, "Items")?;
        let limit = request.limit.clamp(1, 500).to_string();
        let years = jellyfin_year_filter(request.min_year, request.max_year)?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("UserId", &self.user_id)
                .append_pair("Recursive", "true")
                .append_pair("IncludeItemTypes", "Audio")
                .append_pair("StartIndex", "0")
                .append_pair("Limit", &limit)
                .append_pair("Fields", TRACK_FIELDS)
                .append_pair("SortBy", "Random")
                .append_pair("SortOrder", "Ascending");
            if let Some(years) = years.as_deref() {
                query.append_pair("Years", years);
            }
            if let Some(genre_id) = request.genre_id.as_ref() {
                query.append_pair("GenreIds", raw_item_id(genre_id.as_str()));
            } else if let Some(genre_name) = request
                .genre_name
                .as_deref()
                .filter(|name| !name.is_empty())
            {
                query.append_pair("Genres", genre_name);
            }
            match request.played_filter {
                PlayedFilter::All => {}
                PlayedFilter::Unplayed => {
                    query.append_pair("IsPlayed", "false");
                }
                PlayedFilter::Played => {
                    query.append_pair("IsPlayed", "true");
                }
            }
        }

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(response.items.into_iter().map(track_from_item).collect())
    }
}

impl JellyfinSource {
    pub(crate) async fn generated_tracks(
        &self,
        request: GeneratedTracksRequest,
    ) -> SourceResult<Vec<Track>> {
        if self.use_instant_mix {
            return self.instant_mix_tracks(&request.seed, request.limit).await;
        }
        if let library::RadioSeed::Track(track_id) = &request.seed {
            let tracks = self.similar_tracks(track_id, request.limit).await?;
            if !tracks.is_empty() {
                return Ok(tracks);
            }
        }
        self.instant_mix_tracks(&request.seed, request.limit).await
    }
}

impl JellyfinSource {
    pub(crate) async fn read_playlist(
        &self,
        playlist_id: &PlaylistId,
    ) -> SourceResult<PlaylistSnapshot> {
        let raw_playlist_id = raw_item_id(playlist_id.as_str());
        let mut playlist_url = endpoint(&self.base_url, &format!("Items/{raw_playlist_id}"))?;
        playlist_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Fields", PLAYLIST_FIELDS);
        let playlist = playlist_from_item(self.get_json::<JellyfinItem>(playlist_url).await?);

        let entries = self
            .read_playlist_entries(raw_playlist_id)
            .await?
            .ok_or_else(|| {
                SourceError::Other("Jellyfin returned an incomplete playlist".to_string())
            })?;
        Ok(PlaylistSnapshot { playlist, entries })
    }
}

impl JellyfinSource {
    pub(crate) async fn resolve_stream(
        &self,
        request: &StreamRequest,
    ) -> SourceResult<StreamDescriptor> {
        stream_descriptor(
            &self.base_url,
            &self.user_id,
            &self.device_id,
            &self.access_token,
            self.trust_invalid_cert,
            request,
        )
    }
}

impl JellyfinSource {
    pub(crate) async fn image_bytes(
        &self,
        image_ref: &ImageRef,
        size: u32,
    ) -> SourceResult<ImageBytes> {
        let image_kind = if image_ref.item_id.starts_with("jellyfin:backdrop:") {
            "Backdrop"
        } else {
            "Primary"
        };
        let mut url = endpoint(
            &self.base_url,
            &format!(
                "Items/{}/Images/{}",
                raw_item_id(&image_ref.item_id),
                image_kind
            ),
        )?;
        url.query_pairs_mut()
            .append_pair("fillWidth", &size.max(1).to_string())
            .append_pair("fillHeight", &size.max(1).to_string())
            .append_pair("quality", "90");
        if let Some(tag) = image_ref.tag.as_deref().filter(|tag| !tag.is_empty()) {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        send_bytes(
            self.client
                .get(url)
                .header(header::AUTHORIZATION, self.authorization.clone()),
        )
        .await
    }
}

impl JellyfinSource {
    pub(crate) async fn set_favorite(
        &self,
        item_id: FavoriteItemId,
        favorite: bool,
    ) -> SourceResult<()> {
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
}

impl RemotePlaylistSource for JellyfinSource {
    async fn create_playlist(&self, name: &str, track_ids: &[TrackId]) -> SourceResult<PlaylistId> {
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
    async fn rename_playlist(&self, playlist_id: &PlaylistId, name: &str) -> SourceResult<()> {
        let url = endpoint(
            &self.base_url,
            &format!("Playlists/{}", raw_item_id(playlist_id.as_str())),
        )?;
        let body = UpdatePlaylistDto {
            name: Some(name.to_string()),
        };
        self.send_unit(self.client.post(url).json(&body)).await
    }
    async fn delete_playlist(&self, playlist_id: &PlaylistId) -> SourceResult<()> {
        let url = endpoint(
            &self.base_url,
            &format!("Items/{}", raw_item_id(playlist_id.as_str())),
        )?;
        self.send_unit(self.client.delete(url)).await
    }
    async fn add_playlist_tracks(
        &self,
        playlist_id: &PlaylistId,
        track_ids: &[TrackId],
    ) -> SourceResult<()> {
        for track_ids in track_ids.chunks(50) {
            let mut url = endpoint(
                &self.base_url,
                &format!("Playlists/{}/Items", raw_item_id(playlist_id.as_str())),
            )?;
            url.query_pairs_mut()
                .append_pair("userId", &self.user_id)
                .append_pair("ids", &raw_track_ids(track_ids).join(","));
            self.send_unit(self.client.post(url)).await?;
        }
        Ok(())
    }
    async fn remove_playlist_entries(
        &self,
        playlist_id: &PlaylistId,
        entry_ids: &[String],
    ) -> SourceResult<()> {
        for entry_ids in entry_ids.chunks(50) {
            let mut url = endpoint(
                &self.base_url,
                &format!("Playlists/{}/Items", raw_item_id(playlist_id.as_str())),
            )?;
            url.query_pairs_mut()
                .append_pair("entryIds", &entry_ids.join(","));
            self.send_unit(self.client.delete(url)).await?;
        }
        Ok(())
    }
    async fn move_playlist_entry(
        &self,
        playlist_id: &PlaylistId,
        entry_id: &str,
        new_index: usize,
    ) -> SourceResult<()> {
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

    async fn read_playlist_snapshot(
        &self,
        playlist_id: &PlaylistId,
    ) -> SourceResult<PlaylistSnapshot> {
        JellyfinSource::read_playlist(self, playlist_id).await
    }
}

impl JellyfinSource {
    pub(crate) async fn lyrics(
        &self,
        track_id: &TrackId,
        search: LyricsSearch,
    ) -> SourceResult<Option<NativeLyrics>> {
        match search {
            LyricsSearch::ServerOnly => self.server_lyrics(track_id).await,
            LyricsSearch::ServerThenRemote => {
                if let Some(lyrics) = self.server_lyrics(track_id).await? {
                    return Ok(Some(lyrics));
                }
                self.remote_lyrics(track_id).await
            }
            LyricsSearch::RemoteThenServer => {
                if let Some(lyrics) = self.remote_lyrics(track_id).await? {
                    return Ok(Some(lyrics));
                }
                self.server_lyrics(track_id).await
            }
        }
    }
}

impl JellyfinSource {
    pub(crate) async fn report_playback(&self, report: PlaybackReport) -> SourceResult<()> {
        let path = match report.kind {
            PlaybackReportKind::Started => "Sessions/Playing",
            PlaybackReportKind::Progress => "Sessions/Playing/Progress",
            PlaybackReportKind::QualifiedPlay => return Ok(()),
            PlaybackReportKind::Stopped => "Sessions/Playing/Stopped",
        };
        let url = endpoint(&self.base_url, path)?;
        let body = PlaybackReportDto::from_report(report);
        self.send_unit(self.client.post(url).json(&body)).await
    }
}

pub(super) async fn public_server_name(
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
pub(super) async fn send_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> SourceResult<T> {
    remote_http::json(
        request,
        JELLYFIN_HTTP,
        BodyLimit {
            max_bytes: JELLYFIN_JSON_MAX_BYTES,
            context: "Jellyfin JSON response",
        },
    )
    .await
}
pub(super) async fn send_unit(request: reqwest::RequestBuilder) -> SourceResult<()> {
    remote_http::unit(request, JELLYFIN_HTTP).await
}
pub(super) async fn send_bytes(request: reqwest::RequestBuilder) -> SourceResult<ImageBytes> {
    remote_http::bytes(
        request,
        JELLYFIN_HTTP,
        BodyLimit {
            max_bytes: JELLYFIN_IMAGE_MAX_BYTES,
            context: "Jellyfin image response",
        },
    )
    .await
}
pub(super) fn build_client(trust_invalid_cert: bool) -> SourceResult<Client> {
    build_client_with_timeouts(
        trust_invalid_cert,
        JELLYFIN_CONNECT_TIMEOUT,
        JELLYFIN_REQUEST_TIMEOUT,
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
        JELLYFIN_HTTP,
    )
}

pub(super) fn stream_descriptor(
    base_url: &Url,
    user_id: &str,
    device_id: &str,
    access_token: &str,
    trust_invalid_certificate: bool,
    request: &StreamRequest,
) -> SourceResult<StreamDescriptor> {
    let raw_track_id = raw_item_id(request.track_id.as_str());
    let max_bitrate = request
        .quality
        .max_bitrate_kbps()
        .map(|kbps| kbps.saturating_mul(1_000).to_string());

    let mut url = endpoint(base_url, &format!("Audio/{raw_track_id}/stream"))?;
    let static_stream = if max_bitrate.is_some() {
        "false"
    } else {
        "true"
    };
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("UserId", user_id)
            .append_pair("DeviceId", device_id)
            .append_pair("Static", static_stream)
            .append_pair("api_key", access_token);
        if let Some(max_bitrate) = &max_bitrate {
            query
                .append_pair("MaxStreamingBitrate", max_bitrate)
                .append_pair("TranscodingContainer", "mp3")
                .append_pair("AudioCodec", "mp3");
        }
    }
    let mut redacted_url = url.clone();
    {
        let mut redacted_query = redacted_url.query_pairs_mut();
        redacted_query
            .clear()
            .append_pair("UserId", user_id)
            .append_pair("DeviceId", device_id)
            .append_pair("Static", static_stream)
            .append_pair("api_key", "<redacted>");
        if let Some(max_bitrate) = &max_bitrate {
            redacted_query
                .append_pair("MaxStreamingBitrate", max_bitrate)
                .append_pair("TranscodingContainer", "mp3")
                .append_pair("AudioCodec", "mp3");
        }
    }
    Ok(
        StreamDescriptor::with_redacted(url.to_string(), redacted_url.to_string())
            .with_trust_invalid_certificate(trust_invalid_certificate),
    )
}

pub(crate) fn normalize_base_url(raw: &str) -> SourceResult<Url> {
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
pub(super) fn endpoint(base_url: &Url, path: &str) -> SourceResult<Url> {
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
pub(super) fn auth_header(config: &JellyfinClientConfig, token: Option<&str>) -> String {
    let mut value = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        config.client_name, config.device_name, config.device_id, config.client_version
    );
    if let Some(token) = token {
        value.push_str(&format!(", Token=\"{token}\""));
    }
    value
}
pub(super) fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}
pub(super) fn jellyfin_year_filter(
    min_year: Option<u16>,
    max_year: Option<u16>,
) -> SourceResult<Option<String>> {
    if min_year.is_none() && max_year.is_none() {
        return Ok(None);
    }
    let min = min_year.unwrap_or(1850);
    let max = max_year.unwrap_or(2050);
    if min > max {
        return Err(SourceError::Other(
            "minimum year cannot be greater than maximum year".to_string(),
        ));
    }
    Ok(Some(
        (min..=max)
            .map(|year| year.to_string())
            .collect::<Vec<_>>()
            .join(","),
    ))
}
pub(crate) fn jellyfin_id(kind: &str, id: &str) -> String {
    format!("jellyfin:{kind}:{id}")
}
pub(super) fn raw_track_ids(track_ids: &[TrackId]) -> Vec<String> {
    track_ids
        .iter()
        .map(|id| raw_item_id(id.as_str()).to_string())
        .collect()
}
pub(super) fn stable_source_id(input: &str) -> String {
    format!("{:016x}", stable_hash(input))
}
pub(crate) fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
pub(super) fn ticks_to_millis(ticks: Option<i64>) -> Option<u64> {
    ticks.map(|value| (value.max(0) / 10_000) as u64)
}

impl JellyfinSource {
    pub(super) async fn item_page(
        &self,
        include_types: &str,
        offset: usize,
        limit: usize,
    ) -> SourceResult<ItemQueryResult> {
        self.item_page_sorted(include_types, offset, limit, "SortName", "Ascending")
            .await
    }

    async fn item_page_sorted(
        &self,
        include_types: &str,
        offset: usize,
        limit: usize,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<ItemQueryResult> {
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
            .append_pair("StartIndex", &offset.to_string())
            .append_pair("Limit", &limit.to_string())
            .append_pair("Fields", fields)
            .append_pair("SortBy", sort_by)
            .append_pair("SortOrder", sort_order);

        self.get_json::<ItemQueryResult>(url).await
    }

    pub(super) async fn home_album_section(
        &self,
        kind: SourceHomeSectionKind,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<SourceHomeSection> {
        let page = self
            .item_page_sorted(
                "MusicAlbum",
                0,
                library::HOME_SECTION_ITEM_LIMIT,
                sort_by,
                sort_order,
            )
            .await?;
        Ok(SourceHomeSection {
            kind,
            items: page
                .items
                .into_iter()
                .map(|item| HomeItemId::Album(AlbumId::new(jellyfin_id("album", &item.id))))
                .collect(),
        })
    }

    pub(super) async fn home_track_section(
        &self,
        kind: SourceHomeSectionKind,
        sort_by: &str,
        sort_order: &str,
    ) -> SourceResult<SourceHomeSection> {
        let page = self
            .item_page_sorted(
                "Audio",
                0,
                library::HOME_SECTION_ITEM_LIMIT,
                sort_by,
                sort_order,
            )
            .await?;
        Ok(SourceHomeSection {
            kind,
            items: page
                .items
                .into_iter()
                .map(|item| HomeItemId::Track(TrackId::new(jellyfin_id("track", &item.id))))
                .collect(),
        })
    }

    pub(super) async fn people_page(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> SourceResult<ItemQueryResult> {
        let mut url = endpoint(&self.base_url, path)?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &offset.to_string())
            .append_pair("Limit", &limit.to_string())
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags,ProviderIds",
            );

        self.get_json::<ItemQueryResult>(url).await
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
        seed: &library::RadioSeed,
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

    fn instant_mix_url(&self, seed: &library::RadioSeed) -> SourceResult<Url> {
        match seed {
            library::RadioSeed::Track(track_id) => endpoint(
                &self.base_url,
                &format!("Songs/{}/InstantMix", raw_item_id(track_id.as_str())),
            ),
            library::RadioSeed::Album(album_id) => endpoint(
                &self.base_url,
                &format!("Albums/{}/InstantMix", raw_item_id(album_id.as_str())),
            ),
            library::RadioSeed::Artist(artist_id) => endpoint(
                &self.base_url,
                &format!("Artists/{}/InstantMix", raw_item_id(artist_id.as_str())),
            ),
            library::RadioSeed::Playlist(playlist_id) => endpoint(
                &self.base_url,
                &format!("Playlists/{}/InstantMix", raw_item_id(playlist_id.as_str())),
            ),
            library::RadioSeed::Genre { id, name: _ } => {
                let mut url = endpoint(&self.base_url, "MusicGenres/InstantMix")?;
                url.query_pairs_mut()
                    .append_pair("Id", raw_item_id(id.as_str()));
                Ok(url)
            }
        }
    }

    pub(super) async fn music_genre_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> SourceResult<ItemQueryResult> {
        let mut url = endpoint(&self.base_url, "MusicGenres")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &offset.to_string())
            .append_pair("Limit", &limit.to_string())
            .append_pair("IncludeItemTypes", "Audio,MusicAlbum")
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount,ImageTags",
            )
            .append_pair("SortBy", "SortName");

        self.get_json::<ItemQueryResult>(url).await
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

    pub(super) async fn get_json<T: DeserializeOwned>(&self, url: Url) -> SourceResult<T> {
        send_json(
            self.client
                .get(url)
                .header(header::AUTHORIZATION, self.authorization.clone()),
        )
        .await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> SourceResult<T> {
        send_json(request.header(header::AUTHORIZATION, self.authorization.clone())).await
    }

    async fn send_unit(&self, request: reqwest::RequestBuilder) -> SourceResult<()> {
        send_unit(request.header(header::AUTHORIZATION, self.authorization.clone())).await
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

pub(super) fn lyrics_from_dto(origin: NativeLyricsOrigin, dto: LyricDto) -> NativeLyrics {
    NativeLyrics {
        origin,
        documents: vec![NativeLyricsDocument {
            role: NativeLyricsRole::Original,
            language: None,
            offset_millis: 0,
            lines: dto
                .lyrics
                .unwrap_or_default()
                .into_iter()
                .filter_map(|line| {
                    let text = line.text.unwrap_or_default();
                    (!text.trim().is_empty()).then_some(NativeLyricLine {
                        text,
                        start_millis: ticks_to_millis(line.start),
                        end_millis: None,
                        cue_lines: Vec::new(),
                    })
                })
                .collect(),
            agents: Vec::new(),
        }],
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PublicSystemInfo {
    pub(super) server_name: Option<String>,
    pub(super) local_address: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct AuthenticateByNameRequest {
    pub(super) username: String,
    #[serde(rename = "Pw")]
    pub(super) password: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct AuthenticationResult {
    pub(super) access_token: String,
    pub(super) server_id: Option<String>,
    pub(super) user: JellyfinUser,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct JellyfinUser {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct CreatePlaylistDto {
    pub(super) name: String,
    pub(super) ids: Vec<String>,
    pub(super) user_id: Option<String>,
    pub(super) media_type: Option<String>,
    pub(super) is_public: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct UpdatePlaylistDto {
    pub(super) name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PlaylistCreationResult {
    pub(super) id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct LyricDto {
    pub(super) lyrics: Option<Vec<LyricLineDto>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct LyricLineDto {
    pub(super) text: Option<String>,
    pub(super) start: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct RemoteLyricInfoDto {
    pub(super) id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PlaybackReportDto {
    pub(super) can_seek: bool,
    pub(super) item_id: String,
    pub(super) is_paused: bool,
    pub(super) is_muted: bool,
    pub(super) position_ticks: i64,
    pub(super) volume_level: i32,
    pub(super) play_method: &'static str,
    pub(super) repeat_mode: &'static str,
    pub(super) playback_order: &'static str,
    pub(super) failed: bool,
}

impl PlaybackReportDto {
    pub(super) fn from_report(report: PlaybackReport) -> Self {
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
