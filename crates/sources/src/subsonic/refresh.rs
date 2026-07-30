use std::collections::{BTreeMap, HashMap, HashSet};

use library::{CandidateBatch, GenreCredit, HomeFacts, ImageRef, ProviderFreshness};

use super::*;
use crate::source::{BatchEmitter, SourceReadProgress, SourceReadStage};

const ALBUM_REQUEST_SIZE: usize = 500;
const TRACK_REQUEST_SIZE: usize = 20_000;
const FRESHNESS_VERSION: u32 = 2;

impl SubsonicSource {
    pub(crate) async fn check_freshness(
        &self,
        accepted: Option<&ProviderFreshness>,
    ) -> SourceResult<crate::SourceFreshness> {
        let body: ScanStatusBody = self.get_json("getScanStatus", &[]).await?;
        if body.scan_status.scanning {
            return Ok(crate::SourceFreshness::Busy);
        }
        let current = freshness(body.scan_status);
        if accepted == Some(&current) {
            Ok(crate::SourceFreshness::Unchanged)
        } else {
            Ok(crate::SourceFreshness::Changed(current))
        }
    }

    pub(crate) async fn read_facts(
        &self,
        emitter: &mut BatchEmitter<'_>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<(Option<ProviderFreshness>, HomeFacts)> {
        let mut relation_genres = BTreeMap::<GenreId, String>::new();

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Albums, 0));
        let album_images = self
            .emit_albums(emitter, &mut relation_genres, progress, cancelled)
            .await?;

        check_cancelled(cancelled)?;
        let music_folders = self.read_music_folders().await?;
        emitter
            .emit_async(CandidateBatch::MusicFolders(music_folders.clone()))
            .await?;

        progress(stage(SourceReadStage::Tracks, 0));
        self.emit_tracks(
            &music_folders,
            &album_images,
            emitter,
            &mut relation_genres,
            progress,
            cancelled,
        )
        .await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Artists, 0));
        emitter
            .emit_async(CandidateBatch::Artists(self.get_all_artists().await?))
            .await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Genres, 0));
        let mut genres = self.read_genres().await?;
        let mut genre_ids = genres
            .iter()
            .map(|genre| genre.id.clone())
            .collect::<HashSet<_>>();
        genres.extend(relation_genres.into_iter().filter_map(|(id, name)| {
            genre_ids.insert(id.clone()).then_some(Genre {
                id,
                name,
                image_ref: None,
            })
        }));
        emitter.emit_async(CandidateBatch::Genres(genres)).await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Playlists, 0));
        self.emit_playlists(emitter, progress, cancelled).await?;

        check_cancelled(cancelled)?;
        progress(SourceReadProgress {
            stage: SourceReadStage::Home,
            completed: 0,
            total: Some(4),
        });
        let home = HomeFacts::Source {
            sections: self.read_home_sections().await?,
        };
        let freshness = Some(self.read_freshness().await?);

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Finalizing, 0));
        Ok((freshness, home))
    }

    pub(super) async fn emit_albums(
        &self,
        emitter: &mut BatchEmitter<'_>,
        relation_genres: &mut BTreeMap<GenreId, String>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<HashMap<String, ImageRef>> {
        let mut offset = 0_usize;
        let mut seen = HashSet::new();
        let mut album_images = HashMap::new();
        loop {
            check_cancelled(cancelled)?;
            let body: AlbumListBody = self
                .get_json(
                    "getAlbumList2",
                    &[
                        ("type", "alphabeticalByName".to_string()),
                        ("size", ALBUM_REQUEST_SIZE.to_string()),
                        ("offset", offset.to_string()),
                    ],
                )
                .await?;
            let page = body.album_list.album;
            if page.is_empty() {
                return Ok(album_images);
            }
            let page_len = page.len();
            offset = offset.checked_add(page.len()).ok_or_else(|| {
                SourceError::Other("OpenSubsonic album offset overflowed".to_string())
            })?;
            let mut albums = Vec::with_capacity(page.len());
            for album in page {
                let raw_id = raw_id_string(&album.id);
                if !seen.insert(raw_id.clone()) {
                    return Err(SourceError::Other(
                        "OpenSubsonic repeated an album page".to_string(),
                    ));
                }
                let album = album_from_dto(self, album);
                if let Some(image) = &album.image_ref {
                    album_images.insert(raw_id, image.clone());
                }
                collect_genres(&album.relations.genres, relation_genres);
                albums.push(album);
            }
            emitter.emit_async(CandidateBatch::Albums(albums)).await?;
            progress(stage(SourceReadStage::Albums, offset));
            if page_len < ALBUM_REQUEST_SIZE {
                return Ok(album_images);
            }
        }
    }

    pub(super) async fn emit_tracks(
        &self,
        music_folders: &[MusicFolder],
        album_images: &HashMap<String, ImageRef>,
        emitter: &mut BatchEmitter<'_>,
        relation_genres: &mut BTreeMap<GenreId, String>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<()> {
        let scopes = if music_folders.is_empty() {
            vec![None]
        } else {
            music_folders.iter().map(Some).collect()
        };
        let mut seen_tracks = HashSet::<TrackId>::new();
        let mut emitted = 0;
        for folder in scopes {
            let mut offset = 0_usize;
            let mut scope_ids = HashSet::new();
            loop {
                check_cancelled(cancelled)?;
                let mut extra = vec![
                    ("query", String::new()),
                    ("artistCount", "0".to_string()),
                    ("artistOffset", "0".to_string()),
                    ("albumCount", "0".to_string()),
                    ("albumOffset", "0".to_string()),
                    ("songCount", TRACK_REQUEST_SIZE.to_string()),
                    ("songOffset", offset.to_string()),
                ];
                if let Some(folder) = folder {
                    extra.push(("musicFolderId", raw_item_id(folder.id.as_str()).to_string()));
                }
                let body: SearchBody = self.get_json("search3", &extra).await?;
                let page = body
                    .search_result
                    .and_then(|result| result.song)
                    .unwrap_or_default();
                if page.is_empty() {
                    break;
                }
                offset = offset.checked_add(page.len()).ok_or_else(|| {
                    SourceError::Other("OpenSubsonic track offset overflowed".to_string())
                })?;
                let mut tracks = Vec::with_capacity(page.len());
                for song in page {
                    let raw_id = raw_id_string(&song.id);
                    if !scope_ids.insert(raw_id) {
                        return Err(SourceError::Other(
                            "OpenSubsonic repeated a track page".to_string(),
                        ));
                    }
                    let album_image = song
                        .album_id
                        .as_ref()
                        .map(raw_id_string)
                        .and_then(|id| album_images.get(&id))
                        .cloned();
                    let mut track = track_from_dto(self, song);
                    if album_image.is_some() {
                        track.image_ref = album_image;
                    }
                    if !seen_tracks.insert(track.id.clone()) {
                        continue;
                    }
                    if let Some(folder) = folder {
                        track.relations.music_folders.push(folder.id.clone());
                    }
                    collect_genres(&track.relations.genres, relation_genres);
                    tracks.push(track);
                }
                emitted += tracks.len();
                emitter.emit_async(CandidateBatch::Tracks(tracks)).await?;
                progress(stage(SourceReadStage::Tracks, emitted));
            }
        }
        Ok(())
    }

    async fn read_music_folders(&self) -> SourceResult<Vec<MusicFolder>> {
        let body: MusicFoldersBody = self.get_json("getMusicFolders", &[]).await?;
        Ok(body
            .music_folders
            .music_folder
            .into_iter()
            .map(|folder| MusicFolder {
                id: MusicFolderId::new(self.id("music-folder", &folder.id.0)),
                name: folder.name,
                image_ref: None,
            })
            .collect())
    }

    async fn read_genres(&self) -> SourceResult<Vec<Genre>> {
        let body: GenresBody = self.get_json("getGenres", &[]).await?;
        let mut genres = body
            .genres
            .genre
            .into_iter()
            .map(|genre| genre_from_dto(self, genre))
            .collect::<Vec<_>>();
        genres.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(genres)
    }

    async fn emit_playlists(
        &self,
        emitter: &mut BatchEmitter<'_>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<()> {
        let body: PlaylistsBody = self.get_json("getPlaylists", &[]).await?;
        let mut playlists = body
            .playlists
            .map(|playlists| playlists.playlist)
            .unwrap_or_default();
        playlists
            .sort_by_key(|playlist| playlist.name.as_deref().unwrap_or_default().to_lowercase());
        let total = playlists.len();
        for (position, playlist) in playlists.into_iter().enumerate() {
            check_cancelled(cancelled)?;
            let id = PlaylistId::new(self.id("playlist", &raw_id_string(&playlist.id)));
            emitter
                .emit_async(CandidateBatch::Playlists(vec![
                    self.read_playlist(&id).await?,
                ]))
                .await?;
            progress(SourceReadProgress {
                stage: SourceReadStage::Playlists,
                completed: position + 1,
                total: Some(total),
            });
        }
        Ok(())
    }

    async fn read_home_sections(&self) -> SourceResult<Vec<SourceHomeSection>> {
        let mut sections = Vec::new();
        for kind in [
            SourceHomeSectionKind::MostPlayed,
            SourceHomeSectionKind::NewlyAdded,
            SourceHomeSectionKind::RecentlyPlayed,
            SourceHomeSectionKind::RecentlyReleased,
        ] {
            let section = self.read_home_section(kind).await?;
            if !section.items.is_empty() {
                sections.push(section);
            }
        }
        Ok(sections)
    }

    pub(crate) async fn read_home_section(
        &self,
        kind: SourceHomeSectionKind,
    ) -> SourceResult<SourceHomeSection> {
        let (list_type, mut extra) = match kind {
            SourceHomeSectionKind::MostPlayed => ("frequent", Vec::new()),
            SourceHomeSectionKind::NewlyAdded => ("newest", Vec::new()),
            SourceHomeSectionKind::RecentlyPlayed => ("recent", Vec::new()),
            SourceHomeSectionKind::RecentlyReleased => (
                "byYear",
                vec![
                    ("fromYear", current_year().to_string()),
                    ("toYear", "0".to_string()),
                ],
            ),
        };
        extra.push(("type", list_type.to_string()));
        extra.push(("size", library::HOME_SECTION_ITEM_LIMIT.to_string()));
        let body: AlbumListBody = self.get_json("getAlbumList2", &extra).await?;
        Ok(SourceHomeSection {
            kind,
            items: body
                .album_list
                .album
                .into_iter()
                .map(|album| {
                    HomeItemId::Album(AlbumId::new(self.id("album", &raw_id_string(&album.id))))
                })
                .collect(),
        })
    }

    async fn read_freshness(&self) -> SourceResult<ProviderFreshness> {
        let body: ScanStatusBody = self.get_json("getScanStatus", &[]).await?;
        Ok(freshness(body.scan_status))
    }
}

fn freshness(status: ScanStatus) -> ProviderFreshness {
    let mut marker = Vec::new();
    marker.extend_from_slice(&status.count.to_le_bytes());
    marker.extend_from_slice(&status.folder_count.unwrap_or_default().to_le_bytes());
    if let Some(last_scan) = status.last_scan {
        marker.extend_from_slice(&(last_scan.len() as u64).to_le_bytes());
        marker.extend_from_slice(last_scan.as_bytes());
    } else {
        marker.extend_from_slice(&0_u64.to_le_bytes());
    }
    ProviderFreshness {
        version: FRESHNESS_VERSION,
        marker,
    }
}

fn collect_genres(genres: &[GenreCredit], target: &mut BTreeMap<GenreId, String>) {
    for genre in genres {
        target
            .entry(genre.id.clone())
            .or_insert_with(|| genre.name.clone());
    }
}

fn stage(stage: SourceReadStage, completed: usize) -> SourceReadProgress {
    SourceReadProgress {
        stage,
        completed,
        total: None,
    }
}

fn check_cancelled(cancelled: &(dyn Fn() -> bool + Send + Sync)) -> SourceResult<()> {
    if cancelled() {
        Err(SourceError::Cancelled)
    } else {
        Ok(())
    }
}
