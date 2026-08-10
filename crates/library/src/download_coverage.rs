use std::collections::{HashMap, HashSet};

use crate::{
    AlbumId, ArtistId, GenreId, LoadedState, MoodId, MusicFolderId, PlaylistId, SmartPlaylistId,
    Track, TrackId,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DownloadCollection {
    Album(AlbumId),
    Artist(ArtistId),
    Genre(GenreId),
    Mood(MoodId),
    Playlist(PlaylistId),
    SmartPlaylist(SmartPlaylistId),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DownloadCoverageKey {
    collection: DownloadCollection,
    music_folder_id: Option<MusicFolderId>,
}

#[derive(Clone, Copy, Debug, Default)]
struct DownloadCoverageCount {
    total: usize,
    downloaded: usize,
}

#[derive(Debug, Default)]
pub(crate) struct DownloadCoverage {
    counts: HashMap<DownloadCoverageKey, DownloadCoverageCount>,
    memberships: HashMap<TrackId, Vec<DownloadCoverageKey>>,
    #[cfg(test)]
    status_reads: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    smart_playlist_membership_writes: std::sync::atomic::AtomicUsize,
}

impl DownloadCoverage {
    pub(crate) fn build(state: &LoadedState) -> Self {
        let mut coverage = Self::default();
        for track in state.tracks.values() {
            let mut collections = HashSet::new();
            if let Some(album_id) = &track.album_id {
                collections.insert(DownloadCollection::Album(album_id.clone()));
                if let Some(album) = state.albums.get(album_id) {
                    collections.extend(
                        album
                            .relations
                            .album_artists
                            .iter()
                            .chain(&album.relations.artists)
                            .map(|credit| DownloadCollection::Artist(credit.id.clone())),
                    );
                    collections.extend(
                        album
                            .relations
                            .genres
                            .iter()
                            .map(|credit| DownloadCollection::Genre(credit.id.clone())),
                    );
                }
            }
            collections.extend(
                track
                    .relations
                    .artists
                    .iter()
                    .chain(&track.relations.album_artists)
                    .map(|credit| DownloadCollection::Artist(credit.id.clone())),
            );
            collections.extend(
                track
                    .relations
                    .genres
                    .iter()
                    .map(|credit| DownloadCollection::Genre(credit.id.clone())),
            );
            collections.extend(
                track
                    .relations
                    .moods
                    .iter()
                    .map(|credit| DownloadCollection::Mood(credit.id.clone())),
            );
            if let Some(playlists) = state.track_playlists.get(&track.id) {
                collections.extend(playlists.iter().filter_map(|slot| {
                    state
                        .playlists
                        .get_slot(*slot)
                        .map(|playlist| DownloadCollection::Playlist(playlist.playlist.id.clone()))
                }));
            }

            let mut scopes = HashSet::from([None]);
            scopes.extend(track.relations.music_folders.iter().cloned().map(Some));
            let downloaded = state.downloaded_files.contains_key(&track.id);
            for music_folder_id in scopes {
                for collection in &collections {
                    coverage.add(
                        track.id.clone(),
                        DownloadCoverageKey {
                            collection: collection.clone(),
                            music_folder_id: music_folder_id.clone(),
                        },
                        downloaded,
                    );
                }
            }
        }

        coverage.add_smart_playlists(state);
        coverage
    }

    pub(crate) fn rebuild_smart_playlists(&mut self, state: &LoadedState) {
        self.counts
            .retain(|key, _| !matches!(key.collection, DownloadCollection::SmartPlaylist(_)));
        for keys in self.memberships.values_mut() {
            keys.retain(|key| !matches!(key.collection, DownloadCollection::SmartPlaylist(_)));
        }
        self.memberships.retain(|_, keys| !keys.is_empty());
        self.add_smart_playlists(state);
    }

    fn replace_smart_playlist_memberships(
        &mut self,
        track: &Track,
        changes: &[(SmartPlaylistId, bool)],
        downloaded: bool,
    ) {
        #[cfg(test)]
        self.smart_playlist_membership_writes
            .fetch_add(changes.len(), std::sync::atomic::Ordering::Relaxed);
        for (id, member) in changes {
            for music_folder_id in
                std::iter::once(None).chain(track.relations.music_folders.iter().cloned().map(Some))
            {
                let key = DownloadCoverageKey {
                    collection: DownloadCollection::SmartPlaylist(id.clone()),
                    music_folder_id,
                };
                if *member {
                    self.add(track.id.clone(), key, downloaded);
                } else {
                    self.remove(&track.id, &key, downloaded);
                }
            }
        }
    }

    fn add_smart_playlists(&mut self, state: &LoadedState) {
        let smart_playlist_ids = state.smart_playlists.keys().cloned().collect::<Vec<_>>();
        let scopes = std::iter::once(None)
            .chain(state.music_folders.keys().cloned().map(Some))
            .collect::<Vec<_>>();
        for smart_playlist_id in smart_playlist_ids {
            for music_folder_id in &scopes {
                for slot in crate::smart_playlists::download_track_slots(
                    state,
                    &smart_playlist_id,
                    music_folder_id.as_ref(),
                ) {
                    let Some(track) = state.tracks.get_slot(slot) else {
                        continue;
                    };
                    self.add(
                        track.id.clone(),
                        DownloadCoverageKey {
                            collection: DownloadCollection::SmartPlaylist(
                                smart_playlist_id.clone(),
                            ),
                            music_folder_id: music_folder_id.clone(),
                        },
                        state.downloaded_files.contains_key(&track.id),
                    );
                }
            }
        }
    }

    pub(crate) fn replace_downloaded(&mut self, downloaded: &HashSet<TrackId>) {
        for count in self.counts.values_mut() {
            count.downloaded = 0;
        }
        for track_id in downloaded {
            self.set_downloaded(track_id, true);
        }
    }

    pub(crate) fn set_downloaded(&mut self, track_id: &TrackId, downloaded: bool) {
        let Some(keys) = self.memberships.get(track_id) else {
            return;
        };
        for key in keys {
            let count = self
                .counts
                .get_mut(key)
                .expect("download membership aggregate must exist");
            if downloaded {
                count.downloaded = count.downloaded.saturating_add(1).min(count.total);
            } else {
                count.downloaded = count.downloaded.saturating_sub(1);
            }
        }
    }

    pub(crate) fn album(&self, id: &AlbumId, music_folder_id: Option<&MusicFolderId>) -> bool {
        self.is_downloaded(
            DownloadCollection::Album(id.clone()),
            music_folder_id.cloned(),
        )
    }

    pub(crate) fn artist(&self, id: &ArtistId, music_folder_id: Option<&MusicFolderId>) -> bool {
        self.is_downloaded(
            DownloadCollection::Artist(id.clone()),
            music_folder_id.cloned(),
        )
    }

    pub(crate) fn genre(&self, id: &GenreId, music_folder_id: Option<&MusicFolderId>) -> bool {
        self.is_downloaded(
            DownloadCollection::Genre(id.clone()),
            music_folder_id.cloned(),
        )
    }

    pub(crate) fn mood(&self, id: &MoodId, music_folder_id: Option<&MusicFolderId>) -> bool {
        self.is_downloaded(
            DownloadCollection::Mood(id.clone()),
            music_folder_id.cloned(),
        )
    }

    pub(crate) fn playlist(&self, id: &PlaylistId) -> bool {
        self.is_downloaded(DownloadCollection::Playlist(id.clone()), None)
    }

    pub(crate) fn smart_playlist(
        &self,
        id: &SmartPlaylistId,
        music_folder_id: Option<&MusicFolderId>,
    ) -> bool {
        self.is_downloaded(
            DownloadCollection::SmartPlaylist(id.clone()),
            music_folder_id.cloned(),
        )
    }

    fn add(&mut self, track_id: TrackId, key: DownloadCoverageKey, downloaded: bool) {
        let memberships = self.memberships.entry(track_id).or_default();
        if memberships.contains(&key) {
            return;
        }
        memberships.push(key.clone());
        let count = self.counts.entry(key).or_default();
        count.total = count.total.saturating_add(1);
        if downloaded {
            count.downloaded = count.downloaded.saturating_add(1);
        }
    }

    fn remove(&mut self, track_id: &TrackId, key: &DownloadCoverageKey, downloaded: bool) {
        let Some(memberships) = self.memberships.get_mut(track_id) else {
            return;
        };
        let Some(index) = memberships.iter().position(|accepted| accepted == key) else {
            return;
        };
        memberships.swap_remove(index);
        let remove_memberships = memberships.is_empty();
        if remove_memberships {
            self.memberships.remove(track_id);
        }

        let Some(count) = self.counts.get_mut(key) else {
            return;
        };
        count.total = count.total.saturating_sub(1);
        if downloaded {
            count.downloaded = count.downloaded.saturating_sub(1);
        }
        if count.total == 0 {
            self.counts.remove(key);
        }
    }

    fn is_downloaded(
        &self,
        collection: DownloadCollection,
        music_folder_id: Option<MusicFolderId>,
    ) -> bool {
        self.status(collection, music_folder_id).1
    }

    pub(crate) fn status(
        &self,
        collection: DownloadCollection,
        music_folder_id: Option<MusicFolderId>,
    ) -> (bool, bool) {
        #[cfg(test)]
        self.status_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(count) = self.counts.get(&DownloadCoverageKey {
            collection,
            music_folder_id,
        }) else {
            return (false, false);
        };
        (
            count.downloaded > 0,
            count.total > 0 && count.downloaded == count.total,
        )
    }

    #[cfg(test)]
    pub(crate) fn smart_playlist_membership_writes(&self) -> usize {
        self.smart_playlist_membership_writes
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub(crate) fn rebuild_download_coverage(state: &mut LoadedState) {
    state.download_coverage = DownloadCoverage::build(state);
}

pub(crate) fn rebuild_smart_playlist_download_coverage(state: &mut LoadedState) {
    let mut coverage = std::mem::take(&mut state.download_coverage);
    coverage.rebuild_smart_playlists(state);
    state.download_coverage = coverage;
}

pub(crate) fn replace_smart_playlist_download_memberships(
    state: &mut LoadedState,
    track: &Track,
    changes: &[(SmartPlaylistId, bool)],
) {
    let downloaded = state.downloaded_files.contains_key(&track.id);
    let mut coverage = std::mem::take(&mut state.download_coverage);
    coverage.replace_smart_playlist_memberships(track, changes, downloaded);
    state.download_coverage = coverage;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lookup_reads_one_aggregate_regardless_of_collection_size() {
        let album_id = AlbumId::fake(1);
        let key = DownloadCoverageKey {
            collection: DownloadCollection::Album(album_id.clone()),
            music_folder_id: None,
        };
        let mut coverage = DownloadCoverage::default();
        coverage.counts.insert(
            key.clone(),
            DownloadCoverageCount {
                total: 10_000,
                downloaded: 1,
            },
        );
        coverage
            .memberships
            .insert(TrackId::fake(2), std::iter::repeat_n(key, 10_000).collect());

        assert_eq!(
            coverage.status(DownloadCollection::Album(album_id), None),
            (true, false)
        );
        assert_eq!(
            coverage
                .status_reads
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }
}
