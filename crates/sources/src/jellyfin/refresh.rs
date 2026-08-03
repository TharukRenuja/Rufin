use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;

use library::{ArtistId, CandidateBatch, GenreId, HomeFacts, SourceHomeSectionKind};

use super::*;
use crate::source::{
    BatchEmitter, SourceLibraryChangeRead, SourceLibraryItemId, SourceReadProgress, SourceReadStage,
};

const CHANGED_ITEM_BATCH_SIZE: usize = 100;

impl JellyfinSource {
    pub(crate) async fn read_facts(
        &self,
        emitter: &mut BatchEmitter<'_>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<(Option<library::ProviderFreshness>, HomeFacts)> {
        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Albums, 0, None));
        emit_pages(
            emitter,
            |offset, limit| self.item_page("MusicAlbum", offset, limit),
            |items| CandidateBatch::Albums(items.into_iter().map(album_from_item).collect()),
            progress,
            SourceReadStage::Albums,
            cancelled,
        )
        .await?;

        let music_folders = self.read_music_folders().await?;
        emitter
            .emit_async(CandidateBatch::MusicFolders(music_folders.clone()))
            .await?;
        let mut memberships = self
            .read_music_folder_memberships(&music_folders, cancelled)
            .await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Tracks, 0, None));
        emit_pages(
            emitter,
            |offset, limit| self.item_page("Audio", offset, limit),
            |items| {
                let tracks = items
                    .into_iter()
                    .map(track_from_item)
                    .map(|mut track| {
                        if let Some(folders) = memberships.remove(&track.id) {
                            track.relations.music_folders = folders;
                        }
                        track
                    })
                    .collect();
                CandidateBatch::Tracks(tracks)
            },
            progress,
            SourceReadStage::Tracks,
            cancelled,
        )
        .await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Artists, 0, None));
        let mut artist_ids = HashSet::new();
        for path in ["Artists", "Artists/AlbumArtists"] {
            emit_pages(
                emitter,
                |offset, limit| self.people_page(path, offset, limit),
                |items| {
                    let artists = items
                        .into_iter()
                        .map(artist_from_item)
                        .filter(|artist| artist_ids.insert(artist.id.clone()))
                        .collect();
                    CandidateBatch::Artists(artists)
                },
                progress,
                SourceReadStage::Artists,
                cancelled,
            )
            .await?;
        }

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Genres, 0, None));
        emit_pages(
            emitter,
            |offset, limit| self.music_genre_page(offset, limit),
            |items| CandidateBatch::Genres(items.into_iter().map(genre_from_item).collect()),
            progress,
            SourceReadStage::Genres,
            cancelled,
        )
        .await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Playlists, 0, None));
        self.emit_playlists(emitter, progress, cancelled).await?;

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Home, 0, Some(4)));
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

        check_cancelled(cancelled)?;
        progress(stage(SourceReadStage::Finalizing, 0, None));
        Ok((None, HomeFacts::Source { sections }))
    }

    pub(crate) async fn read_home_section(
        &self,
        kind: SourceHomeSectionKind,
    ) -> SourceResult<library::SourceHomeSection> {
        match kind {
            SourceHomeSectionKind::MostPlayed => {
                self.home_track_section(kind, "PlayCount,SortName", "Descending")
                    .await
            }
            SourceHomeSectionKind::NewlyAdded => {
                self.home_album_section(kind, "DateCreated,SortName", "Descending")
                    .await
            }
            SourceHomeSectionKind::RecentlyPlayed => {
                self.home_track_section(kind, "DatePlayed,SortName", "Descending")
                    .await
            }
            SourceHomeSectionKind::RecentlyReleased => {
                self.home_album_section(kind, "ProductionYear,PremiereDate,SortName", "Descending")
                    .await
            }
        }
    }

    pub(super) async fn read_music_folders(&self) -> SourceResult<Vec<MusicFolder>> {
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
                let image_ref = primary_image_ref("music-folder", &item.id, &item.image_tags);
                item.name.map(|name| MusicFolder {
                    id: MusicFolderId::new(jellyfin_id("music-folder", &item.id)),
                    name,
                    image_ref,
                })
            })
            .collect())
    }

    async fn read_music_folder_memberships(
        &self,
        folders: &[MusicFolder],
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<HashMap<TrackId, Vec<MusicFolderId>>> {
        let mut memberships = HashMap::<TrackId, Vec<MusicFolderId>>::new();
        for folder in folders {
            check_cancelled(cancelled)?;
            let raw_folder_id = raw_item_id(folder.id.as_str()).to_string();
            let mut pages = PageState::default();
            loop {
                check_cancelled(cancelled)?;
                let mut url = endpoint(&self.base_url, "Items")?;
                url.query_pairs_mut()
                    .append_pair("UserId", &self.user_id)
                    .append_pair("ParentId", &raw_folder_id)
                    .append_pair("Recursive", "true")
                    .append_pair("IncludeItemTypes", "Audio")
                    .append_pair("StartIndex", &pages.offset().to_string())
                    .append_pair("Limit", &COLLECTION_PAGE_SIZE.to_string())
                    .append_pair("SortBy", "SortName")
                    .append_pair("SortOrder", "Ascending");
                let page = self.get_json::<ItemQueryResult>(url).await?;
                let count = page.items.len();
                let finished = pages.advance(count, page.total_record_count)?;
                for item in page.items {
                    let values = memberships
                        .entry(TrackId::new(jellyfin_id("track", &item.id)))
                        .or_default();
                    if !values.contains(&folder.id) {
                        values.push(folder.id.clone());
                    }
                }
                if finished {
                    break;
                }
            }
        }
        Ok(memberships)
    }

    async fn emit_playlists(
        &self,
        emitter: &mut BatchEmitter<'_>,
        progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<()> {
        let mut pages = PageState::default();
        loop {
            check_cancelled(cancelled)?;
            let page = self
                .item_page("Playlist", pages.offset(), COLLECTION_PAGE_SIZE)
                .await?;
            let count = page.items.len();
            let finished = pages.advance(count, page.total_record_count)?;
            for playlist in page.items.into_iter().map(playlist_from_item) {
                check_cancelled(cancelled)?;
                let snapshot = self.read_playlist_snapshot(playlist).await?;
                emitter
                    .emit_async(CandidateBatch::Playlists(vec![snapshot]))
                    .await?;
            }
            progress(stage(
                SourceReadStage::Playlists,
                pages.offset(),
                pages.total(),
            ));
            if finished {
                return Ok(());
            }
        }
    }
}

impl JellyfinSource {
    pub(crate) async fn read_library_change(
        &self,
        requested: BTreeSet<String>,
        contains: &(dyn Fn(&SourceLibraryItemId) -> bool + Send + Sync),
    ) -> SourceResult<SourceLibraryChangeRead> {
        if requested.is_empty() {
            return Ok(SourceLibraryChangeRead::Ignored);
        }
        let available = self.items_by_ids(&requested).await?;
        let mut albums = BTreeMap::new();
        let mut tracks = BTreeMap::new();
        let mut artists = BTreeMap::new();
        let mut playlists = BTreeMap::new();
        let mut removed_tracks = BTreeSet::new();
        let mut removed_playlists = BTreeSet::new();
        let mut referenced_albums = BTreeSet::new();

        for raw_id in requested {
            let known = accepted_kinds(&raw_id, contains);
            let Some(item) = available.get(&raw_id) else {
                match known.as_slice() {
                    [] => {}
                    [AcceptedKind::Track] => {
                        removed_tracks.insert(TrackId::new(jellyfin_id("track", &raw_id)));
                    }
                    [AcceptedKind::Playlist] => {
                        removed_playlists.insert(PlaylistId::new(jellyfin_id("playlist", &raw_id)));
                    }
                    _ => return Ok(SourceLibraryChangeRead::Full),
                }
                continue;
            };

            match current_item_kind(item) {
                CurrentItemKind::Track if new_or_only(&known, AcceptedKind::Track) => {
                    let track = track_from_item(item.clone());
                    if let Some(album_id) = track.album_id.as_ref()
                        && let Some(raw_album_id) = raw_entity_id(album_id.as_str(), "album")
                    {
                        referenced_albums.insert(raw_album_id.to_string());
                    }
                    tracks.insert(track.id.clone(), track);
                }
                CurrentItemKind::Album if new_or_only(&known, AcceptedKind::Album) => {
                    let album = album_from_item(item.clone());
                    albums.insert(album.id.clone(), album);
                }
                CurrentItemKind::Artist if known.as_slice() == [AcceptedKind::Artist] => {
                    let artist = artist_from_item(item.clone());
                    artists.insert(artist.id.clone(), artist);
                }
                CurrentItemKind::Playlist if new_or_only(&known, AcceptedKind::Playlist) => {
                    let playlist = playlist_from_item(item.clone());
                    let snapshot = self.read_playlist_snapshot(playlist).await?;
                    playlists.insert(snapshot.playlist.id.clone(), snapshot);
                }
                CurrentItemKind::Other if known.is_empty() => {}
                CurrentItemKind::Track
                | CurrentItemKind::Album
                | CurrentItemKind::Artist
                | CurrentItemKind::Playlist
                | CurrentItemKind::Genre
                | CurrentItemKind::Folder
                | CurrentItemKind::Other => return Ok(SourceLibraryChangeRead::Full),
            }
        }

        let missing_albums = referenced_albums
            .into_iter()
            .filter(|raw_id| !albums.contains_key(&AlbumId::new(jellyfin_id("album", raw_id))))
            .collect::<BTreeSet<_>>();
        let referenced = self.items_by_ids(&missing_albums).await?;
        for raw_id in missing_albums {
            let known = accepted_kinds(&raw_id, contains);
            let Some(item) = referenced.get(&raw_id) else {
                return Ok(SourceLibraryChangeRead::Full);
            };
            if current_item_kind(item) != CurrentItemKind::Album
                || !new_or_only(&known, AcceptedKind::Album)
            {
                return Ok(SourceLibraryChangeRead::Full);
            }
            let album = album_from_item(item.clone());
            albums.insert(album.id.clone(), album);
        }

        if albums.is_empty()
            && tracks.is_empty()
            && artists.is_empty()
            && playlists.is_empty()
            && removed_tracks.is_empty()
            && removed_playlists.is_empty()
        {
            return Ok(SourceLibraryChangeRead::Ignored);
        }
        Ok(SourceLibraryChangeRead::Exact(
            library::SourceLibraryUpdate {
                albums: albums.into_values().collect(),
                tracks: tracks.into_values().collect(),
                artists: artists.into_values().collect(),
                removed_tracks: removed_tracks.into_iter().collect(),
                playlists: playlists.into_values().collect(),
                removed_playlists: removed_playlists.into_iter().collect(),
            },
        ))
    }

    async fn items_by_ids(
        &self,
        ids: &BTreeSet<String>,
    ) -> SourceResult<BTreeMap<String, JellyfinItem>> {
        let ids = ids.iter().cloned().collect::<Vec<_>>();
        let mut items = BTreeMap::new();
        for chunk in ids.chunks(CHANGED_ITEM_BATCH_SIZE) {
            let mut url = endpoint(&self.base_url, "Items")?;
            url.query_pairs_mut()
                .append_pair("UserId", &self.user_id)
                .append_pair("Recursive", "true")
                .append_pair("Ids", &chunk.join(","))
                .append_pair("Limit", &chunk.len().to_string())
                .append_pair("Fields", MIXED_ITEM_FIELDS);
            let response = self.get_json::<ItemQueryResult>(url).await?;
            for item in response.items {
                items.insert(item.id.clone(), item);
            }
        }
        Ok(items)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AcceptedKind {
    Album,
    Track,
    Artist,
    Genre,
    Playlist,
    MusicFolder,
}

fn accepted_kinds(
    raw_id: &str,
    contains: &(dyn Fn(&SourceLibraryItemId) -> bool + Send + Sync),
) -> Vec<AcceptedKind> {
    [
        (
            AcceptedKind::Album,
            SourceLibraryItemId::Album(AlbumId::new(jellyfin_id("album", raw_id))),
        ),
        (
            AcceptedKind::Track,
            SourceLibraryItemId::Track(TrackId::new(jellyfin_id("track", raw_id))),
        ),
        (
            AcceptedKind::Artist,
            SourceLibraryItemId::Artist(ArtistId::new(jellyfin_id("artist", raw_id))),
        ),
        (
            AcceptedKind::Genre,
            SourceLibraryItemId::Genre(GenreId::new(jellyfin_id("genre", raw_id))),
        ),
        (
            AcceptedKind::Playlist,
            SourceLibraryItemId::Playlist(PlaylistId::new(jellyfin_id("playlist", raw_id))),
        ),
        (
            AcceptedKind::MusicFolder,
            SourceLibraryItemId::MusicFolder(MusicFolderId::new(jellyfin_id(
                "music-folder",
                raw_id,
            ))),
        ),
    ]
    .into_iter()
    .filter_map(|(kind, item)| contains(&item).then_some(kind))
    .collect()
}

fn new_or_only(known: &[AcceptedKind], kind: AcceptedKind) -> bool {
    known.is_empty() || known == [kind]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentItemKind {
    Track,
    Album,
    Artist,
    Genre,
    Playlist,
    Folder,
    Other,
}

fn current_item_kind(item: &JellyfinItem) -> CurrentItemKind {
    match item.item_type.as_deref() {
        Some(kind) if kind.eq_ignore_ascii_case("Audio") => CurrentItemKind::Track,
        Some(kind) if kind.eq_ignore_ascii_case("MusicAlbum") => CurrentItemKind::Album,
        Some(kind)
            if kind.eq_ignore_ascii_case("MusicArtist") || kind.eq_ignore_ascii_case("Artist") =>
        {
            CurrentItemKind::Artist
        }
        Some(kind)
            if kind.eq_ignore_ascii_case("MusicGenre") || kind.eq_ignore_ascii_case("Genre") =>
        {
            CurrentItemKind::Genre
        }
        Some(kind) if kind.eq_ignore_ascii_case("Playlist") => CurrentItemKind::Playlist,
        Some(kind)
            if matches!(
                kind.to_ascii_lowercase().as_str(),
                "collectionfolder" | "folder" | "userview"
            ) =>
        {
            CurrentItemKind::Folder
        }
        _ => CurrentItemKind::Other,
    }
}

fn raw_entity_id<'a>(item_id: &'a str, kind: &str) -> Option<&'a str> {
    item_id
        .strip_prefix(&format!("jellyfin:{kind}:"))
        .filter(|raw_id| !raw_id.is_empty())
}

async fn emit_pages<F, Fut, C>(
    emitter: &mut BatchEmitter<'_>,
    mut fetch: F,
    mut transform: C,
    progress: &(dyn Fn(SourceReadProgress) + Send + Sync),
    source_read_stage: SourceReadStage,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> SourceResult<()>
where
    F: FnMut(usize, usize) -> Fut + Send,
    Fut: Future<Output = SourceResult<ItemQueryResult>> + Send,
    C: FnMut(Vec<JellyfinItem>) -> CandidateBatch + Send,
{
    let mut pages = PageState::default();
    loop {
        check_cancelled(cancelled)?;
        let page = fetch(pages.offset(), COLLECTION_PAGE_SIZE).await?;
        let count = page.items.len();
        let finished = pages.advance(count, page.total_record_count)?;
        emitter.emit_async(transform(page.items)).await?;
        progress(stage(source_read_stage, pages.offset(), pages.total()));
        if finished {
            return Ok(());
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PageState {
    offset: usize,
    total: Option<usize>,
}

impl PageState {
    pub(super) fn offset(self) -> usize {
        self.offset
    }

    pub(super) fn total(self) -> Option<usize> {
        self.total
    }

    pub(super) fn advance(
        &mut self,
        count: usize,
        reported_total: Option<usize>,
    ) -> SourceResult<bool> {
        if let Some(reported_total) = reported_total {
            self.total = Some(reported_total);
        }

        if count == 0 {
            return match self.total {
                Some(total) if self.offset < total => Err(incomplete_page()),
                _ => Ok(true),
            };
        }

        self.offset = self.offset.checked_add(count).ok_or_else(incomplete_page)?;
        Ok(self.total.is_some_and(|total| self.offset >= total))
    }
}

fn incomplete_page() -> SourceError {
    SourceError::Other("Jellyfin returned an incomplete page".to_string())
}

fn stage(stage: SourceReadStage, completed: usize, total: Option<usize>) -> SourceReadProgress {
    SourceReadProgress {
        stage,
        completed,
        total,
    }
}

fn check_cancelled(cancelled: &(dyn Fn() -> bool + Send + Sync)) -> SourceResult<()> {
    if cancelled() {
        Err(SourceError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod paging_tests {
    use super::PageState;

    #[test]
    fn short_pages_continue_until_the_declared_total() {
        let mut pages = PageState::default();

        assert!(!pages.advance(1, Some(2)).expect("first page"));
        assert!(pages.advance(1, Some(2)).expect("second page"));
        assert_eq!(pages.offset(), 2);
    }

    #[test]
    fn the_first_declared_total_is_retained() {
        let mut pages = PageState::default();

        assert!(!pages.advance(1, None).expect("page without a total"));
        assert!(pages.advance(1, Some(2)).expect("page declaring the total"));
        assert_eq!(pages.total(), Some(2));
    }

    #[test]
    fn the_latest_declared_total_controls_completion() {
        let mut pages = PageState::default();

        assert!(!pages.advance(1, Some(2)).expect("first page"));
        assert!(!pages.advance(1, Some(3)).expect("larger total"));
        assert!(pages.advance(1, Some(3)).expect("last page"));
        assert_eq!(pages.total(), Some(3));
    }

    #[test]
    fn a_page_offset_overflow_is_rejected() {
        let mut pages = PageState {
            offset: usize::MAX,
            total: None,
        };

        assert!(pages.advance(1, None).is_err());
    }

    #[test]
    fn a_stale_smaller_total_does_not_discard_returned_items() {
        let mut pages = PageState::default();

        assert!(pages.advance(2, Some(1)).expect("returned items"));
        assert_eq!(pages.offset(), 2);
    }

    #[test]
    fn an_empty_page_before_the_declared_total_is_rejected() {
        let mut pages = PageState::default();

        assert!(!pages.advance(1, Some(2)).expect("first page"));
        assert!(pages.advance(0, Some(2)).is_err());
    }

    #[test]
    fn pages_without_a_total_finish_only_when_empty() {
        let mut pages = PageState::default();

        assert!(!pages.advance(1, None).expect("non-empty page"));
        assert!(pages.advance(0, None).expect("empty page"));
    }
}
