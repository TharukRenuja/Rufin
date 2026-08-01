//! Complete read projections over one [`LoadedLibrary`](crate::LoadedLibrary).
//!
//! These are product views, not SQLite pages. Each method holds the loaded
//! read guard only while it gathers shared handles, then releases it before a
//! caller builds GTK models or starts Playback.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    Album, AlbumArtwork, AlbumId, Artist, ArtistId, Folder, FolderId, Genre, GenreId,
    LoadedLibrary, LoadedLibraryResult, Mood, MoodId, MusicFolder, MusicFolderId, Playlist,
    PlaylistId, SmartPlaylistId, Track, TrackId,
    loaded::{
        ItemSlot, LoadedAlbum, LoadedArtist, LoadedGenre, LoadedItems, LoadedMood, LoadedPlaylist,
        LoadedState, TrackSlot,
    },
};

const ARTIST_ARTWORK_LIMIT: usize = 4;
pub(crate) const COLLECTION_ARTWORK_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrackSort {
    Title,
    TrackNumber,
    Artist,
    AlbumArtist,
    Album,
    Year,
    ReleaseDate,
    DateAdded,
    LastPlayed,
    PlayCount,
    UserRating,
    Genre,
    Bpm,
    Duration,
    Favorite,
}

/// One compact ordered Track projection over the selected loaded Library.
///
/// The projection retains four-byte Track slots, not another catalog of Track
/// handles. GTK resolves a shallow Track handle only when it binds a row, and
/// Playback materializes the ordered handles only when the user starts a
/// collection.
#[derive(Clone, Debug)]
pub struct TrackList {
    loaded: Arc<LoadedLibrary>,
    slots: Arc<[TrackSlot]>,
    sorted_by: Option<(TrackSort, bool)>,
}

/// One loaded-Library Track order that can be prepared away from GTK.
///
/// Mounted routes pass [`TrackList`] directly. Collection cards and context
/// actions pass an O(1) Library-owned target, and Rufin derives its compact
/// slot order on the loaded-Play executor before Playback admission.
#[derive(Clone, Debug)]
pub struct TrackSelection {
    kind: TrackSelectionKind,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DownloadStatus {
    pub any: bool,
    pub complete: bool,
}

#[derive(Clone, Debug)]
enum TrackSelectionKind {
    Prepared(TrackList),
    Album {
        loaded: Arc<LoadedLibrary>,
        id: AlbumId,
        music_folder_id: Option<MusicFolderId>,
    },
    Artist {
        loaded: Arc<LoadedLibrary>,
        id: ArtistId,
        music_folder_id: Option<MusicFolderId>,
    },
    Genre {
        loaded: Arc<LoadedLibrary>,
        id: GenreId,
        music_folder_id: Option<MusicFolderId>,
    },
    Mood {
        loaded: Arc<LoadedLibrary>,
        id: MoodId,
        music_folder_id: Option<MusicFolderId>,
    },
    Playlist {
        loaded: Arc<LoadedLibrary>,
        id: PlaylistId,
    },
    SmartPlaylist {
        loaded: Arc<LoadedLibrary>,
        id: SmartPlaylistId,
        music_folder_id: Option<MusicFolderId>,
    },
}

impl TrackSelection {
    pub fn prepared(&self) -> Option<&TrackList> {
        match &self.kind {
            TrackSelectionKind::Prepared(tracks) => Some(tracks),
            TrackSelectionKind::Album { .. }
            | TrackSelectionKind::Artist { .. }
            | TrackSelectionKind::Genre { .. }
            | TrackSelectionKind::Mood { .. }
            | TrackSelectionKind::Playlist { .. }
            | TrackSelectionKind::SmartPlaylist { .. } => None,
        }
    }

    pub fn prepare(self) -> LoadedLibraryResult<TrackList> {
        match self.kind {
            TrackSelectionKind::Prepared(tracks) => Ok(tracks),
            TrackSelectionKind::Album {
                loaded,
                id,
                music_folder_id,
            } => loaded.album_tracks(&id, music_folder_id.as_ref()),
            TrackSelectionKind::Artist {
                loaded,
                id,
                music_folder_id,
            } => loaded.artist_tracks(&id, music_folder_id.as_ref()),
            TrackSelectionKind::Genre {
                loaded,
                id,
                music_folder_id,
            } => loaded.genre_tracks(&id, music_folder_id.as_ref()),
            TrackSelectionKind::Mood {
                loaded,
                id,
                music_folder_id,
            } => loaded.mood_tracks(&id, music_folder_id.as_ref()),
            TrackSelectionKind::Playlist { loaded, id } => loaded.playlist_tracks(&id),
            TrackSelectionKind::SmartPlaylist {
                loaded,
                id,
                music_folder_id,
            } => loaded.smart_playlist_tracks(&id, music_folder_id.as_ref()),
        }
    }

    pub fn download_status(&self) -> LoadedLibraryResult<DownloadStatus> {
        use crate::download_coverage::DownloadCollection;

        let (loaded, collection, music_folder_id) = match &self.kind {
            TrackSelectionKind::Prepared(tracks) => return tracks.download_status(),
            TrackSelectionKind::Album {
                loaded,
                id,
                music_folder_id,
            } => (
                loaded,
                DownloadCollection::Album(id.clone()),
                music_folder_id.clone(),
            ),
            TrackSelectionKind::Artist {
                loaded,
                id,
                music_folder_id,
            } => (
                loaded,
                DownloadCollection::Artist(id.clone()),
                music_folder_id.clone(),
            ),
            TrackSelectionKind::Genre {
                loaded,
                id,
                music_folder_id,
            } => (
                loaded,
                DownloadCollection::Genre(id.clone()),
                music_folder_id.clone(),
            ),
            TrackSelectionKind::Mood {
                loaded,
                id,
                music_folder_id,
            } => (
                loaded,
                DownloadCollection::Mood(id.clone()),
                music_folder_id.clone(),
            ),
            TrackSelectionKind::Playlist { loaded, id } => {
                (loaded, DownloadCollection::Playlist(id.clone()), None)
            }
            TrackSelectionKind::SmartPlaylist {
                loaded,
                id,
                music_folder_id,
            } => (
                loaded,
                DownloadCollection::SmartPlaylist(id.clone()),
                music_folder_id.clone(),
            ),
        };
        let state = loaded.read_state()?;
        let (any, complete) = state.download_coverage.status(collection, music_folder_id);
        Ok(DownloadStatus { any, complete })
    }
}

impl From<TrackList> for TrackSelection {
    fn from(value: TrackList) -> Self {
        Self {
            kind: TrackSelectionKind::Prepared(value),
        }
    }
}

/// One compact Track projection after applying the currently accepted value
/// for a single Track.
#[derive(Clone, Debug)]
pub struct TrackListChange {
    pub tracks: TrackList,
    pub previous_position: Option<u32>,
    pub position: Option<u32>,
    pub order_changed: bool,
}

impl TrackList {
    pub(crate) fn new(
        loaded: Arc<LoadedLibrary>,
        slots: Arc<[TrackSlot]>,
        sorted_by: Option<(TrackSort, bool)>,
    ) -> Self {
        Self {
            loaded,
            slots,
            sorted_by,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn shares_order(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.slots, &other.slots)
    }

    pub fn track(&self, position: usize) -> LoadedLibraryResult<Option<Track>> {
        let Some(slot) = self.slots.get(position).copied() else {
            return Ok(None);
        };
        Ok(self.loaded.read_state()?.tracks.get_slot(slot).cloned())
    }

    pub fn materialize(&self) -> LoadedLibraryResult<Arc<[Track]>> {
        Ok(self.materialize_owned()?.into())
    }

    pub fn materialize_owned(&self) -> LoadedLibraryResult<Vec<Track>> {
        let state = self.loaded.read_state()?;
        self.slots
            .iter()
            .map(|slot| {
                state
                    .tracks
                    .get_slot(*slot)
                    .cloned()
                    .ok_or(crate::LoadedLibraryError::StaleTrackSelection)
            })
            .collect()
    }

    pub fn track_ids(&self) -> LoadedLibraryResult<Arc<[crate::TrackId]>> {
        let state = self.loaded.read_state()?;
        self.slots
            .iter()
            .map(|slot| {
                state
                    .tracks
                    .get_slot(*slot)
                    .map(|track| track.id.clone())
                    .ok_or(crate::LoadedLibraryError::StaleTrackSelection)
            })
            .collect::<LoadedLibraryResult<Vec<_>>>()
            .map(Into::into)
    }

    fn download_status(&self) -> LoadedLibraryResult<DownloadStatus> {
        let state = self.loaded.read_state()?;
        let mut total = 0usize;
        let mut downloaded = 0usize;
        for slot in self.slots.iter() {
            let Some(track) = state.tracks.get_slot(*slot) else {
                continue;
            };
            total += 1;
            downloaded += usize::from(state.downloaded_files.contains_key(&track.id));
        }
        Ok(DownloadStatus {
            any: downloaded > 0,
            complete: total > 0 && downloaded == total,
        })
    }

    pub fn position(&self, track_id: &crate::TrackId) -> LoadedLibraryResult<Option<u32>> {
        let state = self.loaded.read_state()?;
        let Some(slot) = state.tracks.slot(track_id) else {
            return Ok(None);
        };
        Ok(self
            .slots
            .iter()
            .position(|candidate| *candidate == slot)
            .map(|position| u32::try_from(position).expect("Track list position fits GTK")))
    }

    pub fn sorted(&self, sort: TrackSort, descending: bool) -> LoadedLibraryResult<Self> {
        if self.sorted_by == Some((sort, descending)) {
            return Ok(self.clone());
        }
        self.filtered_sorted(|_| true, sort, descending)
    }

    pub fn filtered_sorted(
        &self,
        mut include: impl FnMut(&Track) -> bool,
        sort: TrackSort,
        descending: bool,
    ) -> LoadedLibraryResult<Self> {
        let state = self.loaded.read_state()?;
        let mut tracks = self
            .slots
            .iter()
            .copied()
            .filter_map(|slot| state.tracks.get_slot(slot).map(|track| (slot, track)))
            .filter(|(_, track)| include(track))
            .collect::<Vec<_>>();
        if sort == TrackSort::Title {
            tracks.sort_by_cached_key(|(_, track)| {
                (
                    track.title.to_ascii_lowercase(),
                    track.album.to_ascii_lowercase(),
                    track.disc_number,
                    track.track_number,
                    &track.id,
                )
            });
            if descending {
                tracks.reverse();
            }
        } else {
            tracks.sort_by(|(_, left), (_, right)| compare_tracks(left, right, sort, descending));
        }
        let slots = tracks.into_iter().map(|(slot, _)| slot).collect::<Vec<_>>();
        drop(state);
        Ok(Self::new(
            Arc::clone(&self.loaded),
            slots.into(),
            Some((sort, descending)),
        ))
    }

    pub fn filtered_in_source_order(
        &self,
        mut include: impl FnMut(&Track) -> bool,
        descending: bool,
    ) -> LoadedLibraryResult<Self> {
        let state = self.loaded.read_state()?;
        let mut slots = self
            .slots
            .iter()
            .copied()
            .filter(|slot| state.tracks.get_slot(*slot).is_some_and(&mut include))
            .collect::<Vec<_>>();
        if descending {
            slots.reverse();
        }
        drop(state);
        Ok(Self::new(Arc::clone(&self.loaded), slots.into(), None))
    }

    /// Inserts, removes, or repositions one current Track without rebuilding
    /// the projection from every Track in the loaded Library.
    ///
    /// The caller supplies the route's membership decision. The ordered value
    /// remains a compact slot array; only row binding materializes a Track.
    pub fn with_current_track(
        &self,
        track_id: &crate::TrackId,
        include: impl FnOnce(&Track) -> bool,
    ) -> LoadedLibraryResult<Option<TrackListChange>> {
        let state = self.loaded.read_state()?;
        let Some(slot) = state.tracks.slot(track_id) else {
            return Ok(None);
        };
        let track = state
            .tracks
            .get_slot(slot)
            .expect("a current Track slot must resolve");
        let previous_position = self.slots.iter().position(|candidate| *candidate == slot);
        let included = include(track);
        let Some((sort, descending)) = self.sorted_by else {
            if previous_position.is_some() != included {
                return Ok(None);
            }
            drop(state);
            let position = previous_position
                .map(|position| u32::try_from(position).expect("Track position fits GTK"));
            return Ok(Some(TrackListChange {
                tracks: self.clone(),
                previous_position: position,
                position,
                order_changed: false,
            }));
        };
        let position_is_current = previous_position.is_some_and(|position| {
            let after_previous = position == 0
                || compare_tracks(
                    state
                        .tracks
                        .get_slot(self.slots[position - 1])
                        .expect("a Track projection slot must resolve"),
                    track,
                    sort,
                    descending,
                ) != Ordering::Greater;
            let before_next = position + 1 == self.slots.len()
                || compare_tracks(
                    track,
                    state
                        .tracks
                        .get_slot(self.slots[position + 1])
                        .expect("a Track projection slot must resolve"),
                    sort,
                    descending,
                ) != Ordering::Greater;
            after_previous && before_next
        });
        if (!included && previous_position.is_none()) || (included && position_is_current) {
            drop(state);
            let position = previous_position
                .map(|position| u32::try_from(position).expect("Track position fits GTK"));
            return Ok(Some(TrackListChange {
                tracks: self.clone(),
                previous_position: position,
                position,
                order_changed: false,
            }));
        }

        let mut slots = self.slots.to_vec();
        if let Some(position) = previous_position {
            slots.remove(position);
        }
        let position = if included {
            let position = slots
                .binary_search_by(|candidate| {
                    compare_tracks(
                        state
                            .tracks
                            .get_slot(*candidate)
                            .expect("a Track projection slot must resolve"),
                        track,
                        sort,
                        descending,
                    )
                })
                .unwrap_or_else(|position| position);
            slots.insert(position, slot);
            Some(position)
        } else {
            None
        };
        drop(state);
        Ok(Some(TrackListChange {
            tracks: Self::new(
                Arc::clone(&self.loaded),
                slots.into(),
                Some((sort, descending)),
            ),
            previous_position: previous_position
                .map(|position| u32::try_from(position).expect("Track position fits GTK")),
            position: position
                .map(|position| u32::try_from(position).expect("Track position fits GTK")),
            order_changed: true,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct AlbumSummary {
    pub album: Arc<Album>,
    pub artwork: AlbumArtwork,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct ArtistSummary {
    pub artist: Arc<Artist>,
    pub representative_albums: Arc<[AlbumArtwork]>,
    pub album_count: u32,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct GenreSummary {
    pub genre: Arc<Genre>,
    pub representative_albums: Arc<[AlbumArtwork]>,
    pub album_count: u32,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct MoodSummary {
    pub mood: Arc<Mood>,
    pub representative_albums: Arc<[AlbumArtwork]>,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct PlaylistSummary {
    pub playlist: Arc<Playlist>,
    pub genres: Arc<[Arc<Genre>]>,
    pub representative_albums: Arc<[AlbumArtwork]>,
    pub track_count: u32,
    pub duration_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct AlbumDetail {
    pub summary: AlbumSummary,
    pub tracks: TrackList,
}

#[derive(Clone, Debug)]
pub struct ArtistOverview {
    pub summary: ArtistSummary,
    pub favorite_tracks: TrackList,
    pub albums: Arc<[AlbumSummary]>,
    pub appears_on: Arc<[AlbumSummary]>,
}

#[derive(Clone, Debug)]
pub struct ArtistDiscography {
    pub summary: ArtistSummary,
    pub albums: Arc<[AlbumSummary]>,
    pub appears_on: Arc<[AlbumSummary]>,
}

#[derive(Clone, Debug)]
pub struct ArtistTracks {
    pub summary: ArtistSummary,
    pub tracks: TrackList,
}

#[derive(Clone, Debug)]
pub struct GenreDetail {
    pub summary: GenreSummary,
    pub tracks: TrackList,
}

#[derive(Clone, Debug)]
pub struct MoodDetail {
    pub summary: MoodSummary,
    pub tracks: TrackList,
}

#[derive(Clone, Debug)]
pub struct PlaylistEntryItem {
    pub occurrence_id: String,
    pub track: Track,
}

#[derive(Clone, Debug)]
struct PlaylistEntrySlot {
    occurrence_id: String,
    track: TrackSlot,
}

#[derive(Clone, Debug)]
pub struct PlaylistEntryList {
    loaded: Arc<LoadedLibrary>,
    entries: Arc<[PlaylistEntrySlot]>,
}

impl PlaylistEntryList {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn occurrence_id(&self, position: usize) -> Option<&str> {
        self.entries
            .get(position)
            .map(|entry| entry.occurrence_id.as_str())
    }

    pub fn position_of_occurrence(&self, occurrence_id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.occurrence_id == occurrence_id)
    }

    pub fn occurrence_for_track(
        &self,
        positions: &[u32],
        track_id: &crate::TrackId,
    ) -> LoadedLibraryResult<Option<String>> {
        let state = self.loaded.read_state()?;
        let Some(track) = state.tracks.slot(track_id) else {
            return Ok(None);
        };
        Ok(positions.iter().find_map(|position| {
            self.entries
                .get(*position as usize)
                .filter(|entry| entry.track == track)
                .map(|entry| entry.occurrence_id.clone())
        }))
    }

    pub fn entry(&self, position: usize) -> LoadedLibraryResult<Option<PlaylistEntryItem>> {
        let Some(entry) = self.entries.get(position) else {
            return Ok(None);
        };
        let state = self.loaded.read_state()?;
        Ok(state
            .tracks
            .get_slot(entry.track)
            .cloned()
            .map(|track| PlaylistEntryItem {
                occurrence_id: entry.occurrence_id.clone(),
                track,
            }))
    }

    pub fn positions_by(
        &self,
        mut include: impl FnMut(&Track) -> bool,
        mut compare: impl FnMut(&Track, &Track) -> Ordering,
    ) -> LoadedLibraryResult<Vec<u32>> {
        let state = self.loaded.read_state()?;
        let mut positions = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| {
                state
                    .tracks
                    .get_slot(entry.track)
                    .is_some_and(&mut include)
                    .then(|| u32::try_from(position).expect("Playlist position fits GTK"))
            })
            .collect::<Vec<_>>();
        positions.sort_by(|left, right| {
            let left = &self.entries[*left as usize];
            let right = &self.entries[*right as usize];
            compare(
                state
                    .tracks
                    .get_slot(left.track)
                    .expect("prepared Playlist entry Track must resolve"),
                state
                    .tracks
                    .get_slot(right.track)
                    .expect("prepared Playlist entry Track must resolve"),
            )
        });
        Ok(positions)
    }

    pub fn track_list(&self) -> TrackList {
        TrackList::new(
            Arc::clone(&self.loaded),
            self.entries
                .iter()
                .map(|entry| entry.track)
                .collect::<Vec<_>>()
                .into(),
            None,
        )
    }

    pub fn selected_track_list(&self, positions: &[u32]) -> TrackList {
        TrackList::new(
            Arc::clone(&self.loaded),
            positions
                .iter()
                .filter_map(|position| self.entries.get(*position as usize))
                .map(|entry| entry.track)
                .collect::<Vec<_>>()
                .into(),
            None,
        )
    }
}

#[derive(Clone, Debug)]
pub struct PlaylistDetail {
    pub summary: PlaylistSummary,
    pub entries: PlaylistEntryList,
}

#[derive(Clone, Debug, Default)]
pub struct FolderContents {
    pub folders: Arc<[Folder]>,
    pub tracks: Arc<[Track]>,
}

impl LoadedLibrary {
    pub fn empty_track_list(self: &Arc<Self>) -> TrackList {
        TrackList::new(Arc::clone(self), Arc::from([]), None)
    }

    pub fn track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
        sort: TrackSort,
        descending: bool,
    ) -> LoadedLibraryResult<TrackList> {
        let list = {
            let state = self.read_state()?;
            match music_folder_id {
                Some(folder_id) => TrackList::new(
                    Arc::clone(self),
                    state
                        .music_folder_tracks
                        .get(folder_id)
                        .cloned()
                        .unwrap_or_default()
                        .into(),
                    Some((TrackSort::Title, false)),
                ),
                None => TrackList::new(
                    Arc::clone(self),
                    state.tracks.live_slots().collect::<Vec<_>>().into(),
                    None,
                ),
            }
        };
        list.sorted(sort, descending)
    }

    pub fn favorite_track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
        sort: TrackSort,
        descending: bool,
    ) -> LoadedLibraryResult<TrackList> {
        let list = {
            let state = self.read_state()?;
            TrackList::new(
                Arc::clone(self),
                state
                    .tracks
                    .live_slots()
                    .filter(|slot| {
                        state.tracks.get_slot(*slot).is_some_and(|track| {
                            track.favorite && track_in_scope(track, music_folder_id)
                        })
                    })
                    .collect::<Vec<_>>()
                    .into(),
                None,
            )
        };
        list.sorted(sort, descending)
    }

    pub fn favorite_download_track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let mut seen = HashSet::new();
        let mut slots = state
            .tracks
            .live_slots()
            .filter(|slot| {
                state
                    .tracks
                    .get_slot(*slot)
                    .is_some_and(|track| track.favorite && track_in_scope(track, music_folder_id))
            })
            .collect::<Vec<_>>();
        seen.extend(slots.iter().copied());
        slots.extend(
            state
                .albums
                .values()
                .filter(|album| album.favorite)
                .flat_map(|album| album_track_slots(&state, &album.id))
                .filter(|slot| {
                    seen.insert(*slot)
                        && state
                            .tracks
                            .get_slot(*slot)
                            .is_some_and(|track| track_in_scope(track, music_folder_id))
                }),
        );
        slots.extend(
            state
                .artists
                .values()
                .filter(|artist| artist.favorite)
                .flat_map(|artist| artist_track_slots(&state, &artist.id))
                .filter(|slot| {
                    seen.insert(*slot)
                        && state
                            .tracks
                            .get_slot(*slot)
                            .is_some_and(|track| track_in_scope(track, music_folder_id))
                }),
        );
        Ok(TrackList::new(Arc::clone(self), slots.into(), None))
    }

    pub fn all_playlist_track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let mut seen = HashSet::new();
        let slots = state
            .playlists
            .values()
            .flat_map(|playlist| playlist.entries.iter())
            .filter_map(|entry| state.tracks.slot(&entry.track_id))
            .filter(|slot| {
                seen.insert(*slot)
                    && state
                        .tracks
                        .get_slot(*slot)
                        .is_some_and(|track| track_in_scope(track, music_folder_id))
            })
            .collect::<Vec<_>>();
        Ok(TrackList::new(Arc::clone(self), slots.into(), None))
    }

    pub fn latest_album_track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
        album_limit: usize,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let mut albums = state
            .albums
            .values()
            .filter(|album| {
                album_track_slots(&state, &album.id).iter().any(|slot| {
                    state
                        .tracks
                        .get_slot(*slot)
                        .is_some_and(|track| track_in_scope(track, music_folder_id))
                })
            })
            .collect::<Vec<_>>();
        albums.sort_by(|left, right| {
            right
                .date_added
                .cmp(&left.date_added)
                .then_with(|| right.release_date.cmp(&left.release_date))
                .then_with(|| right.year.cmp(&left.year))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen = HashSet::new();
        let slots = albums
            .into_iter()
            .take(album_limit)
            .flat_map(|album| album_track_slots(&state, &album.id))
            .filter(|slot| {
                seen.insert(*slot)
                    && state
                        .tracks
                        .get_slot(*slot)
                        .is_some_and(|track| track_in_scope(track, music_folder_id))
            })
            .collect::<Vec<_>>();
        Ok(TrackList::new(Arc::clone(self), slots.into(), None))
    }

    pub fn history_track_list(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        Ok(TrackList::new(
            Arc::clone(self),
            state
                .recent_plays
                .iter()
                .filter_map(|play| state.tracks.slot(&play.track_id))
                .filter(|slot| {
                    state
                        .tracks
                        .get_slot(*slot)
                        .is_some_and(|track| track_in_scope(track, music_folder_id))
                })
                .collect::<Vec<_>>()
                .into(),
            None,
        ))
    }

    pub fn track_selection(self: &Arc<Self>, track_id: &TrackId) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = state
            .tracks
            .slot(track_id)
            .filter(|slot| state.tracks.get_slot(*slot).is_some())
            .into_iter()
            .collect::<Vec<_>>()
            .into();
        Ok(TrackList::new(Arc::clone(self), slots, None))
    }

    pub fn album_track_selection(
        self: &Arc<Self>,
        album_id: &AlbumId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::Album {
                loaded: Arc::clone(self),
                id: album_id.clone(),
                music_folder_id: music_folder_id.cloned(),
            },
        }
    }

    pub fn artist_track_selection(
        self: &Arc<Self>,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::Artist {
                loaded: Arc::clone(self),
                id: artist_id.clone(),
                music_folder_id: music_folder_id.cloned(),
            },
        }
    }

    pub fn genre_track_selection(
        self: &Arc<Self>,
        genre_id: &GenreId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::Genre {
                loaded: Arc::clone(self),
                id: genre_id.clone(),
                music_folder_id: music_folder_id.cloned(),
            },
        }
    }

    pub fn mood_track_selection(
        self: &Arc<Self>,
        mood_id: &MoodId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::Mood {
                loaded: Arc::clone(self),
                id: mood_id.clone(),
                music_folder_id: music_folder_id.cloned(),
            },
        }
    }

    pub fn playlist_track_selection(self: &Arc<Self>, playlist_id: &PlaylistId) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::Playlist {
                loaded: Arc::clone(self),
                id: playlist_id.clone(),
            },
        }
    }

    pub fn smart_playlist_track_selection(
        self: &Arc<Self>,
        smart_playlist_id: &SmartPlaylistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> TrackSelection {
        TrackSelection {
            kind: TrackSelectionKind::SmartPlaylist {
                loaded: Arc::clone(self),
                id: smart_playlist_id.clone(),
                music_folder_id: music_folder_id.cloned(),
            },
        }
    }

    pub fn is_album_downloaded(
        &self,
        album_id: &AlbumId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<bool> {
        let state = self.read_state()?;
        Ok(state.download_coverage.album(album_id, music_folder_id))
    }

    pub fn is_artist_downloaded(
        &self,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<bool> {
        let state = self.read_state()?;
        Ok(state.download_coverage.artist(artist_id, music_folder_id))
    }

    pub fn is_genre_downloaded(
        &self,
        genre_id: &GenreId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<bool> {
        let state = self.read_state()?;
        Ok(state.download_coverage.genre(genre_id, music_folder_id))
    }

    pub fn is_mood_downloaded(
        &self,
        mood_id: &MoodId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<bool> {
        let state = self.read_state()?;
        Ok(state.download_coverage.mood(mood_id, music_folder_id))
    }

    pub fn is_playlist_downloaded(&self, playlist_id: &PlaylistId) -> LoadedLibraryResult<bool> {
        let state = self.read_state()?;
        Ok(state.download_coverage.playlist(playlist_id))
    }

    fn album_tracks(
        self: &Arc<Self>,
        album_id: &AlbumId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = album_track_slots(&state, album_id);
        Ok(TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            None,
        ))
    }

    fn artist_tracks(
        self: &Arc<Self>,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = artist_track_slots(&state, artist_id);
        Ok(TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            None,
        ))
    }

    fn genre_tracks(
        self: &Arc<Self>,
        genre_id: &GenreId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = genre_track_slots(&state, genre_id);
        Ok(TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            None,
        ))
    }

    fn mood_tracks(
        self: &Arc<Self>,
        mood_id: &MoodId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        let slots = mood_track_slots(&state, mood_id);
        Ok(TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            None,
        ))
    }

    fn playlist_tracks(
        self: &Arc<Self>,
        playlist_id: &PlaylistId,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        Ok(TrackList::new(
            Arc::clone(self),
            state
                .playlists
                .get(playlist_id)
                .into_iter()
                .flat_map(|playlist| playlist.entries.iter())
                .filter_map(|entry| state.tracks.slot(&entry.track_id))
                .filter(|slot| state.tracks.get_slot(*slot).is_some())
                .collect::<Vec<_>>()
                .into(),
            None,
        ))
    }

    pub fn albums(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[AlbumSummary]>> {
        let state = self.read_state()?;
        let mut albums = state
            .albums
            .values()
            .filter_map(|album| album_summary(&state, album, music_folder_id))
            .collect::<Vec<_>>();
        albums.sort_by(|left, right| compare_albums(&left.album, &right.album));
        Ok(albums.into())
    }

    pub fn album_summary(
        &self,
        album_id: &AlbumId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<AlbumSummary>> {
        let state = self.read_state()?;
        Ok(state
            .albums
            .get(album_id)
            .and_then(|album| album_summary(&state, album, music_folder_id)))
    }

    pub fn artists(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[ArtistSummary]>> {
        let state = self.read_state()?;
        let mut artists = state
            .artists
            .iter()
            .filter_map(|(_, artist)| {
                let (summary, has_artist_credit, _) =
                    artist_summary_and_credits(&state, artist, music_folder_id)?;
                has_artist_credit.then_some(summary)
            })
            .collect::<Vec<_>>();
        artists.sort_by(|left, right| {
            text_cmp(&left.artist.name, &right.artist.name)
                .then(left.artist.id.cmp(&right.artist.id))
        });
        Ok(artists.into())
    }

    pub fn artist_summary(
        &self,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<ArtistSummary>> {
        let state = self.read_state()?;
        Ok(state
            .artists
            .get(artist_id)
            .and_then(|artist| artist_summary(&state, artist, music_folder_id)))
    }

    pub fn album_artists(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[ArtistSummary]>> {
        let state = self.read_state()?;
        let mut artists = state
            .artists
            .iter()
            .filter_map(|(_, artist)| {
                let (summary, _, has_album_artist_credit) =
                    artist_summary_and_credits(&state, artist, music_folder_id)?;
                has_album_artist_credit.then_some(summary)
            })
            .collect::<Vec<_>>();
        artists.sort_by(|left, right| {
            text_cmp(&left.artist.name, &right.artist.name)
                .then(left.artist.id.cmp(&right.artist.id))
        });
        Ok(artists.into())
    }

    pub fn genres(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[GenreSummary]>> {
        let state = self.read_state()?;
        let mut seen_tracks = vec![false; state.tracks.slot_capacity()];
        let mut seen_albums = vec![false; state.albums.slot_capacity()];
        let mut genres = state
            .genres
            .iter()
            .filter_map(|(_, genre)| {
                seen_tracks.fill(false);
                seen_albums.fill(false);
                genre_summary_with_seen(
                    &state,
                    genre,
                    music_folder_id,
                    |slot| !std::mem::replace(&mut seen_tracks[slot.index()], true),
                    |slot| !std::mem::replace(&mut seen_albums[slot.index()], true),
                )
            })
            .collect::<Vec<_>>();
        genres.sort_by(|left, right| {
            text_cmp(&left.genre.name, &right.genre.name).then(left.genre.id.cmp(&right.genre.id))
        });
        Ok(genres.into())
    }

    pub fn genre_summary(
        &self,
        genre_id: &GenreId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<GenreSummary>> {
        let state = self.read_state()?;
        Ok(state
            .genres
            .get(genre_id)
            .and_then(|genre| genre_summary(&state, genre, music_folder_id)))
    }

    pub fn moods(
        &self,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[MoodSummary]>> {
        let state = self.read_state()?;
        let mut moods = state
            .moods
            .values()
            .filter_map(|mood| mood_summary(&state, mood, music_folder_id))
            .collect::<Vec<_>>();
        moods.sort_by(|left, right| {
            text_cmp(&left.mood.name, &right.mood.name).then(left.mood.id.cmp(&right.mood.id))
        });
        Ok(moods.into())
    }

    pub fn playlists(&self) -> LoadedLibraryResult<Arc<[PlaylistSummary]>> {
        let state = self.read_state()?;
        let mut playlists = state
            .playlists
            .values()
            .map(|playlist| playlist_summary(&state, playlist))
            .collect::<Vec<_>>();
        playlists.sort_by(|left, right| {
            text_cmp(&left.playlist.name, &right.playlist.name)
                .then(left.playlist.id.cmp(&right.playlist.id))
        });
        Ok(playlists.into())
    }

    pub fn playlist_summary(
        &self,
        playlist_id: &PlaylistId,
    ) -> LoadedLibraryResult<Option<PlaylistSummary>> {
        let state = self.read_state()?;
        Ok(state
            .playlists
            .get(playlist_id)
            .map(|playlist| playlist_summary(&state, playlist)))
    }

    pub fn music_folders(&self) -> LoadedLibraryResult<Arc<[Arc<MusicFolder>]>> {
        let state = self.read_state()?;
        let mut folders = state.music_folders.values().cloned().collect::<Vec<_>>();
        folders
            .sort_by(|left, right| text_cmp(&left.name, &right.name).then(left.id.cmp(&right.id)));
        Ok(folders.into())
    }

    pub fn album_detail(
        self: &Arc<Self>,
        album_id: &AlbumId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<AlbumDetail>> {
        let state = self.read_state()?;
        let Some(album) = state.albums.get(album_id) else {
            return Ok(None);
        };
        let Some(summary) = album_summary(&state, album, music_folder_id) else {
            return Ok(None);
        };
        let slots = album_track_slots(&state, album_id);
        Ok(Some(AlbumDetail {
            tracks: TrackList::new(
                Arc::clone(self),
                resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
                Some((TrackSort::TrackNumber, false)),
            ),
            summary,
        }))
    }

    pub fn album_details(
        self: &Arc<Self>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Arc<[AlbumDetail]>> {
        let state = self.read_state()?;
        Ok(state
            .albums
            .values()
            .filter_map(|album| {
                let slots = album_track_slots(&state, &album.id);
                Some(AlbumDetail {
                    summary: album_summary(&state, album, music_folder_id)?,
                    tracks: TrackList::new(
                        Arc::clone(self),
                        resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
                        Some((TrackSort::TrackNumber, false)),
                    ),
                })
            })
            .collect::<Vec<_>>()
            .into())
    }

    pub fn artist_overview(
        self: &Arc<Self>,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<ArtistOverview>> {
        let state = self.read_state()?;
        let Some(artist) = state.artists.get(artist_id) else {
            return Ok(None);
        };
        let Some(summary) = artist_summary(&state, artist, music_folder_id) else {
            return Ok(None);
        };
        let (albums, appears_on) = artist_album_items(&state, artist_id, music_folder_id);
        let favorite_tracks = TrackList::new(
            Arc::clone(self),
            artist_favorite_track_slots(&state, artist_id, music_folder_id),
            Some((TrackSort::Album, false)),
        );
        Ok(Some(ArtistOverview {
            summary,
            favorite_tracks,
            albums: albums.into(),
            appears_on: appears_on.into(),
        }))
    }

    pub fn artist_favorite_tracks(
        self: &Arc<Self>,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<TrackList> {
        let state = self.read_state()?;
        Ok(TrackList::new(
            Arc::clone(self),
            artist_favorite_track_slots(&state, artist_id, music_folder_id),
            Some((TrackSort::Album, false)),
        ))
    }

    pub fn artist_discography(
        &self,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<ArtistDiscography>> {
        let state = self.read_state()?;
        let Some(artist) = state.artists.get(artist_id) else {
            return Ok(None);
        };
        let Some(summary) = artist_summary(&state, artist, music_folder_id) else {
            return Ok(None);
        };
        let (albums, appears_on) = artist_album_items(&state, artist_id, music_folder_id);
        Ok(Some(ArtistDiscography {
            summary,
            albums: albums.into(),
            appears_on: appears_on.into(),
        }))
    }

    pub fn artist_track_detail(
        self: &Arc<Self>,
        artist_id: &ArtistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<ArtistTracks>> {
        let state = self.read_state()?;
        let Some(artist) = state.artists.get(artist_id) else {
            return Ok(None);
        };
        let Some(summary) = artist_summary(&state, artist, music_folder_id) else {
            return Ok(None);
        };
        let slots = artist_track_slots(&state, artist_id);
        let tracks = TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            Some((TrackSort::Album, false)),
        );
        Ok(Some(ArtistTracks { summary, tracks }))
    }

    pub fn genre_detail(
        self: &Arc<Self>,
        genre_id: &GenreId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<GenreDetail>> {
        let state = self.read_state()?;
        let Some(genre) = state.genres.get(genre_id) else {
            return Ok(None);
        };
        let Some(summary) = genre_summary(&state, genre, music_folder_id) else {
            return Ok(None);
        };
        let slots = genre_track_slots(&state, genre_id);
        let tracks = TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            Some((TrackSort::Album, false)),
        );
        if music_folder_id.is_some() && tracks.is_empty() {
            return Ok(None);
        }
        Ok(Some(GenreDetail { summary, tracks }))
    }

    pub fn mood_detail(
        self: &Arc<Self>,
        mood_id: &MoodId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> LoadedLibraryResult<Option<MoodDetail>> {
        let state = self.read_state()?;
        let Some(mood) = state.moods.get(mood_id) else {
            return Ok(None);
        };
        let slots = mood_track_slots(&state, mood_id);
        let tracks = TrackList::new(
            Arc::clone(self),
            resolve_scoped_slots(Some(&slots), &state.tracks, music_folder_id),
            Some((TrackSort::Album, false)),
        );
        if music_folder_id.is_some() && tracks.is_empty() {
            return Ok(None);
        }
        let Some(summary) = mood_summary(&state, mood, music_folder_id) else {
            return Ok(None);
        };
        Ok(Some(MoodDetail { summary, tracks }))
    }

    pub fn playlist_detail(
        self: &Arc<Self>,
        playlist_id: &PlaylistId,
    ) -> LoadedLibraryResult<Option<PlaylistDetail>> {
        let state = self.read_state()?;
        let Some(playlist) = state.playlists.get(playlist_id) else {
            return Ok(None);
        };
        Ok(Some(PlaylistDetail {
            summary: playlist_summary(&state, playlist),
            entries: PlaylistEntryList {
                loaded: Arc::clone(self),
                entries: playlist
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        let track = state.tracks.slot(&entry.track_id)?;
                        state.tracks.get_slot(track)?;
                        Some(PlaylistEntrySlot {
                            occurrence_id: entry.occurrence_id.clone(),
                            track,
                        })
                    })
                    .collect::<Vec<_>>()
                    .into(),
            },
        }))
    }

    pub fn local_folder_contents(
        &self,
        folder_id: Option<&FolderId>,
    ) -> LoadedLibraryResult<Option<FolderContents>> {
        let state = self.read_state()?;
        if let Some(folder_id) = folder_id
            && !state.local_folders.contains_key(folder_id)
        {
            return Ok(None);
        }
        let child_ids = state.local_folder_children.get(&folder_id.cloned());
        let folders = child_ids
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|id| state.local_folders.get(id))
            .map(|folder| folder.folder.clone())
            .collect::<Vec<_>>()
            .into();
        let tracks = resolve_index(
            state
                .local_folder_tracks
                .get(&folder_id.cloned())
                .map(Vec::as_slice),
            &state.tracks,
        );
        Ok(Some(FolderContents { folders, tracks }))
    }
}

fn resolve_index<Id, Value>(
    ids: Option<&[ItemSlot<Id>]>,
    values: &LoadedItems<Id, Value>,
) -> Arc<[Value]>
where
    Id: Clone + Eq + std::hash::Hash,
    Value: Clone,
{
    ids.unwrap_or(&[])
        .iter()
        .filter_map(|slot| values.get_slot(*slot).cloned())
        .collect::<Vec<_>>()
        .into()
}

fn album_track_slots(state: &LoadedState, album_id: &AlbumId) -> Vec<TrackSlot> {
    sorted_track_slots(
        state
            .albums
            .get(album_id)
            .into_iter()
            .flat_map(|album| album.tracks.iter().copied()),
        state,
        TrackSort::TrackNumber,
    )
}

pub(super) fn artist_track_slots(state: &LoadedState, artist_id: &ArtistId) -> Vec<TrackSlot> {
    sorted_track_slots(
        artist_relationship_track_slots(state, artist_id),
        state,
        TrackSort::Album,
    )
}

fn artist_relationship_track_slots(state: &LoadedState, artist_id: &ArtistId) -> Vec<TrackSlot> {
    let Some(artist) = state.artists.get(artist_id) else {
        return Vec::new();
    };
    unique_track_slots(
        artist.tracks.iter().copied().chain(
            artist
                .albums
                .iter()
                .filter_map(|slot| state.albums.get_slot(*slot))
                .flat_map(|album| album.tracks.iter().copied()),
        ),
        state,
    )
}

pub(super) fn genre_track_slots(state: &LoadedState, genre_id: &GenreId) -> Vec<TrackSlot> {
    sorted_track_slots(
        genre_relationship_track_slots(state, genre_id),
        state,
        TrackSort::Album,
    )
}

fn genre_relationship_track_slots(state: &LoadedState, genre_id: &GenreId) -> Vec<TrackSlot> {
    let Some(genre) = state.genres.get(genre_id) else {
        return Vec::new();
    };
    unique_track_slots(genre_relationship_tracks(state, genre), state)
}

fn genre_relationship_tracks<'a>(
    state: &'a LoadedState,
    genre: &'a LoadedGenre,
) -> impl Iterator<Item = TrackSlot> + 'a {
    genre.tracks.iter().copied().chain(
        genre
            .albums
            .iter()
            .filter_map(|slot| state.albums.get_slot(*slot))
            .flat_map(|album| album.tracks.iter().copied()),
    )
}

fn mood_track_slots(state: &LoadedState, mood_id: &MoodId) -> Vec<TrackSlot> {
    sorted_track_slots(
        mood_relationship_track_slots(state, mood_id),
        state,
        TrackSort::Album,
    )
}

fn mood_relationship_track_slots(state: &LoadedState, mood_id: &MoodId) -> Vec<TrackSlot> {
    unique_track_slots(
        state
            .moods
            .get(mood_id)
            .into_iter()
            .flat_map(|mood| mood.tracks.iter().copied()),
        state,
    )
}

fn unique_track_slots(
    slots: impl IntoIterator<Item = TrackSlot>,
    state: &LoadedState,
) -> Vec<TrackSlot> {
    let mut seen = HashSet::new();
    slots
        .into_iter()
        .filter(|slot| state.tracks.get_slot(*slot).is_some() && seen.insert(*slot))
        .collect()
}

fn sorted_track_slots(
    slots: impl IntoIterator<Item = TrackSlot>,
    state: &LoadedState,
    sort: TrackSort,
) -> Vec<TrackSlot> {
    let mut slots = unique_track_slots(slots, state);
    slots.sort_by(|left, right| {
        compare_tracks(
            state
                .tracks
                .get_slot(*left)
                .expect("projection Track slot must resolve"),
            state
                .tracks
                .get_slot(*right)
                .expect("projection Track slot must resolve"),
            sort,
            false,
        )
    });
    slots
}

fn artist_album_slots(state: &LoadedState, artist_id: &ArtistId) -> Vec<ItemSlot<AlbumId>> {
    let Some(artist) = state.artists.get(artist_id) else {
        return Vec::new();
    };
    unique_album_slots(
        artist.albums.iter().copied().chain(
            artist
                .tracks
                .iter()
                .filter_map(|slot| state.tracks.get_slot(*slot))
                .filter_map(|track| track.album_id.as_ref())
                .filter_map(|album_id| state.albums.slot(album_id)),
        ),
        state,
    )
}

fn genre_relationship_albums<'a>(
    state: &'a LoadedState,
    genre: &'a LoadedGenre,
) -> impl Iterator<Item = ItemSlot<AlbumId>> + 'a {
    genre.albums.iter().copied().chain(
        genre
            .tracks
            .iter()
            .filter_map(|slot| state.tracks.get_slot(*slot))
            .filter_map(|track| track.album_id.as_ref())
            .filter_map(|album_id| state.albums.slot(album_id)),
    )
}

fn unique_album_slots(
    slots: impl IntoIterator<Item = ItemSlot<AlbumId>>,
    state: &LoadedState,
) -> Vec<ItemSlot<AlbumId>> {
    let mut seen = HashSet::new();
    slots
        .into_iter()
        .filter(|slot| state.albums.get_slot(*slot).is_some() && seen.insert(*slot))
        .collect()
}

fn resolve_scoped_slots(
    slots: Option<&[TrackSlot]>,
    tracks: &LoadedItems<crate::TrackId, Track>,
    music_folder_id: Option<&MusicFolderId>,
) -> Arc<[TrackSlot]> {
    slots
        .unwrap_or(&[])
        .iter()
        .copied()
        .filter(|slot| {
            tracks
                .get_slot(*slot)
                .is_some_and(|track| track_in_scope(track, music_folder_id))
        })
        .collect::<Vec<_>>()
        .into()
}

pub(crate) fn track_in_scope(track: &Track, music_folder_id: Option<&MusicFolderId>) -> bool {
    music_folder_id.is_none_or(|folder_id| {
        track
            .relations
            .music_folders
            .iter()
            .any(|candidate| candidate == folder_id)
    })
}

pub(crate) fn album_in_scope(
    state: &LoadedState,
    album: &Album,
    music_folder_id: Option<&MusicFolderId>,
) -> bool {
    music_folder_id.is_none()
        || state.albums.get(&album.id).is_some_and(|relationship| {
            relationship.tracks.iter().any(|slot| {
                state
                    .tracks
                    .get_slot(*slot)
                    .is_some_and(|track| track_in_scope(track, music_folder_id))
            })
        })
}

fn album_projection(
    state: &LoadedState,
    album: &LoadedAlbum,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<(u32, u32, Option<Track>)> {
    let mut track_count = 0u32;
    let mut duration_seconds = 0u32;
    let mut representative_track = None;
    let mut found_scoped_track = false;
    for slot in &album.tracks {
        let Some(track) = state.tracks.get_slot(*slot) else {
            continue;
        };
        if !track_in_scope(track, music_folder_id) {
            continue;
        }
        found_scoped_track = true;
        track_count = track_count.saturating_add(1);
        duration_seconds = duration_seconds.saturating_add(track.duration_seconds);
        let current_has_artwork = representative_track.as_ref().is_some_and(track_has_artwork);
        if representative_track.is_none() || (!current_has_artwork && track_has_artwork(track)) {
            representative_track = Some(track.clone());
        }
    }
    if music_folder_id.is_some() && !found_scoped_track {
        return None;
    }
    Some((track_count, duration_seconds, representative_track))
}

pub(crate) fn album_artwork(
    state: &LoadedState,
    album: &LoadedAlbum,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<AlbumArtwork> {
    let (_, _, representative_track) = album_projection(state, album, music_folder_id)?;
    Some(AlbumArtwork {
        album: Arc::clone(&album.album),
        representative_track,
    })
}

pub(crate) fn album_summary(
    state: &LoadedState,
    album: &LoadedAlbum,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<AlbumSummary> {
    let (track_count, duration_seconds, representative_track) =
        album_projection(state, album, music_folder_id)?;
    Some(AlbumSummary {
        album: Arc::clone(&album.album),
        artwork: AlbumArtwork {
            album: Arc::clone(&album.album),
            representative_track,
        },
        track_count,
        duration_seconds,
    })
}

fn track_has_artwork(track: &Track) -> bool {
    track.local_artwork.is_some() || track.image_ref.is_some()
}

fn artist_summary_and_credits(
    state: &LoadedState,
    artist: &LoadedArtist,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<(ArtistSummary, bool, bool)> {
    let mut track_count = 0u32;
    let mut duration_seconds = 0u32;
    let mut found_scoped_track = false;
    let mut has_artist_credit = false;
    let mut has_album_artist_credit = false;
    for slot in artist_relationship_track_slots(state, &artist.id) {
        let Some(track) = state.tracks.get_slot(slot) else {
            continue;
        };
        if !track_in_scope(track, music_folder_id) {
            continue;
        }
        found_scoped_track = true;
        track_count = track_count.saturating_add(1);
        duration_seconds = duration_seconds.saturating_add(track.duration_seconds);
        has_artist_credit |= track
            .relations
            .artists
            .iter()
            .any(|credit| credit.id == artist.id);
        has_album_artist_credit |= track
            .relations
            .album_artists
            .iter()
            .any(|credit| credit.id == artist.id);
    }
    if music_folder_id.is_some() && !found_scoped_track {
        return None;
    }

    let mut albums = artist_album_slots(state, &artist.id)
        .into_iter()
        .filter_map(|slot| state.albums.get_slot(slot))
        .filter_map(|album| {
            let summary = album_summary(state, album, music_folder_id)?;
            has_album_artist_credit |= album
                .relations
                .album_artists
                .iter()
                .any(|credit| credit.id == artist.id);
            Some((
                album_is_primary_for_artist(state, album, &artist.id),
                summary,
            ))
        })
        .collect::<Vec<_>>();
    albums.sort_by(|(left_primary, left), (right_primary, right)| {
        right_primary
            .cmp(left_primary)
            .then(left.album.year.cmp(&right.album.year))
            .then_with(|| compare_albums(&left.album, &right.album))
    });
    let album_count = u32::try_from(albums.len()).unwrap_or(u32::MAX);
    let representative_albums = albums
        .iter()
        .take(ARTIST_ARTWORK_LIMIT)
        .map(|(_, album)| album.artwork.clone())
        .collect::<Vec<_>>()
        .into();
    Some((
        ArtistSummary {
            artist: Arc::clone(&artist.artist),
            representative_albums,
            album_count,
            track_count,
            duration_seconds,
        },
        has_artist_credit,
        has_album_artist_credit,
    ))
}

fn artist_summary(
    state: &LoadedState,
    artist: &LoadedArtist,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<ArtistSummary> {
    artist_summary_and_credits(state, artist, music_folder_id).map(|(summary, _, _)| summary)
}

fn artist_album_items(
    state: &LoadedState,
    artist_id: &ArtistId,
    music_folder_id: Option<&MusicFolderId>,
) -> (Vec<AlbumSummary>, Vec<AlbumSummary>) {
    let mut albums = Vec::new();
    let mut appears_on = Vec::new();
    for album_slot in artist_album_slots(state, artist_id) {
        let Some(album) = state.albums.get_slot(album_slot) else {
            continue;
        };
        let Some(item) = album_summary(state, album, music_folder_id) else {
            continue;
        };
        if album_is_primary_for_artist(state, album, artist_id) {
            albums.push(item);
        } else {
            appears_on.push(item);
        }
    }
    albums.sort_by(compare_artist_album_item);
    appears_on.sort_by(compare_artist_album_item);
    (albums, appears_on)
}

fn compare_artist_album_item(left: &AlbumSummary, right: &AlbumSummary) -> Ordering {
    left.album
        .year
        .cmp(&right.album.year)
        .then_with(|| text_cmp(&left.album.title, &right.album.title))
        .then(left.album.id.cmp(&right.album.id))
}

fn artist_favorite_track_slots(
    state: &LoadedState,
    artist_id: &ArtistId,
    music_folder_id: Option<&MusicFolderId>,
) -> Arc<[TrackSlot]> {
    let mut slots = artist_relationship_track_slots(state, artist_id)
        .into_iter()
        .filter(|slot| {
            state
                .tracks
                .get_slot(*slot)
                .is_some_and(|track| track.favorite && track_in_scope(track, music_folder_id))
        })
        .collect::<Vec<_>>();
    slots.sort_by(|left, right| {
        compare_tracks(
            state
                .tracks
                .get_slot(*left)
                .expect("favorite Artist Track slot must resolve"),
            state
                .tracks
                .get_slot(*right)
                .expect("favorite Artist Track slot must resolve"),
            TrackSort::Album,
            false,
        )
    });
    slots.into()
}

fn album_is_primary_for_artist(state: &LoadedState, album: &Album, artist_id: &ArtistId) -> bool {
    distinct_album_artist_ids(album).any(|id| id == artist_id)
        || state
            .albums
            .get(&album.id)
            .into_iter()
            .flat_map(|relationship| relationship.tracks.iter())
            .filter_map(|slot| state.tracks.get_slot(*slot))
            .flat_map(|track| track.relations.album_artists.iter())
            .any(|credit| &credit.id == artist_id)
}

fn distinct_album_artist_ids(album: &Album) -> impl Iterator<Item = &ArtistId> {
    album
        .relations
        .album_artists
        .iter()
        .chain(album.relations.artists.iter())
        .map(|credit| &credit.id)
}

pub(crate) fn genre_summary(
    state: &LoadedState,
    genre: &LoadedGenre,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<GenreSummary> {
    let mut seen_tracks = HashSet::new();
    let mut seen_albums = HashSet::new();
    genre_summary_with_seen(
        state,
        genre,
        music_folder_id,
        |slot| seen_tracks.insert(slot),
        |slot| seen_albums.insert(slot),
    )
}

fn genre_summary_with_seen<TrackSeen, AlbumSeen>(
    state: &LoadedState,
    genre: &LoadedGenre,
    music_folder_id: Option<&MusicFolderId>,
    mut track_not_seen: TrackSeen,
    mut album_not_seen: AlbumSeen,
) -> Option<GenreSummary>
where
    TrackSeen: FnMut(TrackSlot) -> bool,
    AlbumSeen: FnMut(ItemSlot<AlbumId>) -> bool,
{
    if music_folder_id.is_none() && genre.tracks.is_empty() && genre.albums.is_empty() {
        return None;
    }
    let mut track_count = 0u32;
    let mut duration_seconds = 0u32;
    let mut found_scoped_track = false;
    for slot in genre_relationship_tracks(state, genre) {
        if !track_not_seen(slot) {
            continue;
        }
        let Some(track) = state.tracks.get_slot(slot) else {
            continue;
        };
        if !track_in_scope(track, music_folder_id) {
            continue;
        }
        found_scoped_track = true;
        track_count = track_count.saturating_add(1);
        duration_seconds = duration_seconds.saturating_add(track.duration_seconds);
    }
    if music_folder_id.is_some() && !found_scoped_track {
        return None;
    }

    let mut albums = genre_relationship_albums(state, genre)
        .filter(|slot| album_not_seen(*slot))
        .filter_map(|slot| state.albums.get_slot(slot))
        .filter_map(|album| {
            if !album_in_scope(state, album, music_folder_id) {
                return None;
            }
            let direct = album
                .relations
                .genres
                .iter()
                .any(|credit| credit.id == genre.id);
            Some((direct, album))
        })
        .collect::<Vec<_>>();
    let album_count = u32::try_from(albums.len()).unwrap_or(u32::MAX);
    let mut compare = |(left_direct, left): &(bool, &LoadedAlbum),
                       (right_direct, right): &(bool, &LoadedAlbum)| {
        right_direct
            .cmp(left_direct)
            .then_with(|| compare_albums(left, right))
    };
    if albums.len() > COLLECTION_ARTWORK_LIMIT {
        albums.select_nth_unstable_by(COLLECTION_ARTWORK_LIMIT, &mut compare);
        albums.truncate(COLLECTION_ARTWORK_LIMIT);
    }
    albums.sort_unstable_by(compare);
    let representative_albums = albums
        .into_iter()
        .filter_map(|(_, album)| album_artwork(state, album, music_folder_id))
        .collect::<Vec<_>>()
        .into();
    Some(GenreSummary {
        genre: Arc::clone(&genre.genre),
        representative_albums,
        album_count,
        track_count,
        duration_seconds,
    })
}

fn mood_summary(
    state: &LoadedState,
    mood: &LoadedMood,
    music_folder_id: Option<&MusicFolderId>,
) -> Option<MoodSummary> {
    let mut track_count = 0u32;
    let mut duration_seconds = 0u32;
    let mut found_scoped_track = false;
    let mut seen_albums = HashSet::new();
    let mut representative_albums = Vec::new();
    for slot in mood_relationship_track_slots(state, &mood.id) {
        let Some(track) = state.tracks.get_slot(slot) else {
            continue;
        };
        if !track_in_scope(track, music_folder_id) {
            continue;
        }
        found_scoped_track = true;
        track_count = track_count.saturating_add(1);
        duration_seconds = duration_seconds.saturating_add(track.duration_seconds);
        if representative_albums.len() >= COLLECTION_ARTWORK_LIMIT {
            continue;
        }
        let Some(album_id) = &track.album_id else {
            continue;
        };
        if !seen_albums.insert(album_id.clone()) {
            continue;
        }
        let Some(album) = state.albums.get(album_id) else {
            continue;
        };
        if let Some(artwork) = album_artwork(state, album, music_folder_id) {
            representative_albums.push(artwork);
        }
    }
    if music_folder_id.is_some() && !found_scoped_track {
        return None;
    }
    Some(MoodSummary {
        mood: Arc::clone(&mood.mood),
        representative_albums: representative_albums.into(),
        track_count,
        duration_seconds,
    })
}

fn playlist_summary(state: &LoadedState, playlist: &LoadedPlaylist) -> PlaylistSummary {
    let mut track_count = 0u32;
    let mut duration_seconds = 0u32;
    let mut genre_counts = HashMap::<GenreId, usize>::new();
    let mut seen_albums = HashSet::new();
    let mut representative_albums = Vec::new();
    for entry in playlist.entries.iter() {
        let Some(track) = state.tracks.get(&entry.track_id) else {
            continue;
        };
        track_count = track_count.saturating_add(1);
        duration_seconds = duration_seconds.saturating_add(track.duration_seconds);
        let mut seen_genres = HashSet::new();
        for genre_id in track.relations.genres.iter().map(|genre| &genre.id) {
            if seen_genres.insert(genre_id) {
                *genre_counts.entry(genre_id.clone()).or_default() += 1;
            }
        }
        if representative_albums.len() >= COLLECTION_ARTWORK_LIMIT {
            continue;
        }
        let Some(album_id) = &track.album_id else {
            continue;
        };
        if !seen_albums.insert(album_id.clone()) {
            continue;
        }
        let Some(album) = state.albums.get(album_id) else {
            continue;
        };
        if let Some(artwork) = album_artwork(state, album, None) {
            representative_albums.push(artwork);
        }
    }

    let mut genres = genre_counts.into_iter().collect::<Vec<_>>();
    genres.sort_by(|(left_id, left_count), (right_id, right_count)| {
        right_count.cmp(left_count).then_with(|| {
            let left = state.genres.get(left_id);
            let right = state.genres.get(right_id);
            match (left, right) {
                (Some(left), Some(right)) => {
                    text_cmp(&left.name, &right.name).then(left.id.cmp(&right.id))
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => left_id.cmp(right_id),
            }
        })
    });
    PlaylistSummary {
        playlist: Arc::clone(&playlist.playlist),
        genres: genres
            .into_iter()
            .take(2)
            .filter_map(|(id, _)| state.genres.get(&id))
            .map(|genre| Arc::clone(&genre.genre))
            .collect::<Vec<_>>()
            .into(),
        representative_albums: representative_albums.into(),
        track_count,
        duration_seconds,
    }
}
fn compare_albums(left: &crate::Album, right: &crate::Album) -> Ordering {
    text_cmp(&left.title, &right.title)
        .then_with(|| text_cmp(&left.artist, &right.artist))
        .then(left.year.cmp(&right.year))
        .then(left.id.cmp(&right.id))
}

pub fn compare_tracks(left: &Track, right: &Track, field: TrackSort, descending: bool) -> Ordering {
    let missing =
        track_sort_value_missing(left, field).cmp(&track_sort_value_missing(right, field));
    if missing != Ordering::Equal {
        return missing;
    }

    let primary = match field {
        TrackSort::TrackNumber => left
            .disc_number
            .cmp(&right.disc_number)
            .then(left.track_number.cmp(&right.track_number)),
        TrackSort::Artist => text_cmp(&left.artist, &right.artist),
        TrackSort::AlbumArtist => text_cmp(
            left.relations
                .album_artists
                .first()
                .map_or(left.artist.as_str(), |credit| credit.name.as_str()),
            right
                .relations
                .album_artists
                .first()
                .map_or(right.artist.as_str(), |credit| credit.name.as_str()),
        ),
        TrackSort::Album => text_cmp(&left.album, &right.album),
        TrackSort::Year => left.year.cmp(&right.year),
        TrackSort::ReleaseDate => left.release_date.cmp(&right.release_date),
        TrackSort::DateAdded => left.date_added.cmp(&right.date_added),
        TrackSort::LastPlayed => left.last_played.cmp(&right.last_played),
        TrackSort::PlayCount => left.play_count.cmp(&right.play_count),
        TrackSort::UserRating => left.user_rating.cmp(&right.user_rating),
        TrackSort::Genre => text_cmp(first_genre_name(left), first_genre_name(right)),
        TrackSort::Bpm => left.bpm.cmp(&right.bpm),
        TrackSort::Duration => left.duration_seconds.cmp(&right.duration_seconds),
        TrackSort::Favorite => left.favorite.cmp(&right.favorite),
        TrackSort::Title => text_cmp(&left.title, &right.title),
    }
    .then_with(|| text_cmp(&left.album, &right.album))
    .then(left.disc_number.cmp(&right.disc_number))
    .then(left.track_number.cmp(&right.track_number))
    .then_with(|| text_cmp(&left.title, &right.title))
    .then(left.id.cmp(&right.id));

    if descending {
        primary.reverse()
    } else {
        primary
    }
}

fn track_sort_value_missing(track: &Track, field: TrackSort) -> bool {
    match field {
        TrackSort::ReleaseDate => track.release_date.is_none(),
        TrackSort::DateAdded => track.date_added.is_none(),
        TrackSort::LastPlayed => track.last_played.is_none(),
        TrackSort::PlayCount => track.play_count.is_none(),
        TrackSort::UserRating => track.user_rating.is_none(),
        TrackSort::Bpm => track.bpm.is_none(),
        TrackSort::Title
        | TrackSort::TrackNumber
        | TrackSort::Artist
        | TrackSort::AlbumArtist
        | TrackSort::Album
        | TrackSort::Year
        | TrackSort::Genre
        | TrackSort::Duration
        | TrackSort::Favorite => false,
    }
}

fn first_genre_name(track: &Track) -> &str {
    track
        .relations
        .genres
        .iter()
        .min_by(|left, right| text_cmp(&left.name, &right.name))
        .map_or("", |genre| genre.name.as_str())
}

fn text_cmp(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}
