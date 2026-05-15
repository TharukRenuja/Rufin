use serde::{Deserialize, Serialize};

use crate::domain::{ServerId, Track, TrackId};

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub track_id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: u32,
}

impl QueueEntry {
    fn from_track(id: QueueEntryId, track: &Track) -> Self {
        Self {
            id,
            track_id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_seconds: track.duration_seconds,
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
            repeat_mode: RepeatMode::Off,
            shuffle: ShuffleState::default(),
            shuffle_order: Vec::new(),
            shuffle_position: None,
            next_entry_number: 1,
            progress_seconds: 0,
        }
    }

    pub fn restore(snapshot: QueueSnapshot) -> Self {
        let current_index = snapshot
            .current_index
            .filter(|index| *index < snapshot.entries.len());
        let mut engine = Self {
            server_id: snapshot.server_id,
            next_entry_number: next_entry_number(&snapshot.entries),
            entries: snapshot.entries,
            current_index,
            repeat_mode: snapshot.repeat_mode,
            shuffle: snapshot.shuffle,
            shuffle_order: Vec::new(),
            shuffle_position: None,
            progress_seconds: snapshot.progress_seconds,
        };
        engine.rebuild_shuffle_order();
        engine
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            server_id: self.server_id.clone(),
            entries: self.entries.clone(),
            current_index: self.current_index,
            repeat_mode: self.repeat_mode,
            shuffle: self.shuffle.clone(),
            progress_seconds: self.progress_seconds,
        }
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.current_index.and_then(|index| self.entries.get(index))
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }

    pub fn shuffle(&self) -> &ShuffleState {
        &self.shuffle
    }

    pub fn play_now(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        self.entries.clear();
        self.entries.push(entry);
        self.current_index = Some(0);
        self.progress_seconds = 0;
        self.rebuild_shuffle_order();
        id
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
        id
    }

    pub fn append(&mut self, track: &Track) -> QueueEntryId {
        let entry = self.entry_from_track(track);
        let id = entry.id.clone();
        self.entries.push(entry);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
        self.rebuild_shuffle_order();
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

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_index = None;
        self.shuffle_order.clear();
        self.shuffle_position = None;
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

    pub fn next_track(&mut self) -> Option<&QueueEntry> {
        let next_index = self.next_index()?;
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
        QueueEntry::from_track(id, track)
    }

    fn next_index(&self) -> Option<usize> {
        let current = self.current_index?;
        if self.repeat_mode == RepeatMode::One {
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
        if self.repeat_mode == RepeatMode::One {
            return Some(current);
        }

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
        self.shuffle_order
            .sort_by_key(|index| stable_shuffle_key(seed, self.entries[*index].id.as_str()));
        self.sync_shuffle_position();
    }

    fn sync_shuffle_position(&mut self) {
        self.shuffle_position = self.current_index.and_then(|current| {
            self.shuffle_order
                .iter()
                .position(|shuffled| *shuffled == current)
        });
    }
}

fn stable_shuffle_key(seed: u64, value: &str) -> u64 {
    value
        .bytes()
        .fold(seed ^ 0x9e37_79b9_7f4a_7c15, |hash, byte| {
            hash.rotate_left(5) ^ u64::from(byte)
        })
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

#[cfg(test)]
mod tests {
    use super::{QueueEngine, RepeatMode};
    use crate::domain::{AlbumId, ServerId, Track, TrackId};

    fn track(number: u32) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: AlbumId::fake(1),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
        }
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
        assert_eq!(queue.next_track(), None);
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
    fn repeat_one_keeps_current_track() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));
        queue.append(&track(2));
        queue.set_repeat_mode(RepeatMode::One);

        assert_eq!(
            queue.next_track().map(|entry| &entry.track_id),
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
    fn clear_removes_entries_and_current_track() {
        let mut queue = QueueEngine::new(ServerId::fake(1));
        queue.append(&track(1));

        queue.clear();

        assert!(queue.entries().is_empty());
        assert!(queue.current().is_none());
    }
}
