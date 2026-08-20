use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use ::library::{PlaylistEdit, PlaylistEntryItem, PlaylistEntryList, PlaylistId, SourceId, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::glib;

use super::collection_context::install_dynamic_playlist_entry_context_menu;
use super::playlist_entry_model::{
    PlaylistEntryModel, PlaylistEntryProjectionRequest, PlaylistEntryRow, PreparedPlaylistEntries,
};
use crate::favorites::{
    favorite_button_is_active, favorite_icon_button, set_favorite_button_active, track_favorite_key,
};
use crate::localization::{bind_search_placeholder, bind_widget_tooltip, localized_column};
use crate::shell::Shell;
use crate::shell::cover::{ArtworkTile, LARGE_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::route::RouteCurrentTrack;
use localization::{msgid, tr};

use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};

use super::collections::{
    CollectionTableProjection, LibraryCollectionProjection, LibraryPresentationProjection,
    dynamic_collection_table, library_route_inset, track_grid_field_links,
};
use super::columns::{
    ROW_INDEX_COLUMN_TITLE, TrackRowPlayingIndicator, set_track_row_index_text,
    track_column_fit_width, track_column_width, track_is_downloaded, track_row_index_cell,
};
use super::detail_links::{
    DetailLinkBinding, DetailLinks, track_album_artist_links, track_artist_links,
};
use super::factory_cells::FactoryCells;
use super::grid_cells::{
    CollectionGridCardCell, CollectionGridProjection, ReusableCollectionGridCell, collection_grid,
};
use super::library_fields::{
    COLLECTION_GRID_MAX_CARD_WIDTH, item_at_from_item, play_count_column_width, track_field,
};
use super::route::Route;
use super::route_layout::PRIMARY_ROUTE_HORIZONTAL_INSET;
use super::route_shell::LibraryToolbarProjection;
use super::table_sizing::route_column_view_initial_width_with_inset;

const PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH: i32 = 30;
const PLAYLIST_ENTRY_NUMBER_WIDTH: i32 = 24;
const PLAYLIST_ENTRY_COVER_WIDTH: i32 = 36;
const PLAYLIST_ENTRY_COLUMN_GAP: i32 = 8;
const PLAYLIST_ENTRY_TITLE_MAX_CHARS: i32 = 44;
const PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH: i32 = 320;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 220;

#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryContextMenuAction {
    pub(crate) playlist_id: PlaylistId,
    pub(crate) occurrence_id: String,
    pub(crate) title: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PlaylistEntryContextMenuState {
    pub(crate) track: Track,
    pub(crate) occurrence_id: String,
    pub(crate) remove_action: Option<PlaylistEntryContextMenuAction>,
}

#[derive(Clone)]
pub(crate) struct PlaylistEntryTablePlayingState {
    model: glib::WeakRef<PlaylistEntryModel>,
    current: Rc<RefCell<Option<PlaylistEntryCurrentSelection>>>,
    indicator: TrackRowPlayingIndicator,
}

#[derive(Clone, Debug)]
struct PlaylistEntryCurrentSelection {
    track_id: ::library::TrackId,
    source_rank: Option<usize>,
    source_order: bool,
}

fn playlist_entry_current_selection(
    current: Option<&RouteCurrentTrack>,
    source_id: Option<&SourceId>,
    expected_context_id: &str,
    source_context_id: &str,
) -> Option<PlaylistEntryCurrentSelection> {
    current
        .zip(source_id)
        .filter(|(current, source_id)| &current.source_id == *source_id)
        .map(|(current, _)| {
            let context = current.context.as_ref();
            let source_order =
                context.is_some_and(|context| context.context_id == source_context_id);
            let source_rank = context
                .filter(|context| context.context_id == expected_context_id || source_order)
                .map(|context| context.source_rank);
            PlaylistEntryCurrentSelection {
                track_id: current.track_id.clone(),
                source_rank,
                source_order,
            }
        })
}

impl PlaylistEntryTablePlayingState {
    fn new(model: &PlaylistEntryModel, indicator: TrackRowPlayingIndicator) -> Self {
        let current = Rc::new(RefCell::new(None));
        let model_current = Rc::clone(&current);
        let model_indicator = indicator.clone();
        model.connect_items_changed(move |model, _, _, _| {
            model_indicator.set_position(playlist_entry_playing_position(
                model,
                model_current.borrow().as_ref(),
            ));
        });
        Self {
            model: model.downgrade(),
            current,
            indicator,
        }
    }

    pub(crate) fn set_now_playing_track(
        &self,
        current: Option<&RouteCurrentTrack>,
        source_id: Option<&SourceId>,
        expected_context_id: &str,
        source_context_id: &str,
    ) {
        let current = playlist_entry_current_selection(
            current,
            source_id,
            expected_context_id,
            source_context_id,
        );
        *self.current.borrow_mut() = current;
        self.sync();
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.indicator.set_paused(paused);
    }

    pub(crate) fn is_bound(&self) -> bool {
        self.model.upgrade().is_some()
    }

    fn sync(&self) {
        let Some(model) = self.model.upgrade() else {
            return;
        };
        self.indicator.set_position(playlist_entry_playing_position(
            &model,
            self.current.borrow().as_ref(),
        ));
    }
}

fn playlist_entry_playing_position(
    model: &PlaylistEntryModel,
    current: Option<&PlaylistEntryCurrentSelection>,
) -> u32 {
    current
        .and_then(|current| {
            model.occurrence_for_current(
                &current.track_id,
                current.source_rank,
                current.source_order,
            )
        })
        .and_then(|entry_id| model.visible_position(&entry_id))
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

#[derive(Clone)]
struct PlaylistEntryCellState {
    menu: Rc<RefCell<Option<PlaylistEntryContextMenuState>>>,
    row: Rc<Cell<Option<usize>>>,
    links: Rc<RefCell<Option<DetailLinkBinding>>>,
    downloaded: Rc<RefCell<Option<gtk::Image>>>,
}
#[derive(Clone)]
struct PlaylistEntryTitleCell {
    state: PlaylistEntryCellState,
    cover: ArtworkTile,
    title: gtk::Label,
    artist: gtk::Label,
}

#[derive(Clone)]
struct PlaylistEntryImageCell {
    state: PlaylistEntryCellState,
    cover: ArtworkTile,
}

#[derive(Clone)]
pub(crate) struct PlaylistEntriesView {
    widget: gtk::Widget,
    collection: LibraryCollectionProjection,
    toolbar: LibraryToolbarProjection,
    model: PlaylistEntryModel,
    search: gtk::SearchEntry,
    stack: gtk::Stack,
    toolbar_widget: gtk::Widget,
}

impl PlaylistEntriesView {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn item_navigation(&self) -> crate::shell::route::MountedRouteItemNavigation {
        self.collection.item_navigation()
    }

    pub(crate) fn source_play_request(
        &self,
        placement: playback::QueuePlacement,
        shuffled_start: bool,
    ) -> Option<playback::LoadedPlayRequest> {
        self.model.source_play_request(placement, shuffled_start)
    }

    pub(crate) fn projection_request(&self) -> PlaylistEntryProjectionRequest {
        self.model.projection_request()
    }

    pub(crate) fn connect_search_request(
        &self,
        callback: impl Fn(PlaylistEntryProjectionRequest) + 'static,
    ) {
        let model = self.model.clone();
        self.search.connect_search_changed(move |_| {
            callback(model.projection_request());
        });
    }

    pub(crate) fn replace_prepared(&self, entries: PreparedPlaylistEntries) -> bool {
        let empty = entries.entries_is_empty();
        if !self.model.replace_prepared(entries) {
            return false;
        }
        self.toolbar_widget.set_visible(!empty);
        self.stack
            .set_visible_child_name(if empty { "empty" } else { "content" });
        true
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &LibraryListSettings,
    ) {
        if key != LibraryListKey::PlaylistTracks {
            return;
        }
        self.model.apply_settings(settings);
        self.collection.apply_settings(settings);
        self.toolbar.apply(key, settings);
    }
}

impl Shell {
    pub(crate) fn playlist_entries_view(
        self: &Rc<Self>,
        playlist_id: PlaylistId,
        initial_entries: PlaylistEntryList,
        initial_positions: Vec<u32>,
    ) -> PlaylistEntriesView {
        let settings = self
            .settings
            .current
            .borrow()
            .library_list(LibraryListKey::PlaylistTracks);
        let selected = self
            .selected_library()
            .as_deref()
            .map(|selected| (selected.source_id.clone(), selected.source_session_epoch))
            .expect("a playlist route requires one selected source");
        let model = PlaylistEntryModel::new_prepared(
            selected.0,
            selected.1,
            playlist_id.clone(),
            initial_entries,
            initial_positions,
            &settings,
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_width_request(1);

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        search.set_width_request(1);
        self.set_route_search(Some(search.clone()));
        let toolbar =
            self.library_toolbar_projection(LibraryListKey::PlaylistTracks, search.clone());
        let toolbar_widget = library_route_inset(toolbar.widget());
        toolbar_widget.set_visible(!model.source_is_empty());
        wrapper.append(&toolbar_widget);

        let collection = playlist_entries_collection_projection(
            self,
            model.clone(),
            playlist_id,
            PRIMARY_ROUTE_HORIZONTAL_INSET,
        );
        self.refresh_current_route_now_playing_selections();

        {
            let shell = Rc::clone(self);
            let model = model.clone();
            search.connect_search_changed(move |entry| {
                model.set_query(&entry.text());
                shell.refresh_current_route_now_playing_selections();
            });
        }
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(
            &library_route_inset(self.placeholder_view("Tracks", msgid("No tracks here yet"))),
            Some("empty"),
        );
        stack.add_named(&collection.scrolling_widget(), Some("content"));
        stack.set_visible_child_name(if model.source_is_empty() {
            "empty"
        } else {
            "content"
        });
        wrapper.append(&stack);

        PlaylistEntriesView {
            widget: wrapper.upcast(),
            collection,
            toolbar,
            model,
            search,
            stack,
            toolbar_widget,
        }
    }
}

pub(crate) fn playlist_entries_collection_projection(
    shell: &Rc<Shell>,
    model: PlaylistEntryModel,
    playlist_id: PlaylistId,
    content_inset: i32,
) -> LibraryCollectionProjection {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    let playing_indicator = TrackRowPlayingIndicator::new();
    let playlist_playing = PlaylistEntryTablePlayingState::new(&model, playing_indicator.clone());
    let source_id = shell
        .selected_library()
        .as_deref()
        .map(|selected| Some(selected.source_id.clone()))
        .expect("a playlist route requires one selected source");
    let current_playlist_playing = playlist_playing.clone();
    let selection_source_id = source_id.clone();
    let selection_model = model.clone();
    shell.register_current_route_track_selection(Rc::new(move |current| {
        if !current_playlist_playing.is_bound() {
            return false;
        }
        let expected_context_id = selection_model.visible_context_id();
        let source_context_id = selection_model.source_context_id();
        current_playlist_playing.set_now_playing_track(
            current,
            selection_source_id.as_ref(),
            &expected_context_id,
            &source_context_id,
        );
        current_playlist_playing.set_paused(current.is_some_and(|current| current.paused));
        true
    }));
    let play_entry = {
        let queue = shell.products.playback.queue.clone();
        let model = model.clone();
        Rc::new(move |position: u32, _: PlaylistEntryRow| {
            if let Some(request) = model.visible_play_request(position as usize) {
                queue.play_loaded(request);
            }
        }) as Rc<dyn Fn(u32, PlaylistEntryRow)>
    };

    let settings = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks);
    let build_shell = Rc::clone(shell);
    let build_model = model.clone();
    let build_playlist_id = playlist_id;
    let build_selection = selection.clone();
    let build_play_entry = Rc::clone(&play_entry);
    let build_playing_indicator = playing_indicator;
    let collection = LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(playlist_entry_table_projection(
                    &build_shell,
                    build_model.clone(),
                    build_playlist_id.clone(),
                    build_selection.clone(),
                    Rc::clone(&build_play_entry),
                    build_playing_indicator.clone(),
                    content_inset,
                ))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => {
                LibraryPresentationProjection::Grid(playlist_entry_grid_projection(
                    &build_shell,
                    build_model.clone(),
                    build_playlist_id.clone(),
                    Rc::clone(&build_play_entry),
                ))
            }
        }),
    );
    collection
}

fn playlist_entry_table_projection(
    shell: &Rc<Shell>,
    model: PlaylistEntryModel,
    playlist_id: PlaylistId,
    selection: gtk::SingleSelection,
    play_entry: Rc<dyn Fn(u32, PlaylistEntryRow)>,
    playing_indicator: TrackRowPlayingIndicator,
    content_inset: i32,
) -> CollectionTableProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks)
        .row_fields;
    let fixed_columns = vec![(
        playlist_entry_reorder_column(shell, model.clone(), playlist_id.clone()),
        PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH,
    )];
    let column_shell = Rc::clone(shell);
    let column_model = model.clone();
    let column_playlist_id = playlist_id;
    let column_playing_indicator = playing_indicator;
    let activate = move |position, row| play_entry(position, row);
    let table = dynamic_collection_table(
        shell,
        LibraryListKey::PlaylistTracks,
        model,
        &fields,
        fixed_columns,
        move |field| {
            playlist_entry_column_for_field(
                &column_shell,
                field,
                column_model.clone(),
                column_playlist_id.clone(),
                column_playing_indicator.clone(),
            )
        },
        |field| track_column_fit_width(LibraryListKey::PlaylistTracks, field),
        false,
        Some(Box::new(activate)),
        Some(selection),
        route_column_view_initial_width_with_inset(shell, content_inset),
    );
    let widget = table.widget();
    widget.add_css_class("track-table");
    widget.add_css_class("playlist-entry-table");
    table
}

fn playlist_entry_grid_projection(
    shell: &Rc<Shell>,
    model: PlaylistEntryModel,
    playlist_id: PlaylistId,
    play_entry: Rc<dyn Fn(u32, PlaylistEntryRow)>,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::PlaylistTracks)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let cell_model = model.clone();
    let cell_playlist_id = playlist_id;
    collection_grid(
        model,
        &fields,
        move |fields| {
            PlaylistEntryGridCell::new(
                Rc::clone(&cell_shell),
                fields,
                cell_model.clone(),
                cell_playlist_id.clone(),
            )
        },
        move |position, row| play_entry(position, row),
    )
}

fn playlist_entry_column_for_field(
    shell: &Rc<Shell>,
    field: LibraryField,
    model: PlaylistEntryModel,
    playlist_id: PlaylistId,
    playing_indicator: TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => {
            playlist_entry_number_column(shell, model, playlist_id, playing_indicator)
        }
        LibraryField::Image => playlist_entry_image_column(shell, model, playlist_id),
        LibraryField::TitleMerged => {
            playlist_entry_title_column(shell, model, playlist_id, playing_indicator)
        }
        LibraryField::Favorite => playlist_entry_favorite_column(shell, model, playlist_id),
        LibraryField::Album => playlist_entry_album_column(shell, model, playlist_id),
        LibraryField::PlayCount => playlist_entry_play_count_column(shell, model, playlist_id),
        LibraryField::Artist => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            model,
            playlist_id,
            |entry| track_field(&entry.track, LibraryField::Artist),
            Some(Rc::new(|entry: &PlaylistEntryItem| {
                track_artist_links(&entry.track)
            })),
        ),
        LibraryField::AlbumArtist => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            model,
            playlist_id,
            |entry| track_field(&entry.track, LibraryField::AlbumArtist),
            Some(Rc::new(|entry: &PlaylistEntryItem| {
                track_album_artist_links(&entry.track)
            })),
        ),
        _ => playlist_entry_text_column(
            shell,
            field.title(),
            track_column_width(LibraryListKey::PlaylistTracks, field),
            model,
            playlist_id,
            move |entry| track_field(&entry.track, field),
            None,
        ),
    }
}

struct PlaylistEntryGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    model: PlaylistEntryModel,
    playlist_id: PlaylistId,
    cover: ArtworkTile,
    state: PlaylistEntryCellState,
}

impl PlaylistEntryGridCell {
    fn new(
        shell: Rc<Shell>,
        fields: &[LibraryField],
        model: PlaylistEntryModel,
        playlist_id: PlaylistId,
    ) -> Self {
        let cover = ArtworkTile::new_elastic_square();
        let body = CollectionGridCardCell::new(&shell, fields, cover.widget());
        let state = playlist_entry_cell_state();
        body.set_download_badge(playlist_entry_download_badge(&shell, &state));
        install_dynamic_playlist_entry_context_menu(&body.card, &shell, Rc::clone(&state.menu));
        Self {
            body,
            shell,
            model,
            playlist_id,
            cover,
            state,
        }
    }
}

impl ReusableCollectionGridCell<PlaylistEntryRow> for PlaylistEntryGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, row: PlaylistEntryRow) {
        let Some(entry) = self.model.entry_for_row(&row) else {
            return;
        };
        self.shell.bind_artwork_tile(
            &self.cover,
            ArtworkBinding::track(&entry.track),
            COLLECTION_GRID_MAX_CARD_WIDTH,
            LARGE_COVER_SIZE,
        );
        self.body.bind(&entry.track.title, |field| {
            track_grid_field_links(&entry.track, field)
        });
        bind_playlist_entry_cell_state(&self.state, row, &entry, &self.playlist_id);
        bind_playlist_entry_download_badge(&self.shell, &self.state, &entry.track);
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
        let Some(entry) = self.model.entry_for_row(&PlaylistEntryRow {
            source_index: original_index,
            display_index: 0,
        }) else {
            return;
        };
        self.body.bind(&entry.track.title, |field| {
            track_grid_field_links(&entry.track, field)
        });
    }
}

fn playlist_entry_cell_state() -> PlaylistEntryCellState {
    PlaylistEntryCellState {
        menu: Rc::new(RefCell::new(None)),
        row: Rc::new(Cell::new(None)),
        links: Rc::new(RefCell::new(None)),
        downloaded: Rc::new(RefCell::new(None)),
    }
}
fn bind_playlist_entry_cell_state(
    state: &PlaylistEntryCellState,
    row: PlaylistEntryRow,
    entry: &PlaylistEntryItem,
    playlist_id: &PlaylistId,
) {
    state.row.set(Some(row.source_index));
    *state.menu.borrow_mut() = Some(PlaylistEntryContextMenuState {
        track: entry.track.clone(),
        occurrence_id: entry.occurrence_id.clone(),
        remove_action: Some(PlaylistEntryContextMenuAction {
            playlist_id: playlist_id.clone(),
            occurrence_id: entry.occurrence_id.clone(),
            title: entry.track.title.clone(),
        }),
    });
}
fn clear_playlist_entry_cell_state(state: &PlaylistEntryCellState) {
    state.row.set(None);
    state.menu.borrow_mut().take();
    if let Some(links) = state.links.borrow().as_ref() {
        links.clear();
    }
    if let Some(downloaded) = state.downloaded.borrow().as_ref() {
        downloaded.set_visible(false);
    }
}
fn playlist_entry_download_badge(shell: &Rc<Shell>, state: &PlaylistEntryCellState) -> gtk::Image {
    let current = Rc::clone(&state.menu);
    let badge = shell.download_badge(false, move |selected| {
        current.borrow().as_ref().is_some_and(|state| {
            selected
                .library
                .is_downloaded(&state.track.id)
                .unwrap_or(false)
        })
    });
    state.downloaded.replace(Some(badge.clone()));
    badge
}
fn bind_playlist_entry_download_badge(
    shell: &Shell,
    state: &PlaylistEntryCellState,
    track: &Track,
) {
    if let Some(downloaded) = state.downloaded.borrow().as_ref() {
        shell.set_download_badge_visible(downloaded, track_is_downloaded(shell, track));
    }
}
fn setup_playlist_entry_link_label(
    label: &gtk::Label,
    shell: &Rc<Shell>,
    state: &PlaylistEntryCellState,
) {
    label.add_css_class("table-link-label");
    state
        .links
        .replace(Some(DetailLinkBinding::new(label, shell)));
}
fn setup_playlist_entry_cell_actions(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
    state: &PlaylistEntryCellState,
) {
    install_dynamic_playlist_entry_context_menu(target, shell, Rc::clone(&state.menu));

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let source = shell.selected_source_operations();
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
        let Some(source) = source.as_ref() else {
            return false;
        };
        let after = y > f64::from(target.height()) / 2.0;
        let Some(new_index) = entries.drop_index(&entry_id, target_index, after) else {
            return false;
        };
        source.edit_playlist(PlaylistEdit::MoveEntry {
            playlist_id: playlist_id.clone(),
            occurrence_id: entry_id,
            new_index,
        });
        true
    });
    target.add_controller(drop_target);
}
fn playlist_entry_reorder_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let drag = playlist_entry_drag_handle(&state);
        setup_playlist_entry_cell_actions(
            &drag,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&drag));
        setup_cells.insert(item, state);
    });
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
            return;
        };
        let Some(state) = bind_cells.get(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, &entry, &playlist_id);
    });
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(state) = unbind_cells.get(item)
        {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_REORDER_COLUMN_WIDTH);
    column.set_resizable(false);
    column
}
fn playlist_entry_number_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
    playing_indicator: TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cell = track_row_index_cell("");
        setup_playlist_entry_cell_actions(
            &cell,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&cell));
        setup_cells.insert(item, state);
    });
    let bind_playing_indicator = playing_indicator.clone();
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Overlay>().ok())
        else {
            return;
        };
        let Some(state) = bind_cells.get(item) else {
            return;
        };
        set_track_row_index_text(&cell, &(row.display_index + 1).to_string());
        bind_playing_indicator.bind(cell.upcast_ref(), item.position());
        bind_playlist_entry_cell_state(&state, row, &entry, &playlist_id);
    });
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(cell) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Overlay>().ok())
            {
                playing_indicator.unbind(cell.upcast_ref());
                set_track_row_index_text(&cell, "");
            }
            if let Some(state) = unbind_cells.get(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(ROW_INDEX_COLUMN_TITLE), Some(factory));
    column.set_fixed_width(PLAYLIST_ENTRY_NUMBER_WIDTH);
    column
}
fn playlist_entry_image_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cover = ArtworkTile::new(PLAYLIST_ENTRY_COVER_WIDTH);
        let widget = cover.widget();
        setup_playlist_entry_cell_actions(
            &widget,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&widget));
        setup_cells.insert(item, PlaylistEntryImageCell { state, cover });
    });
    let bind_shell = Rc::clone(shell);
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
            return;
        };
        let Some(cell) = bind_cells.get(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::track(&entry.track),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );
        bind_playlist_entry_cell_state(&cell.state, row, &entry, &playlist_id);
    });
    let clear_shell = Rc::clone(shell);
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(cell) = unbind_cells.get(item) {
            clear_shell.clear_artwork_tile(&cell.cover);
            clear_playlist_entry_cell_state(&cell.state);
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });
    let column = localized_column("Image", &factory);
    column.set_fixed_width(PLAYLIST_ENTRY_COVER_WIDTH);
    column
}
fn playlist_entry_favorite_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let button = favorite_icon_button("Favorite track");
        setup_playlist_entry_cell_actions(
            &button,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        let favorite_state = Rc::clone(&state.menu);
        setup_shell.register_dynamic_favorite_button(
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
                ::library::FavoriteItemId::Track(track.id.clone()),
                !favorite_button_is_active(button),
                Some(button),
            );
        });
        item.set_child(Some(&button));
        setup_cells.insert(item, state);
    });
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
            return;
        };
        let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        else {
            return;
        };
        set_favorite_button_active(&button, entry.track.favorite);
        let Some(state) = bind_cells.get(item) else {
            return;
        };
        bind_playlist_entry_cell_state(&state, row, &entry, &playlist_id);
    });
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(state) = unbind_cells.get(item)
        {
            clear_playlist_entry_cell_state(&state);
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(track_column_width(
        LibraryListKey::PlaylistTracks,
        LibraryField::Favorite,
    ));
    column
}
fn playlist_entry_album_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        "Album",
        PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH,
        entries,
        playlist_id,
        |entry| entry.track.album.clone(),
        Some(Rc::new(|entry: &PlaylistEntryItem| {
            DetailLinks::route(
                &entry.track.album,
                entry.track.album_id.clone().map(Route::AlbumDetail),
            )
        })),
    )
}
fn playlist_entry_play_count_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
) -> gtk::ColumnViewColumn {
    playlist_entry_text_column(
        shell,
        msgid("Plays"),
        play_count_column_width(),
        entries,
        playlist_id,
        |entry| playlist_entry_play_count_text(entry.track.play_count),
        None,
    )
}
fn playlist_entry_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
    value: F,
    links: Option<Rc<dyn Fn(&PlaylistEntryItem) -> DetailLinks>>,
) -> gtk::ColumnViewColumn
where
    F: Fn(&PlaylistEntryItem) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let value = Rc::new(value);
    let has_link = links.is_some();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
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
        if title == "Title" {
            root.set_spacing(5);
            root.append(&playlist_entry_download_badge(&setup_shell, &state));
        }

        setup_playlist_entry_cell_actions(
            &root,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&root));
        setup_cells.insert(item, state);
    });
    let bind_shell = Rc::clone(shell);
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
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
        let Some(state) = bind_cells.get(item) else {
            return;
        };
        if let Some(links) = links.as_ref()
            && let Some(binding) = state.links.borrow().as_ref()
        {
            binding.bind(links(&entry));
        } else {
            label.set_text(&(value)(&entry));
        }
        bind_playlist_entry_cell_state(&state, row, &entry, &playlist_id);
        bind_playlist_entry_download_badge(&bind_shell, &state, &entry.track);
    });
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(label) = item
                .child()
                .and_then(|child| child.downcast::<gtk::Box>().ok())
                .and_then(|root| root.first_child())
                .and_then(|child| child.downcast::<gtk::Label>().ok())
            {
                label.set_text("");
            }
            if let Some(state) = unbind_cells.get(item) {
                clear_playlist_entry_cell_state(&state);
            }
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
fn playlist_entry_title_column(
    shell: &Rc<Shell>,
    entries: PlaylistEntryModel,
    playlist_id: PlaylistId,
    playing_indicator: TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let cells = FactoryCells::new();
    let setup_shell = Rc::clone(shell);
    let setup_entries = entries.clone();
    let setup_playlist_id = playlist_id.clone();
    let setup_cells = cells.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let state = playlist_entry_cell_state();
        let cover = ArtworkTile::new(PLAYLIST_ENTRY_COVER_WIDTH);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_halign(gtk::Align::Fill);
        labels.set_width_request(1);
        let title = playlist_entry_text_label("", "", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        title.add_css_class("playlist-entry-title");
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        title_row.append(&title);
        title_row.append(&playlist_entry_download_badge(&setup_shell, &state));
        let artist = playlist_entry_text_label("", "muted", PLAYLIST_ENTRY_TITLE_MAX_CHARS);
        setup_playlist_entry_link_label(&artist, &setup_shell, &state);
        labels.append(&title_row);
        labels.append(&artist);
        let cell = playlist_title_cell(cover.widget(), labels.upcast());
        setup_playlist_entry_cell_actions(
            &cell,
            &setup_shell,
            setup_entries.clone(),
            setup_playlist_id.clone(),
            &state,
        );
        item.set_child(Some(&cell));
        setup_cells.insert(
            item,
            PlaylistEntryTitleCell {
                state,
                cover,
                title,
                artist,
            },
        );
    });
    let bind_shell = Rc::clone(shell);
    let unbind_shell = Rc::clone(shell);
    let bind_playing_indicator = playing_indicator.clone();
    let bind_cells = cells.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item_at_from_item::<PlaylistEntryRow>(item) else {
            return;
        };
        let Some(entry) = entries.entry_for_row(&row) else {
            return;
        };
        let Some(cell) = bind_cells.get(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::track(&entry.track),
            PLAYLIST_ENTRY_COVER_WIDTH,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&entry.track.title);
        bind_playing_indicator.bind(cell.title.upcast_ref(), item.position());
        if let Some(links) = cell.state.links.borrow().as_ref() {
            links.bind(track_artist_links(&entry.track));
        }
        bind_playlist_entry_cell_state(&cell.state, row, &entry, &playlist_id);
        bind_playlist_entry_download_badge(&bind_shell, &cell.state, &entry.track);
    });
    let unbind_cells = cells.clone();
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            if let Some(cell) = unbind_cells.get(item) {
                unbind_shell.clear_artwork_tile(&cell.cover);
                cell.title.set_text("");
                playing_indicator.unbind(cell.title.upcast_ref());
                cell.artist.set_text("");
                clear_playlist_entry_cell_state(&cell.state);
            }
        }
    });
    factory.connect_teardown(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            cells.remove(item);
        }
    });
    let column = localized_column("Title", &factory);
    column.set_fixed_width(PLAYLIST_ENTRY_TITLE_COLUMN_WIDTH);
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
            .map(|state| state.occurrence_id.clone())?;
        Some(gtk::gdk::ContentProvider::for_value(&entry_id.to_value()))
    });
    drag.add_controller(drag_source);
    drag
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
    occurrence_id: String,
    title: String,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove from Playlist"))
        .body(format!("Remove \"{title}\" from this playlist?"))
        .build();
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("remove", &tr("Remove"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let source = shell.selected_source_operations();
    dialog.connect_response(None, move |_, response| {
        if response == "remove"
            && let Some(source) = source.as_ref()
        {
            source.edit_playlist(PlaylistEdit::RemoveEntries {
                playlist_id: playlist_id.clone(),
                occurrence_ids: vec![occurrence_id.clone()],
            });
        }
    });
    shell.present_selected_dialog(&dialog);
}
