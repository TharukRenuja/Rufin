use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use ::library::{
    Playlist, PlaylistEntry, PlaylistId, SourceId, Track, TrackId,
    play_context::{
        PlayContext, PlayContextDescriptor, PlayContextOrder, PlaylistSort, context_id,
    },
};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::PlaylistEntryPlayRequest;
use sources::SourcePlaylistOperation;

use super::collection_context::install_dynamic_playlist_entry_context_menu;
use crate::favorites::{
    favorite_button_is_active, favorite_icon_button, set_favorite_button_active, track_favorite_key,
};
use crate::interactions::{add_dynamic_link_hover, add_label_click};
use crate::localization::{bind_widget_tooltip, localized_column};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{ArtworkTile, GRID_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::route::RouteCurrentTrack;
use localization::{msgid, tr};

use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};

use super::collections::{
    CollectionTableProjection, LibraryCollectionProjection, LibraryPresentationProjection,
    dynamic_collection_table, track_grid_field_route,
};
use super::columns::{track_column_fit_width, track_column_width};
use super::detail_links::track_artist_route;
use super::grid_cells::{
    CollectionGridCardCell, CollectionGridProjection, ReusableCollectionGridCell, collection_grid,
};
use super::library_fields::{item_at, item_at_from_item, play_count_column_width, track_field};
use super::route::Route;
use super::table_links::list_item_storage_key;
use super::table_sizing::route_column_view_initial_width_with_inset;

const PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH: i32 = 30;
const PLAYLIST_ENTRY_NUMBER_WIDTH: i32 = 24;
const PLAYLIST_ENTRY_COVER_WIDTH: i32 = 36;
const PLAYLIST_ENTRY_COLUMN_GAP: i32 = 8;
const PLAYLIST_ENTRY_TITLE_MAX_CHARS: i32 = 44;
const PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH: i32 = 320;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 220;
pub(crate) type PlaylistEntrySelectionHandle = Rc<RefCell<Option<Rc<dyn Fn(&str)>>>>;

pub(crate) fn playlist_operation_supported(
    shell: &Shell,
    playlist: &Playlist,
    operation: SourcePlaylistOperation,
) -> bool {
    playlist.owner.is_some_and(|owner| {
        shell
            .products
            .library
            .playlist_operation_supported(owner, operation)
    })
}

#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryContextMenuAction {
    pub(crate) playlist_id: PlaylistId,
    pub(crate) entry_id: String,
    pub(crate) title: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryContextMenuState {
    pub(crate) track: Track,
    pub(crate) entry_id: String,
    pub(crate) remove_action: Option<PlaylistEntryContextMenuAction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaylistEntrySort {
    Order,
    Title,
    Artist,
    Album,
}

#[derive(Clone)]
pub(crate) struct PlaylistEntryTableSelection {
    entries: Weak<RefCell<Vec<PlaylistEntry>>>,
    model: glib::WeakRef<gio::ListStore>,
    selection: glib::WeakRef<gtk::SingleSelection>,
    index: Rc<RefCell<PlaylistEntrySelectionIndex>>,
    current: Rc<RefCell<Option<PlaylistEntryCurrentSelection>>>,
    selected_entry_id: Rc<RefCell<Option<String>>>,
}

#[derive(Clone, Debug)]
struct PlaylistEntryCurrentSelection {
    track_id: TrackId,
    source_rank: Option<usize>,
}

#[derive(Clone, Debug)]
struct PlaylistEntrySelectionItem {
    entry_id: String,
    track_id: TrackId,
}

#[derive(Default)]
struct PlaylistEntrySelectionIndex {
    position_by_entry_id: HashMap<String, u32>,
    first_entry_by_track: HashMap<TrackId, String>,
    entry_by_source_rank: HashMap<usize, PlaylistEntrySelectionItem>,
}

impl PlaylistEntrySelectionIndex {
    fn rebuild(&mut self, entries: &[PlaylistEntry], model: &gio::ListStore) {
        self.position_by_entry_id.clear();
        self.first_entry_by_track.clear();
        self.entry_by_source_rank.clear();
        for position in 0..model.n_items() {
            let Some(row) = item_at::<PlaylistEntryTableRow>(model, position) else {
                continue;
            };
            let Some(entry) = entries.get(row.original_index) else {
                continue;
            };
            self.position_by_entry_id
                .insert(entry.entry_id.clone(), position);
            self.first_entry_by_track
                .entry(entry.track.id.clone())
                .or_insert_with(|| entry.entry_id.clone());
            self.entry_by_source_rank.insert(
                position as usize,
                PlaylistEntrySelectionItem {
                    entry_id: entry.entry_id.clone(),
                    track_id: entry.track.id.clone(),
                },
            );
        }
    }

    fn entry_id_for_current(&self, current: &PlaylistEntryCurrentSelection) -> Option<&str> {
        if let Some(source_rank) = current.source_rank
            && let Some(item) = self.entry_by_source_rank.get(&source_rank)
            && item.track_id == current.track_id
        {
            return Some(&item.entry_id);
        }
        self.first_entry_by_track
            .get(&current.track_id)
            .map(String::as_str)
    }
}

fn playlist_entry_current_selection(
    current: Option<&RouteCurrentTrack>,
    source_id: Option<&SourceId>,
    expected_context_id: &str,
) -> Option<PlaylistEntryCurrentSelection> {
    current
        .zip(source_id)
        .filter(|(current, source_id)| &current.source_id == *source_id)
        .map(|(current, _)| PlaylistEntryCurrentSelection {
            track_id: current.track_id.clone(),
            source_rank: current
                .context
                .as_ref()
                .filter(|context| context.context_id == expected_context_id)
                .map(|context| context.source_rank),
        })
}

impl PlaylistEntryTableSelection {
    fn new(
        entries: Rc<RefCell<Vec<PlaylistEntry>>>,
        model: &gio::ListStore,
        selection: &gtk::SingleSelection,
        selected_entry_id: Rc<RefCell<Option<String>>>,
    ) -> Self {
        Self {
            entries: Rc::downgrade(&entries),
            model: model.downgrade(),
            selection: selection.downgrade(),
            index: Rc::new(RefCell::new(PlaylistEntrySelectionIndex::default())),
            current: Rc::new(RefCell::new(None)),
            selected_entry_id,
        }
    }

    fn select_entry_id(&self, entry_id: &str) {
        *self.selected_entry_id.borrow_mut() = Some(entry_id.to_string());
        self.sync();
    }

    fn clear(&self) {
        self.selected_entry_id.borrow_mut().take();
        self.sync();
    }

    pub(crate) fn select_now_playing_track(
        &self,
        current: Option<&RouteCurrentTrack>,
        source_id: Option<&SourceId>,
        expected_context_id: &str,
    ) {
        let current = playlist_entry_current_selection(current, source_id, expected_context_id);
        *self.current.borrow_mut() = current;
        let entry_id = self.current.borrow().as_ref().and_then(|current| {
            self.index
                .borrow()
                .entry_id_for_current(current)
                .map(str::to_string)
        });
        if let Some(entry_id) = entry_id {
            self.select_entry_id(&entry_id);
        } else {
            self.clear();
        }
    }

    pub(crate) fn is_bound(&self) -> bool {
        self.entries.upgrade().is_some()
            && self.model.upgrade().is_some()
            && self.selection.upgrade().is_some()
    }

    fn sync(&self) {
        let Some(selection) = self.selection.upgrade() else {
            return;
        };
        sync_playlist_entry_selection(&selection, &self.index.borrow(), &self.selected_entry_id);
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
fn source_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| query.to_string())
}
fn playlist_entry_sort_descriptor(sort: PlaylistEntrySort) -> PlaylistSort {
    match sort {
        PlaylistEntrySort::Order => PlaylistSort::Position,
        PlaylistEntrySort::Title => PlaylistSort::Title,
        PlaylistEntrySort::Artist => PlaylistSort::Artist,
        PlaylistEntrySort::Album => PlaylistSort::Album,
    }
}

fn playlist_entry_context_id(playlist_id: &PlaylistId, state: &PlaylistEntryListState) -> String {
    context_id(&PlayContext {
        descriptor: PlayContextDescriptor::Playlist {
            playlist_id: playlist_id.clone(),
        },
        order: PlayContextOrder::Playlist {
            query: source_query(&state.query),
            sort: playlist_entry_sort_descriptor(state.sort),
            descending: state.descending,
        },
    })
}
#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryListState {
    pub(crate) query: String,
    pub(crate) sort: PlaylistEntrySort,
    pub(crate) descending: bool,
}
impl PlaylistEntryListState {
    pub(crate) fn for_settings(settings: &LibraryListSettings) -> Self {
        Self {
            query: String::new(),
            sort: PlaylistEntrySort::for_field(settings.sort_key),
            descending: settings.descending,
        }
    }

    pub(crate) fn apply_settings(&mut self, settings: &LibraryListSettings) {
        self.sort = PlaylistEntrySort::for_field(settings.sort_key);
        self.descending = settings.descending;
    }
}
#[derive(Clone)]
pub(crate) struct PlaylistEntryTableRow {
    pub(crate) original_index: usize,
    pub(crate) display_index: usize,
}
fn playlist_entry_selection_position(
    index: &PlaylistEntrySelectionIndex,
    selected_entry_id: &RefCell<Option<String>>,
) -> u32 {
    let selected_entry_id = selected_entry_id.borrow();
    let Some(entry_id) = selected_entry_id.as_ref() else {
        return gtk::INVALID_LIST_POSITION;
    };
    index
        .position_by_entry_id
        .get(entry_id)
        .copied()
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

fn sync_playlist_entry_selection(
    selection: &gtk::SingleSelection,
    index: &PlaylistEntrySelectionIndex,
    selected_entry_id: &RefCell<Option<String>>,
) {
    let selected = playlist_entry_selection_position(index, selected_entry_id);
    if selection.selected() != selected {
        selection.set_selected(selected);
    }
}

fn connect_playlist_entry_model_selection_sync(
    model: &gio::ListStore,
    selection: &gtk::SingleSelection,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    index: Rc<RefCell<PlaylistEntrySelectionIndex>>,
    current: Rc<RefCell<Option<PlaylistEntryCurrentSelection>>>,
    selected_entry_id: Rc<RefCell<Option<String>>>,
) {
    let selection = selection.downgrade();
    model.connect_items_changed(move |model, _, _, _| {
        index.borrow_mut().rebuild(&entries.borrow(), model);
        if let Some(current) = current.borrow().as_ref() {
            *selected_entry_id.borrow_mut() = index
                .borrow()
                .entry_id_for_current(current)
                .map(str::to_string);
        }
        let Some(selection) = selection.upgrade() else {
            return;
        };
        sync_playlist_entry_selection(&selection, &index.borrow(), &selected_entry_id);
    });
}

#[derive(Clone)]
struct PlaylistEntryCellState {
    menu: Rc<RefCell<Option<PlaylistEntryContextMenuState>>>,
    row: Rc<Cell<Option<usize>>>,
    link_route: Rc<RefCell<Option<Route>>>,
}
#[derive(Clone)]
struct PlaylistEntryTitleCell {
    cover: ArtworkTile,
    title: gtk::Label,
    artist: gtk::Label,
}
thread_local! {
    static PLAYLIST_ENTRY_CELL_STATES: RefCell<HashMap<usize, PlaylistEntryCellState>> = RefCell::new(HashMap::new());
    static PLAYLIST_ENTRY_TITLE_CELLS: RefCell<HashMap<usize, PlaylistEntryTitleCell>> = RefCell::new(HashMap::new());
    static PLAYLIST_ENTRY_IMAGE_CELLS: RefCell<HashMap<usize, ArtworkTile>> = RefCell::new(HashMap::new());
}

pub(crate) fn playlist_entries_collection_projection(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    state: Rc<RefCell<PlaylistEntryListState>>,
    playlist_id: PlaylistId,
    content_inset: i32,
    selection_handle: Option<PlaylistEntrySelectionHandle>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> (LibraryCollectionProjection, gio::ListStore) {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    let selected_entry_id = Rc::new(RefCell::new(None::<String>));
    let playlist_selection = PlaylistEntryTableSelection::new(
        Rc::clone(&entries),
        &model,
        &selection,
        Rc::clone(&selected_entry_id),
    );
    let select_entry_id: Rc<dyn Fn(&str)> = Rc::new({
        let playlist_selection = playlist_selection.clone();
        move |entry_id| {
            playlist_selection.select_entry_id(entry_id);
        }
    });
    if let Some(selection_handle) = selection_handle.as_ref() {
        *selection_handle.borrow_mut() = Some(Rc::clone(&select_entry_id));
    }
    let source_id = shell
        .library
        .query
        .borrow()
        .as_ref()
        .map(|query| query.source_id().clone());
    let selection_playlist_id = playlist_id.clone();
    let selection_state = Rc::clone(&state);
    let current_playlist_selection = playlist_selection.clone();
    shell.register_current_route_track_selection(Rc::new(move |current| {
        if !current_playlist_selection.is_bound() {
            return false;
        }
        let expected_context_id =
            playlist_entry_context_id(&selection_playlist_id, &selection_state.borrow());
        current_playlist_selection.select_now_playing_track(
            current,
            source_id.as_ref(),
            &expected_context_id,
        );
        true
    }));
    let play_entry = {
        let controller = shell.products.playback.queue.clone();
        let playlist_id = playlist_id.clone();
        let entries = Rc::clone(&entries);
        let state = Rc::clone(&state);
        Rc::new(move |position: u32, row: PlaylistEntryTableRow| {
            let entries = entries.borrow();
            let Some(entry) = entries.get(row.original_index) else {
                return;
            };
            let state = state.borrow();
            controller.play_playlist_entry(PlaylistEntryPlayRequest {
                playlist_id: playlist_id.clone(),
                entry: entry.clone(),
                source_index: position as usize,
                query: source_query(&state.query),
                sort: playlist_entry_sort_descriptor(state.sort),
                descending: state.descending,
                shuffled_start: false,
            });
        }) as Rc<dyn Fn(u32, PlaylistEntryTableRow)>
    };

    {
        let index = Rc::clone(&playlist_selection.index);
        let selected_entry_id = Rc::clone(&selected_entry_id);
        selection.connect_selection_changed(move |selection, _, _| {
            sync_playlist_entry_selection(selection, &index.borrow(), &selected_entry_id);
        });
    }
    connect_playlist_entry_model_selection_sync(
        &model,
        &selection,
        Rc::clone(&entries),
        Rc::clone(&playlist_selection.index),
        Rc::clone(&playlist_selection.current),
        Rc::clone(&selected_entry_id),
    );

    let settings = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks);
    let build_shell = Rc::clone(shell);
    let build_model = model.clone();
    let build_entries = Rc::clone(&entries);
    let build_playlist_id = playlist_id;
    let build_select_entry_id = Rc::clone(&select_entry_id);
    let build_selection = selection;
    let build_play_entry = Rc::clone(&play_entry);
    let collection = LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(playlist_entry_table_projection(
                    &build_shell,
                    build_model.clone(),
                    Rc::clone(&build_entries),
                    build_playlist_id.clone(),
                    Rc::clone(&build_select_entry_id),
                    build_selection.clone(),
                    Rc::clone(&build_play_entry),
                    content_inset,
                    can_remove_entries,
                    can_reorder_entries,
                ))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => {
                LibraryPresentationProjection::Grid(playlist_entry_grid_projection(
                    &build_shell,
                    build_model.clone(),
                    Rc::clone(&build_entries),
                    build_playlist_id.clone(),
                    Rc::clone(&build_select_entry_id),
                    Rc::clone(&build_play_entry),
                    can_remove_entries,
                ))
            }
        }),
    );
    (collection, model)
}

fn playlist_entry_table_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    selection: gtk::SingleSelection,
    play_entry: Rc<dyn Fn(u32, PlaylistEntryTableRow)>,
    content_inset: i32,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> CollectionTableProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks)
        .row_fields;
    let fixed_columns = if can_reorder_entries {
        vec![(
            playlist_entry_reorder_column(
                shell,
                Rc::clone(&entries),
                playlist_id.clone(),
                Rc::clone(&select_entry_id),
                can_remove_entries,
                can_reorder_entries,
            ),
            PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH,
        )]
    } else {
        Vec::new()
    };
    let column_shell = Rc::clone(shell);
    let column_entries = Rc::clone(&entries);
    let column_playlist_id = playlist_id;
    let column_select_entry_id = Rc::clone(&select_entry_id);
    let activate = move |position, row| play_entry(position, row);
    let table = dynamic_collection_table(
        model,
        &fields,
        fixed_columns,
        move |field| {
            playlist_entry_column_for_field(
                &column_shell,
                field,
                Rc::clone(&column_entries),
                column_playlist_id.clone(),
                Rc::clone(&column_select_entry_id),
                can_remove_entries,
                can_reorder_entries,
            )
        },
        |field| track_column_fit_width(LibraryListKey::PlaylistTracks, field),
        false,
        Some(Box::new(activate)),
        Some(selection.upcast()),
        route_column_view_initial_width_with_inset(shell, content_inset),
    );
    let widget = table.widget();
    widget.add_css_class("track-table");
    widget.add_css_class("playlist-entry-table");
    table
}

fn playlist_entry_grid_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    play_entry: Rc<dyn Fn(u32, PlaylistEntryTableRow)>,
    can_remove_entries: bool,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let cell_entries = Rc::clone(&entries);
    let cell_playlist_id = playlist_id;
    let cell_select_entry_id = Rc::clone(&select_entry_id);
    collection_grid(
        model,
        &fields,
        move |fields| {
            PlaylistEntryGridCell::new(
                Rc::clone(&cell_shell),
                fields,
                Rc::clone(&cell_entries),
                cell_playlist_id.clone(),
                Rc::clone(&cell_select_entry_id),
                can_remove_entries,
            )
        },
        move |position, row| play_entry(position, row),
    )
}

fn playlist_entry_column_for_field(
    shell: &Rc<Shell>,
    field: LibraryField,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => playlist_entry_number_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::Image => playlist_entry_image_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::TitleMerged => playlist_entry_title_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::Favorite => playlist_entry_favorite_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::Album => playlist_entry_album_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::PlayCount => playlist_entry_play_count_column(
            shell,
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
        ),
        LibraryField::Artist => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
            |entry| track_field(&entry.track, LibraryField::Artist),
            Some(Rc::new(|entry: &PlaylistEntry| {
                track_artist_route(&entry.track)
            })),
        ),
        LibraryField::AlbumArtist => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
            |entry| track_field(&entry.track, LibraryField::AlbumArtist),
            Some(Rc::new(|entry: &PlaylistEntry| {
                entry
                    .track
                    .album_artist_credits
                    .first()
                    .map(|artist| Route::ArtistDetail(artist.id.clone()))
            })),
        ),
        _ => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            entries,
            playlist_id,
            select_entry_id,
            can_remove_entries,
            can_reorder_entries,
            move |entry| track_field(&entry.track, field),
            None,
        ),
    }
}

struct PlaylistEntryGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    can_remove_entries: bool,
    cover: ArtworkTile,
    state: PlaylistEntryCellState,
}

impl PlaylistEntryGridCell {
    fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        entries: Rc<RefCell<Vec<PlaylistEntry>>>,
        playlist_id: PlaylistId,
        _select_entry_id: Rc<dyn Fn(&str)>,
        can_remove_entries: bool,
    ) -> Self {
        let cover = ArtworkTile::new_elastic_square(0);
        let body = CollectionGridCardCell::new(&shell, fields, cover.widget());
        let state = playlist_entry_cell_state();
        install_dynamic_playlist_entry_context_menu(&body.card, &shell, Rc::clone(&state.menu));
        Self {
            body,
            shell,
            entries,
            playlist_id,
            can_remove_entries,
            cover,
            state,
        }
    }
}

impl ReusableCollectionGridCell<PlaylistEntryTableRow> for PlaylistEntryGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, row: PlaylistEntryTableRow) {
        let entries = self.entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        self.shell.bind_artwork_tile(
            &self.cover,
            ArtworkBinding::track(&entry.track),
            stable_seed(entry.track.id.as_str()),
            GRID_COVER_SIZE as i32,
            GRID_COVER_SIZE,
        );
        self.body.bind(&entry.track.title, |field| {
            (
                track_field(&entry.track, field),
                track_grid_field_route(&entry.track, field),
            )
        });
        bind_playlist_entry_cell_state(
            &self.state,
            row,
            entry,
            &self.playlist_id,
            self.can_remove_entries,
        );
    }

    fn clear(&self) {
        self.shell.clear_artwork_tile(&self.cover);
        self.body.clear();
        clear_playlist_entry_cell_state(&self.state);
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        let Some(original_index) = self.state.row.get() else {
            return;
        };
        let entries = self.entries.borrow();
        let Some(entry) = entries.get(original_index) else {
            return;
        };
        self.body.bind(&entry.track.title, |field| {
            (
                track_field(&entry.track, field),
                track_grid_field_route(&entry.track, field),
            )
        });
    }
}
pub(crate) fn rebuild_playlist_entries_model(
    model: &gio::ListStore,
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) {
    let rows = playlist_entries_for_state(entries, state)
        .into_iter()
        .enumerate()
        .map(|(display_index, original_index)| {
            glib::BoxedAnyObject::new(PlaylistEntryTableRow {
                original_index,
                display_index,
            })
        })
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &rows);
}
pub(crate) fn playlist_entries_for_state(
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) -> Vec<usize> {
    let query = state.query.trim().to_lowercase();
    let mut rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || playlist_entry_matches_query(entry, &query))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        let ordering = compare_playlist_entry(entries, *left, *right, state.sort);
        if state.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}
pub(crate) fn playlist_entry_matches_query(entry: &PlaylistEntry, query: &str) -> bool {
    entry.track.title.to_lowercase().contains(query)
        || entry.track.artist.to_lowercase().contains(query)
        || entry.track.album.to_lowercase().contains(query)
}
fn playlist_entry_for_row<'a>(
    entries: &'a [PlaylistEntry],
    row: &PlaylistEntryTableRow,
) -> Option<&'a PlaylistEntry> {
    entries.get(row.original_index)
}

fn playlist_entry_cell_state() -> PlaylistEntryCellState {
    PlaylistEntryCellState {
        menu: Rc::new(RefCell::new(None)),
        row: Rc::new(Cell::new(None)),
        link_route: Rc::new(RefCell::new(None)),
    }
}
fn store_playlist_entry_cell_state(item: &gtk::ListItem, state: PlaylistEntryCellState) {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| {
        states.borrow_mut().insert(key, state);
    });
}
fn playlist_entry_cell_state_for_item(item: &gtk::ListItem) -> Option<PlaylistEntryCellState> {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| states.borrow().get(&key).cloned())
}
fn remove_playlist_entry_cell_state(item: &gtk::ListItem) {
    let key = list_item_storage_key(item);
    PLAYLIST_ENTRY_CELL_STATES.with(|states| {
        states.borrow_mut().remove(&key);
    });
}
fn bind_playlist_entry_cell_state(
    state: &PlaylistEntryCellState,
    row: PlaylistEntryTableRow,
    entry: &PlaylistEntry,
    playlist_id: &PlaylistId,
    can_remove_entries: bool,
) {
    state.row.set(Some(row.original_index));
    *state.menu.borrow_mut() = Some(PlaylistEntryContextMenuState {
        track: entry.track.clone(),
        entry_id: entry.entry_id.clone(),
        remove_action: can_remove_entries.then(|| PlaylistEntryContextMenuAction {
            playlist_id: playlist_id.clone(),
            entry_id: entry.entry_id.clone(),
            title: entry.track.title.clone(),
        }),
    });
}
fn clear_playlist_entry_cell_state(state: &PlaylistEntryCellState) {
    state.row.set(None);
    state.menu.borrow_mut().take();
    state.link_route.borrow_mut().take();
}
fn setup_playlist_entry_link_label(
    label: &gtk::Label,
    shell: &Rc<Shell>,
    state: &PlaylistEntryCellState,
) {
    label.add_css_class("table-link-label");
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), label);
    let shell = Rc::clone(shell);
    let route = Rc::clone(&state.link_route);
    add_label_click(label, move || {
        if let Some(route) = route.borrow().clone() {
            shell.navigate(route);
        }
    });
}
fn setup_playlist_entry_cell_actions(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    state: &PlaylistEntryCellState,
    _select_entry_id: Rc<dyn Fn(&str)>,
    can_reorder_entries: bool,
) {
    install_dynamic_playlist_entry_context_menu(target, shell, Rc::clone(&state.menu));

    if !can_reorder_entries {
        return;
    }
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let library = shell.products.library.clone();
    let target = target.as_ref().clone();
    let target_for_drop = target.downgrade();
    let row_state = Rc::clone(&state.row);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(entry_id) = value.get::<String>() else {
            return false;
        };
        let Some(target_index) = row_state.get() else {
            return false;
        };
        let Some(target) = target_for_drop.upgrade() else {
            return false;
        };
        let after = y > f64::from(target.height()) / 2.0;
        let Some(new_index) =
            playlist_drop_index(&entries.borrow(), &entry_id, target_index, after)
        else {
            return false;
        };
        library.move_playlist_entry(playlist_id.clone(), entry_id, new_index);
        true
    });
    target.add_controller(drop_target);
}
fn playlist_entry_reorder_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let drag = playlist_entry_drag_handle(&state);
        setup_playlist_entry_cell_actions(
            &drag,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&setup_select_entry_id),
            can_reorder_entries,
        );
        item.set_child(Some(&drag));
        store_playlist_entry_cell_state(item, state);
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(state) = playlist_entry_cell_state_for_item(item)
        {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_number_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let label = gtk::Label::new(None);
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        setup_playlist_entry_cell_actions(
            &label,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&setup_select_entry_id),
            can_reorder_entries,
        );
        item.set_child(Some(&label));
        store_playlist_entry_cell_state(item, state);
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        label.set_text(&(row.display_index + 1).to_string());
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(label) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Label>().ok())
            {
                label.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_NUMBER_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_image_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cover = ArtworkTile::new(PLAYLIST_ENTRY_COVER_WIDTH, 0);
        let widget = cover.widget();
        setup_playlist_entry_cell_actions(
            &widget,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&select_entry_id),
            can_reorder_entries,
        );
        item.set_child(Some(&widget));
        store_playlist_entry_cell_state(item, state);
        let key = list_item_storage_key(item);
        PLAYLIST_ENTRY_IMAGE_CELLS.with(|cells| {
            cells.borrow_mut().insert(key, cover);
        });
    });
    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let key = list_item_storage_key(item);
        let Some(cover) =
            PLAYLIST_ENTRY_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
        else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cover,
            ArtworkBinding::track(&entry.track),
            stable_seed(entry.track.id.as_str()),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    let clear_shell = Rc::clone(shell);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let key = list_item_storage_key(item);
        if let Some(cover) =
            PLAYLIST_ENTRY_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
        {
            clear_shell.clear_artwork_tile(&cover);
        }
        if let Some(state) = playlist_entry_cell_state_for_item(item) {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = list_item_storage_key(item);
            PLAYLIST_ENTRY_IMAGE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
            remove_playlist_entry_cell_state(item);
        }
    });
    let column = localized_column("Image", &factory);
    column.set_fixed_width(PLAYLIST_ENTRY_COVER_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_favorite_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let button = favorite_icon_button("Favorite track");
        setup_playlist_entry_cell_actions(
            &button,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&select_entry_id),
            can_reorder_entries,
        );
        let favorite_state = Rc::clone(&state.menu);
        setup_shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_state
                    .borrow()
                    .as_ref()
                    .map(|state| track_favorite_key(&state.track.id))
            }),
            &button,
        );
        let favorite_shell = Rc::clone(&setup_shell);
        let click_state = Rc::clone(&state.menu);
        button.connect_clicked(move |button| {
            let Some(track) = click_state
                .borrow()
                .as_ref()
                .map(|state| state.track.clone())
            else {
                return;
            };
            favorite_shell.set_favorite_with_feedback(
                ::library::FavoriteItemId::Track(track.id),
                !favorite_button_is_active(button),
                Some(button),
            );
        });
        item.set_child(Some(&button));
        store_playlist_entry_cell_state(item, state);
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        else {
            return;
        };
        set_favorite_button_active(&button, entry.track.favorite);
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(state) = playlist_entry_cell_state_for_item(item)
        {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(track_column_width(
        LibraryListKey::PlaylistTracks,
        LibraryField::Favorite,
    ));
    column.set_resizable(false);
    column
}
fn playlist_entry_album_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        "Album",
        PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        entries,
        playlist_id,
        select_entry_id,
        can_remove_entries,
        can_reorder_entries,
        |entry| entry.track.album.clone(),
        Some(Rc::new(|entry: &PlaylistEntry| {
            Some(Route::AlbumDetail(entry.track.album_id.clone()))
        })),
    )
}
fn playlist_entry_play_count_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        msgid("Plays"),
        play_count_column_width(),
        entries,
        playlist_id,
        select_entry_id,
        can_remove_entries,
        can_reorder_entries,
        |entry| playlist_entry_play_count_text(entry.track.play_count),
        None,
    )
}
fn playlist_entry_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
    value: F,
    route: Option<Rc<dyn Fn(&PlaylistEntry) -> Option<Route>>>,
) -> gtk::ColumnViewColumn
where
    F: Fn(&PlaylistEntry) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    let has_link = route.is_some();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Fill);
        root.set_hexpand(true);

        let label = gtk::Label::new(None);
        label.add_css_class("table-link-label");
        label.add_css_class("muted");
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_width_request(1);
        label.set_wrap(false);
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if has_link {
            setup_playlist_entry_link_label(&label, &setup_shell, &state);
        }
        root.append(&label);

        setup_playlist_entry_cell_actions(
            &root,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&setup_select_entry_id),
            can_reorder_entries,
        );
        item.set_child(Some(&root));
        store_playlist_entry_cell_state(item, state);
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .and_then(|root| root.first_child())
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        label.set_text(&(value)(entry));
        if let Some(route) = route.as_ref() {
            *state.link_route.borrow_mut() = route(entry);
        }
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(label) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Box>().ok())
                .and_then(|root| root.first_child())
                .and_then(|child| child.downcast::<gtk::Label>().ok())
            {
                label.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            remove_playlist_entry_cell_state(item);
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}
fn playlist_entry_title_column(
    shell: &Rc<Shell>,
    entries: Rc<RefCell<Vec<PlaylistEntry>>>,
    playlist_id: PlaylistId,
    select_entry_id: Rc<dyn Fn(&str)>,
    can_remove_entries: bool,
    can_reorder_entries: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = Rc::clone(&entries);
    let setup_playlist_id = playlist_id.clone();
    let setup_select_entry_id = Rc::clone(&select_entry_id);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cover = ArtworkTile::new(PLAYLIST_ENTRY_COVER_WIDTH, 0);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_halign(gtk::Align::Fill);
        labels.set_width_request(1);
        let title = playlist_entry_text_label("", "", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        title.add_css_class("playlist-entry-title");
        let artist = playlist_entry_text_label("", "muted", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        setup_playlist_entry_link_label(&artist, &setup_shell, &state);
        labels.append(&title);
        labels.append(&artist);
        let cell = playlist_title_cell(cover.widget(), labels.upcast());
        setup_playlist_entry_cell_actions(
            &cell,
            &setup_shell,
            Rc::clone(&setup_entries),
            setup_playlist_id.clone(),
            &state,
            Rc::clone(&setup_select_entry_id),
            can_reorder_entries,
        );
        item.set_child(Some(&cell));
        store_playlist_entry_cell_state(item, state);
        let key = list_item_storage_key(item);
        PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                PlaylistEntryTitleCell {
                    cover,
                    title,
                    artist,
                },
            );
        });
    });
    let bind_shell = Rc::clone(shell);
    let unbind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryTableRow>(item) else {
            return;
        };
        let entries = entries.borrow();
        let Some(entry) = playlist_entry_for_row(&entries, &row) else {
            return;
        };
        let key = list_item_storage_key(item);
        let Some(cell) = PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
        else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::track(&entry.track),
            stable_seed(entry.track.id.as_str()),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&entry.track.title);
        cell.artist.set_text(&entry.track.artist);
        let Some(state) = playlist_entry_cell_state_for_item(item) else {
            return;
        };
        *state.link_route.borrow_mut() = track_artist_route(&entry.track);
        bind_playlist_entry_cell_state(&state, row, entry, &playlist_id, can_remove_entries);
    });
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = list_item_storage_key(item);
            if let Some(cell) =
                PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
            {
                unbind_shell.clear_artwork_tile(&cell.cover);
                cell.title.set_text("");
                cell.artist.set_text("");
            }
            if let Some(state) = playlist_entry_cell_state_for_item(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = list_item_storage_key(item);
            PLAYLIST_ENTRY_TITLE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
            remove_playlist_entry_cell_state(item);
        }
    });
    let column = localized_column("Title", &factory);
    column.set_fixed_width(PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_drag_handle(state: &PlaylistEntryCellState) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    bind_widget_tooltip(&drag, "Drag to reorder");
    drag.set_width_request(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let drag_source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let menu_state = Rc::clone(&state.menu);
    drag_source.connect_prepare(move |_, _, _| {
        let entry_id = menu_state
            .borrow()
            .as_ref()
            .map(|state| state.entry_id.clone())?;
        Some(gtk::gdk::ContentProvider::for_value(&entry_id.to_value()))
    });
    drag.add_controller(drag_source);
    drag
}
pub(crate) fn compare_playlist_entry(
    entries: &[PlaylistEntry],
    left: usize,
    right: usize,
    sort: PlaylistEntrySort,
) -> std::cmp::Ordering {
    let Some(left_entry) = entries.get(left) else {
        return std::cmp::Ordering::Equal;
    };
    let Some(right_entry) = entries.get(right) else {
        return std::cmp::Ordering::Equal;
    };
    match sort {
        PlaylistEntrySort::Order => left.cmp(&right),
        PlaylistEntrySort::Title => cmp_text(&left_entry.track.title, &right_entry.track.title),
        PlaylistEntrySort::Artist => cmp_text(&left_entry.track.artist, &right_entry.track.artist),
        PlaylistEntrySort::Album => cmp_text(&left_entry.track.album, &right_entry.track.album),
    }
    .then_with(|| left.cmp(&right))
}
pub(crate) fn cmp_text(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}
pub(crate) fn playlist_title_cell(cover: gtk::Widget, labels: gtk::Widget) -> gtk::Widget {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.append(&cover);
    title.append(&labels);
    title.upcast()
}
pub(crate) fn playlist_drop_index(
    entries: &[PlaylistEntry],
    dragged_entry_id: &str,
    target_index: usize,
    after: bool,
) -> Option<usize> {
    let source_index = entries
        .iter()
        .position(|entry| entry.entry_id == dragged_entry_id)?;
    let mut new_index = if after {
        target_index.saturating_add(1)
    } else {
        target_index
    };
    if source_index < new_index {
        new_index = new_index.saturating_sub(1);
    }
    (source_index != new_index).then_some(new_index)
}
pub(crate) fn playlist_entry_text_label(
    text: &str,
    css_class: &str,
    max_width_chars: i32,
) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.0);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_wrap(false);
    label.set_single_line_mode(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}
pub(crate) fn playlist_entry_play_count_text(value: Option<u32>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
pub(crate) fn confirm_remove_playlist_entry(
    shell: &Rc<Shell>,
    playlist_id: PlaylistId,
    entry_id: String,
    title: String,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove from Playlist"))
        .body(format!("Remove \"{title}\" from this playlist?"))
        .build();
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("remove", &tr("Remove"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let library = shell.products.library.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "remove" {
            library.remove_playlist_entry(playlist_id.clone(), entry_id.clone());
        }
    });
    present_light_dismiss_dialog(&dialog, &shell.chrome.window);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::route::RouteCurrentTrackContext;

    #[test]
    fn duplicate_playlist_track_uses_matching_context_rank() {
        let source_id = SourceId::fake(1);
        let track_id = TrackId::fake(1);
        let mut index = PlaylistEntrySelectionIndex::default();
        index
            .first_entry_by_track
            .insert(track_id.clone(), "first".into());
        index.entry_by_source_rank.insert(
            0,
            PlaylistEntrySelectionItem {
                entry_id: "first".into(),
                track_id: track_id.clone(),
            },
        );
        index.entry_by_source_rank.insert(
            1,
            PlaylistEntrySelectionItem {
                entry_id: "second".into(),
                track_id: track_id.clone(),
            },
        );
        let current = RouteCurrentTrack {
            source_id: source_id.clone(),
            track_id,
            occurrence: playback::OccurrenceId::new("second-occurrence"),
            context: Some(RouteCurrentTrackContext {
                context_id: "matching-context".into(),
                source_rank: 1,
            }),
        };
        let exact =
            playlist_entry_current_selection(Some(&current), Some(&source_id), "matching-context")
                .expect("current playlist occurrence");

        assert_eq!(index.entry_id_for_current(&exact), Some("second"));
        assert!(
            playlist_entry_current_selection(
                Some(&current),
                Some(&SourceId::fake(2)),
                "matching-context",
            )
            .is_none()
        );
    }

    #[test]
    fn model_selection_sync_does_not_retain_the_model() {
        gtk::init().expect("initialize GTK");
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        connect_playlist_entry_model_selection_sync(
            &model,
            &selection,
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(PlaylistEntrySelectionIndex::default())),
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(None)),
        );
        let weak_model = model.downgrade();
        let weak_selection = selection.downgrade();

        drop(selection);
        drop(model);

        assert!(weak_selection.upgrade().is_none());
        assert!(weak_model.upgrade().is_none());
    }
}
