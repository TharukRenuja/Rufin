use std::{cell::RefCell, cmp::Ordering, collections::HashMap};

use gtk::{gio, glib, prelude::*};
use library::{PlaylistEntryItem, PlaylistEntryList, PlaylistId, SourceId, Track, TrackId};
use playback::{LoadedPlayRequest, QueuePlacement, SourceSessionEpoch};

use crate::{LibraryField, LibraryListSettings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaylistEntrySort {
    Order,
    Title,
    Artist,
    Album,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlaylistEntryProjectionRequest {
    query: String,
    sort: PlaylistEntrySort,
    descending: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedPlaylistEntries {
    entries: PlaylistEntryList,
    visible: Vec<u32>,
    request: PlaylistEntryProjectionRequest,
}

impl PreparedPlaylistEntries {
    pub(crate) fn entries_is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl PlaylistEntrySort {
    fn for_field(field: LibraryField) -> Self {
        match field {
            LibraryField::Title => Self::Title,
            LibraryField::Artist => Self::Artist,
            LibraryField::Album => Self::Album,
            _ => Self::Order,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryRow {
    pub(crate) source_index: usize,
    pub(crate) display_index: usize,
}

struct PlaylistEntryModelState {
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    playlist_id: PlaylistId,
    entries: PlaylistEntryList,
    visible: Vec<u32>,
    rows: HashMap<u32, glib::WeakRef<glib::BoxedAnyObject>>,
    query: String,
    sort: PlaylistEntrySort,
    descending: bool,
}

impl PlaylistEntryModelState {
    #[cfg(test)]
    fn new(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        playlist_id: PlaylistId,
        entries: PlaylistEntryList,
        settings: &LibraryListSettings,
    ) -> Self {
        let mut state = Self {
            source_id,
            source_session_epoch,
            playlist_id,
            entries,
            visible: Vec::new(),
            rows: HashMap::new(),
            query: String::new(),
            sort: PlaylistEntrySort::for_field(settings.sort_key),
            descending: settings.descending,
        };
        state.rebuild_visible();
        state
    }

    fn new_prepared(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        playlist_id: PlaylistId,
        entries: PlaylistEntryList,
        visible: Vec<u32>,
        settings: &LibraryListSettings,
    ) -> Self {
        Self {
            source_id,
            source_session_epoch,
            playlist_id,
            entries,
            visible,
            rows: HashMap::new(),
            query: String::new(),
            sort: PlaylistEntrySort::for_field(settings.sort_key),
            descending: settings.descending,
        }
    }

    fn rebuild_visible(&mut self) {
        let query = self.query.to_lowercase();
        let sort = self.sort;
        self.visible = self
            .entries
            .positions_by(
                |track| query.is_empty() || track_matches_query(track, &query),
                |left, right| compare_tracks(left, right, sort),
            )
            .expect("a mounted Playlist keeps its loaded Library available");
        if self.descending {
            self.visible.reverse();
        }
        self.rows.clear();
    }

    fn visible_context_id(&self) -> String {
        format!(
            "playlist:{}:{:?}:{}:{}",
            self.playlist_id.as_str(),
            self.sort,
            self.descending,
            self.query
        )
    }

    fn source_context_id(&self) -> String {
        format!("playlist:{}", self.playlist_id.as_str())
    }

    fn entry_at_source(&self, source_index: usize) -> Option<PlaylistEntryItem> {
        self.entries.entry(source_index).ok().flatten()
    }

    fn entry_at_visible(&self, position: usize) -> Option<PlaylistEntryItem> {
        let source_index = *self.visible.get(position)? as usize;
        self.entry_at_source(source_index)
    }
}

mod imp {
    use gio::subclass::prelude::*;

    use super::*;

    #[derive(Default)]
    pub(crate) struct PlaylistEntryModel {
        pub(super) state: RefCell<Option<PlaylistEntryModelState>>,
        #[cfg(test)]
        pub(super) row_materializations: std::cell::Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistEntryModel {
        const NAME: &'static str = "RufinPlaylistEntryModel";
        type Type = super::PlaylistEntryModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for PlaylistEntryModel {}

    impl ListModelImpl for PlaylistEntryModel {
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
            let source_index = *state.visible.get(position as usize)? as usize;
            #[cfg(test)]
            self.row_materializations
                .set(self.row_materializations.get() + 1);
            let row = glib::BoxedAnyObject::new(PlaylistEntryRow {
                source_index,
                display_index: position as usize,
            });
            state.rows.insert(position, row.downgrade());
            Some(row.upcast())
        }
    }
}

glib::wrapper! {
    pub(crate) struct PlaylistEntryModel(
        ObjectSubclass<imp::PlaylistEntryModel>
    ) @implements gio::ListModel;
}

impl PlaylistEntryModel {
    #[cfg(test)]
    pub(crate) fn new(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        playlist_id: PlaylistId,
        entries: PlaylistEntryList,
        settings: &LibraryListSettings,
    ) -> Self {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let model: Self = glib::Object::new();
        model.imp().state.replace(Some(PlaylistEntryModelState::new(
            source_id,
            source_session_epoch,
            playlist_id,
            entries,
            settings,
        )));
        model
    }

    pub(crate) fn new_prepared(
        source_id: SourceId,
        source_session_epoch: SourceSessionEpoch,
        playlist_id: PlaylistId,
        entries: PlaylistEntryList,
        visible: Vec<u32>,
        settings: &LibraryListSettings,
    ) -> Self {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let model: Self = glib::Object::new();
        model
            .imp()
            .state
            .replace(Some(PlaylistEntryModelState::new_prepared(
                source_id,
                source_session_epoch,
                playlist_id,
                entries,
                visible,
                settings,
            )));
        model
    }

    pub(crate) fn source_is_empty(&self) -> bool {
        self.with_state(|state| state.entries.is_empty())
    }

    pub(crate) fn projection_request(&self) -> PlaylistEntryProjectionRequest {
        self.with_state(|state| PlaylistEntryProjectionRequest {
            query: state.query.clone(),
            sort: state.sort,
            descending: state.descending,
        })
    }

    pub(crate) fn replace_prepared(&self, prepared: PreparedPlaylistEntries) -> bool {
        let result = self.with_state_mut(|state| {
            let current = PlaylistEntryProjectionRequest {
                query: state.query.clone(),
                sort: state.sort,
                descending: state.descending,
            };
            if current != prepared.request {
                return None;
            }
            let old_len = state.visible.len();
            state.entries = prepared.entries;
            state.visible = prepared.visible;
            state.rows.clear();
            Some((old_len, state.visible.len()))
        });
        let Some((old_len, new_len)) = result else {
            return false;
        };
        self.items_changed(0, position_u32(old_len), position_u32(new_len));
        true
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

    pub(crate) fn apply_settings(&self, settings: &LibraryListSettings) -> bool {
        let sort = PlaylistEntrySort::for_field(settings.sort_key);
        let (old_len, new_len, changed) = self.with_state_mut(|state| {
            if state.sort == sort && state.descending == settings.descending {
                return (state.visible.len(), state.visible.len(), false);
            }
            let old_len = state.visible.len();
            state.sort = sort;
            state.descending = settings.descending;
            state.rebuild_visible();
            (old_len, state.visible.len(), true)
        });
        if changed {
            self.items_changed(0, position_u32(old_len), position_u32(new_len));
        }
        changed
    }

    pub(crate) fn entry_for_row(&self, row: &PlaylistEntryRow) -> Option<PlaylistEntryItem> {
        self.with_state(|state| state.entry_at_source(row.source_index))
    }

    pub(crate) fn source_context_id(&self) -> String {
        self.with_state(PlaylistEntryModelState::source_context_id)
    }

    pub(crate) fn visible_context_id(&self) -> String {
        self.with_state(PlaylistEntryModelState::visible_context_id)
    }

    pub(crate) fn source_play_request(
        &self,
        placement: QueuePlacement,
        shuffled_start: bool,
    ) -> Option<LoadedPlayRequest> {
        self.with_state(|state| {
            LoadedPlayRequest::context(
                state.source_id.clone(),
                state.source_session_epoch,
                state.entries.track_list(),
                0,
                placement,
                state.source_context_id(),
                shuffled_start,
            )
        })
    }

    pub(crate) fn visible_play_request(&self, position: usize) -> Option<LoadedPlayRequest> {
        self.with_state(|state| {
            state.visible.get(position)?;
            LoadedPlayRequest::context(
                state.source_id.clone(),
                state.source_session_epoch,
                state.entries.selected_track_list(&state.visible),
                position,
                QueuePlacement::Now,
                state.visible_context_id(),
                false,
            )
        })
    }

    pub(crate) fn occurrence_for_current(
        &self,
        track_id: &TrackId,
        source_rank: Option<usize>,
        source_order: bool,
    ) -> Option<String> {
        self.with_state(|state| {
            let entry = match source_rank {
                Some(source_rank) if source_order => state.entry_at_source(source_rank),
                Some(source_rank) => state.entry_at_visible(source_rank),
                None => {
                    return state
                        .entries
                        .occurrence_for_track(&state.visible, track_id)
                        .ok()
                        .flatten();
                }
            }?;
            (entry.track.id == *track_id).then_some(entry.occurrence_id)
        })
    }

    pub(crate) fn visible_position(&self, occurrence_id: &str) -> Option<u32> {
        self.with_state(|state| {
            state
                .visible
                .iter()
                .enumerate()
                .find_map(|(position, source)| {
                    state
                        .entries
                        .occurrence_id(*source as usize)
                        .filter(|candidate| *candidate == occurrence_id)
                        .map(|_| position_u32(position))
                })
        })
    }

    pub(crate) fn drop_index(
        &self,
        dragged_occurrence_id: &str,
        target_source_index: usize,
        after: bool,
    ) -> Option<usize> {
        self.with_state(|state| {
            let source_index = state
                .entries
                .position_of_occurrence(dragged_occurrence_id)?;
            let mut new_index = if after {
                target_source_index.saturating_add(1)
            } else {
                target_source_index
            };
            if source_index < new_index {
                new_index = new_index.saturating_sub(1);
            }
            (source_index != new_index).then_some(new_index)
        })
    }

    fn with_state<R>(&self, read: impl FnOnce(&PlaylistEntryModelState) -> R) -> R {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let state = self.imp().state.borrow();
        read(state.as_ref().expect("Playlist entry model is initialized"))
    }

    fn with_state_mut<R>(&self, write: impl FnOnce(&mut PlaylistEntryModelState) -> R) -> R {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        let mut state = self.imp().state.borrow_mut();
        write(state.as_mut().expect("Playlist entry model is initialized"))
    }

    #[cfg(test)]
    pub(crate) fn retained_state(&self) -> (usize, usize, usize) {
        use glib::subclass::prelude::ObjectSubclassIsExt;

        self.with_state(|state| {
            (
                state.entries.len(),
                state.visible.len(),
                self.imp().row_materializations.get(),
            )
        })
    }
}

pub(crate) fn prepare_playlist_entry_positions(
    entries: &PlaylistEntryList,
    settings: &LibraryListSettings,
) -> Result<Vec<u32>, String> {
    let sort = PlaylistEntrySort::for_field(settings.sort_key);
    let mut positions = entries
        .positions_by(|_| true, |left, right| compare_tracks(left, right, sort))
        .map_err(|error| error.to_string())?;
    if settings.descending {
        positions.reverse();
    }
    Ok(positions)
}

pub(crate) fn prepare_playlist_entry_projection(
    entries: PlaylistEntryList,
    request: PlaylistEntryProjectionRequest,
) -> Result<PreparedPlaylistEntries, String> {
    let query = request.query.trim().to_lowercase();
    let mut visible = entries
        .positions_by(
            |track| query.is_empty() || track_matches_query(track, &query),
            |left, right| compare_tracks(left, right, request.sort),
        )
        .map_err(|error| error.to_string())?;
    if request.descending {
        visible.reverse();
    }
    Ok(PreparedPlaylistEntries {
        entries,
        visible,
        request,
    })
}

fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
}

fn compare_tracks(left: &Track, right: &Track, sort: PlaylistEntrySort) -> Ordering {
    match sort {
        PlaylistEntrySort::Order => Ordering::Equal,
        PlaylistEntrySort::Title => cmp_text(&left.title, &right.title),
        PlaylistEntrySort::Artist => cmp_text(&left.artist, &right.artist),
        PlaylistEntrySort::Album => cmp_text(&left.album, &right.album),
    }
}

fn cmp_text(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

fn position_u32(position: usize) -> u32 {
    u32::try_from(position).expect("prepared Playlist count fits a GTK list position")
}

#[cfg(test)]
mod tests {
    use gtk::prelude::ListModelExt;

    use super::*;
    use crate::{LibraryListKey, test_support};

    #[test]
    fn playlist_model_filters_sorts_plays_and_materializes_rows_lazily() {
        let mut alpha = test_support::track(1, "Alpha");
        alpha.album = "Plain Album".to_string();
        let mut beta = test_support::track(2, "Beta");
        beta.album = "Needle Album".to_string();
        let source_id = SourceId::fake(1);
        let playlist = test_support::playlist(1, "List");
        let playlist_id = playlist.id.clone();
        let loaded = test_support::loaded_source(
            source_id.clone(),
            Vec::new(),
            vec![alpha.clone(), beta.clone()],
            vec![test_support::playlist_snapshot(
                playlist,
                [
                    ("entry-alpha", alpha.id.clone()),
                    ("entry-beta", beta.id.clone()),
                ],
            )],
        );
        let detail = loaded
            .playlist_detail(&playlist_id)
            .expect("read Playlist")
            .expect("Playlist");
        let mut settings = LibraryListSettings::for_key(LibraryListKey::PlaylistTracks);
        let model = PlaylistEntryModel::new(
            source_id,
            SourceSessionEpoch::new(1),
            playlist_id,
            detail.entries,
            &settings,
        );

        assert_eq!(model.retained_state(), (2, 2, 0));
        let first = model.item(0).expect("first row");
        assert_eq!(model.item(0).as_ref(), Some(&first));
        assert_eq!(model.retained_state(), (2, 2, 1));

        model.set_query(" needle ");
        assert_eq!(model.retained_state(), (2, 1, 1));
        let filtered = model
            .visible_play_request(0)
            .expect("filtered Playlist playback");
        assert!(matches!(
            &filtered.tracks,
            playback::LoadedTrackSelection::Shallow(_)
        ));
        let filtered_tracks = filtered
            .tracks
            .materialize()
            .expect("filtered Playlist order");
        assert_eq!(filtered_tracks.len(), 1);
        assert_eq!(filtered_tracks[0].id, beta.id);

        model.set_query("");
        settings.sort_key = LibraryField::Album;
        settings.descending = true;
        model.apply_settings(&settings);
        let visible = model
            .visible_play_request(0)
            .expect("sorted Playlist playback");
        let visible_tracks = visible
            .tracks
            .materialize()
            .expect("visible Playlist order");
        assert_eq!(
            visible_tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>(),
            vec![alpha.id.clone(), beta.id.clone()]
        );
        let source = model
            .source_play_request(QueuePlacement::Next, false)
            .expect("canonical Playlist playback");
        assert_eq!(source.placement, QueuePlacement::Next);
        let source_tracks = source
            .tracks
            .materialize()
            .expect("canonical Playlist order");
        assert_eq!(
            source_tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>(),
            vec![alpha.id.clone(), beta.id.clone()]
        );
    }

    #[test]
    fn playlist_model_keeps_duplicate_occurrences_and_exact_drag_positions() {
        let repeated = test_support::track(1, "Repeated");
        let other = test_support::track(2, "Other");
        let source_id = SourceId::fake(2);
        let playlist = test_support::playlist(2, "Duplicates");
        let playlist_id = playlist.id.clone();
        let loaded = test_support::loaded_source(
            source_id.clone(),
            Vec::new(),
            vec![repeated.clone(), other.clone()],
            vec![test_support::playlist_snapshot(
                playlist,
                [
                    ("first", repeated.id.clone()),
                    ("second", repeated.id.clone()),
                    ("third", other.id.clone()),
                ],
            )],
        );
        let entries = loaded
            .playlist_detail(&playlist_id)
            .expect("read Playlist")
            .expect("Playlist")
            .entries;
        let model = PlaylistEntryModel::new(
            source_id,
            SourceSessionEpoch::new(1),
            playlist_id,
            entries,
            &LibraryListSettings::for_key(LibraryListKey::PlaylistTracks),
        );

        assert_eq!(
            model.occurrence_for_current(&repeated.id, Some(1), true),
            Some("second".to_string())
        );
        assert_eq!(
            model.occurrence_for_current(&repeated.id, Some(1), false),
            Some("second".to_string())
        );
        assert_eq!(
            model.occurrence_for_current(&repeated.id, None, false),
            Some("first".to_string())
        );
        assert_eq!(model.visible_position("second"), Some(1));
        assert_eq!(model.drop_index("first", 2, false), Some(1));
        assert_eq!(model.drop_index("first", 2, true), Some(2));
        assert_eq!(model.drop_index("third", 0, false), Some(0));
        assert_eq!(model.drop_index("second", 1, false), None);
        assert_eq!(model.retained_state(), (3, 3, 0));
    }

    #[test]
    fn prepared_playlist_update_keeps_occurrences_and_rejects_a_stale_request() {
        let alpha = test_support::track(1, "Alpha");
        let beta = test_support::track(2, "Beta");
        let source_id = SourceId::fake(3);
        let original = test_support::playlist(3, "Original");
        let original_id = original.id.clone();
        let replacement = test_support::playlist(4, "Replacement");
        let replacement_id = replacement.id.clone();
        let loaded = test_support::loaded_source(
            source_id.clone(),
            Vec::new(),
            vec![alpha.clone(), beta.clone()],
            vec![
                test_support::playlist_snapshot(
                    original,
                    [
                        ("original-alpha", alpha.id.clone()),
                        ("original-beta", beta.id.clone()),
                    ],
                ),
                test_support::playlist_snapshot(
                    replacement,
                    [
                        ("replacement-beta", beta.id.clone()),
                        ("replacement-alpha-first", alpha.id.clone()),
                        ("replacement-alpha-second", alpha.id.clone()),
                    ],
                ),
            ],
        );
        let original_entries = loaded
            .playlist_detail(&original_id)
            .expect("read original Playlist")
            .expect("original Playlist")
            .entries;
        let replacement_entries = loaded
            .playlist_detail(&replacement_id)
            .expect("read replacement Playlist")
            .expect("replacement Playlist")
            .entries;
        let model = PlaylistEntryModel::new(
            source_id,
            SourceSessionEpoch::new(1),
            original_id,
            original_entries,
            &LibraryListSettings::for_key(LibraryListKey::PlaylistTracks),
        );

        model.set_query("alpha");
        let prepared = prepare_playlist_entry_projection(
            replacement_entries.clone(),
            model.projection_request(),
        )
        .expect("prepare replacement");
        assert!(model.replace_prepared(prepared));
        assert_eq!(model.retained_state(), (3, 2, 0));
        assert_eq!(
            model.occurrence_for_current(&alpha.id, Some(2), true),
            Some("replacement-alpha-second".to_string())
        );

        let stale =
            prepare_playlist_entry_projection(replacement_entries, model.projection_request())
                .expect("prepare stale replacement");
        model.set_query("beta");
        assert!(!model.replace_prepared(stale));
        assert_eq!(model.retained_state(), (3, 1, 0));
    }
}
