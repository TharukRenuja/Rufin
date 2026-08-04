use std::{cell::RefCell, collections::HashMap};

use gtk::{gio, glib, prelude::*};
use library::{AcceptedTrackReplacement, LibraryQueryResult, SourceId, Track, TrackId, TrackList};
use playback::{LoadedPlayRequest, QueuePlacement, SourceSessionEpoch};

use crate::LibraryListSettings;

use super::models::track_matches_query;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrackProjectionRequest {
    pub(crate) query: String,
    pub(crate) settings: LibraryListSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedTrackProjection {
    source: TrackList,
    visible: TrackList,
    request: TrackProjectionRequest,
}

/// Prepares one complete Track model projection away from GTK.
///
/// The mounted model remains the only row and selection owner. This function
/// only derives its compact visible order from the accepted source order.
pub(crate) fn prepare_track_projection(
    source: TrackList,
    request: TrackProjectionRequest,
) -> LibraryQueryResult<PreparedTrackProjection> {
    let visible = visible_tracks(&source, &request.query, &request.settings)?;
    Ok(PreparedTrackProjection {
        source,
        visible,
        request,
    })
}

struct TrackModelState {
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    source: TrackList,
    visible: TrackList,
    rows: HashMap<u32, glib::WeakRef<glib::BoxedAnyObject>>,
    query: String,
    settings: LibraryListSettings,
    point_change: Option<TrackPointChange>,
    #[cfg(test)]
    order_rebuilds: usize,
    #[cfg(test)]
    point_updates: usize,
    #[cfg(test)]
    point_order_slot_copies: usize,
    #[cfg(test)]
    point_notified_items: usize,
}

#[derive(Clone, Debug)]
struct TrackPointChange {
    id: TrackId,
    previous_position: Option<u32>,
    position: Option<u32>,
}

impl TrackModelState {
    fn new(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        source: TrackList,
        settings: LibraryListSettings,
    ) -> Self {
        let visible = visible_tracks(&source, "", &settings)
            .expect("a mounted Track list keeps its loaded Library available");
        Self {
            source_id,
            source_session_epoch,
            source,
            visible,
            rows: HashMap::new(),
            query: String::new(),
            settings,
            point_change: None,
            #[cfg(test)]
            order_rebuilds: 1,
            #[cfg(test)]
            point_updates: 0,
            #[cfg(test)]
            point_order_slot_copies: 0,
            #[cfg(test)]
            point_notified_items: 0,
        }
    }

    fn rebuild_visible(&mut self) {
        self.visible = visible_tracks(&self.source, &self.query, &self.settings)
            .expect("a mounted Track list keeps its loaded Library available");
        self.rows.clear();
        #[cfg(test)]
        {
            self.order_rebuilds += 1;
        }
    }
}

mod imp {
    use gio::subclass::prelude::*;

    use super::*;

    #[derive(Default)]
    pub(crate) struct TrackCollectionModel {
        pub(super) state: RefCell<Option<TrackModelState>>,
        #[cfg(test)]
        pub(super) row_materializations: std::cell::Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackCollectionModel {
        const NAME: &'static str = "RufinTrackCollectionModel";
        type Type = super::TrackCollectionModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for TrackCollectionModel {}

    impl ListModelImpl for TrackCollectionModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            self.state
                .borrow()
                .as_ref()
                .map_or(0, |state| position_u32(state.visible.len()))
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let mut borrowed = self.state.borrow_mut();
            let state = borrowed.as_mut()?;
            if let Some(row) = state.rows.get(&position).and_then(glib::WeakRef::upgrade) {
                return Some(row.upcast());
            }
            state.rows.retain(|_, row| row.upgrade().is_some());
            let track = state.visible.track(position as usize).ok().flatten()?;
            #[cfg(test)]
            self.row_materializations
                .set(self.row_materializations.get() + 1);
            let row = glib::BoxedAnyObject::new(track);
            state.rows.insert(position, row.downgrade());
            Some(row.upcast())
        }
    }
}

glib::wrapper! {
    pub(crate) struct TrackCollectionModel(
        ObjectSubclass<imp::TrackCollectionModel>
    ) @implements gio::ListModel;
}

impl TrackCollectionModel {
    pub(crate) fn new(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        tracks: TrackList,
        settings: LibraryListSettings,
    ) -> Self {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let model: Self = glib::Object::new();
        model.imp().state.replace(Some(TrackModelState::new(
            source_id,
            source_session_epoch,
            tracks,
            settings,
        )));
        model
    }

    pub(crate) fn source_is_empty(&self) -> bool {
        self.with_state(|state| state.source.is_empty())
    }

    pub(crate) fn play_request(
        &self,
        anchor_index: usize,
        placement: QueuePlacement,
        context_base: &str,
        shuffled_start: bool,
    ) -> Option<LoadedPlayRequest> {
        self.with_state(|state| {
            LoadedPlayRequest::context(
                state.source_id.clone(),
                state.source_session_epoch,
                state.visible.clone(),
                anchor_index,
                placement,
                visible_context_id(state, context_base),
                shuffled_start,
            )
        })
    }

    pub(crate) fn visible_context_id(&self, context_base: &str) -> String {
        self.with_state(|state| visible_context_id(state, context_base))
    }

    pub(crate) fn source_play_request(
        &self,
        placement: QueuePlacement,
        context_id: &str,
        shuffled_start: bool,
    ) -> Option<LoadedPlayRequest> {
        self.with_state(|state| {
            LoadedPlayRequest::context(
                state.source_id.clone(),
                state.source_session_epoch,
                state.source.clone(),
                0,
                placement,
                context_id,
                shuffled_start,
            )
        })
    }

    pub(crate) fn visible_count(&self) -> usize {
        self.with_state(|state| state.visible.len())
    }

    #[cfg(test)]
    pub(crate) fn query(&self) -> String {
        self.with_state(|state| state.query.clone())
    }

    pub(crate) fn settings(&self) -> LibraryListSettings {
        self.with_state(|state| state.settings.clone())
    }

    pub(crate) fn projection_request(&self) -> TrackProjectionRequest {
        self.with_state(|state| TrackProjectionRequest {
            query: state.query.clone(),
            settings: state.settings.clone(),
        })
    }

    pub(crate) fn position(&self, track_id: &TrackId) -> Option<u32> {
        self.with_state(|state| state.visible.position(track_id).ok().flatten())
    }

    pub(crate) fn played_at(&self, position: u32) -> Option<i64> {
        self.with_state(|state| state.visible.played_at(position as usize))
    }

    pub(crate) fn played_at_text(&self, position: u32) -> String {
        self.played_at(position)
            .map(history_played_at_text)
            .unwrap_or_default()
    }

    pub(crate) fn position_for_current(
        &self,
        track_id: &TrackId,
        source_rank: Option<usize>,
    ) -> Option<u32> {
        self.with_state(|state| {
            source_rank
                .and_then(|position| {
                    state
                        .visible
                        .track(position)
                        .ok()
                        .flatten()
                        .filter(|track| &track.id == track_id)
                        .and_then(|_| u32::try_from(position).ok())
                })
                .or_else(|| state.visible.position(track_id).ok().flatten())
        })
    }

    /// Translates the known selected position through the exact point change
    /// currently being announced by `items_changed`.
    ///
    /// The model signal is synchronous, so this hint exists only while its
    /// matching notification is running. Other model changes fall back to the
    /// ordinary identity lookup.
    pub(crate) fn selection_position_after_point_change(
        &self,
        track_id: &TrackId,
        selected_position: u32,
    ) -> Option<u32> {
        self.with_state(|state| {
            let change = state.point_change.as_ref()?;
            if &change.id == track_id {
                return Some(change.position.unwrap_or(gtk::INVALID_LIST_POSITION));
            }
            if selected_position == gtk::INVALID_LIST_POSITION {
                return Some(selected_position);
            }
            match (change.previous_position, change.position) {
                (Some(previous), Some(position)) if previous < position => {
                    if selected_position == previous {
                        None
                    } else if selected_position > previous && selected_position <= position {
                        Some(selected_position - 1)
                    } else {
                        Some(selected_position)
                    }
                }
                (Some(previous), Some(position)) if position < previous => {
                    if selected_position == previous {
                        None
                    } else if selected_position >= position && selected_position < previous {
                        Some(selected_position + 1)
                    } else {
                        Some(selected_position)
                    }
                }
                (Some(previous), None) => {
                    if selected_position == previous {
                        None
                    } else if selected_position > previous {
                        Some(selected_position - 1)
                    } else {
                        Some(selected_position)
                    }
                }
                (None, Some(position)) if selected_position >= position => {
                    Some(selected_position + 1)
                }
                (Some(_), Some(_)) | (None, Some(_)) | (None, None) => Some(selected_position),
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn track_at(&self, position: u32) -> Option<Track> {
        self.with_state(|state| state.visible.track(position as usize).ok().flatten())
    }

    pub(crate) fn set_query(&self, query: &str) -> bool {
        let query = query.trim();
        let (old_len, new_len, changed) = self.with_state_mut(|state| {
            if state.query == query {
                return (state.visible.len(), state.visible.len(), false);
            }
            let old_len = state.visible.len();
            state.query = query.to_string();
            state.rebuild_visible();
            (old_len, state.visible.len(), true)
        });
        if changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        changed
    }

    pub(crate) fn apply_settings(&self, settings: LibraryListSettings) -> bool {
        let (old_len, new_len, changed) = self.with_state_mut(|state| {
            let order_changed = state.settings.sort_key != settings.sort_key
                || state.settings.descending != settings.descending;
            state.settings = settings;
            if !order_changed {
                return (state.visible.len(), state.visible.len(), false);
            }
            let old_len = state.visible.len();
            state.rebuild_visible();
            (old_len, state.visible.len(), true)
        });
        if changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        changed
    }

    /// Admits a complete order prepared by the mounted read worker.
    ///
    /// A query or settings change that happened while the worker ran rejects
    /// the stale result. Swapping the compact orders, clearing bound rows, and
    /// notifying the model are the only GTK-thread work.
    pub(crate) fn replace_prepared(&self, prepared: PreparedTrackProjection) -> bool {
        let result = self.with_state_mut(|state| {
            let current = TrackProjectionRequest {
                query: state.query.clone(),
                settings: state.settings.clone(),
            };
            if current != prepared.request {
                return None;
            }
            let old_len = state.visible.len();
            state.source = prepared.source;
            state.visible = prepared.visible;
            state.rows.clear();
            state.point_change = None;
            #[cfg(test)]
            {
                state.order_rebuilds += 1;
            }
            Some((old_len, state.visible.len()))
        });
        let Some((old_len, new_len)) = result else {
            return false;
        };
        self.items_changed(0, position_u32(old_len), position_u32(new_len));
        true
    }

    /// Applies the ordinary one-Track update path without rereading and
    /// resorting the complete route projection.
    ///
    /// Multi-Track changes and deletions return `false` so the caller can
    /// request a fresh complete projection.
    pub(crate) fn apply_track_replacement(
        &self,
        replacements: &[AcceptedTrackReplacement],
        include: impl Fn(&Track) -> bool,
    ) -> bool {
        let [replacement] = replacements else {
            return false;
        };
        let Some(track) = replacement.track.as_ref() else {
            return false;
        };
        let result = self.with_state_mut(|state| {
            let source_include = include(track);
            let source_and_visible_share_order = state.source.shares_order(&state.visible);
            let Some(source) = state
                .source
                .with_current_track(&replacement.id, |_| source_include)
                .ok()
                .flatten()
            else {
                return None;
            };
            let query = state.query.to_lowercase();
            let visible_include =
                source_include && (query.is_empty() || track_matches_query(track, &query));
            let reuse_source_order =
                source_and_visible_share_order && source_include == visible_include;
            let visible = if reuse_source_order {
                source.clone()
            } else {
                let Some(visible) = state
                    .visible
                    .with_current_track(&replacement.id, |_| visible_include)
                    .ok()
                    .flatten()
                else {
                    return None;
                };
                visible
            };
            #[cfg(test)]
            {
                state.point_updates += 1;
                if source.order_changed {
                    state.point_order_slot_copies += state.source.len();
                }
                if visible.order_changed && !reuse_source_order {
                    state.point_order_slot_copies += state.visible.len();
                }
            }
            state.source = source.tracks;
            state.visible = visible.tracks;
            match (visible.previous_position, visible.position) {
                (Some(previous), Some(position)) => {
                    let first = previous.min(position);
                    let last = previous.max(position);
                    state
                        .rows
                        .retain(|candidate, _| *candidate < first || *candidate > last);
                }
                (Some(previous), None) => {
                    state.rows.retain(|candidate, _| *candidate < previous);
                }
                (None, Some(position)) => {
                    state.rows.retain(|candidate, _| *candidate < position);
                }
                (None, None) => {}
            }
            Some((visible.previous_position, visible.position))
        });
        let Some((previous, position)) = result else {
            return false;
        };
        let _notified_items = match (previous, position) {
            (Some(previous), Some(position)) => {
                let start = previous.min(position);
                let count = previous.max(position) - start + 1;
                self.with_state_mut(|state| {
                    state.point_change = Some(TrackPointChange {
                        id: replacement.id.clone(),
                        previous_position: Some(previous),
                        position: Some(position),
                    });
                });
                self.items_changed(start, count, count);
                count
            }
            (Some(previous), None) => {
                self.with_state_mut(|state| {
                    state.point_change = Some(TrackPointChange {
                        id: replacement.id.clone(),
                        previous_position: Some(previous),
                        position: None,
                    });
                });
                self.items_changed(previous, 1, 0);
                1
            }
            (None, Some(position)) => {
                self.with_state_mut(|state| {
                    state.point_change = Some(TrackPointChange {
                        id: replacement.id.clone(),
                        previous_position: None,
                        position: Some(position),
                    });
                });
                self.items_changed(position, 0, 1);
                1
            }
            (None, None) => 0,
        };
        self.with_state_mut(|state| {
            state.point_change = None;
            #[cfg(test)]
            {
                state.point_notified_items += _notified_items as usize;
            }
        });
        true
    }

    fn with_state<R>(&self, read: impl FnOnce(&TrackModelState) -> R) -> R {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let state = self.imp().state.borrow();
        read(state.as_ref().expect("Tracks model is initialized"))
    }

    fn with_state_mut<R>(&self, write: impl FnOnce(&mut TrackModelState) -> R) -> R {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let mut state = self.imp().state.borrow_mut();
        write(state.as_mut().expect("Tracks model is initialized"))
    }

    #[cfg(test)]
    pub(crate) fn test_stats(&self) -> TrackModelTestStats {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let row_materializations = self.imp().row_materializations.get();
        self.with_state(|state| TrackModelTestStats {
            order_rebuilds: state.order_rebuilds,
            point_updates: state.point_updates,
            point_order_slot_copies: state.point_order_slot_copies,
            point_notified_items: state.point_notified_items,
            row_materializations,
            live_rows: state
                .rows
                .values()
                .filter(|row| row.upgrade().is_some())
                .count(),
        })
    }
}

fn visible_context_id(state: &TrackModelState, context_base: &str) -> String {
    format!(
        "{context_base}|query={}|sort={:?}|descending={}",
        state.query, state.settings.sort_key, state.settings.descending
    )
}

fn position_u32(position: usize) -> u32 {
    u32::try_from(position).expect("prepared Tracks count fits a GTK list position")
}

fn visible_tracks(
    source: &TrackList,
    query: &str,
    settings: &LibraryListSettings,
) -> LibraryQueryResult<TrackList> {
    if source.has_played_at() {
        let query = query.to_lowercase();
        return source.filtered_in_source_order(
            |track| query.is_empty() || track_matches_query(track, &query),
            !settings.descending,
        );
    }
    if settings.sort_key == crate::LibraryField::RowIndex {
        let query = query.to_lowercase();
        return source.filtered_in_source_order(
            |track| query.is_empty() || track_matches_query(track, &query),
            settings.descending,
        );
    }
    let sort = settings.sort_key.track_sort();
    if query.is_empty() {
        source.sorted(sort, settings.descending)
    } else {
        let query = query.to_lowercase();
        source.filtered_sorted(
            |track| track_matches_query(track, &query),
            sort,
            settings.descending,
        )
    }
}

fn history_played_at_text(played_at: i64) -> String {
    glib::DateTime::from_unix_local(played_at)
        .and_then(|date| date.format("%Y-%m-%d %H:%M"))
        .map(String::from)
        .unwrap_or_else(|_| played_at.to_string())
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrackModelTestStats {
    pub(crate) order_rebuilds: usize,
    pub(crate) point_updates: usize,
    pub(crate) point_order_slot_copies: usize,
    pub(crate) point_notified_items: usize,
    pub(crate) row_materializations: usize,
    pub(crate) live_rows: usize,
}
