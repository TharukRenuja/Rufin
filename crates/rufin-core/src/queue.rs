use std::fmt::Write as _;

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
pub struct QueueBatchKey(String);

impl QueueBatchKey {
    #[allow(dead_code)]
    fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        assert!(!value.is_empty(), "QueueBatchKey cannot be empty");
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueSourceSnapshot {
    pub source_key: PlaySourceKey,
    pub batch_key: QueueBatchKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_source_items: Option<usize>,
    pub materialized_start: usize,
    pub materialized_len: usize,
    pub anchor_index: usize,
    pub capped: bool,
    pub materialized_track_ids: Vec<TrackId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueSourceInput {
    pub source_key: PlaySourceKey,
    pub total_source_items: Option<usize>,
    pub materialized_start: usize,
    pub materialized_len: usize,
    pub capped: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueReplacement {
    pub source: QueueReplacementSource,
    pub items: Vec<QueueItemInput>,
    pub anchor: QueueAnchor,
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
        source_item_id: Option<String>,
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
    SourceOccurrence {
        track_id: TrackId,
        source_index: usize,
        source_item_id: Option<String>,
    },
    Position(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueError {
    EmptyReplacement,
    AnchorNotFound,
    SourceLengthMismatch,
    SourceIndexMismatch,
    WrongItemKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QueueEntryOrigin {
    Source {
        source_key: PlaySourceKey,
        occurrence_key: String,
        source_index: usize,
        source_item_id: Option<String>,
        batch_key: QueueBatchKey,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_snapshot: Option<QueueSourceSnapshot>,
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
    source_snapshot: Option<QueueSourceSnapshot>,
    #[allow(dead_code)]
    next_batch_number: u64,
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
            source_snapshot: None,
            next_batch_number: 1,
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
            next_batch_number: next_batch_number(&entries),
            entries,
            current_index,
            repeat_mode: snapshot.repeat_mode,
            shuffle: snapshot.shuffle,
            shuffle_order: snapshot.shuffle_order,
            shuffle_position: None,
            source_snapshot: snapshot.source_snapshot,
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
            source_snapshot: self.source_snapshot.clone(),
        }
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.current_index.and_then(|index| self.entries.get(index))
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
        self.replace_all(QueueReplacement {
            source: QueueReplacementSource::Manual,
            items: vec![QueueItemInput::Manual {
                track: track.clone(),
            }],
            anchor: QueueAnchor::Position(0),
        })
        .expect("single manual replacement is valid")
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
        let insert_index = self.current_index.map_or(0, |index| index + 1);
        self.entries.insert(insert_index, entry);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
        self.rebuild_shuffle_order();
        self.move_shuffle_entry_after_current(&id);
        id
    }

    pub fn append(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        let new_index = self.entries.len();
        self.entries.push(entry);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
        if self.shuffle.enabled {
            self.shuffle_order.push(new_index);
            self.sync_shuffle_position();
        } else {
            self.rebuild_shuffle_order();
        }
        id
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

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_index = None;
        self.shuffle_order.clear();
        self.shuffle_position = None;
        self.source_snapshot = None;
        self.progress_seconds = 0;
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

    fn entry_from_track(&mut self, track: &Track) -> QueueEntry {
        let id = QueueEntryId::new(format!("queue-{}", self.next_entry_number));
        self.next_entry_number += 1;
        let mut entry = QueueEntry::from_track(id, track);
        entry.origin = Some(QueueEntryOrigin::Manual {
            shuffle_key: manual_shuffle_key(&entry),
        });
        entry
    }

    fn next_batch_key(&mut self) -> QueueBatchKey {
        let batch_key = QueueBatchKey::new(format!("batch-{}", self.next_batch_number));
        self.next_batch_number += 1;
        batch_key
    }

    fn replace_all_source(
        &mut self,
        source: QueueSourceInput,
        items: Vec<QueueItemInput>,
        anchor: QueueAnchor,
    ) -> Result<QueueEntryId, QueueError> {
        if items.len() != source.materialized_len {
            return Err(QueueError::SourceLengthMismatch);
        }
        let QueueAnchor::SourceOccurrence {
            track_id: anchor_track_id,
            source_index: anchor_source_index,
            source_item_id: anchor_source_item_id,
        } = anchor
        else {
            return Err(QueueError::AnchorNotFound);
        };

        let mut source_items = Vec::with_capacity(items.len());
        let mut anchor_index = None;
        let mut matching_anchors = 0usize;
        for (materialized_index, item) in items.into_iter().enumerate() {
            let QueueItemInput::Source {
                track,
                source_index,
                source_item_id,
            } = item
            else {
                return Err(QueueError::WrongItemKind);
            };
            let Some(expected_source_index) =
                source.materialized_start.checked_add(materialized_index)
            else {
                return Err(QueueError::SourceIndexMismatch);
            };
            if source_index != expected_source_index {
                return Err(QueueError::SourceIndexMismatch);
            }
            if track.id == anchor_track_id
                && source_index == anchor_source_index
                && source_item_id == anchor_source_item_id
            {
                matching_anchors += 1;
                anchor_index = Some(materialized_index);
            }
            source_items.push((track, source_index, source_item_id));
        }

        if matching_anchors != 1 {
            return Err(QueueError::AnchorNotFound);
        }
        let anchor_index = anchor_index.expect("matching anchor records its index");
        let Some(expected_anchor_source_index) =
            source.materialized_start.checked_add(anchor_index)
        else {
            return Err(QueueError::SourceIndexMismatch);
        };
        if expected_anchor_source_index != anchor_source_index {
            return Err(QueueError::AnchorNotFound);
        }

        let batch_key = self.next_batch_key();
        let materialized_track_ids = source_items
            .iter()
            .map(|(track, _, _)| track.id.clone())
            .collect::<Vec<_>>();
        let entries = source_items
            .into_iter()
            .map(|(track, source_index, source_item_id)| {
                let mut entry = self.entry_from_track(&track);
                entry.origin = Some(QueueEntryOrigin::Source {
                    source_key: source.source_key.clone(),
                    occurrence_key: source_occurrence_key(
                        &source.source_key,
                        source_index,
                        source_item_id.as_deref(),
                        &track.id,
                    ),
                    source_index,
                    source_item_id: source_item_id.clone(),
                    batch_key: batch_key.clone(),
                    shuffle_key: source_shuffle_key(
                        &source.source_key,
                        source_index,
                        source_item_id.as_deref(),
                        &track.id,
                    ),
                });
                entry
            })
            .collect();

        self.entries = entries;
        let anchored_id = self.entries[anchor_index].id.clone();
        self.source_snapshot = Some(QueueSourceSnapshot {
            source_key: source.source_key,
            batch_key,
            total_source_items: source.total_source_items,
            materialized_start: source.materialized_start,
            materialized_len: source.materialized_len,
            anchor_index,
            capped: source.capped,
            materialized_track_ids,
        });
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
        let anchored_id = self.entries[anchor_index].id.clone();
        self.source_snapshot = None;
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
        let anchored_id = self.entries[anchor_index].id.clone();
        self.source_snapshot = None;
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
        self.shuffle_order = (0..self.entries.len()).collect();
        let seed = self.shuffle.seed;
        self.shuffle_order.sort_by_key(|index| {
            stable_shuffle_key(seed, entry_shuffle_key(&self.entries[*index]))
        });
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

fn source_occurrence_key(
    source_key: &PlaySourceKey,
    source_index: usize,
    source_item_id: Option<&str>,
    track_id: &TrackId,
) -> String {
    stable_source_entry_key(
        "source-occurrence",
        source_key,
        source_index,
        source_item_id,
        track_id,
    )
}

fn source_shuffle_key(
    source_key: &PlaySourceKey,
    source_index: usize,
    source_item_id: Option<&str>,
    track_id: &TrackId,
) -> QueueShuffleKey {
    QueueShuffleKey::new(stable_source_entry_key(
        "source-shuffle",
        source_key,
        source_index,
        source_item_id,
        track_id,
    ))
}

fn stable_source_entry_key(
    prefix: &str,
    source_key: &PlaySourceKey,
    source_index: usize,
    source_item_id: Option<&str>,
    track_id: &TrackId,
) -> String {
    format!(
        "{}|source={}|source-index={}|source-item={}|track={}",
        prefix,
        stable_play_source_key(source_key),
        source_index,
        stable_optional_str(source_item_id),
        escape_component(track_id.as_str())
    )
}

fn stable_play_source_key(source_key: &PlaySourceKey) -> String {
    format!(
        "descriptor={};order={}",
        stable_play_source_descriptor(&source_key.descriptor),
        stable_source_order(&source_key.order)
    )
}

fn stable_play_source_descriptor(descriptor: &PlaySourceDescriptor) -> String {
    match descriptor {
        PlaySourceDescriptor::Album {
            album_id,
            selected_music_folder_id,
        } => format!(
            "album;album-id={};music-folder={}",
            escape_component(album_id.as_str()),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::Playlist { playlist_id } => {
            format!(
                "playlist;playlist-id={}",
                escape_component(playlist_id.as_str())
            )
        }
        PlaySourceDescriptor::SmartPlaylist {
            smart_playlist_id,
            definition_fingerprint,
            selected_music_folder_id,
        } => format!(
            "smart-playlist;smart-playlist-id={};definition-fingerprint={};music-folder={}",
            escape_component(smart_playlist_id.as_str()),
            escape_component(definition_fingerprint),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::FolderLoaded {
            path,
            selected_music_folder_id,
        } => format!(
            "folder-loaded;path={};music-folder={}",
            stable_string_list(path),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::ArtistTracks {
            artist_id,
            scope,
            selected_music_folder_id,
        } => format!(
            "artist-tracks;artist-id={};scope={};music-folder={}",
            escape_component(artist_id.as_str()),
            stable_artist_track_scope(scope),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::GenreTracks {
            genre_id,
            selected_music_folder_id,
        } => format!(
            "genre-tracks;genre-id={};music-folder={}",
            escape_component(genre_id.as_str()),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::FavoriteTracks {
            selected_music_folder_id,
        } => format!(
            "favorite-tracks;music-folder={}",
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::SearchResults {
            query,
            selected_music_folder_id,
        } => format!(
            "search-results;query={};music-folder={}",
            escape_component(query),
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::GlobalTracks {
            selected_music_folder_id,
        } => format!(
            "global-tracks;music-folder={}",
            stable_optional_id(selected_music_folder_id.as_ref().map(MusicFolderId::as_str))
        ),
        PlaySourceDescriptor::HomeCollection { section_id, source } => format!(
            "home-collection;section-id={};source={}",
            escape_component(section_id),
            stable_play_source_descriptor(source)
        ),
    }
}

fn stable_artist_track_scope(scope: &ArtistTrackScope) -> &'static str {
    match scope {
        ArtistTrackScope::MainArtist => "main-artist",
        ArtistTrackScope::AllCredits => "all-credits",
    }
}

fn stable_source_order(order: &SourceOrder) -> String {
    match order {
        SourceOrder::Canonical => "canonical".to_string(),
        SourceOrder::LibraryDisplayed { filter_key, sort } => format!(
            "library-displayed;filter-key={};sort={}",
            stable_optional_str(filter_key.as_deref()),
            stable_track_sort(sort)
        ),
        SourceOrder::PlaylistDisplayed {
            query,
            sort,
            descending,
        } => format!(
            "playlist-displayed;query={};sort={};descending={}",
            stable_optional_str(query.as_deref()),
            stable_playlist_entry_sort(sort),
            descending
        ),
        SourceOrder::FolderDisplayed { query, sort } => format!(
            "folder-displayed;query={};sort={}",
            stable_optional_str(query.as_deref()),
            stable_track_sort(sort)
        ),
        SourceOrder::SearchDisplayed { sort } => {
            format!("search-displayed;sort={}", stable_search_sort(sort))
        }
        SourceOrder::SmartPlaylistDefinition {
            sort,
            limit,
            skip_count,
        } => format!(
            "smart-playlist-definition;sort={};limit={};skip-count={}",
            stable_smart_playlist_sort(sort),
            stable_optional_usize(*limit),
            skip_count
        ),
    }
}

fn stable_track_sort(sort: &TrackSortDescriptor) -> &'static str {
    match sort {
        TrackSortDescriptor::Album => "album",
        TrackSortDescriptor::Artist => "artist",
        TrackSortDescriptor::DateAdded => "date-added",
        TrackSortDescriptor::Title => "title",
        TrackSortDescriptor::TrackNumber => "track-number",
    }
}

fn stable_playlist_entry_sort(sort: &PlaylistEntrySortDescriptor) -> &'static str {
    match sort {
        PlaylistEntrySortDescriptor::Position => "position",
        PlaylistEntrySortDescriptor::Title => "title",
        PlaylistEntrySortDescriptor::Album => "album",
        PlaylistEntrySortDescriptor::Artist => "artist",
    }
}

fn stable_search_sort(sort: &SearchSortDescriptor) -> &'static str {
    match sort {
        SearchSortDescriptor::Relevance => "relevance",
        SearchSortDescriptor::Title => "title",
    }
}

fn stable_smart_playlist_sort(sort: &SmartPlaylistSortDescriptor) -> &'static str {
    match sort {
        SmartPlaylistSortDescriptor::Definition => "definition",
    }
}

fn stable_optional_id(value: Option<&str>) -> String {
    stable_optional_str(value)
}

fn stable_optional_str(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("present:{}", escape_component(value)),
        None => "absent".to_string(),
    }
}

fn stable_optional_usize(value: Option<usize>) -> String {
    match value {
        Some(value) => format!("present:{value}"),
        None => "absent".to_string(),
    }
}

fn stable_string_list(values: &[String]) -> String {
    let mut output = format!("len={}", values.len());
    for value in values {
        output.push(':');
        output.push_str(&escape_component(value));
    }
    output
}

fn escape_component(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                escaped.push(char::from(byte));
            }
            _ => {
                write!(&mut escaped, "%{byte:02X}").expect("writing to a string cannot fail");
            }
        }
    }
    escaped
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

fn next_batch_number(entries: &[QueueEntry]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| entry.origin.as_ref())
        .filter_map(|origin| match origin {
            QueueEntryOrigin::Source { batch_key, .. } => Some(batch_key),
            QueueEntryOrigin::Manual { .. }
            | QueueEntryOrigin::Random { .. }
            | QueueEntryOrigin::AutoDj { .. }
            | QueueEntryOrigin::RestoredUnknown { .. } => None,
        })
        .filter_map(|batch_key| batch_key.as_str().strip_prefix("batch-"))
        .filter_map(|number| number.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn valid_shuffle_order(shuffle_order: &[usize], entries_len: usize) -> bool {
    if shuffle_order.len() != entries_len {
        return false;
    }
    let mut seen = vec![false; entries_len];
    for index in shuffle_order {
        if *index >= entries_len || seen[*index] {
            return false;
        }
        seen[*index] = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        PlaySourceDescriptor, PlaySourceKey, PlaylistEntrySortDescriptor, QueueAnchor, QueueEngine,
        QueueEntryOrigin, QueueError, QueueItemInput, QueueReplacement, QueueReplacementSource,
        QueueSourceInput, RepeatMode, SourceOrder,
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

    fn source_item(track: Track, source_index: usize, id: &str) -> QueueItemInput {
        QueueItemInput::Source {
            track,
            source_index,
            source_item_id: Some(id.to_string()),
        }
    }

    #[test]
    fn replace_all_source_preserves_order_and_current_index() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let id = queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key("playlist"),
                    total_source_items: Some(4),
                    materialized_start: 0,
                    materialized_len: 4,
                    capped: false,
                }),
                items: vec![
                    source_item(track(1), 0, "a"),
                    source_item(track(2), 1, "b"),
                    source_item(track(3), 2, "c"),
                    source_item(track(4), 3, "d"),
                ],
                anchor: QueueAnchor::SourceOccurrence {
                    track_id: TrackId::fake(3),
                    source_index: 2,
                    source_item_id: Some("c".to_string()),
                },
            })
            .unwrap();

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
    fn replace_all_source_rejects_anchor_with_wrong_occurrence_id() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(9));
        let before = queue.snapshot();

        let result = queue.replace_all(QueueReplacement {
            source: QueueReplacementSource::Source(QueueSourceInput {
                source_key: source_key("playlist"),
                total_source_items: Some(2),
                materialized_start: 0,
                materialized_len: 2,
                capped: false,
            }),
            items: vec![source_item(track(1), 0, "a"), source_item(track(1), 1, "b")],
            anchor: QueueAnchor::SourceOccurrence {
                track_id: TrackId::fake(1),
                source_index: 1,
                source_item_id: Some("a".to_string()),
            },
        });

        assert_eq!(result, Err(QueueError::AnchorNotFound));
        assert_eq!(queue.snapshot(), before);
    }

    #[test]
    fn snapshots_persist_source_snapshot() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        let source = QueueSourceInput {
            source_key: source_key("album-order"),
            total_source_items: Some(3),
            materialized_start: 0,
            materialized_len: 3,
            capped: false,
        };
        let replacement = QueueReplacement {
            source: QueueReplacementSource::Source(source.clone()),
            items: vec![
                QueueItemInput::Source {
                    track: track(1),
                    source_index: 0,
                    source_item_id: Some("entry-a".to_string()),
                },
                QueueItemInput::Source {
                    track: track(2),
                    source_index: 1,
                    source_item_id: Some("entry-b".to_string()),
                },
                QueueItemInput::Source {
                    track: track(3),
                    source_index: 2,
                    source_item_id: Some("entry-c".to_string()),
                },
            ],
            anchor: QueueAnchor::SourceOccurrence {
                track_id: TrackId::fake(2),
                source_index: 1,
                source_item_id: Some("entry-b".to_string()),
            },
        };

        queue.replace_all(replacement).unwrap();
        let snapshot = queue.snapshot();

        assert_eq!(
            snapshot
                .source_snapshot
                .as_ref()
                .map(|snapshot| snapshot.anchor_index),
            Some(1)
        );
        assert_eq!(
            snapshot
                .source_snapshot
                .as_ref()
                .map(|snapshot| snapshot.materialized_track_ids.clone()),
            Some(vec![TrackId::fake(1), TrackId::fake(2), TrackId::fake(3)])
        );
    }

    #[test]
    fn source_origin_keys_use_stable_components() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key("stable|origin"),
                    total_source_items: Some(1),
                    materialized_start: 2,
                    materialized_len: 1,
                    capped: false,
                }),
                items: vec![source_item(track(3), 2, "entry:c")],
                anchor: QueueAnchor::SourceOccurrence {
                    track_id: TrackId::fake(3),
                    source_index: 2,
                    source_item_id: Some("entry:c".to_string()),
                },
            })
            .unwrap();

        let Some(QueueEntryOrigin::Source {
            occurrence_key,
            shuffle_key,
            ..
        }) = queue.current().and_then(|entry| entry.origin.as_ref())
        else {
            panic!("source replacement should assign source origin");
        };
        let shuffle_key = shuffle_key.as_str();

        for key in [occurrence_key.as_str(), shuffle_key] {
            assert!(!key.contains("PlaySource"));
            assert!(!key.contains("Some("));
            assert!(!key.contains("TrackId"));
            assert!(!key.contains('"'));
            assert!(key.contains("playlist-7"));
            assert!(key.contains("track-3"));
            assert!(key.contains("source-item=present:entry%3Ac"));
            assert!(key.contains("query=present:stable%7Corigin"));
        }
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
        snapshot.source_snapshot = None;

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
    fn appends_and_moves_to_next_track() {
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
    fn play_next_remains_next_when_shuffle_is_enabled() {
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
    fn queue_entries_keep_navigation_ids_from_tracks() {
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
    fn remove_current_advances_to_valid_entry() {
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
    fn reorder_moves_entries_without_changing_current_track() {
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
    fn activate_jumps_to_existing_queue_entry() {
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
    fn move_after_current_preserves_current_playback() {
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
    fn move_after_current_remains_next_when_shuffle_is_enabled() {
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
    fn end_of_stream_repeat_one_keeps_current_track() {
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
    fn same_source_and_seed_shuffle_is_independent_of_prior_queue_history() {
        let source = QueueSourceInput {
            source_key: source_key("shuffle"),
            total_source_items: Some(4),
            materialized_start: 0,
            materialized_len: 4,
            capped: false,
        };
        let replacement = || QueueReplacement {
            source: QueueReplacementSource::Source(source.clone()),
            items: vec![
                source_item(track(1), 0, "a"),
                source_item(track(2), 1, "b"),
                source_item(track(3), 2, "c"),
                source_item(track(4), 3, "d"),
            ],
            anchor: QueueAnchor::SourceOccurrence {
                track_id: TrackId::fake(2),
                source_index: 1,
                source_item_id: Some("b".to_string()),
            },
        };

        let mut first = QueueEngine::new(ServerId::fake(1));
        first.append(&track(90));
        first.append(&track(91));
        first.replace_all(replacement()).unwrap();
        first.set_shuffle(true, 77);

        let mut second = QueueEngine::new(ServerId::fake(1));
        second.append(&track(10));
        second.replace_all(replacement()).unwrap();
        second.set_shuffle(true, 77);

        assert_eq!(first.snapshot().shuffle_order, second.snapshot().shuffle_order);
    }

    #[test]
    fn disabling_shuffle_returns_to_source_order_from_current_display_index() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue
            .replace_all(QueueReplacement {
                source: QueueReplacementSource::Source(QueueSourceInput {
                    source_key: source_key("display"),
                    total_source_items: Some(3),
                    materialized_start: 0,
                    materialized_len: 3,
                    capped: false,
                }),
                items: vec![
                    source_item(track(1), 0, "a"),
                    source_item(track(2), 1, "b"),
                    source_item(track(3), 2, "c"),
                ],
                anchor: QueueAnchor::SourceOccurrence {
                    track_id: TrackId::fake(2),
                    source_index: 1,
                    source_item_id: Some("b".to_string()),
                },
            })
            .unwrap();

        queue.set_shuffle(true, 51);
        queue.set_shuffle(false, 51);

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
            Some(&TrackId::fake(3))
        );
    }

    #[test]
    fn restored_valid_shuffle_order_is_used_until_rebuild() {
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
    fn enabling_shuffle_starts_traversal_at_current_track() {
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
    fn appending_while_shuffled_adds_new_tracks_after_existing_traversal() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.append(&track(3));
        queue.set_shuffle(true, 99);

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
    fn clear_removes_entries_and_current_track() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));

        queue.clear();

        assert!(queue.entries().is_empty());
        assert!(queue.current().is_none());
    }
}
