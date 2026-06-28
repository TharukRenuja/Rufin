use serde::{Deserialize, Serialize};

use crate::domain::{
    AlbumId, ArtistId, GenreId, ImageRef, MusicFolderId, PlaylistId, ServerId, SmartPlaylistId,
    Track, TrackId,
};

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct QueueEntryId(String);

impl QueueEntryId {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        assert!(!value.is_empty(), "QueueEntryId cannot be empty");
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for QueueEntryId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShuffleState {
    pub enabled: bool,
    pub seed: u64,
}

impl Default for ShuffleState {
    fn default() -> Self {
        Self {
            enabled: false,
            seed: 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct QueueShuffleKey(String);

impl QueueShuffleKey {
    fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        assert!(!value.is_empty(), "QueueShuffleKey cannot be empty");
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AutoDjReason {
    Similarity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlaylistEntrySortDescriptor {
    Position,
    Title,
    Album,
    Artist,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TrackSortDescriptor {
    Album,
    Artist,
    DateAdded,
    Title,
    TrackNumber,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SearchSortDescriptor {
    Relevance,
    Title,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SmartPlaylistSortDescriptor {
    Definition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ArtistTrackScope {
    MainArtist,
    AllCredits,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlaySourceDescriptor {
    Album {
        album_id: AlbumId,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    Playlist {
        playlist_id: PlaylistId,
    },
    SmartPlaylist {
        smart_playlist_id: SmartPlaylistId,
        definition_fingerprint: String,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    FolderLoaded {
        path: Vec<String>,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    ArtistTracks {
        artist_id: ArtistId,
        scope: ArtistTrackScope,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    GenreTracks {
        genre_id: GenreId,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    FavoriteTracks {
        selected_music_folder_id: Option<MusicFolderId>,
    },
    SearchResults {
        query: String,
        selected_music_folder_id: Option<MusicFolderId>,
    },
    GlobalTracks {
        selected_music_folder_id: Option<MusicFolderId>,
    },
    HomeCollection {
        section_id: String,
        source: Box<PlaySourceDescriptor>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SourceOrder {
    Canonical,
    LibraryDisplayed {
        filter_key: Option<String>,
        sort: TrackSortDescriptor,
    },
    PlaylistDisplayed {
        query: Option<String>,
        sort: PlaylistEntrySortDescriptor,
        descending: bool,
    },
    FolderDisplayed {
        query: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter_key: Option<String>,
        sort: TrackSortDescriptor,
    },
    SearchDisplayed {
        sort: SearchSortDescriptor,
    },
    SmartPlaylistDefinition {
        sort: SmartPlaylistSortDescriptor,
        limit: Option<usize>,
        skip_count: usize,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlaySourceKey {
    pub descriptor: PlaySourceDescriptor,
    pub order: SourceOrder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueSourceInput {
    pub source_key: PlaySourceKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueReplacement {
    pub source: QueueReplacementSource,
    pub items: Vec<QueueItemInput>,
    pub anchor: QueueAnchor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueInsertion {
    pub source: QueueInsertionSource,
    pub items: Vec<QueueItemInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueInsertionSource {
    Manual,
    AutoDj {
        generated_from_track_id: TrackId,
        reason: AutoDjReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueReplacementSource {
    Source(QueueSourceInput),
    Manual,
    Random { seed: u64, requested_limit: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueItemInput {
    Source {
        track: Track,
        source_index: usize,
    },
    Manual {
        track: Track,
    },
    Generated {
        track: Track,
        generated_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueAnchor {
    Position(usize),
    SourcePosition { position: usize, track_id: TrackId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueError {
    EmptyReplacement,
    AnchorNotFound,
    WrongItemKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QueueEntryOrigin {
    Source {
        shuffle_key: QueueShuffleKey,
    },
    Manual {
        shuffle_key: QueueShuffleKey,
    },
    Random {
        seed: u64,
        random_index: usize,
        shuffle_key: QueueShuffleKey,
    },
    AutoDj {
        generated_from_track_id: TrackId,
        generated_index: usize,
        reason: AutoDjReason,
        shuffle_key: QueueShuffleKey,
    },
    RestoredUnknown {
        restored_index: usize,
        shuffle_key: QueueShuffleKey,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub track_id: TrackId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<AlbumId>,
    pub title: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<ArtistId>,
    pub album: String,
    #[serde(default)]
    pub year: u16,
    pub duration_seconds: u32,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<QueueEntryOrigin>,
}

impl QueueEntry {
    fn from_track(id: QueueEntryId, track: &Track) -> Self {
        Self {
            id,
            track_id: track.id.clone(),
            album_id: Some(track.album_id.clone()),
            title: track.title.clone(),
            artist: track.artist.clone(),
            artist_id: track.artist_id.clone(),
            album: track.album.clone(),
            year: track.year,
            duration_seconds: track.duration_seconds,
            favorite: track.favorite,
            image_ref: track.image_ref.clone(),
            local_path: track.local_path.clone(),
            source_format: track.source_format.clone(),
            origin: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueSnapshot {
    pub server_id: ServerId,
    pub entries: Vec<QueueEntry>,
    pub current_index: Option<usize>,
    pub repeat_mode: RepeatMode,
    pub shuffle: ShuffleState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shuffle_order: Vec<usize>,
    pub progress_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct QueueEngine {
    server_id: ServerId,
    entries: Vec<QueueEntry>,
    current_index: Option<usize>,
    repeat_mode: RepeatMode,
    shuffle: ShuffleState,
    shuffle_order: Vec<usize>,
    shuffle_position: Option<usize>,
    next_entry_number: u64,
    progress_seconds: u32,
}

impl QueueEngine {
    pub fn new(server_id: ServerId) -> Self {
        Self {
            server_id,
            entries: Vec::new(),
            current_index: None,
            repeat_mode: RepeatMode::All,
            shuffle: ShuffleState::default(),
            shuffle_order: Vec::new(),
            shuffle_position: None,
            next_entry_number: 1,
            progress_seconds: 0,
        }
    }

    pub fn restore(snapshot: QueueSnapshot) -> Self {
        let mut entries = snapshot.entries;
        repair_missing_origins(&mut entries);
        let current_index = snapshot
            .current_index
            .filter(|index| *index < entries.len());
        let mut engine = Self {
            server_id: snapshot.server_id,
            next_entry_number: next_entry_number(&entries),
            entries,
            current_index,
            repeat_mode: snapshot.repeat_mode,
            shuffle: snapshot.shuffle,
            shuffle_order: snapshot.shuffle_order,
            shuffle_position: None,
            progress_seconds: snapshot.progress_seconds,
        };
        if engine.shuffle.enabled
            && valid_shuffle_order(&engine.shuffle_order, engine.entries.len())
        {
            engine.sync_shuffle_position();
        } else {
            engine.rebuild_shuffle_order();
        }
        engine
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            server_id: self.server_id.clone(),
            entries: self.entries.clone(),
            current_index: self.current_index,
            repeat_mode: self.repeat_mode,
            shuffle: self.shuffle.clone(),
            shuffle_order: if self.shuffle.enabled {
                self.shuffle_order.clone()
            } else {
                Vec::new()
            },
            progress_seconds: self.progress_seconds,
        }
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn server_id(&self) -> &ServerId {
        &self.server_id
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.current_index.and_then(|index| self.entries.get(index))
    }

    pub fn next_after_end_of_stream(&self) -> Option<&QueueEntry> {
        self.next_index(RepeatOneBehavior::Stay)
            .and_then(|index| self.entries.get(index))
    }

    pub fn remaining_after_current(&self) -> usize {
        if self.shuffle.enabled {
            return self
                .shuffle_position
                .map(|position| self.shuffle_order.len().saturating_sub(position + 1))
                .unwrap_or_default();
        }
        self.current_index
            .map(|index| self.entries.len().saturating_sub(index + 1))
            .unwrap_or_default()
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }

    pub fn shuffle(&self) -> &ShuffleState {
        &self.shuffle
    }

    pub fn progress_seconds(&self) -> u32 {
        self.progress_seconds
    }

    pub fn set_progress_seconds(&mut self, progress_seconds: u32) {
        self.progress_seconds = progress_seconds;
    }

    pub fn set_track_favorite(&mut self, track_id: &TrackId, favorite: bool) {
        for entry in &mut self.entries {
            if entry.track_id == *track_id {
                entry.favorite = favorite;
            }
        }
    }

    pub fn play_now(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        self.entries = vec![entry];
        self.current_index = Some(0);
        self.progress_seconds = 0;
        self.rebuild_shuffle_order();
        id
    }

    pub fn replace_all(
        &mut self,
        replacement: QueueReplacement,
    ) -> Result<QueueEntryId, QueueError> {
        if replacement.items.is_empty() {
            return Err(QueueError::EmptyReplacement);
        }

        match replacement.source {
            QueueReplacementSource::Source(source) => {
                self.replace_all_source(source, replacement.items, replacement.anchor)
            }
            QueueReplacementSource::Manual => {
                self.replace_all_manual(replacement.items, replacement.anchor)
            }
            QueueReplacementSource::Random {
                seed,
                requested_limit: _,
            } => self.replace_all_random(seed, replacement.items, replacement.anchor),
        }
    }

    pub fn play_next(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        self.insert_entries_next(vec![entry]);
        id
    }

    pub fn append(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        self.append_entries_last(vec![entry]);
        id
    }

    pub fn insert_next(
        &mut self,
        insertion: QueueInsertion,
    ) -> Result<Vec<QueueEntryId>, QueueError> {
        let entries = self.entries_from_insertion(insertion)?;
        Ok(self.insert_entries_next(entries))
    }

    fn insert_entries_next(&mut self, entries: Vec<QueueEntry>) -> Vec<QueueEntryId> {
        let ids = entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return ids;
        }
        let insert_index = self
            .current_index
            .map_or(0, |index| index.saturating_add(1))
            .min(self.entries.len());
        let inserted_count = ids.len();
        let previous_current_index = self.current_index;
        let previous_shuffle_order = self.shuffle_order.clone();
        let previous_len = self.entries.len();
        self.entries.splice(insert_index..insert_index, entries);
        if self.current_index.is_none() {
            self.current_index = Some(insert_index);
        }
        if self.shuffle.enabled
            && previous_current_index.is_some()
            && valid_shuffle_order(&previous_shuffle_order, previous_len)
        {
            self.shuffle_order = previous_shuffle_order
                .into_iter()
                .map(|index| {
                    if index >= insert_index {
                        index.saturating_add(inserted_count)
                    } else {
                        index
                    }
                })
                .collect();
            let Some(current_position) = self
                .shuffle_order
                .iter()
                .position(|index| Some(*index) == previous_current_index)
            else {
                self.rebuild_shuffle_order();
                return ids;
            };

            let insert_end = insert_index.saturating_add(inserted_count);
            let mut inserted_block = (insert_index..insert_end).collect::<Vec<_>>();
            let first_inserted = inserted_block.remove(0);
            let seed = self.shuffle.seed;
            inserted_block.sort_by_key(|index| stable_entry_sort_key(&self.entries, seed, *index));
            inserted_block.insert(0, first_inserted);
            let splice_start = current_position.saturating_add(1);
            self.shuffle_order
                .splice(splice_start..splice_start, inserted_block);
            self.sync_shuffle_position();
        } else {
            self.rebuild_shuffle_order();
        }
        ids
    }

    pub fn append_last(
        &mut self,
        insertion: QueueInsertion,
    ) -> Result<Vec<QueueEntryId>, QueueError> {
        let entries = self.entries_from_insertion(insertion)?;
        Ok(self.append_entries_last(entries))
    }

    fn append_entries_last(&mut self, entries: Vec<QueueEntry>) -> Vec<QueueEntryId> {
        let ids = entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return ids;
        }
        let first_inserted_index = self.entries.len();
        let previous_current_index = self.current_index;
        let previous_shuffle_order = self.shuffle_order.clone();
        let previous_len = self.entries.len();
        self.entries.extend(entries);
        if self.current_index.is_none() {
            self.current_index = Some(first_inserted_index);
        }
        if self.shuffle.enabled
            && previous_current_index.is_some()
            && valid_shuffle_order(&previous_shuffle_order, previous_len)
        {
            self.shuffle_order = previous_shuffle_order;
            let mut appended_indices =
                (first_inserted_index..self.entries.len()).collect::<Vec<_>>();
            let seed = self.shuffle.seed;
            appended_indices
                .sort_by_key(|index| stable_entry_sort_key(&self.entries, seed, *index));
            self.shuffle_order.extend(appended_indices);
            self.sync_shuffle_position();
        } else {
            self.rebuild_shuffle_order();
        }
        ids
    }

    pub fn remove(&mut self, entry_id: &QueueEntryId) -> Option<QueueEntry> {
        let remove_index = self
            .entries
            .iter()
            .position(|entry| entry.id == *entry_id)?;
        let removed = self.entries.remove(remove_index);

        self.current_index = match (self.current_index, self.entries.is_empty()) {
            (_, true) => None,
            (Some(current), false) if remove_index < current => Some(current - 1),
            (Some(current), false) if remove_index == current && current >= self.entries.len() => {
                Some(self.entries.len() - 1)
            }
            (current, false) => current,
        };

        self.rebuild_shuffle_order();
        Some(removed)
    }

    pub fn activate(&mut self, entry_id: &QueueEntryId) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.id == *entry_id) else {
            return false;
        };
        self.current_index = Some(index);
        self.progress_seconds = 0;
        self.sync_shuffle_position();
        true
    }

    pub fn activate_source_occurrence(
        &mut self,
        source_key: &PlaySourceKey,
        source_index: usize,
        track_id: &TrackId,
    ) -> Option<QueueEntryId> {
        let shuffle_key = source_shuffle_key(source_key, source_index, track_id);
        let entry_id = self
            .entries
            .iter()
            .find(|entry| {
                entry.track_id == *track_id
                    && matches!(
                        entry.origin.as_ref(),
                        Some(QueueEntryOrigin::Source {
                            shuffle_key: entry_shuffle_key,
                        }) if *entry_shuffle_key == shuffle_key
                    )
            })?
            .id
            .clone();
        self.activate(&entry_id).then_some(entry_id)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_index = None;
        self.shuffle_order.clear();
        self.shuffle_position = None;
        self.progress_seconds = 0;
    }

    pub fn clear_except_current(&mut self) -> bool {
        let Some(current) = self.current().cloned() else {
            self.clear();
            return false;
        };
        self.entries = vec![current];
        self.current_index = Some(0);
        self.rebuild_shuffle_order();
        true
    }

    pub fn trim_auto_dj_history(&mut self, keep: usize) -> bool {
        let Some(current_index) = self.current_index else {
            return false;
        };
        let played_indices = self.played_indices_before_current(current_index);
        let auto_dj_indices = played_indices
            .into_iter()
            .filter(|index| {
                self.entries.get(*index).is_some_and(|entry| {
                    matches!(entry.origin, Some(QueueEntryOrigin::AutoDj { .. }))
                })
            })
            .collect::<Vec<_>>();
        if auto_dj_indices.len() <= keep {
            return false;
        }
        let remove_count = auto_dj_indices.len() - keep;
        auto_dj_indices
            .get(..remove_count)
            .is_some_and(|indices| self.remove_indices(indices))
    }

    pub fn reorder(&mut self, entry_id: &QueueEntryId, new_index: usize) -> bool {
        let Some(old_index) = self.entries.iter().position(|entry| entry.id == *entry_id) else {
            return false;
        };
        let current_id = self.current().map(|entry| entry.id.clone());
        let entry = self.entries.remove(old_index);
        let target = new_index.min(self.entries.len());
        self.entries.insert(target, entry);

        if let Some(current_id) = current_id {
            self.current_index = self
                .entries
                .iter()
                .position(|entry| entry.id == current_id)
                .or(Some(target));
        }

        self.rebuild_shuffle_order();
        true
    }

    pub fn move_after_current(&mut self, entry_id: &QueueEntryId) -> bool {
        let Some(current_id) = self.current().map(|entry| entry.id.clone()) else {
            return false;
        };
        if current_id == *entry_id {
            return true;
        }

        let Some(old_index) = self.entries.iter().position(|entry| entry.id == *entry_id) else {
            return false;
        };
        let entry = self.entries.remove(old_index);
        let Some(current_index) = self.entries.iter().position(|entry| entry.id == current_id)
        else {
            return false;
        };
        let target = (current_index + 1).min(self.entries.len());
        self.entries.insert(target, entry);
        self.current_index = Some(current_index);
        self.rebuild_shuffle_order();
        self.move_shuffle_entry_after_current(entry_id);
        true
    }

    fn played_indices_before_current(&self, current_index: usize) -> Vec<usize> {
        if self.shuffle.enabled
            && valid_shuffle_order(&self.shuffle_order, self.entries.len())
            && let Some(position) = self.shuffle_position.or_else(|| {
                self.shuffle_order
                    .iter()
                    .position(|index| *index == current_index)
            })
        {
            return self.shuffle_order.iter().take(position).copied().collect();
        }
        (0..current_index).collect()
    }

    fn remove_indices(&mut self, indices: &[usize]) -> bool {
        if indices.is_empty() {
            return false;
        }
        let mut remove = vec![false; self.entries.len()];
        for index in indices {
            if let Some(slot) = remove.get_mut(*index) {
                *slot = true;
            }
        }
        if !remove.iter().any(|remove| *remove) {
            return false;
        }

        let mut old_to_new = vec![None; self.entries.len()];
        let mut entries = Vec::with_capacity(
            self.entries.len() - remove.iter().filter(|remove| **remove).count(),
        );
        for (index, entry) in self.entries.drain(..).enumerate() {
            if remove.get(index).copied().unwrap_or(false) {
                continue;
            }
            if let Some(mapped) = old_to_new.get_mut(index) {
                *mapped = Some(entries.len());
            }
            entries.push(entry);
        }
        self.entries = entries;
        self.current_index = self
            .current_index
            .and_then(|index| old_to_new.get(index).and_then(|mapped| *mapped));
        self.shuffle_order = self
            .shuffle_order
            .iter()
            .filter_map(|index| old_to_new.get(*index).and_then(|mapped| *mapped))
            .collect();
        if self.shuffle.enabled && valid_shuffle_order(&self.shuffle_order, self.entries.len()) {
            self.sync_shuffle_position();
        } else {
            self.rebuild_shuffle_order();
        }
        true
    }

    pub fn next_track(&mut self) -> Option<&QueueEntry> {
        let next_index = self.next_index(RepeatOneBehavior::Advance)?;
        self.current_index = Some(next_index);
        self.progress_seconds = 0;
        self.sync_shuffle_position();
        self.current()
    }

    pub fn advance_after_end_of_stream(&mut self) -> Option<&QueueEntry> {
        let next_index = self.next_index(RepeatOneBehavior::Stay)?;
        self.current_index = Some(next_index);
        self.progress_seconds = 0;
        self.sync_shuffle_position();
        self.current()
    }

    pub fn previous_track(&mut self) -> Option<&QueueEntry> {
        let previous_index = self.previous_index()?;
        self.current_index = Some(previous_index);
        self.progress_seconds = 0;
        self.sync_shuffle_position();
        self.current()
    }

    pub fn set_repeat_mode(&mut self, repeat_mode: RepeatMode) {
        self.repeat_mode = repeat_mode;
    }

    pub fn set_shuffle(&mut self, enabled: bool, seed: u64) {
        self.shuffle = ShuffleState { enabled, seed };
        self.rebuild_shuffle_order();
    }

    pub fn start_first_shuffled(&mut self) {
        if !self.shuffle.enabled || self.entries.is_empty() {
            return;
        }
        self.rebuild_shuffle_order_unpinned();
        self.current_index = self.shuffle_order.first().copied();
        self.progress_seconds = 0;
        self.sync_shuffle_position();
    }

    pub fn start_first_shuffled_with_seed_avoiding(
        &mut self,
        seed: u64,
        avoid_track_id: Option<&TrackId>,
    ) {
        if !self.shuffle.enabled || self.entries.is_empty() {
            return;
        }
        self.shuffle.seed = seed;
        self.rebuild_shuffle_order_unpinned();
        let selected_position = self
            .shuffle_order
            .iter()
            .position(|index| {
                self.entries
                    .get(*index)
                    .is_some_and(|entry| Some(&entry.track_id) != avoid_track_id)
            })
            .unwrap_or(0);
        let selected_index = self.shuffle_order.remove(selected_position);
        self.shuffle_order.insert(0, selected_index);
        self.current_index = Some(selected_index);
        self.progress_seconds = 0;
        self.sync_shuffle_position();
    }

    fn entry_from_track(&mut self, track: &Track) -> QueueEntry {
        let id = QueueEntryId::new(format!("queue-{}", self.next_entry_number));
        self.next_entry_number += 1;
        let mut entry = QueueEntry::from_track(id, track);
        entry.origin = Some(QueueEntryOrigin::Manual {
            shuffle_key: manual_shuffle_key(&entry),
        });
        entry
    }

    fn entries_from_insertion(
        &mut self,
        insertion: QueueInsertion,
    ) -> Result<Vec<QueueEntry>, QueueError> {
        if insertion.items.is_empty() {
            return Err(QueueError::EmptyReplacement);
        }

        match insertion.source {
            QueueInsertionSource::Manual => {
                let mut tracks = Vec::with_capacity(insertion.items.len());
                for item in insertion.items {
                    let QueueItemInput::Manual { track } = item else {
                        return Err(QueueError::WrongItemKind);
                    };
                    tracks.push(track);
                }

                Ok(tracks
                    .into_iter()
                    .map(|track| {
                        let mut entry = self.entry_from_track(&track);
                        entry.origin = Some(QueueEntryOrigin::Manual {
                            shuffle_key: manual_shuffle_key(&entry),
                        });
                        entry
                    })
                    .collect())
            }
            QueueInsertionSource::AutoDj {
                generated_from_track_id,
                reason,
            } => {
                let mut generated_items = Vec::with_capacity(insertion.items.len());
                for item in insertion.items {
                    let QueueItemInput::Generated {
                        track,
                        generated_index,
                    } = item
                    else {
                        return Err(QueueError::WrongItemKind);
                    };
                    generated_items.push((track, generated_index));
                }

                Ok(generated_items
                    .into_iter()
                    .map(|(track, generated_index)| {
                        let mut entry = self.entry_from_track(&track);
                        entry.origin = Some(QueueEntryOrigin::AutoDj {
                            generated_from_track_id: generated_from_track_id.clone(),
                            generated_index,
                            reason: reason.clone(),
                            shuffle_key: auto_dj_shuffle_key(
                                &generated_from_track_id,
                                generated_index,
                                &reason,
                                &track.id,
                            ),
                        });
                        entry
                    })
                    .collect())
            }
        }
    }

    fn replace_all_source(
        &mut self,
        source: QueueSourceInput,
        items: Vec<QueueItemInput>,
        anchor: QueueAnchor,
    ) -> Result<QueueEntryId, QueueError> {
        let QueueAnchor::SourcePosition {
            position: anchor_index,
            track_id: anchor_track_id,
        } = anchor
        else {
            return Err(QueueError::AnchorNotFound);
        };
        if anchor_index >= items.len() {
            return Err(QueueError::AnchorNotFound);
        }

        let mut source_items = Vec::with_capacity(items.len());
        for item in items {
            let QueueItemInput::Source {
                track,
                source_index,
            } = item
            else {
                return Err(QueueError::WrongItemKind);
            };
            source_items.push((track, source_index));
        }
        if source_items
            .get(anchor_index)
            .is_none_or(|(track, _)| track.id != anchor_track_id)
        {
            return Err(QueueError::AnchorNotFound);
        }

        let entries = source_items
            .into_iter()
            .map(|(track, source_index)| {
                let mut entry = self.entry_from_track(&track);
                entry.origin = Some(QueueEntryOrigin::Source {
                    shuffle_key: source_shuffle_key(&source.source_key, source_index, &track.id),
                });
                entry
            })
            .collect();

        self.entries = entries;
        let anchored_id = self
            .entries
            .get(anchor_index)
            .ok_or(QueueError::AnchorNotFound)?
            .id
            .clone();
        self.current_index = Some(anchor_index);
        self.progress_seconds = 0;
        self.rebuild_shuffle_order();
        Ok(anchored_id)
    }

    fn replace_all_manual(
        &mut self,
        items: Vec<QueueItemInput>,
        anchor: QueueAnchor,
    ) -> Result<QueueEntryId, QueueError> {
        let QueueAnchor::Position(anchor_index) = anchor else {
            return Err(QueueError::AnchorNotFound);
        };
        if anchor_index >= items.len() {
            return Err(QueueError::AnchorNotFound);
        }

        let mut tracks = Vec::with_capacity(items.len());
        for item in items {
            let QueueItemInput::Manual { track } = item else {
                return Err(QueueError::WrongItemKind);
            };
            tracks.push(track);
        }

        self.entries = tracks
            .into_iter()
            .map(|track| {
                let mut entry = self.entry_from_track(&track);
                entry.origin = Some(QueueEntryOrigin::Manual {
                    shuffle_key: manual_shuffle_key(&entry),
                });
                entry
            })
            .collect();
        let anchored_id = self
            .entries
            .get(anchor_index)
            .ok_or(QueueError::AnchorNotFound)?
            .id
            .clone();
        self.current_index = Some(anchor_index);
        self.progress_seconds = 0;
        self.rebuild_shuffle_order();
        Ok(anchored_id)
    }

    fn replace_all_random(
        &mut self,
        seed: u64,
        items: Vec<QueueItemInput>,
        anchor: QueueAnchor,
    ) -> Result<QueueEntryId, QueueError> {
        let QueueAnchor::Position(anchor_index) = anchor else {
            return Err(QueueError::AnchorNotFound);
        };
        if anchor_index >= items.len() {
            return Err(QueueError::AnchorNotFound);
        }

        let mut tracks = Vec::with_capacity(items.len());
        for (index, item) in items.into_iter().enumerate() {
            match item {
                QueueItemInput::Manual { track } => tracks.push((track, index)),
                QueueItemInput::Generated {
                    track,
                    generated_index,
                } => tracks.push((track, generated_index)),
                QueueItemInput::Source { .. } => return Err(QueueError::WrongItemKind),
            }
        }

        self.entries = tracks
            .into_iter()
            .map(|(track, random_index)| {
                let mut entry = self.entry_from_track(&track);
                entry.origin = Some(QueueEntryOrigin::Random {
                    seed,
                    random_index,
                    shuffle_key: random_shuffle_key(seed, random_index, &track.id),
                });
                entry
            })
            .collect();
        let anchored_id = self
            .entries
            .get(anchor_index)
            .ok_or(QueueError::AnchorNotFound)?
            .id
            .clone();
        self.current_index = Some(anchor_index);
        self.progress_seconds = 0;
        self.rebuild_shuffle_order();
        Ok(anchored_id)
    }

    fn next_index(&self, repeat_one: RepeatOneBehavior) -> Option<usize> {
        let current = self.current_index?;
        if self.repeat_mode == RepeatMode::One && repeat_one == RepeatOneBehavior::Stay {
            return Some(current);
        }

        if self.shuffle.enabled {
            let position = self.shuffle_position?;
            if let Some(next) = self.shuffle_order.get(position + 1) {
                return Some(*next);
            }
            return (self.repeat_mode == RepeatMode::All)
                .then(|| self.shuffle_order.first().copied())
                .flatten();
        }

        if current + 1 < self.entries.len() {
            Some(current + 1)
        } else if self.repeat_mode == RepeatMode::All {
            Some(0)
        } else {
            None
        }
    }

    fn previous_index(&self) -> Option<usize> {
        let current = self.current_index?;

        if self.shuffle.enabled {
            let position = self.shuffle_position?;
            if position > 0 {
                return self.shuffle_order.get(position - 1).copied();
            }
            return (self.repeat_mode == RepeatMode::All)
                .then(|| self.shuffle_order.last().copied())
                .flatten();
        }

        if current > 0 {
            Some(current - 1)
        } else if self.repeat_mode == RepeatMode::All {
            self.entries.len().checked_sub(1)
        } else {
            None
        }
    }

    fn rebuild_shuffle_order(&mut self) {
        self.rebuild_shuffle_order_unpinned();
        if self.shuffle.enabled
            && let Some(current_index) = self.current_index
            && let Some(position) = self
                .shuffle_order
                .iter()
                .position(|index| *index == current_index)
        {
            self.shuffle_order.remove(position);
            self.shuffle_order.insert(0, current_index);
        }
        self.sync_shuffle_position();
    }

    fn rebuild_shuffle_order_unpinned(&mut self) {
        self.shuffle_order = (0..self.entries.len()).collect();
        let seed = self.shuffle.seed;
        self.shuffle_order
            .sort_by_key(|index| stable_entry_sort_key(&self.entries, seed, *index));
    }

    fn sync_shuffle_position(&mut self) {
        self.shuffle_position = self.current_index.and_then(|current| {
            self.shuffle_order
                .iter()
                .position(|shuffled| *shuffled == current)
        });
    }

    fn move_shuffle_entry_after_current(&mut self, entry_id: &QueueEntryId) {
        if !self.shuffle.enabled {
            return;
        }
        let Some(current_index) = self.current_index else {
            return;
        };
        let Some(entry_index) = self.entries.iter().position(|entry| entry.id == *entry_id) else {
            return;
        };
        if entry_index == current_index {
            self.sync_shuffle_position();
            return;
        }
        let Some(current_position) = self
            .shuffle_order
            .iter()
            .position(|index| *index == current_index)
        else {
            self.sync_shuffle_position();
            return;
        };
        let Some(entry_position) = self
            .shuffle_order
            .iter()
            .position(|index| *index == entry_index)
        else {
            self.sync_shuffle_position();
            return;
        };
        let entry = self.shuffle_order.remove(entry_position);
        let target = if entry_position < current_position {
            current_position
        } else {
            current_position + 1
        }
        .min(self.shuffle_order.len());
        self.shuffle_order.insert(target, entry);
        self.sync_shuffle_position();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepeatOneBehavior {
    Advance,
    Stay,
}

fn stable_shuffle_key(seed: u64, value: &str) -> u64 {
    value
        .bytes()
        .fold(seed ^ 0x9e37_79b9_7f4a_7c15, |hash, byte| {
            hash.rotate_left(5) ^ u64::from(byte)
        })
}

fn stable_entry_sort_key(entries: &[QueueEntry], seed: u64, index: usize) -> u64 {
    entries
        .get(index)
        .map(|entry| stable_shuffle_key(seed, entry_shuffle_key(entry)))
        .unwrap_or(u64::MAX)
}

fn entry_shuffle_key(entry: &QueueEntry) -> &str {
    match entry.origin.as_ref() {
        Some(QueueEntryOrigin::Source { shuffle_key, .. })
        | Some(QueueEntryOrigin::Manual { shuffle_key })
        | Some(QueueEntryOrigin::Random { shuffle_key, .. })
        | Some(QueueEntryOrigin::AutoDj { shuffle_key, .. })
        | Some(QueueEntryOrigin::RestoredUnknown { shuffle_key, .. }) => shuffle_key.as_str(),
        None => entry.id.as_str(),
    }
}

fn source_shuffle_key(
    source_key: &PlaySourceKey,
    source_index: usize,
    track_id: &TrackId,
) -> QueueShuffleKey {
    QueueShuffleKey::new(format!(
        "source-shuffle|source={}|source-index={}|track={}",
        stable_play_source_key(source_key),
        source_index,
        escape_component(track_id.as_str())
    ))
}

fn stable_play_source_key(source_key: &PlaySourceKey) -> String {
    serde_json::to_string(source_key).unwrap_or_else(|_| "unavailable".to_string())
}

fn escape_component(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                escaped.push(char::from(byte));
            }
            _ => {
                escaped.push('%');
                escaped.push(hex_digit(byte >> 4));
                escaped.push(hex_digit(byte & 0x0f));
            }
        }
    }
    escaped
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'A' + value - 10),
    }
}

fn manual_shuffle_key(entry: &QueueEntry) -> QueueShuffleKey {
    QueueShuffleKey::new(format!("manual-{}", entry.id.as_str()))
}

fn random_shuffle_key(seed: u64, random_index: usize, track_id: &TrackId) -> QueueShuffleKey {
    QueueShuffleKey::new(format!(
        "random:{seed}:{random_index}:{}",
        escape_component(track_id.as_str())
    ))
}

fn auto_dj_shuffle_key(
    generated_from_track_id: &TrackId,
    generated_index: usize,
    reason: &AutoDjReason,
    track_id: &TrackId,
) -> QueueShuffleKey {
    QueueShuffleKey::new(format!(
        "auto-dj|generated-from={}|generated-index={}|reason={}|track={}",
        escape_component(generated_from_track_id.as_str()),
        generated_index,
        stable_auto_dj_reason(reason),
        escape_component(track_id.as_str())
    ))
}

fn stable_auto_dj_reason(reason: &AutoDjReason) -> &'static str {
    match reason {
        AutoDjReason::Similarity => "similarity",
    }
}

fn next_entry_number(entries: &[QueueEntry]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| entry.id.as_str().strip_prefix("queue-"))
        .filter_map(|number| number.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn restored_shuffle_key(index: usize, entry: &QueueEntry) -> QueueShuffleKey {
    QueueShuffleKey::new(format!("restored-{index}-{}", entry.id.as_str()))
}

fn repair_missing_origins(entries: &mut [QueueEntry]) {
    for (index, entry) in entries.iter_mut().enumerate() {
        if entry.origin.is_none() {
            entry.origin = Some(QueueEntryOrigin::RestoredUnknown {
                restored_index: index,
                shuffle_key: restored_shuffle_key(index, entry),
            });
        }
    }
}

fn valid_shuffle_order(shuffle_order: &[usize], entries_len: usize) -> bool {
    if shuffle_order.len() != entries_len {
        return false;
    }
    let mut seen = vec![false; entries_len];
    for index in shuffle_order {
        let Some(slot) = seen.get_mut(*index) else {
            return false;
        };
        if *slot {
            return false;
        }
        *slot = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        PlaySourceDescriptor, PlaySourceKey, PlaylistEntrySortDescriptor, QueueAnchor, QueueEngine,
        QueueEntryOrigin, QueueError, QueueInsertion, QueueInsertionSource, QueueItemInput,
        QueueReplacement, QueueReplacementSource, QueueSourceInput, RepeatMode, SourceOrder,
    };
    use crate::domain::{AlbumId, ServerId, Track, TrackId};

    fn track(number: u32) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: AlbumId::fake(1),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
        }
    }

    #[allow(dead_code)]
    fn source_key(label: &str) -> PlaySourceKey {
        PlaySourceKey {
            descriptor: PlaySourceDescriptor::Playlist {
                playlist_id: crate::domain::PlaylistId::fake(7),
            },
            order: SourceOrder::PlaylistDisplayed {
                query: Some(label.to_string()),
                sort: PlaylistEntrySortDescriptor::Position,
                descending: false,
            },
        }
    }

    fn source_item(track: Track) -> QueueItemInput {
        let source_index = usize::from(track.track_number.saturating_sub(1));
        source_item_at(track, source_index)
    }

    fn source_item_at(track: Track, source_index: usize) -> QueueItemInput {
        QueueItemInput::Source {
            track,
            source_index,
        }
    }

    #[test]
    fn queue_preserve_index() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let id = queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key("playlist"),
                }),
                items: vec![
                    source_item(track(1)),
                    source_item(track(2)),
                    source_item(track(3)),
                    source_item(track(4)),
                ],
                anchor: QueueAnchor::SourcePosition {
                    position: 2,
                    track_id: TrackId::fake(3),
                },
            })
            .expect("source queue replacement should be valid");

        assert_eq!(
            queue
                .entries()
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<Vec<_>>(),
            vec![
                TrackId::fake(1),
                TrackId::fake(2),
                TrackId::fake(3),
                TrackId::fake(4)
            ]
        );
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(3))
        );
        assert_eq!(queue.current().map(|entry| &entry.id), Some(&id));
        assert_eq!(queue.snapshot().current_index, Some(2));
    }

    #[test]
    fn queue_activate_source_occurrence() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let source_key = source_key("playlist");
        let repeated = track(1);
        queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key.clone(),
                }),
                items: vec![
                    source_item_at(repeated.clone(), 0),
                    source_item_at(repeated.clone(), 1),
                    source_item_at(track(2), 2),
                ],
                anchor: QueueAnchor::SourcePosition {
                    position: 0,
                    track_id: repeated.id.clone(),
                },
            })
            .expect("source queue replacement should be valid");
        let ids = queue
            .entries()
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();

        let activated = queue
            .activate_source_occurrence(&source_key, 1, &repeated.id)
            .expect("source occurrence should be materialized");

        assert_eq!(Some(&activated), ids.get(1));
        assert_eq!(queue.snapshot().current_index, Some(1));
        assert_eq!(
            queue
                .entries()
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn queue_reject_anchor() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(9));
        let before = queue.snapshot();

        let result = queue.replace_all(QueueReplacement {
            source: QueueReplacementSource::Source(QueueSourceInput {
                source_key: source_key("playlist"),
            }),
            items: vec![source_item(track(1)), source_item(track(2))],
            anchor: QueueAnchor::SourcePosition {
                position: 1,
                track_id: TrackId::fake(1),
            },
        });

        assert_eq!(result, Err(QueueError::AnchorNotFound));
        assert_eq!(queue.snapshot(), before);
    }

    #[test]
    fn old_snapshots_repair_missing_origins() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));

        let mut snapshot = queue.snapshot();
        for entry in &mut snapshot.entries {
            entry.origin = None;
        }

        let restored = QueueEngine::restore(snapshot);

        assert_eq!(restored.entries().len(), 2);
        assert!(matches!(
            restored.entries()[0].origin.as_ref(),
            Some(QueueEntryOrigin::RestoredUnknown {
                restored_index: 0,
                ..
            })
        ));
        assert!(matches!(
            restored.entries()[1].origin.as_ref(),
            Some(QueueEntryOrigin::RestoredUnknown {
                restored_index: 1,
                ..
            })
        ));
    }

    #[test]
    fn queue_track_next() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));

        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
    }

    #[test]
    fn play_next_inserts_after_current() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(3));
        queue.play_next(&track(2));

        assert_eq!(queue.entries()[1].track_id, TrackId::fake(2));
    }

    #[test]
    fn queue_play_enabled() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.append(&track(4));
        queue.set_shuffle(true, 99);
        queue.play_next(&track(3));

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(3))
        );
    }

    #[test]
    fn queue_track_id() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let mut track = track(1);
        track.album_id = AlbumId::fake(9);
        track.artist_id = Some(crate::domain::ArtistId::fake(7));
        track.local_path = Some("/music/album/track.flac".to_string());
        track.source_format = Some("flac".to_string());

        queue.play_now(&track);
        let entry = queue.current().expect("current entry");

        assert_eq!(entry.album_id, Some(AlbumId::fake(9)));
        assert_eq!(entry.artist_id, Some(crate::domain::ArtistId::fake(7)));
        assert_eq!(entry.year, 2026);
        assert_eq!(entry.local_path.as_deref(), Some("/music/album/track.flac"));
        assert_eq!(entry.source_format.as_deref(), Some("flac"));
    }

    #[test]
    fn queue_advance_entry() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let first = queue.append(&track(1));
        queue.append(&track(2));

        queue.remove(&first);

        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
    }

    #[test]
    fn reorder_move_entry() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        let third = queue.append(&track(3));

        assert!(queue.reorder(&third, 0));

        assert_eq!(queue.entries()[0].track_id, TrackId::fake(3));
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
    }

    #[test]
    fn queue_peek_advancing() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.set_progress_seconds(42);

        assert_eq!(
            queue
                .next_after_end_of_stream()
                .map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
        assert_eq!(queue.progress_seconds(), 42);
    }

    #[test]
    fn queue_jump_entry() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        let second = queue.append(&track(2));
        queue.set_progress_seconds(42);

        assert!(queue.activate(&second));

        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
        assert_eq!(queue.progress_seconds(), 0);
    }

    #[test]
    fn queue_preserve_playback() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        let third = queue.append(&track(3));
        queue.set_progress_seconds(42);

        assert!(queue.move_after_current(&third));

        assert_eq!(
            queue
                .entries()
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<Vec<_>>(),
            vec![TrackId::fake(1), TrackId::fake(3), TrackId::fake(2)]
        );
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
        assert_eq!(queue.progress_seconds(), 42);
    }

    #[test]
    fn queue_move_enabled() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        let third = queue.append(&track(3));
        queue.set_shuffle(true, 99);

        assert!(queue.move_after_current(&third));

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(3))
        );
    }

    #[test]
    fn end_stream_repeat() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.set_repeat_mode(RepeatMode::One);

        assert_eq!(
            queue
                .advance_after_end_of_stream()
                .map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
    }

    #[test]
    fn manual_next_ignores_repeat_one() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.set_repeat_mode(RepeatMode::One);

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
    }

    #[test]
    fn manual_previous_ignores_repeat_one() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.next_track();
        queue.set_repeat_mode(RepeatMode::One);

        assert_eq!(
            queue.previous_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
    }

    #[test]
    fn repeat_all_wraps_at_end() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.set_repeat_mode(RepeatMode::All);

        queue.next_track();

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
    }

    #[test]
    fn shuffle_order_is_deterministic() {
        let mut left = QueueEngine::new(ServerId::fake(1));
        let mut right = QueueEngine::new(ServerId::fake(1));
        for number in 1..=5 {
            left.append(&track(number));
            right.append(&track(number));
        }

        left.set_shuffle(true, 99);
        right.set_shuffle(true, 99);

        let left_order = left.shuffle_order.clone();
        let right_order = right.shuffle_order.clone();

        assert_eq!(left_order, right_order);
    }

    #[test]
    fn queue_source_history() {
        let source = QueueSourceInput {
            source_key: source_key("shuffle"),
        };
        let replacement = || QueueReplacement {
            source: QueueReplacementSource::Source(source.clone()),
            items: vec![
                source_item(track(1)),
                source_item(track(2)),
                source_item(track(3)),
                source_item(track(4)),
            ],
            anchor: QueueAnchor::SourcePosition {
                position: 1,
                track_id: TrackId::fake(2),
            },
        };

        let mut first = QueueEngine::new(ServerId::fake(1));
        first.append(&track(90));
        first.append(&track(91));
        first
            .replace_all(replacement())
            .expect("source queue replacement should be valid");
        first.set_shuffle(true, 77);

        let mut second = QueueEngine::new(ServerId::fake(1));
        second.append(&track(10));
        second
            .replace_all(replacement())
            .expect("source queue replacement should be valid");
        second.set_shuffle(true, 77);

        assert_eq!(
            first.snapshot().shuffle_order,
            second.snapshot().shuffle_order
        );
    }

    #[test]
    fn queue_return_index() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key("display"),
                }),
                items: vec![
                    source_item(track(1)),
                    source_item(track(2)),
                    source_item(track(3)),
                ],
                anchor: QueueAnchor::SourcePosition {
                    position: 1,
                    track_id: TrackId::fake(2),
                },
            })
            .expect("source queue replacement should be valid");

        queue.set_shuffle(true, 51);
        queue.set_shuffle(false, 51);

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(3))
        );
    }

    #[test]
    fn queue_restored_rebuild() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.append(&track(3));
        queue.set_shuffle(true, 42);

        let mut snapshot = queue.snapshot();
        snapshot.shuffle_order = vec![1, 2, 0];
        snapshot.current_index = Some(1);

        let restored = QueueEngine::restore(snapshot);

        assert_eq!(restored.snapshot().shuffle_order, vec![1, 2, 0]);
    }

    #[test]
    fn enabling_shuffle_start() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.append(&track(3));
        queue.next_track();

        queue.set_shuffle(true, 99);

        assert_eq!(queue.shuffle_order.first().copied(), Some(1));
        assert_eq!(queue.shuffle_position, Some(0));
    }

    #[test]
    fn shuffled_start_can_avoid_previous_track() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let first = track(1);
        let second = track(2);
        queue.append(&first);
        queue.append(&second);
        queue.set_shuffle(true, 1);

        queue.start_first_shuffled_with_seed_avoiding(17, Some(&first.id));

        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&second.id)
        );
        assert_eq!(queue.shuffle_position, Some(0));
        assert_eq!(queue.shuffle.seed, 17);
    }

    #[test]
    fn appending_exhausted_shuffled() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.append(&track(3));
        queue.set_shuffle(true, 99);
        queue.set_repeat_mode(RepeatMode::Off);

        while queue.remaining_after_current() > 0 {
            queue.next_track();
        }
        assert_eq!(queue.remaining_after_current(), 0);

        queue.append(&track(4));

        assert_eq!(queue.remaining_after_current(), 1);
        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(4))
        );
    }

    #[test]
    fn queue_start_traversal() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.set_shuffle(true, 19);
        queue.set_repeat_mode(RepeatMode::Off);

        let inserted = queue
            .append_last(QueueInsertion {
                source: QueueInsertionSource::Manual,
                items: vec![
                    QueueItemInput::Manual { track: track(1) },
                    QueueItemInput::Manual { track: track(2) },
                    QueueItemInput::Manual { track: track(3) },
                ],
            })
            .expect("manual append should be valid");

        assert_eq!(inserted.len(), 3);
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(1))
        );
        assert_eq!(queue.shuffle_order.first().copied(), Some(0));
        assert_eq!(queue.shuffle_position, Some(0));
        let next_track_id = queue
            .next_track()
            .map(|entry| entry.track_id.clone())
            .expect("shuffled queue has a next item");
        assert_ne!(next_track_id, TrackId::fake(1));
        assert!([TrackId::fake(2), TrackId::fake(3)].contains(&next_track_id));
    }

    #[test]
    fn trim_auto_dj_history_keeps_recent_generated_entries() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue
            .append_last(QueueInsertion {
                source: QueueInsertionSource::AutoDj {
                    generated_from_track_id: TrackId::fake(1),
                    reason: super::AutoDjReason::Similarity,
                },
                items: (2..=7)
                    .map(|number| QueueItemInput::Generated {
                        track: track(number),
                        generated_index: number as usize,
                    })
                    .collect(),
            })
            .expect("auto dj append");
        for _ in 0..5 {
            queue.next_track();
        }

        assert!(queue.trim_auto_dj_history(2));
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.entries.len(), 5);
        assert_eq!(
            snapshot.entries[snapshot.current_index.expect("current")].track_id,
            TrackId::fake(6)
        );
        assert_eq!(
            snapshot
                .entries
                .iter()
                .filter(|entry| matches!(entry.origin, Some(QueueEntryOrigin::AutoDj { .. })))
                .count(),
            4
        );
    }

    #[test]
    fn snapshot_restores_queue_state() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.next_track();
        queue.set_repeat_mode(RepeatMode::All);
        queue.set_shuffle(true, 7);

        let restored = QueueEngine::restore(queue.snapshot());

        assert_eq!(
            restored.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
        assert_eq!(restored.repeat_mode(), RepeatMode::All);
        assert!(restored.shuffle().enabled);
    }

    #[test]
    fn snapshot_restores_shuffle_order_history() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        for number in 1..=5 {
            queue.append(&track(number));
        }
        queue.set_shuffle(true, 99);
        let order = queue.shuffle_order.clone();
        queue.next_track();

        let restored = QueueEngine::restore(queue.snapshot());

        assert_eq!(restored.shuffle_order, order);
        assert_eq!(restored.shuffle_position, queue.shuffle_position);
        assert_eq!(
            restored.current().map(|entry| &entry.track_id),
            queue.current().map(|entry| &entry.track_id)
        );
    }

    #[test]
    fn clear_remove_entry() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));

        queue.clear();

        assert!(queue.entries().is_empty());
        assert!(queue.current().is_none());
    }

    #[test]
    fn clear_keeps_current() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let _first = queue.append(&track(1));
        let current = queue.append(&track(2));
        let _third = queue.append(&track(3));
        assert!(queue.activate(&current));
        queue.set_progress_seconds(42);

        assert!(queue.clear_except_current());

        assert_eq!(queue.entries().len(), 1);
        assert_eq!(
            queue.current().map(|entry| &entry.track_id),
            Some(&TrackId::fake(2))
        );
        assert_eq!(queue.snapshot().current_index, Some(0));
        assert_eq!(queue.progress_seconds(), 42);
    }
}
