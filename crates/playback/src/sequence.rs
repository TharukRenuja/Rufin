use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use library::{SourceId, Track, TrackId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct OccurrenceId(String);

impl OccurrenceId {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        assert!(!value.is_empty(), "OccurrenceId cannot be empty");
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for OccurrenceId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for OccurrenceId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for OccurrenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Provenance {
    Context {
        context_id: String,
        source_rank: usize,
    },
    Manual,
    Random,
    Radio,
    AutoDj,
    Legacy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SequenceEntry {
    pub occurrence: OccurrenceId,
    pub track: Track,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchItem {
    pub track: Track,
    pub provenance: Provenance,
}

impl BatchItem {
    pub fn new(track: Track, provenance: Provenance) -> Self {
        Self { track, provenance }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Batch {
    items: Vec<BatchItem>,
    shuffle_seed: u64,
    random_start: bool,
}

impl Batch {
    pub fn new(items: Vec<BatchItem>) -> Self {
        Self {
            items,
            shuffle_seed: 0,
            random_start: false,
        }
    }

    pub fn with_shuffle_intent(mut self, seed: u64, random_start: bool) -> Self {
        self.shuffle_seed = seed;
        self.random_start = random_start;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    Replace { anchor_index: usize },
    AfterCurrent,
    End,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SequenceError {
    #[error("a playback batch cannot be empty")]
    EmptyBatch,
    #[error("the playback batch anchor is outside the batch")]
    InvalidAnchor,
    #[error("the playback traversal is not an exact occurrence permutation")]
    InvalidTraversal,
    #[error("the playback checkpoint contains duplicate occurrences")]
    DuplicateOccurrence,
    #[error("the selected playback occurrence is missing")]
    MissingSelectedOccurrence,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub(crate) source_id: SourceId,
    pub(crate) entries: Vec<SequenceEntry>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) repeat_mode: RepeatMode,
    pub(crate) shuffle_enabled: bool,
    pub(crate) traversal: Vec<usize>,
    pub(crate) traversal_position: Option<usize>,
    pub(crate) revision: u64,
    pub(crate) progress_millis: u64,
    next_occurrence_number: u64,
    track_counts: HashMap<TrackId, usize>,
}

pub(crate) struct RestoredSequence {
    pub source_id: SourceId,
    pub entries: Vec<SequenceEntry>,
    pub selected: Option<OccurrenceId>,
    pub repeat_mode: RepeatMode,
    pub shuffle_enabled: bool,
    pub traversal: Vec<OccurrenceId>,
    pub revision: u64,
    pub progress_millis: u64,
}

impl Sequence {
    pub fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            entries: Vec::new(),
            selected_index: None,
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            traversal: Vec::new(),
            traversal_position: None,
            revision: 0,
            progress_millis: 0,
            next_occurrence_number: 1,
            track_counts: HashMap::new(),
        }
    }

    pub(crate) fn restore(restored: RestoredSequence) -> Result<Self, SequenceError> {
        let RestoredSequence {
            source_id,
            entries,
            selected,
            repeat_mode,
            shuffle_enabled,
            traversal,
            revision,
            progress_millis,
        } = restored;
        let positions = occurrence_positions(&entries)?;
        let selected_index = selected
            .as_ref()
            .map(|occurrence| {
                positions
                    .get(occurrence)
                    .copied()
                    .ok_or(SequenceError::MissingSelectedOccurrence)
            })
            .transpose()?;
        let traversal = if shuffle_enabled {
            traversal_indices(&traversal, &positions, entries.len())?
        } else {
            (0..entries.len()).collect()
        };
        let traversal_position = selected_index
            .and_then(|selected| traversal.iter().position(|index| *index == selected));
        let track_counts = track_counts(&entries);
        Ok(Self {
            source_id,
            next_occurrence_number: next_occurrence_number(&entries),
            entries,
            selected_index,
            repeat_mode,
            shuffle_enabled,
            traversal,
            traversal_position,
            revision,
            progress_millis,
            track_counts,
        })
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn entries(&self) -> &[SequenceEntry] {
        &self.entries
    }

    pub fn selected(&self) -> Option<&SequenceEntry> {
        self.selected_index
            .and_then(|index| self.entries.get(index))
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn occurrence(&self, occurrence: &OccurrenceId) -> Option<&SequenceEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.occurrence == occurrence)
    }

    pub fn contains_track(&self, track_id: &TrackId) -> bool {
        self.track_counts.contains_key(track_id)
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }

    pub fn shuffle_enabled(&self) -> bool {
        self.shuffle_enabled
    }

    pub fn traversal(&self) -> Vec<&OccurrenceId> {
        self.traversal
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| &entry.occurrence)
            .collect()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn progress_millis(&self) -> u64 {
        self.progress_millis
    }

    pub fn set_progress_millis(&mut self, progress_millis: u64) {
        self.progress_millis = progress_millis;
    }

    pub fn remaining_after_selected(&self) -> usize {
        self.traversal_position
            .map(|position| self.traversal.len().saturating_sub(position + 1))
            .unwrap_or_default()
    }

    pub fn peek_next_eos(&self) -> Option<&SequenceEntry> {
        self.next_index(true)
            .and_then(|index| self.entries.get(index))
    }

    pub(crate) fn next_index_eos(&self) -> Option<usize> {
        self.next_index(true)
    }

    pub fn apply_batch(
        &mut self,
        batch: Batch,
        placement: Placement,
    ) -> Result<Vec<OccurrenceId>, SequenceError> {
        if batch.items.is_empty() {
            return Err(SequenceError::EmptyBatch);
        }
        if let Placement::Replace { anchor_index } = placement
            && anchor_index >= batch.items.len()
        {
            return Err(SequenceError::InvalidAnchor);
        }
        let changes_selection =
            matches!(placement, Placement::Replace { .. }) || self.selected_index.is_none();

        let previous_track_id = self.selected().map(|entry| entry.track.id.clone());
        let mut selected_anchor = match placement {
            Placement::Replace { anchor_index } => anchor_index,
            Placement::AfterCurrent | Placement::End => 0,
        };
        let mut batch_traversal = if self.shuffle_enabled {
            shuffled_indices(batch.items.len(), batch.shuffle_seed)
        } else {
            (0..batch.items.len()).collect()
        };
        if self.shuffle_enabled {
            match placement {
                Placement::Replace { .. } if batch.random_start => {
                    if let Some(previous_track_id) = previous_track_id.as_ref()
                        && let Some(position) = batch_traversal.iter().position(|index| {
                            batch
                                .items
                                .get(*index)
                                .is_some_and(|item| &item.track.id != previous_track_id)
                        })
                    {
                        batch_traversal.swap(0, position);
                    }
                    selected_anchor = batch_traversal[0];
                }
                Placement::Replace { anchor_index } => {
                    pin_index(&mut batch_traversal, anchor_index);
                }
                Placement::AfterCurrent => pin_index(&mut batch_traversal, 0),
                Placement::End => {}
            }
        }
        let mut inserted = Vec::with_capacity(batch.items.len());
        let mut new_entries = Vec::with_capacity(batch.items.len());
        for item in batch.items {
            let occurrence = self.next_occurrence();
            inserted.push(occurrence.clone());
            new_entries.push(SequenceEntry {
                occurrence,
                track: item.track,
                provenance: item.provenance,
            });
        }
        match placement {
            Placement::Replace { .. } => {
                self.entries = new_entries;
                self.selected_index = Some(selected_anchor);
                self.traversal = if self.shuffle_enabled {
                    batch_traversal
                } else {
                    (0..self.entries.len()).collect()
                };
            }
            placement => {
                let batch_order = batch_traversal
                    .iter()
                    .copied()
                    .map(|index| inserted[index].clone())
                    .collect::<Vec<_>>();
                self.insert_batch(
                    new_entries,
                    &batch_order,
                    placement == Placement::AfterCurrent,
                );
            }
        }
        if changes_selection {
            self.progress_millis = 0;
        }
        self.revision = self.revision.wrapping_add(1);
        self.sync_traversal_position();
        self.rebuild_track_counts();
        Ok(inserted)
    }

    pub fn activate(&mut self, occurrence: &OccurrenceId) -> bool {
        let Some(index) = self.occurrence_index(occurrence) else {
            return false;
        };
        self.activate_index(index)
    }

    pub(crate) fn occurrence_index(&self, occurrence: &OccurrenceId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| &entry.occurrence == occurrence)
    }

    pub(crate) fn context_index(
        &self,
        context_id: &str,
        track_id: &TrackId,
        source_rank: usize,
    ) -> Option<usize> {
        self.entries.iter().position(|entry| {
            &entry.track.id == track_id
                && matches!(
                    &entry.provenance,
                    Provenance::Context {
                        context_id: entry_context,
                        source_rank: entry_rank,
                    } if entry_context == context_id && *entry_rank == source_rank
                )
        })
    }

    pub(crate) fn activate_index(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.selected_index = Some(index);
        self.progress_millis = 0;
        self.sync_traversal_position();
        true
    }

    pub fn remove(&mut self, occurrence: &OccurrenceId) -> Option<SequenceEntry> {
        let remove_index = self
            .entries
            .iter()
            .position(|entry| &entry.occurrence == occurrence)?;
        let selected = self.selected().map(|entry| entry.occurrence.clone());
        let removing_selected = selected.as_ref() == Some(occurrence);
        let successor = removing_selected
            .then(|| self.successor_after_removed_selected())
            .flatten();
        let traversal = self.traversal_occurrences();
        let removed = self.entries.remove(remove_index);
        let selected = if removing_selected {
            successor
        } else {
            selected
        };
        self.rebuild_from_occurrences(
            traversal
                .into_iter()
                .filter(|candidate| candidate != occurrence)
                .collect(),
            selected.as_ref(),
        );
        if removing_selected {
            self.progress_millis = 0;
        }
        self.revision = self.revision.wrapping_add(1);
        self.rebuild_track_counts();
        Some(removed)
    }

    pub fn reorder(&mut self, occurrence: &OccurrenceId, new_index: usize) -> bool {
        let Some(old_index) = self
            .entries
            .iter()
            .position(|entry| &entry.occurrence == occurrence)
        else {
            return false;
        };
        let selected = self.selected().map(|entry| entry.occurrence.clone());
        let traversal = self.traversal_occurrences();
        let entry = self.entries.remove(old_index);
        let target = new_index.min(self.entries.len());
        self.entries.insert(target, entry);
        let traversal = if self.shuffle_enabled {
            traversal
        } else {
            self.entries
                .iter()
                .map(|entry| entry.occurrence.clone())
                .collect()
        };
        self.rebuild_from_occurrences(traversal, selected.as_ref());
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn move_after_current(&mut self, occurrence: &OccurrenceId) -> bool {
        let Some(current) = self.selected().map(|entry| entry.occurrence.clone()) else {
            return false;
        };
        if &current == occurrence {
            return false;
        }
        let Some(old_index) = self
            .entries
            .iter()
            .position(|entry| &entry.occurrence == occurrence)
        else {
            return false;
        };
        let mut traversal = self.traversal_occurrences();
        let entry = self.entries.remove(old_index);
        let Some(current_index) = self
            .entries
            .iter()
            .position(|entry| entry.occurrence == current)
        else {
            return false;
        };
        self.entries.insert(current_index + 1, entry);
        traversal.retain(|candidate| candidate != occurrence);
        let Some(current_position) = traversal.iter().position(|candidate| candidate == &current)
        else {
            return false;
        };
        traversal.insert(current_position + 1, occurrence.clone());
        if !self.shuffle_enabled {
            traversal = self
                .entries
                .iter()
                .map(|entry| entry.occurrence.clone())
                .collect();
        }
        self.rebuild_from_occurrences(traversal, Some(&current));
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub(crate) fn clear(&mut self) -> bool {
        let changed = !self.entries.is_empty();
        self.entries.clear();
        self.track_counts.clear();
        self.traversal.clear();
        self.selected_index = None;
        self.traversal_position = None;
        self.progress_millis = 0;
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    pub(crate) fn clear_upcoming(&mut self) -> bool {
        let Some(current) = self.selected().cloned() else {
            return self.clear();
        };
        if self.entries.len() == 1 {
            return false;
        }
        self.entries = vec![current];
        self.selected_index = Some(0);
        self.traversal = vec![0];
        self.traversal_position = Some(0);
        self.rebuild_track_counts();
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn trim_auto_dj_history(&mut self, keep: usize) -> bool {
        let Some(position) = self.traversal_position else {
            return false;
        };
        let auto_dj_history = self
            .traversal
            .iter()
            .take(position)
            .filter_map(|index| self.entries.get(*index))
            .filter(|entry| entry.provenance == Provenance::AutoDj)
            .map(|entry| entry.occurrence.clone())
            .collect::<Vec<_>>();
        let remove_count = auto_dj_history.len().saturating_sub(keep);
        if remove_count == 0 {
            return false;
        }
        let removed = auto_dj_history
            .into_iter()
            .take(remove_count)
            .collect::<HashSet<_>>();
        let selected = self.selected().map(|entry| entry.occurrence.clone());
        let traversal = self
            .traversal_occurrences()
            .into_iter()
            .filter(|occurrence| !removed.contains(occurrence))
            .collect();
        self.entries
            .retain(|entry| !removed.contains(&entry.occurrence));
        self.rebuild_from_occurrences(traversal, selected.as_ref());
        self.revision = self.revision.wrapping_add(1);
        self.rebuild_track_counts();
        true
    }

    pub fn advance_manual(&mut self) -> Option<&SequenceEntry> {
        self.advance(false)
    }

    pub fn advance_eos(&mut self) -> Option<&SequenceEntry> {
        self.advance(true)
    }

    pub fn previous(&mut self) -> Option<&SequenceEntry> {
        let previous_position = self.previous_position()?;
        self.select_traversal_position(previous_position)
    }

    pub fn peek_previous(&self) -> Option<&SequenceEntry> {
        self.previous_position()
            .and_then(|position| self.traversal.get(position))
            .and_then(|index| self.entries.get(*index))
    }

    pub fn upcoming(&self, limit: usize) -> Vec<&SequenceEntry> {
        if limit == 0 || self.repeat_mode == RepeatMode::One || self.traversal.is_empty() {
            return Vec::new();
        }
        let (start, maximum) = match self.traversal_position {
            Some(position) => (
                position.saturating_add(1),
                self.entries.len().saturating_sub(1),
            ),
            None => (0, self.entries.len()),
        };
        let maximum = limit.min(maximum);
        let mut upcoming = Vec::with_capacity(maximum);
        for offset in 0..maximum {
            let position = start.saturating_add(offset);
            let position = if position < self.traversal.len() {
                position
            } else if self.repeat_mode == RepeatMode::All {
                position % self.traversal.len()
            } else {
                break;
            };
            if let Some(entry) = self
                .traversal
                .get(position)
                .and_then(|index| self.entries.get(*index))
            {
                upcoming.push(entry);
            }
        }
        upcoming
    }

    pub fn set_repeat_mode(&mut self, repeat_mode: RepeatMode) {
        self.repeat_mode = repeat_mode;
    }

    pub fn set_shuffle_seed(&mut self, enabled: bool, seed: u64) -> bool {
        let mut traversal = if enabled {
            shuffled_indices(self.entries.len(), seed)
        } else {
            (0..self.entries.len()).collect()
        };
        if enabled
            && let Some(selected) = self.selected_index
            && let Some(position) = traversal.iter().position(|index| *index == selected)
        {
            traversal.swap(0, position);
        }
        self.install_shuffle(enabled, traversal)
    }

    pub fn hydrate_tracks(&mut self, tracks: Vec<Track>) {
        self.replace_track_facts(tracks);
    }

    pub fn refresh_tracks(&mut self, tracks: Vec<Track>) -> bool {
        let changed = self.replace_track_facts(tracks);
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    fn replace_track_facts(&mut self, tracks: Vec<Track>) -> bool {
        let replacements = tracks
            .into_iter()
            .map(|track| (track.id.clone(), track))
            .collect::<HashMap<_, _>>();
        let mut changed = false;
        for entry in &mut self.entries {
            if let Some(track) = replacements.get(&entry.track.id)
                && &entry.track != track
            {
                entry.track = track.clone();
                changed = true;
            }
        }
        changed
    }

    fn insert_batch(
        &mut self,
        new_entries: Vec<SequenceEntry>,
        batch_order: &[OccurrenceId],
        after_current: bool,
    ) {
        let selected = self.selected().map(|entry| entry.occurrence.clone());
        let mut traversal = self.traversal_occurrences();
        let insert_index = if after_current {
            self.selected_index
                .map(|index| index + 1)
                .unwrap_or_default()
        } else {
            self.entries.len()
        };
        self.entries.splice(insert_index..insert_index, new_entries);
        let selected = selected.or_else(|| batch_order.first().cloned());
        if self.shuffle_enabled {
            let traversal_insert = if after_current {
                selected
                    .as_ref()
                    .and_then(|current| traversal.iter().position(|id| id == current))
                    .map(|position| position + 1)
                    .unwrap_or_default()
            } else {
                traversal.len()
            };
            traversal.splice(
                traversal_insert..traversal_insert,
                batch_order.iter().cloned(),
            );
        } else {
            traversal = self
                .entries
                .iter()
                .map(|entry| entry.occurrence.clone())
                .collect();
        }
        self.rebuild_from_occurrences(traversal, selected.as_ref());
    }

    fn advance(&mut self, eos: bool) -> Option<&SequenceEntry> {
        let next = self.next_index(eos)?;
        self.selected_index = Some(next);
        self.progress_millis = 0;
        self.sync_traversal_position();
        self.selected()
    }

    fn next_index(&self, eos: bool) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let Some(position) = self.traversal_position else {
            return self.traversal.first().copied();
        };
        if eos && self.repeat_mode == RepeatMode::One {
            return self.selected_index;
        }
        self.traversal.get(position + 1).copied().or_else(|| {
            (self.repeat_mode == RepeatMode::All)
                .then(|| self.traversal.first().copied())
                .flatten()
        })
    }

    fn previous_position(&self) -> Option<usize> {
        let position = self.traversal_position?;
        if position > 0 {
            Some(position - 1)
        } else if self.repeat_mode == RepeatMode::All {
            self.traversal.len().checked_sub(1)
        } else {
            None
        }
    }

    fn select_traversal_position(&mut self, position: usize) -> Option<&SequenceEntry> {
        self.selected_index = self.traversal.get(position).copied();
        self.traversal_position = self.selected_index.map(|_| position);
        self.progress_millis = 0;
        self.selected()
    }

    fn successor_after_removed_selected(&self) -> Option<OccurrenceId> {
        let position = self.traversal_position?;
        self.traversal
            .get(position + 1)
            .or_else(|| {
                (self.repeat_mode == RepeatMode::All)
                    .then(|| self.traversal.first())
                    .flatten()
            })
            .and_then(|index| self.entries.get(*index))
            .map(|entry| entry.occurrence.clone())
            .filter(|occurrence| {
                self.selected()
                    .is_none_or(|entry| occurrence != &entry.occurrence)
            })
    }

    fn next_occurrence(&mut self) -> OccurrenceId {
        let occurrence = OccurrenceId::new(format!("occurrence-{}", self.next_occurrence_number));
        self.next_occurrence_number = self.next_occurrence_number.wrapping_add(1).max(1);
        occurrence
    }

    fn traversal_occurrences(&self) -> Vec<OccurrenceId> {
        self.traversal
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| entry.occurrence.clone())
            .collect()
    }

    fn rebuild_from_occurrences(
        &mut self,
        traversal: Vec<OccurrenceId>,
        selected: Option<&OccurrenceId>,
    ) {
        let positions = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.occurrence.clone(), index))
            .collect::<HashMap<_, _>>();
        self.traversal = traversal
            .into_iter()
            .filter_map(|occurrence| positions.get(&occurrence).copied())
            .collect();
        self.selected_index = selected.and_then(|occurrence| positions.get(occurrence).copied());
        self.sync_traversal_position();
    }

    fn sync_traversal_position(&mut self) {
        self.traversal_position = self.selected_index.and_then(|selected| {
            self.traversal
                .iter()
                .position(|candidate| *candidate == selected)
        });
    }

    fn install_shuffle(&mut self, enabled: bool, traversal: Vec<usize>) -> bool {
        if self.shuffle_enabled == enabled && self.traversal == traversal {
            return false;
        }
        self.shuffle_enabled = enabled;
        self.traversal = traversal;
        self.sync_traversal_position();
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn rebuild_track_counts(&mut self) {
        self.track_counts = track_counts(&self.entries);
    }
}

fn track_counts(entries: &[SequenceEntry]) -> HashMap<TrackId, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(entry.track.id.clone()).or_insert(0) += 1;
    }
    counts
}

fn shuffled_indices(len: usize, mut state: u64) -> Vec<usize> {
    let mut indices = (0..len).collect::<Vec<_>>();
    if state == 0 {
        state = 0x9e37_79b9_7f4a_7c15;
    }
    for index in (1..indices.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        indices.swap(index, (state as usize) % (index + 1));
    }
    indices
}

fn pin_index(indices: &mut [usize], pinned: usize) {
    if let Some(position) = indices.iter().position(|index| *index == pinned) {
        indices.swap(0, position);
    }
}

fn occurrence_positions(
    entries: &[SequenceEntry],
) -> Result<HashMap<OccurrenceId, usize>, SequenceError> {
    let mut positions = HashMap::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if positions.insert(entry.occurrence.clone(), index).is_some() {
            return Err(SequenceError::DuplicateOccurrence);
        }
    }
    Ok(positions)
}

fn traversal_indices(
    traversal: &[OccurrenceId],
    positions: &HashMap<OccurrenceId, usize>,
    len: usize,
) -> Result<Vec<usize>, SequenceError> {
    if traversal.len() != len {
        return Err(SequenceError::InvalidTraversal);
    }
    let mut seen = HashSet::with_capacity(len);
    let mut indices = Vec::with_capacity(len);
    for occurrence in traversal {
        if !seen.insert(occurrence) {
            return Err(SequenceError::InvalidTraversal);
        }
        indices.push(
            positions
                .get(occurrence)
                .copied()
                .ok_or(SequenceError::InvalidTraversal)?,
        );
    }
    Ok(indices)
}

fn next_occurrence_number(entries: &[SequenceEntry]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| entry.occurrence.as_str().strip_prefix("occurrence-"))
        .filter_map(|number| number.parse::<u64>().ok())
        .max()
        .unwrap_or_default()
        .saturating_add(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, SourceId, Track, TrackId};

    use super::*;

    #[test]
    fn one_batch_serves_every_placement_without_per_item_transitions() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let first = sequence
            .apply_batch(batch(&[1, 2, 3]), Placement::Replace { anchor_index: 1 })
            .expect("replace batch");
        assert_eq!(sequence.revision(), 1);
        assert_eq!(
            sequence.selected().map(|entry| &entry.occurrence),
            Some(&first[1])
        );

        let next = sequence
            .apply_batch(batch(&[4, 5]), Placement::AfterCurrent)
            .expect("next batch");
        assert_eq!(sequence.revision(), 2);
        assert_eq!(
            sequence
                .entries()
                .iter()
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>(),
            [1, 2, 4, 5, 3]
                .into_iter()
                .map(TrackId::fake)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            sequence.peek_next_eos().map(|entry| &entry.occurrence),
            Some(&next[0])
        );

        sequence.set_progress_millis(42_000);

        sequence
            .apply_batch(batch(&[6, 7]), Placement::End)
            .expect("end batch");
        assert_eq!(sequence.revision(), 3);
        assert_eq!(sequence.progress_millis(), 42_000);
        assert_eq!(
            sequence
                .entries()
                .last()
                .map(|entry| entry.track.id.clone()),
            Some(TrackId::fake(7))
        );
    }

    #[test]
    fn shuffled_repeat_one_only_controls_eos() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let ids = sequence
            .apply_batch(batch(&[1, 2, 3]), Placement::Replace { anchor_index: 0 })
            .expect("replace batch");
        assert!(sequence.set_shuffle_seed(true, 0x1234_5678));
        let traversal = sequence
            .traversal()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        sequence.set_repeat_mode(RepeatMode::One);

        assert_eq!(
            sequence.advance_eos().map(|entry| &entry.occurrence),
            Some(&ids[0])
        );
        assert_eq!(
            sequence.advance_manual().map(|entry| &entry.occurrence),
            traversal.get(1)
        );
    }

    #[test]
    fn sequence_owns_shuffled_batch_start_and_play_next_order() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(batch(&[1]), Placement::Replace { anchor_index: 0 })
            .expect("initial track");
        assert!(sequence.set_shuffle_seed(true, 17));

        sequence
            .apply_batch(
                batch(&[1, 2, 3]).with_shuffle_intent(23, true),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("random replacement");
        assert_ne!(
            sequence.selected().map(|entry| entry.track.id.clone()),
            Some(TrackId::fake(1))
        );

        let inserted = sequence
            .apply_batch(
                batch(&[4, 5, 6]).with_shuffle_intent(29, false),
                Placement::AfterCurrent,
            )
            .expect("play next batch");
        assert_eq!(
            sequence.peek_next_eos().map(|entry| &entry.occurrence),
            Some(&inserted[0])
        );
    }

    #[test]
    fn activation_targets_the_exact_duplicate_occurrence() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let ids = sequence
            .apply_batch(batch(&[7, 7]), Placement::Replace { anchor_index: 0 })
            .expect("duplicate batch");

        assert!(sequence.activate(&ids[1]));
        assert_eq!(
            sequence.selected().map(|entry| &entry.occurrence),
            Some(&ids[1])
        );
    }

    #[test]
    fn seeded_shuffle_pins_current_and_new_seeds_can_choose_a_new_order() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let ids = sequence
            .apply_batch(
                batch(&[1, 2, 3, 4, 5, 6, 7, 8]),
                Placement::Replace { anchor_index: 3 },
            )
            .expect("replace batch");

        let revision = sequence.revision();
        assert!(sequence.set_shuffle_seed(true, 0x1234_5678_9abc_def0));
        assert_eq!(sequence.revision(), revision + 1);
        let first = sequence
            .traversal()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(first.first(), Some(&ids[3]));
        assert_eq!(first.iter().collect::<HashSet<_>>().len(), ids.len());

        let revision = sequence.revision();
        assert!(!sequence.set_shuffle_seed(true, 0x1234_5678_9abc_def0));
        assert_eq!(sequence.revision(), revision);
        assert!(sequence.set_shuffle_seed(true, 0xfedc_ba98_7654_3210));
        let second = sequence
            .traversal()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(second.first(), Some(&ids[3]));
        assert_ne!(second, first);

        assert!(sequence.set_shuffle_seed(false, 0));
        assert!(!sequence.shuffle_enabled());
        assert_eq!(
            sequence
                .traversal()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn five_thousand_entry_replace_keeps_one_revision_and_the_complete_traversal() {
        const LEN: usize = 5_000;
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(
                batch_range(0, LEN as u32),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("initial large batch");
        assert!(sequence.set_shuffle_seed(true, 0x1234_5678));

        let revision = sequence.revision();
        let batch = Batch::new(
            (LEN as u32..(LEN * 2) as u32)
                .map(|number| BatchItem::new(track(number), Provenance::Manual))
                .collect(),
        )
        .with_shuffle_intent(0x8765_4321, false);
        let anchor = LEN / 2;
        let inserted = sequence
            .apply_batch(
                batch,
                Placement::Replace {
                    anchor_index: anchor,
                },
            )
            .expect("replace large batch");

        assert_eq!(sequence.revision(), revision + 1);
        assert_eq!(sequence.entries().len(), LEN);
        assert_eq!(
            sequence.selected().map(|entry| &entry.occurrence),
            Some(&inserted[anchor])
        );
        let traversal = sequence
            .traversal()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(traversal.first(), Some(&inserted[anchor]));
        assert_eq!(traversal.len(), LEN);
        assert_eq!(
            sequence.page(crate::QueuePageQuery::at(0)).rows.len(),
            crate::MAX_QUEUE_PAGE_SIZE
        );

        sequence.set_repeat_mode(RepeatMode::All);
        let selected = sequence
            .selected()
            .map(|entry| entry.occurrence.clone())
            .expect("selected occurrence");
        let mut visited = HashSet::with_capacity(LEN);
        for _ in 0..LEN {
            visited.insert(
                sequence
                    .selected()
                    .map(|entry| entry.occurrence.clone())
                    .expect("selected while traversing"),
            );
            sequence.advance_manual().expect("repeat-all successor");
        }
        assert_eq!(visited.len(), LEN);
        assert_eq!(
            sequence.selected().map(|entry| &entry.occurrence),
            Some(&selected)
        );
    }

    #[test]
    fn upcoming_is_bounded_and_follows_the_live_traversal_without_current() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let ids = sequence
            .apply_batch(
                batch(&[1, 2, 3, 4, 5]),
                Placement::Replace { anchor_index: 3 },
            )
            .expect("replace batch");

        assert_eq!(
            sequence
                .upcoming(10)
                .into_iter()
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>(),
            vec![TrackId::fake(5)]
        );

        sequence.set_repeat_mode(RepeatMode::All);
        assert_eq!(
            sequence
                .upcoming(10)
                .into_iter()
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>(),
            [5, 1, 2, 3]
                .into_iter()
                .map(TrackId::fake)
                .collect::<Vec<_>>()
        );

        assert!(sequence.set_shuffle_seed(true, 19));
        let upcoming = sequence.upcoming(10);
        assert_eq!(upcoming.len(), 4);
        assert!(upcoming.iter().all(|entry| entry.occurrence != ids[3]));

        sequence.set_repeat_mode(RepeatMode::One);
        assert!(sequence.upcoming(10).is_empty());
    }

    #[test]
    fn edits_keep_current_identity_and_only_reset_progress_when_current_is_removed() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let ids = sequence
            .apply_batch(batch(&[1, 2, 3, 4]), Placement::Replace { anchor_index: 1 })
            .expect("replace batch");
        sequence.set_progress_millis(48_000);

        let revision = sequence.revision();
        assert!(!sequence.move_after_current(&ids[1]));
        assert_eq!(sequence.revision(), revision);
        assert!(sequence.reorder(&ids[3], 0));
        assert!(sequence.move_after_current(&ids[3]));
        assert_eq!(
            sequence
                .entries()
                .iter()
                .map(|entry| entry.track.id.clone())
                .collect::<Vec<_>>(),
            [1, 2, 4, 3]
                .into_iter()
                .map(TrackId::fake)
                .collect::<Vec<_>>()
        );
        assert_eq!(sequence.progress_millis(), 48_000);

        sequence.remove(&ids[0]).expect("remove noncurrent");
        assert_eq!(sequence.progress_millis(), 48_000);
        sequence.remove(&ids[1]).expect("remove current");
        assert_eq!(sequence.progress_millis(), 0);
        assert_eq!(
            sequence.selected().map(|entry| &entry.occurrence),
            Some(&ids[3])
        );
    }

    fn batch(numbers: &[u32]) -> Batch {
        Batch::new(
            numbers
                .iter()
                .map(|number| BatchItem::new(track(*number), Provenance::Manual))
                .collect(),
        )
    }

    fn batch_range(start: u32, end: u32) -> Batch {
        Batch::new(
            (start..end)
                .map(|number| BatchItem::new(track(number), Provenance::Manual))
                .collect(),
        )
    }

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
            album_artwork: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }
}
