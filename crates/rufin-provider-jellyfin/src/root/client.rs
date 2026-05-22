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
            .append_pair("UserId", &self.user_id)
            .append_pair("Fields", ITEM_FIELDS);
        let album = album_from_item(self.get_json::<JellyfinItem>(album_url).await?);

        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("ParentId", raw_album_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("SortBy", "ParentIndexNumber,IndexNumber,SortName")
            .append_pair("Fields", ITEM_FIELDS)
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

    async fn music_folders(&self) -> ProviderResult<Vec<MusicFolder>> {
        let mut url = endpoint(&self.base_url, &format!("Users/{}/Views", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeExternalContent", "false");
        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(response
            .items
            .into_iter()
            .filter(|item| {
                item.collection_type
                    .as_deref()
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("music"))
            })
            .filter_map(|item| {
                item.name.map(|name| MusicFolder {
                    id: MusicFolderId::new(jellyfin_id("music-folder", &item.id)),
                    name,
                })
            })
            .collect())
    }

    async fn tracks_in_music_folder(
        &self,
        folder_id: &MusicFolderId,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<Track>> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("ParentId", raw_item_id(folder_id.as_str()))
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("Fields", ITEM_FIELDS)
            .append_pair("SortBy", "SortName")
            .append_pair("SortOrder", "Ascending");
        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(track_from_item).collect(),
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> ProviderResult<FolderDetail> {
        if let Some(folder_id) = folder_id {
            let raw_folder_id = raw_item_id(folder_id.as_str());
            let mut folder_url = endpoint(&self.base_url, &format!("Items/{raw_folder_id}"))?;
            folder_url
                .query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("Fields", ITEM_FIELDS);
            let current_item = self.get_json::<JellyfinItem>(folder_url).await?;
            let folder = folder_from_item(current_item.clone());
            let parent_id =
                parent_folder_id(&current_item).filter(|parent_id| match music_folder_id {
                    Some(music_folder_id) => {
                        raw_item_id(parent_id.as_str()) != raw_item_id(music_folder_id.as_str())
                    }
                    None => true,
                });
            let (folders, tracks) = self.folder_children(raw_folder_id).await?;
            return Ok(FolderDetail {
                folder,
                parent_id,
                folders,
                tracks,
            });
        }

        if let Some(music_folder_id) = music_folder_id {
            let raw_folder_id = raw_item_id(music_folder_id.as_str());
            let mut folder_url = endpoint(&self.base_url, &format!("Items/{raw_folder_id}"))?;
            folder_url
                .query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("Fields", ITEM_FIELDS);
            let folder = folder_from_item(self.get_json::<JellyfinItem>(folder_url).await?);
            let (folders, tracks) = self.folder_children(raw_folder_id).await?;
            return Ok(FolderDetail {
                folder,
                parent_id: None,
                folders,
                tracks,
            });
        }

        let mut folders = self
            .music_folders()
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
        Ok(FolderDetail {
            folder: Folder {
                id: FolderId::new(jellyfin_id("folder", "root")),
                name: "Folders".to_string(),
            },
            parent_id: None,
            folders,
            tracks: Vec::new(),
        })
    }

    async fn random_tracks(&self, request: RandomTrackRequest) -> ProviderResult<Vec<Track>> {
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
                .append_pair("Fields", ITEM_FIELDS)
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
                .append_pair("Fields", ITEM_FIELDS);
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
            .append_pair("Fields", ITEM_FIELDS);
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
            .append_pair("Fields", ITEM_FIELDS);
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
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Fields", ITEM_FIELDS);
        self.get_json::<JellyfinItem>(url)
            .await
            .map(track_from_item)
    }

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor> {
        self.stream_with_request(&StreamRequest::original(track_id.clone()))
            .await
    }

    async fn stream_with_request(
        &self,
        request: &StreamRequest,
    ) -> ProviderResult<StreamDescriptor> {
        let raw_track_id = raw_item_id(request.track_id.as_str());
        let mut url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/stream"))?;
        let max_bitrate = request
            .quality
            .max_bitrate_kbps()
            .map(|kbps| kbps.saturating_mul(1_000).to_string());
        let static_stream = if max_bitrate.is_some() {
            "false"
        } else {
            "true"
        };
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("UserId", &self.user_id)
                .append_pair("DeviceId", DEVICE_ID)
                .append_pair("Static", static_stream)
                .append_pair("api_key", &self.access_token);
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
                .append_pair("UserId", &self.user_id)
                .append_pair("DeviceId", DEVICE_ID)
                .append_pair("Static", static_stream)
                .append_pair("api_key", "<redacted>");
            if let Some(max_bitrate) = &max_bitrate {
                redacted_query
                    .append_pair("MaxStreamingBitrate", max_bitrate)
                    .append_pair("TranscodingContainer", "mp3")
                    .append_pair("AudioCodec", "mp3");
            }
        }
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
            .append_pair("Fields", ITEM_FIELDS);
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
        let search = if allow_remote {
            JellyfinLyricsSearch::ServerThenRemote
        } else {
            JellyfinLyricsSearch::ServerOnly
        };
        self.lyrics_with_search(track_id, search).await
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
pub(crate) fn normalize_base_url(raw: &str) -> ProviderResult<Url> {
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
fn jellyfin_year_filter(
    min_year: Option<u16>,
    max_year: Option<u16>,
) -> ProviderResult<Option<String>> {
    if min_year.is_none() && max_year.is_none() {
        return Ok(None);
    }
    let min = min_year.unwrap_or(1850);
    let max = max_year.unwrap_or(2050);
    if min > max {
        return Err(ProviderError::Other(
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
fn jellyfin_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        lyrics: true,
        playback_reporting: true,
        playlist_mutations: true,
        favorite_mutations: true,
        random_tracks: true,
        random_played_filter: true,
        music_folders: true,
        folder_browsing: true,
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
fn ticks_to_millis(ticks: Option<i64>) -> Option<u64> {
    ticks.map(|value| (value.max(0) / 10_000) as u64)
}
