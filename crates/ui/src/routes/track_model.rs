use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use ::library::{Track, TrackDelta, TrackId};
use gtk::{gio, glib, prelude::*};

use crate::{LibraryField, LibraryListSettings};

use super::models::track_matches_query;

struct TrackLocation {
    source_position: usize,
    display_generation: u64,
    display_position: u32,
    row: Option<glib::WeakRef<glib::BoxedAnyObject>>,
}

struct TrackModelState {
    tracks: Arc<Vec<Track>>,
    order: Vec<usize>,
    locations: HashMap<TrackId, TrackLocation>,
    generation: u64,
    query: String,
    settings: LibraryListSettings,
    #[cfg(test)]
    order_rebuilds: usize,
    #[cfg(test)]
    source_visits: usize,
}

impl TrackModelState {
    fn new(
        tracks: Arc<Vec<Track>>,
        settings: LibraryListSettings,
        initial_tracks_are_sorted: bool,
    ) -> Self {
        let mut state = Self {
            order: Vec::with_capacity(tracks.len()),
            locations: HashMap::with_capacity(tracks.len()),
            tracks,
            generation: 1,
            query: String::new(),
            settings,
            #[cfg(test)]
            order_rebuilds: 0,
            #[cfg(test)]
            source_visits: 0,
        };
        for (source_position, track) in state.tracks.iter().enumerate() {
            state.order.push(source_position);
            #[cfg(test)]
            {
                state.source_visits += 1;
            }
            let replaced = state.locations.insert(
                track.id.clone(),
                TrackLocation {
                    source_position,
                    display_generation: u64::from(initial_tracks_are_sorted),
                    display_position: initial_tracks_are_sorted
                        .then(|| position_u32(source_position))
                        .unwrap_or_default(),
                    row: None,
                },
            );
            assert!(replaced.is_none(), "Tracks model requires unique Track IDs");
        }
        if !initial_tracks_are_sorted {
            let sort = state.settings.sort_key.track_sort();
            let descending = state.settings.descending;
            state.order.sort_by(|left, right| {
                ::library::compare_tracks(
                    &state.tracks[*left],
                    &state.tracks[*right],
                    sort,
                    descending,
                )
            });
            for (display_position, source_position) in state.order.iter().copied().enumerate() {
                let track_id = &state.tracks[source_position].id;
                let location = state
                    .locations
                    .get_mut(track_id)
                    .expect("every displayed Track has a model location");
                location.display_generation = state.generation;
                location.display_position = position_u32(display_position);
            }
        }
        #[cfg(test)]
        {
            state.order_rebuilds = 1;
        }
        state
    }

    fn rebuild_locations(
        &mut self,
        mut previous: HashMap<TrackId, TrackLocation>,
        changed: &HashSet<TrackId>,
    ) {
        let mut locations = HashMap::with_capacity(self.tracks.len());
        for (source_position, track) in self.tracks.iter().enumerate() {
            let row = previous
                .remove(&track.id)
                .filter(|_| !changed.contains(&track.id))
                .and_then(|location| location.row);
            let replaced = locations.insert(
                track.id.clone(),
                TrackLocation {
                    source_position,
                    display_generation: 0,
                    display_position: 0,
                    row,
                },
            );
            assert!(replaced.is_none(), "Tracks model requires unique Track IDs");
        }
        self.locations = locations;
    }

    fn rebuild_order(&mut self) {
        let query = self.query.to_lowercase();
        let mut order = Vec::with_capacity(self.tracks.len());
        for (source_position, track) in self.tracks.iter().enumerate() {
            if query.is_empty() || track_matches_query(track, &query) {
                order.push(source_position);
            }
        }
        #[cfg(test)]
        {
            self.source_visits += self.tracks.len();
        }
        let sort = self.settings.sort_key.track_sort();
        let descending = self.settings.descending;
        order.sort_by(|left, right| {
            ::library::compare_tracks(&self.tracks[*left], &self.tracks[*right], sort, descending)
        });

        self.generation = self.generation.wrapping_add(1);
        for (display_position, source_position) in order.iter().copied().enumerate() {
            let track_id = &self.tracks[source_position].id;
            let location = self
                .locations
                .get_mut(track_id)
                .expect("every displayed Track has a model location");
            location.display_generation = self.generation;
            location.display_position = position_u32(display_position);
        }
        self.order = order;
        #[cfg(test)]
        {
            self.order_rebuilds += 1;
        }
    }

    fn position(&self, track_id: &TrackId) -> Option<u32> {
        self.locations.get(track_id).and_then(|location| {
            (location.display_generation == self.generation).then_some(location.display_position)
        })
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
                .map_or(0, |state| position_u32(state.order.len()))
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let mut state = self.state.borrow_mut();
            let state = state.as_mut()?;
            let TrackModelState {
                tracks,
                order,
                locations,
                ..
            } = state;
            let source_position = *order.get(position as usize)?;
            let track = tracks
                .get(source_position)
                .expect("Tracks display order contains a valid source position");
            let location = locations
                .get_mut(&track.id)
                .expect("displayed Track has a model location");
            if let Some(row) = location.row.as_ref().and_then(glib::WeakRef::upgrade) {
                return Some(row.upcast());
            }
            #[cfg(test)]
            self.row_materializations
                .set(self.row_materializations.get() + 1);
            let row = glib::BoxedAnyObject::new(track.clone());
            location.row = Some(row.downgrade());
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
        tracks: Arc<Vec<Track>>,
        settings: LibraryListSettings,
        initial_tracks_are_sorted: bool,
    ) -> Self {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let model: Self = glib::Object::new();
        model.imp().state.replace(Some(TrackModelState::new(
            tracks,
            settings,
            initial_tracks_are_sorted,
        )));
        model
    }

    pub(crate) fn source_is_empty(&self) -> bool {
        self.with_state(|state| state.tracks.is_empty())
    }

    pub(crate) fn visible_count(&self) -> usize {
        self.with_state(|state| state.order.len())
    }

    pub(crate) fn query(&self) -> String {
        self.with_state(|state| state.query.clone())
    }

    pub(crate) fn settings(&self) -> LibraryListSettings {
        self.with_state(|state| state.settings.clone())
    }

    pub(crate) fn position(&self, track_id: &TrackId) -> Option<u32> {
        self.with_state(|state| state.position(track_id))
    }

    pub(crate) fn track_at(&self, position: u32) -> Option<Track> {
        self.with_track(position, Track::clone)
    }

    pub(crate) fn with_track<R>(&self, position: u32, read: impl FnOnce(&Track) -> R) -> Option<R> {
        self.with_state(|state| {
            let source_position = *state.order.get(position as usize)?;
            state.tracks.get(source_position).map(read)
        })
    }

    pub(crate) fn set_query(&self, query: &str) -> bool {
        let query = query.trim();
        let (old_len, new_len, query_changed, order_changed) = self.with_state_mut(|state| {
            if state.query == query {
                return (state.order.len(), state.order.len(), false, false);
            }
            let old_order = state.order.clone();
            state.query = query.to_string();
            state.rebuild_order();
            (
                old_order.len(),
                state.order.len(),
                true,
                old_order != state.order,
            )
        });
        if order_changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        query_changed
    }

    pub(crate) fn apply_settings(&self, settings: LibraryListSettings) -> bool {
        let (old_len, new_len, changed) = self.with_state_mut(|state| {
            let order_changed = state.settings.sort_key != settings.sort_key
                || state.settings.descending != settings.descending;
            state.settings = settings;
            if !order_changed {
                return (state.order.len(), state.order.len(), false);
            }
            let old_order = state.order.clone();
            state.rebuild_order();
            (old_order.len(), state.order.len(), old_order != state.order)
        });
        if changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        changed
    }

    pub(crate) fn replace_tracks(&self, tracks: Arc<Vec<Track>>) -> Arc<Vec<Track>> {
        let (previous, old_len, new_len, changed) = self.with_state_mut(|state| {
            let previous = std::mem::replace(&mut state.tracks, tracks);
            if state.tracks.as_ref() == previous.as_ref() {
                return (previous, state.order.len(), state.order.len(), false);
            }
            let old_len = state.order.len();
            let previous_locations = std::mem::take(&mut state.locations);
            let changed_ids = changed_track_values(previous.as_ref(), state.tracks.as_ref());
            state.rebuild_locations(previous_locations, &changed_ids);
            state.rebuild_order();
            (previous, old_len, state.order.len(), true)
        });
        if changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        previous
    }

    pub(crate) fn patch(&self, changed_tracks: Vec<Track>, delta: &TrackDelta) {
        if track_delta_can_preserve_order(delta, self.settings().sort_key)
            && self.patch_in_place(&changed_tracks)
        {
            return;
        }
        let old_len = self.visible_count();
        self.with_state_mut(|state| {
            let deleted = delta.deleted.iter().cloned().collect::<HashSet<_>>();
            let mut changed = changed_tracks
                .into_iter()
                .map(|track| (track.id.clone(), track))
                .collect::<HashMap<_, _>>();
            let changed_ids = changed.keys().cloned().collect::<HashSet<_>>();
            let tracks = Arc::make_mut(&mut state.tracks);
            tracks.retain_mut(|track| {
                if deleted.contains(&track.id) {
                    return false;
                }
                if let Some(replacement) = changed.remove(&track.id) {
                    *track = replacement;
                }
                true
            });
            tracks.extend(changed.into_values());
            let previous_locations = std::mem::take(&mut state.locations);
            state.rebuild_locations(previous_locations, &changed_ids);
            state.rebuild_order();
        });
        self.items_changed(0, position_u32(old_len), position_u32(self.visible_count()));
    }

    fn patch_in_place(&self, changed_tracks: &[Track]) -> bool {
        let all_present = self.with_state(|state| {
            changed_tracks
                .iter()
                .all(|track| state.locations.contains_key(&track.id))
        });
        if !all_present {
            return false;
        }
        let mut positions = self.with_state_mut(|state| {
            let generation = state.generation;
            let TrackModelState {
                tracks, locations, ..
            } = state;
            let tracks = Arc::make_mut(tracks);
            let mut positions = Vec::with_capacity(changed_tracks.len());
            for track in changed_tracks {
                let location = locations
                    .get_mut(&track.id)
                    .expect("checked existing Track location");
                tracks[location.source_position] = track.clone();
                location.row = None;
                if location.display_generation == generation {
                    positions.push(location.display_position);
                }
            }
            positions
        });
        positions.sort_unstable();
        positions.dedup();
        if let (Some(first), Some(last)) = (positions.first(), positions.last()) {
            let count = last - first + 1;
            self.items_changed(*first, count, count);
        }
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
    pub(crate) fn source_is(&self, tracks: &Arc<Vec<Track>>) -> bool {
        self.with_state(|state| Arc::ptr_eq(&state.tracks, tracks))
    }

    #[cfg(test)]
    pub(crate) fn test_stats(&self) -> TrackModelTestStats {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let row_materializations = self.imp().row_materializations.get();
        self.with_state(|state| TrackModelTestStats {
            order_rebuilds: state.order_rebuilds,
            source_visits: state.source_visits,
            row_materializations,
            live_rows: state
                .locations
                .values()
                .filter(|location| {
                    location
                        .row
                        .as_ref()
                        .and_then(glib::WeakRef::upgrade)
                        .is_some()
                })
                .count(),
        })
    }
}

fn changed_track_values(previous: &[Track], current: &[Track]) -> HashSet<TrackId> {
    let previous = previous
        .iter()
        .map(|track| (&track.id, track))
        .collect::<HashMap<_, _>>();
    current
        .iter()
        .filter(|track| previous.get(&track.id).is_none_or(|old| *old != *track))
        .map(|track| track.id.clone())
        .collect()
}

fn track_delta_can_preserve_order(delta: &TrackDelta, sort_key: LibraryField) -> bool {
    if !delta.added.is_empty()
        || !delta.deleted.is_empty()
        || !delta.fields.is_empty()
        || !delta.metadata.is_empty()
    {
        return false;
    }
    if !delta.stats.is_empty()
        && matches!(
            sort_key,
            LibraryField::LastPlayed | LibraryField::PlayCount | LibraryField::UserRating
        )
    {
        return false;
    }
    delta.favorite.is_empty() || sort_key != LibraryField::Favorite
}

fn position_u32(position: usize) -> u32 {
    u32::try_from(position).expect("prepared Tracks count fits a GTK list position")
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrackModelTestStats {
    pub(crate) order_rebuilds: usize,
    pub(crate) source_visits: usize,
    pub(crate) row_materializations: usize,
    pub(crate) live_rows: usize,
}
