use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use ::library::{
    ActiveLibraryQuery, Album, Artist, Playlist, SmartPlaylist, SmartPlaylistId, Track, TrackId,
};
use adw::prelude::*;
use gtk::{gio, glib};

use crate::layout::{configure_fill_width_clip, width_allocation_owner};
use crate::shell::Shell;
use crate::shell::route::RouteCurrentTrack;
use crate::{LibraryField, LibraryLayout, LibraryListKey};
use localization::tr;

use super::album_detail::{AlbumCollectionModels, AlbumDetailVirtualList, album_detail_list};
use super::columns::{
    album_column, artist_column, column_fit_width, playlist_column, smart_playlist_column,
    track_column_fit_width, track_column_for_key,
};
use super::detail_links::track_artist_route;
use super::grid_cells::{
    AlbumGridCell, ArtistGridCell, CollectionGridProjection, FixedPageCollectionRow,
    PlaylistGridCell, SmartPlaylistGridCell, TrackGridCell, collection_grid,
    collection_grid_with_minimum_card_width, fixed_page_collection_row,
};
use super::library_fields::{
    ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH, COLLECTION_GRID_CARD_GAP, COLLECTION_GRID_MIN_CARD_WIDTH,
    clear_list_item_child, column_width, compact_header_column_width, grid_label_with_label,
    item_at, item_at_from_item, track_model_item,
};
use super::play_context::LoadedTrackPlayContext;
use super::route::Route;
use super::route_layout::{PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START};
use super::table_sizing::{
    ColumnViewWidthFit, column_view_initial_width, install_column_view_width_fit,
    route_column_view_initial_width,
};

pub(super) const SMART_PLAYLIST_REORDER_WIDTH: i32 = 30;
pub(super) const LIBRARY_TABLE_HEADER_HEIGHT: i32 = 92;
const LIBRARY_TABLE_ROW_HEIGHT: i32 = 58;
// GtkColumnView allocates a 30px header and 64px for each current track row.
const COMPACT_TRACK_TABLE_MAX_VISIBLE_ROWS: usize = 4;
pub(super) const COMPACT_TRACK_TABLE_HEADER_HEIGHT: i32 = 30;
const COMPACT_TRACK_TABLE_ROW_HEIGHT: i32 = 64;
const HOME_GRID_FIELD_COUNT: usize = 2;
const HOME_ALBUM_GRID_FIELDS: [LibraryField; HOME_GRID_FIELD_COUNT] =
    [LibraryField::AlbumArtist, LibraryField::Year];
const HOME_TRACK_GRID_FIELDS: [LibraryField; HOME_GRID_FIELD_COUNT] =
    [LibraryField::Artist, LibraryField::Album];

type TrackModelIndexListener = Rc<dyn Fn(&HashMap<TrackId, u32>) -> bool>;

struct TrackModelIndexInner {
    positions: RefCell<HashMap<TrackId, u32>>,
    listeners: RefCell<Vec<TrackModelIndexListener>>,
    patching: Cell<bool>,
}

#[derive(Clone)]
pub(crate) struct TrackModelIndex {
    model: glib::WeakRef<gio::ListStore>,
    inner: Rc<TrackModelIndexInner>,
}

#[derive(Clone)]
pub(crate) struct TrackTableSelection {
    selection: glib::WeakRef<gtk::SingleSelection>,
    positions: TrackModelIndex,
    selected_track_id: Rc<RefCell<Option<TrackId>>>,
    selected_position: Rc<Cell<u32>>,
}

pub(crate) type TrackTableSelectionHandle = Rc<RefCell<Option<TrackTableSelection>>>;

#[derive(Clone)]
pub(crate) struct CollectionTableProjection {
    table: gtk::ColumnView,
    fixed_columns: Rc<Vec<(gtk::ColumnViewColumn, i32)>>,
    column_for_field: Rc<dyn Fn(LibraryField) -> (gtk::ColumnViewColumn, i32)>,
    fields: Rc<RefCell<Vec<LibraryField>>>,
    width_fit: ColumnViewWidthFit,
}

impl CollectionTableProjection {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.table.clone().upcast()
    }

    pub(crate) fn apply_fields(&self, fields: &[LibraryField]) {
        if self.fields.borrow().as_slice() == fields {
            return;
        }
        while let Some(column) = self
            .table
            .columns()
            .item(0)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        {
            self.table.remove_column(&column);
        }
        let mut active = self.fixed_columns.as_ref().clone();
        for (column, _) in self.fixed_columns.iter() {
            self.table.append_column(column);
        }
        for field in fields {
            let (column, width) = (self.column_for_field)(*field);
            self.table.append_column(&column);
            active.push((column, width));
        }
        *self.fields.borrow_mut() = fields.to_vec();
        self.width_fit.replace(active);
    }

    pub(crate) fn fit_scroller_allocation(&self, scroller: &gtk::ScrolledWindow, width: i32) {
        self.width_fit.fit_scroller_allocation(scroller, width);
    }
}

#[derive(Clone)]
pub(super) enum LibraryPresentationProjection {
    Row(CollectionTableProjection),
    Grid(CollectionGridProjection),
    AlbumDetail(AlbumDetailVirtualList),
}

impl LibraryPresentationProjection {
    fn widget(&self) -> gtk::Widget {
        match self {
            Self::Row(table) => table.widget(),
            Self::Grid(grid) => grid.widget(),
            Self::AlbumDetail(detail) => detail.widget(),
        }
    }

    fn apply_fields(&self, settings: &crate::LibraryListSettings) {
        match self {
            Self::Row(table) => table.apply_fields(&settings.row_fields),
            Self::Grid(grid) => grid.apply_fields(&settings.grid_fields),
            Self::AlbumDetail(_) => {}
        }
    }

    fn attach_scroller(&self, scroller: &gtk::ScrolledWindow) {
        if let Self::AlbumDetail(detail) = self {
            detail.attach_scroller(scroller);
        }
    }

    fn fit_scroller_allocation(&self, scroller: &gtk::ScrolledWindow, width: i32) {
        match self {
            Self::Row(table) => table.width_fit.fit_scroller_allocation(scroller, width),
            Self::Grid(grid) => grid.fit_allocation(width),
            Self::AlbumDetail(detail) => detail.fit_allocation(width),
        }
    }
}

#[derive(Clone)]
pub(crate) struct LibraryCollectionProjection {
    host: Rc<RefCell<LibraryCollectionHost>>,
    settings: Rc<RefCell<crate::LibraryListSettings>>,
    presentation: Rc<RefCell<LibraryPresentationProjection>>,
    build: Rc<dyn Fn(LibraryLayout) -> LibraryPresentationProjection>,
}

#[derive(Clone)]
enum LibraryCollectionHost {
    Embedded(gtk::Box),
    Scrolled {
        scroller: gtk::ScrolledWindow,
        margin_start: i32,
        margin_end: i32,
        width_owner: gtk::Widget,
    },
}

impl LibraryCollectionProjection {
    pub(super) fn new(
        settings: crate::LibraryListSettings,
        build: Rc<dyn Fn(LibraryLayout) -> LibraryPresentationProjection>,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);
        let presentation = build(settings.layout);
        presentation.apply_fields(&settings);
        root.append(&presentation.widget());
        Self {
            host: Rc::new(RefCell::new(LibraryCollectionHost::Embedded(root))),
            settings: Rc::new(RefCell::new(settings)),
            presentation: Rc::new(RefCell::new(presentation)),
            build,
        }
    }

    pub(crate) fn scrolling_scroller(&self) -> gtk::ScrolledWindow {
        if let LibraryCollectionHost::Scrolled { scroller, .. } = &*self.host.borrow() {
            return scroller.clone();
        }

        let scroller = gtk::ScrolledWindow::new();
        configure_library_route_scroller(&scroller);
        self.mount_in_scroller(
            &scroller,
            PRIMARY_ROUTE_MARGIN_START,
            PRIMARY_ROUTE_MARGIN_END,
        );
        scroller
    }

    pub(crate) fn scrolling_widget(&self) -> gtk::Widget {
        self.scrolling_scroller();
        let LibraryCollectionHost::Scrolled { width_owner, .. } = &*self.host.borrow() else {
            unreachable!("scrolling_scroller installs the scrolled collection host");
        };
        width_owner.clone()
    }

    pub(crate) fn mount_in_scroller(
        &self,
        scroller: &gtk::ScrolledWindow,
        margin_start: i32,
        margin_end: i32,
    ) -> gtk::Widget {
        let active = self.presentation.borrow().widget();
        let previous_host = self.host.borrow().clone();
        match previous_host {
            LibraryCollectionHost::Embedded(root) => {
                if active.parent().as_ref() == Some(root.upcast_ref()) {
                    root.remove(&active);
                }
            }
            LibraryCollectionHost::Scrolled {
                scroller: previous, ..
            } => previous.set_child(None::<&gtk::Widget>),
        }
        active.set_margin_start(margin_start);
        active.set_margin_end(margin_end);
        scroller.set_child(Some(&active));
        self.presentation.borrow().attach_scroller(scroller);
        let presentation = Rc::clone(&self.presentation);
        let resize_scroller = scroller.clone();
        let width_owner = width_allocation_owner(scroller, move |width| {
            presentation
                .borrow()
                .fit_scroller_allocation(&resize_scroller, width);
        })
        .upcast::<gtk::Widget>();
        self.host.replace(LibraryCollectionHost::Scrolled {
            scroller: scroller.clone(),
            margin_start,
            margin_end,
            width_owner: width_owner.clone(),
        });
        width_owner
    }

    pub(crate) fn apply_settings(&self, settings: &crate::LibraryListSettings) {
        let previous = self.settings.borrow().clone();
        let detail_fields_changed = previous.detail_track_fields != settings.detail_track_fields;
        let rebuild = previous.layout != settings.layout
            || (detail_fields_changed && settings.layout == LibraryLayout::Detail);
        if rebuild {
            let presentation = (self.build)(settings.layout);
            presentation.apply_fields(settings);
            let widget = presentation.widget();
            let host = self.host.borrow().clone();
            match host {
                LibraryCollectionHost::Embedded(root) => {
                    while let Some(child) = root.first_child() {
                        root.remove(&child);
                    }
                    widget.set_margin_start(0);
                    widget.set_margin_end(0);
                    root.append(&widget);
                }
                LibraryCollectionHost::Scrolled {
                    scroller,
                    margin_start,
                    margin_end,
                    ..
                } => {
                    widget.set_margin_start(margin_start);
                    widget.set_margin_end(margin_end);
                    scroller.set_child(Some(&widget));
                    presentation.attach_scroller(&scroller);
                }
            }
            self.presentation.replace(presentation);
        } else {
            self.presentation.borrow().apply_fields(settings);
        }
        *self.settings.borrow_mut() = settings.clone();
    }
}

impl TrackModelIndex {
    pub(crate) fn new(model: &gio::ListStore) -> Self {
        let inner = Rc::new(TrackModelIndexInner {
            positions: RefCell::new(track_position_index(model)),
            listeners: RefCell::new(Vec::new()),
            patching: Cell::new(false),
        });
        let weak_inner = Rc::downgrade(&inner);
        model.connect_items_changed(move |model, _, _, _| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            if inner.patching.get() {
                return;
            }
            inner.positions.replace(track_position_index(model));
            notify_track_index_listeners(&inner);
        });
        Self {
            model: model.downgrade(),
            inner,
        }
    }

    fn connect_changed(&self, listener: TrackModelIndexListener) {
        self.inner.listeners.borrow_mut().push(listener);
    }

    fn position(&self, track_id: &TrackId) -> Option<u32> {
        self.inner.positions.borrow().get(track_id).copied()
    }

    pub(crate) fn replace_existing(&self, replacements: impl IntoIterator<Item = Track>) {
        let Some(model) = self.model.upgrade() else {
            return;
        };
        let positions = self.inner.positions.borrow();
        let mut rows = replacements
            .into_iter()
            .filter_map(|track| {
                positions
                    .get(&track.id)
                    .copied()
                    .map(|position| (position, track))
            })
            .collect::<Vec<_>>();
        drop(positions);
        if rows.is_empty() {
            return;
        }
        rows.sort_unstable_by_key(|(position, _)| *position);

        self.inner.patching.set(true);
        let mut start = 0;
        while start < rows.len() {
            let mut end = start + 1;
            while end < rows.len() && rows[end].0 == rows[end - 1].0 + 1 {
                end += 1;
            }
            let additions = rows[start..end]
                .iter()
                .map(|(_, track)| track_model_item(track.clone()))
                .collect::<Vec<_>>();
            model.splice(rows[start].0, additions.len() as u32, &additions);
            start = end;
        }
        self.inner.patching.set(false);
        notify_track_index_listeners(&self.inner);
    }

    fn is_bound(&self) -> bool {
        self.model.upgrade().is_some()
    }
}

fn notify_track_index_listeners(inner: &TrackModelIndexInner) {
    let positions = inner.positions.borrow();
    inner
        .listeners
        .borrow_mut()
        .retain(|listener| listener(&positions));
}

impl TrackTableSelection {
    pub(crate) fn new(selection: &gtk::SingleSelection, positions: TrackModelIndex) -> Self {
        selection.set_selected(gtk::INVALID_LIST_POSITION);
        let selected_track_id = Rc::new(RefCell::new(None::<TrackId>));
        let selected_position = Rc::new(Cell::new(gtk::INVALID_LIST_POSITION));
        {
            let selected_track_id = Rc::clone(&selected_track_id);
            let selected_position = Rc::clone(&selected_position);
            let selection = selection.downgrade();
            positions.connect_changed(Rc::new(move |positions| {
                let Some(selection) = selection.upgrade() else {
                    return false;
                };
                let position = selected_track_id
                    .borrow()
                    .as_ref()
                    .and_then(|track_id| positions.get(track_id).copied())
                    .unwrap_or(gtk::INVALID_LIST_POSITION);
                selected_position.set(position);
                if selection.selected() != position {
                    selection.set_selected(position);
                }
                true
            }));
        }
        Self {
            selection: selection.downgrade(),
            positions,
            selected_track_id,
            selected_position,
        }
    }

    pub(crate) fn install_guard(&self) {
        let Some(selection) = self.selection.upgrade() else {
            return;
        };
        let selected_position = Rc::clone(&self.selected_position);
        selection.connect_selection_changed(move |selection, _, _| {
            let selected = selected_position.get();
            if selection.selected() != selected {
                selection.set_selected(selected);
            }
        });
    }

    pub(crate) fn select(&self, position: u32) {
        self.selected_position.set(position);
        if let Some(selection) = self.selection.upgrade() {
            selection.set_selected(position);
        }
    }

    fn clear_now_playing(&self) {
        self.selected_track_id.borrow_mut().take();
        self.select(gtk::INVALID_LIST_POSITION);
    }

    fn select_track_id(&self, track_id: &TrackId) {
        *self.selected_track_id.borrow_mut() = Some(track_id.clone());
        let position = self
            .positions
            .position(track_id)
            .unwrap_or(gtk::INVALID_LIST_POSITION);
        self.select(position);
    }

    pub(crate) fn select_now_playing_track(&self, track_id: Option<&TrackId>) {
        if let Some(track_id) = track_id {
            self.select_track_id(track_id);
        } else {
            self.clear_now_playing();
        }
    }

    pub(crate) fn is_bound(&self) -> bool {
        self.positions.is_bound() && self.selection.upgrade().is_some()
    }
}

fn track_position_index(model: &gio::ListStore) -> HashMap<TrackId, u32> {
    let mut positions = HashMap::with_capacity(model.n_items() as usize);
    for position in 0..model.n_items() {
        let Some(track) = item_at::<Track>(model, position) else {
            continue;
        };
        positions.entry(track.id.clone()).or_insert(position);
    }
    positions
}

pub(crate) fn library_route_inset(child: gtk::Widget) -> gtk::Widget {
    child.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
    child.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
    child.set_hexpand(true);
    child.set_halign(gtk::Align::Fill);
    child
}
pub(crate) fn configure_library_route_scroller(scroller: &gtk::ScrolledWindow) {
    scroller.add_css_class("library-route-scroller");
    scroller.add_css_class("route-scroll-owner");
    configure_fill_width_clip(scroller, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(false);
    scroller.set_overlay_scrolling(true);
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
}

pub(crate) fn album_collection_projection(
    shell: &Rc<Shell>,
    models: AlbumCollectionModels,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
) -> LibraryCollectionProjection {
    let settings = shell.settings.current.borrow().library_list(key);
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(album_table(&shell, models.albums(), key))
            }
            LibraryLayout::Detail if key.supports_layout(LibraryLayout::Detail) => {
                LibraryPresentationProjection::AlbumDetail(album_detail_list(
                    &shell,
                    models.detail(),
                    key,
                    query.clone(),
                ))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => LibraryPresentationProjection::Grid(
                album_grid(&shell, models.albums(), key, query.clone()),
            ),
        }),
    )
}

pub(crate) fn artist_collection_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
) -> LibraryCollectionProjection {
    let settings = shell.settings.current.borrow().library_list(key);
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(artist_table(&shell, model.clone(), key))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => LibraryPresentationProjection::Grid(
                artist_grid(&shell, model.clone(), key, query.clone()),
            ),
        }),
    )
}
pub(crate) fn playlist_collection_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> LibraryCollectionProjection {
    let key = LibraryListKey::Playlists;
    let settings = shell.settings.current.borrow().library_list(key);
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(playlist_table(&shell, model.clone()))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => {
                LibraryPresentationProjection::Grid(playlist_grid(&shell, model.clone()))
            }
        }),
    )
}
pub(crate) fn smart_playlist_collection_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    query: ActiveLibraryQuery,
) -> LibraryCollectionProjection {
    let key = LibraryListKey::SmartPlaylists;
    let settings = shell.settings.current.borrow().library_list(key);
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => {
                LibraryPresentationProjection::Row(smart_playlist_table(&shell, model.clone()))
            }
            LibraryLayout::Grid | LibraryLayout::Detail => LibraryPresentationProjection::Grid(
                smart_playlist_grid(&shell, model.clone(), query.clone()),
            ),
        }),
    )
}
pub(crate) fn track_collection_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    settings: crate::LibraryListSettings,
    play_context: Option<LoadedTrackPlayContext>,
    content_inset: i32,
    selection_handle: Option<TrackTableSelectionHandle>,
    positions: TrackModelIndex,
) -> LibraryCollectionProjection {
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Grid => LibraryPresentationProjection::Grid(track_grid(
                &shell,
                model.clone(),
                key,
                play_context.clone(),
            )),
            LibraryLayout::Row | LibraryLayout::Detail => {
                LibraryPresentationProjection::Row(track_table(
                    &shell,
                    model.clone(),
                    key,
                    TrackTableOptions {
                        detail: false,
                        play_context: play_context.clone(),
                        content_inset,
                        selection_handle: selection_handle.clone(),
                        positions: positions.clone(),
                    },
                ))
            }
        }),
    )
}

pub(crate) struct TrackTableOptions {
    pub(crate) detail: bool,
    pub(crate) play_context: Option<LoadedTrackPlayContext>,
    pub(crate) content_inset: i32,
    pub(crate) selection_handle: Option<TrackTableSelectionHandle>,
    pub(crate) positions: TrackModelIndex,
}

pub(super) fn track_model_play_action(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    play_context: LoadedTrackPlayContext,
    position: u32,
    track: Track,
) -> Rc<dyn Fn()> {
    let controller = shell.products.playback.queue.clone();
    let model = model.clone();
    Rc::new(move || {
        play_track_from_model(
            &controller,
            &model,
            Some(&play_context),
            position,
            track.clone(),
        );
    })
}

fn play_track_from_model(
    controller: &playback::QueueHandle,
    model: &gio::ListStore,
    play_context: Option<&LoadedTrackPlayContext>,
    position: u32,
    track: Track,
) {
    let Some(play_context) = play_context else {
        controller.play_now(track);
        return;
    };
    let anchor_index = position as usize;
    let track_id = track.id;
    let lookup_model = model.clone();
    play_context.play_window(
        controller,
        model.n_items() as usize,
        anchor_index,
        move |index| {
            let candidate = item_at::<Track>(&lookup_model, index as u32)?;
            (index != anchor_index || candidate.id == track_id).then_some(candidate)
        },
    );
}

pub(crate) fn album_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
) -> CollectionGridProjection {
    let settings = shell.settings.current.borrow().library_list(key);
    let fields = settings.grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    let minimum_card_width = if key == LibraryListKey::Albums {
        ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH
    } else {
        COLLECTION_GRID_MIN_CARD_WIDTH
    };
    collection_grid_with_minimum_card_width(
        model,
        minimum_card_width,
        &fields,
        move |fields| AlbumGridCell::new(Rc::clone(&cell_shell), fields, query.clone()),
        move |_, album: Album| activate_shell.navigate(Route::AlbumDetail(album.id)),
    )
}

pub(crate) fn artist_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(key)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    collection_grid(
        model,
        &fields,
        move |fields| ArtistGridCell::new(Rc::clone(&cell_shell), fields, query.clone()),
        move |_, artist: Artist| activate_shell.navigate(Route::ArtistDetail(artist.id)),
    )
}
pub(crate) fn playlist_grid(shell: &Rc<Shell>, model: gio::ListStore) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::Playlists)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    collection_grid(
        model,
        &fields,
        move |fields| PlaylistGridCell::new(Rc::clone(&cell_shell), fields),
        move |_, playlist: Playlist| activate_shell.navigate(Route::PlaylistDetail(playlist.id)),
    )
}
pub(crate) fn smart_playlist_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    query: ActiveLibraryQuery,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::SmartPlaylists)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    collection_grid(
        model,
        &fields,
        move |fields| SmartPlaylistGridCell::new(Rc::clone(&cell_shell), fields, query.clone()),
        move |_, playlist: SmartPlaylist| {
            activate_shell.navigate(Route::SmartPlaylistDetail(playlist.id));
        },
    )
}
pub(crate) fn track_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    play_context: Option<LoadedTrackPlayContext>,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(key)
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let cell_model = model.clone();
    let cell_play_context = play_context.clone();
    let controller = shell.products.playback.queue.clone();
    let activate_model = model.clone();
    collection_grid(
        model,
        &fields,
        move |fields| {
            TrackGridCell::new(
                Rc::clone(&cell_shell),
                fields,
                cell_model.clone(),
                cell_play_context.clone(),
            )
        },
        move |position, track: Track| {
            play_track_from_model(
                &controller,
                &activate_model,
                play_context.as_ref(),
                position,
                track,
            );
        },
    )
}
pub(crate) fn home_album_row(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    columns: usize,
    query: ActiveLibraryQuery,
) -> FixedPageCollectionRow {
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    fixed_page_collection_row(
        model,
        columns,
        &HOME_ALBUM_GRID_FIELDS,
        move |fields| AlbumGridCell::new(Rc::clone(&cell_shell), fields, query.clone()),
        move |_, album: Album| activate_shell.navigate(Route::AlbumDetail(album.id)),
    )
}
pub(crate) fn home_track_row(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    columns: usize,
) -> FixedPageCollectionRow {
    let cell_shell = Rc::clone(shell);
    let cell_model = model.clone();
    let controller = shell.products.playback.queue.clone();
    fixed_page_collection_row(
        model,
        columns,
        &HOME_TRACK_GRID_FIELDS,
        move |fields| TrackGridCell::new(Rc::clone(&cell_shell), fields, cell_model.clone(), None),
        move |_, track: Track| {
            controller.play_now(track);
        },
    )
}
pub(crate) fn album_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> CollectionTableProjection {
    let fields = shell.settings.current.borrow().library_list(key).row_fields;
    let activate_shell = Rc::clone(shell);
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        model,
        &fields,
        Vec::new(),
        move |field| album_column(&column_shell, field),
        |field| column_fit_width(field, column_width(field)),
        true,
        Some(Box::new(move |_, album: Album| {
            activate_shell.navigate(Route::AlbumDetail(album.id));
        })),
        None,
        route_column_view_initial_width(shell),
    )
}
pub(crate) fn artist_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
) -> CollectionTableProjection {
    let fields = shell.settings.current.borrow().library_list(key).row_fields;
    let activate_shell = Rc::clone(shell);
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        model,
        &fields,
        Vec::new(),
        move |field| artist_column(&column_shell, field),
        |field| column_fit_width(field, column_width(field)),
        true,
        Some(Box::new(move |_, artist: Artist| {
            activate_shell.navigate(Route::ArtistDetail(artist.id));
        })),
        None,
        route_column_view_initial_width(shell),
    )
}
pub(crate) fn playlist_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> CollectionTableProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::Playlists)
        .row_fields;
    let activate_shell = Rc::clone(shell);
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        model,
        &fields,
        Vec::new(),
        move |field| playlist_column(&column_shell, field),
        |field| column_fit_width(field, playlist_column_width(field)),
        true,
        Some(Box::new(move |_, playlist: Playlist| {
            activate_shell.navigate(Route::PlaylistDetail(playlist.id));
        })),
        None,
        route_column_view_initial_width(shell),
    )
}
pub(crate) fn smart_playlist_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
) -> CollectionTableProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(LibraryListKey::SmartPlaylists)
        .row_fields;
    let reorder_column = smart_playlist_reorder_column(shell);
    let activate_shell = Rc::clone(shell);
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        model,
        &fields,
        vec![(reorder_column, SMART_PLAYLIST_REORDER_WIDTH)],
        move |field| smart_playlist_column(&column_shell, field),
        |field| column_fit_width(field, playlist_column_width(field)),
        true,
        Some(Box::new(move |_, playlist: SmartPlaylist| {
            activate_shell.navigate(Route::SmartPlaylistDetail(playlist.id));
        })),
        None,
        route_column_view_initial_width(shell),
    )
}

fn playlist_column_width(field: LibraryField) -> i32 {
    if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
        220
    } else {
        collection_column_width(field)
    }
}

pub(super) fn collection_column_width(field: LibraryField) -> i32 {
    match field {
        LibraryField::AlbumCount | LibraryField::SongCount => {
            compact_header_column_width(field.title(), 96)
        }
        LibraryField::Duration => compact_header_column_width(field.title(), 128),
        _ => column_width(field),
    }
}

pub(super) fn dynamic_collection_table<T>(
    model: gio::ListStore,
    fields: &[LibraryField],
    fixed_columns: Vec<(gtk::ColumnViewColumn, i32)>,
    column_for_field: impl Fn(LibraryField) -> gtk::ColumnViewColumn + 'static,
    width_for_field: impl Fn(LibraryField) -> i32 + 'static,
    single_click_activate: bool,
    activate: Option<Box<dyn Fn(u32, T)>>,
    selection: Option<gtk::SelectionModel>,
    initial_width: i32,
) -> CollectionTableProjection
where
    T: Clone + 'static,
{
    let column_for_field = Rc::new(move |field| {
        let column = column_for_field(field);
        let width = width_for_field(field);
        (column, width)
    }) as Rc<dyn Fn(LibraryField) -> (gtk::ColumnViewColumn, i32)>;
    let mut active = fixed_columns.clone();
    active.extend(fields.iter().map(|field| column_for_field(*field)));
    let (table, width_fit) = collection_table_with_width(
        model,
        active,
        initial_width,
        single_click_activate,
        activate,
        selection,
    );
    CollectionTableProjection {
        table,
        fixed_columns: Rc::new(fixed_columns),
        column_for_field,
        fields: Rc::new(RefCell::new(fields.to_vec())),
        width_fit,
    }
}

fn collection_table_with_width<T>(
    model: gio::ListStore,
    columns: Vec<(gtk::ColumnViewColumn, i32)>,
    initial_width: i32,
    single_click_activate: bool,
    activate: Option<Box<dyn Fn(u32, T)>>,
    selection: Option<gtk::SelectionModel>,
) -> (gtk::ColumnView, ColumnViewWidthFit)
where
    T: Clone + 'static,
{
    let selection =
        selection.unwrap_or_else(|| gtk::NoSelection::new(Some(model.clone())).upcast());
    let table = gtk::ColumnView::new(Some(selection));
    table.add_css_class("track-table");
    if single_click_activate {
        table.set_single_click_activate(true);
    }
    table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
    table.set_hexpand(true);
    table.set_vexpand(true);
    for (column, _) in &columns {
        table.append_column(column);
    }
    let width_fit = install_column_view_width_fit(&table, columns, initial_width);
    if let Some(activate) = activate {
        table.connect_activate(move |_, position| {
            if let Some(value) = item_at::<T>(&model, position) {
                activate(position, value);
            }
        });
    }
    (table, width_fit)
}

pub(crate) fn smart_playlist_reorder_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(playlist) = item_at_from_item::<SmartPlaylist>(item) else {
            return;
        };
        let handle = smart_playlist_drag_handle(&playlist.id);
        install_smart_playlist_drop_target(&handle, &shell, &playlist.id);
        item.set_child(Some(&handle));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = gtk::ColumnViewColumn::new(None::<&str>, Some(factory));
    column.set_fixed_width(SMART_PLAYLIST_REORDER_WIDTH);
    column
}
pub(crate) fn track_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    options: TrackTableOptions,
) -> CollectionTableProjection {
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let track_selection = TrackTableSelection::new(&selection, options.positions);
    if let Some(selection_handle) = options.selection_handle.as_ref() {
        *selection_handle.borrow_mut() = Some(track_selection.clone());
    }
    let source_id = shell
        .library
        .query
        .borrow()
        .as_ref()
        .map(|query| query.source_id().clone());
    let current_track_selection = track_selection.clone();
    shell.register_current_route_track_selection(Rc::new(move |current| {
        if !current_track_selection.is_bound() {
            return false;
        }
        let track_id = current
            .filter(|current| source_id.as_ref() == Some(&current.source_id))
            .map(|current: &RouteCurrentTrack| &current.track_id);
        current_track_selection.select_now_playing_track(track_id);
        true
    }));
    let fields = if options.detail {
        shell
            .settings
            .current
            .borrow()
            .library_list(key)
            .detail_track_fields
    } else {
        shell.settings.current.borrow().library_list(key).row_fields
    };
    let controller = shell.products.playback.queue.clone();
    let activate_model = model.clone();
    let play_context = options.play_context.clone();
    let activate = Box::new(move |position, track: Track| {
        play_track_from_model(
            &controller,
            &activate_model,
            play_context.as_ref(),
            position,
            track,
        );
    });
    let column_shell = Rc::clone(shell);
    let table = dynamic_collection_table(
        model,
        &fields,
        Vec::new(),
        move |field| track_column_for_key(&column_shell, key, field),
        move |field| track_column_fit_width(key, field),
        false,
        Some(activate),
        Some(selection.upcast()),
        column_view_initial_width(shell, options.content_inset),
    );
    table.table.add_css_class("track-list");
    track_selection.install_guard();
    table
}
pub(crate) fn set_library_table_content_height(
    scroller: &gtk::ScrolledWindow,
    row_count: usize,
    max_visible_rows: Option<usize>,
) {
    let height = max_visible_rows.map_or_else(
        || library_table_content_height(row_count),
        |max_visible_rows| capped_library_table_content_height(row_count, Some(max_visible_rows)),
    );
    scroller.set_min_content_height(height);
    scroller.set_max_content_height(height);
}
pub(crate) fn configure_compact_track_table_scroller(
    scroller: &gtk::ScrolledWindow,
    row_count: usize,
) {
    let visible_rows = row_count.min(COMPACT_TRACK_TABLE_MAX_VISIBLE_ROWS);
    let height =
        COMPACT_TRACK_TABLE_HEADER_HEIGHT + visible_rows as i32 * COMPACT_TRACK_TABLE_ROW_HEIGHT;
    scroller.set_min_content_height(height);
    scroller.set_max_content_height(height);
    scroller.set_propagate_natural_height(false);
}
pub(crate) fn library_table_content_height(row_count: usize) -> i32 {
    capped_library_table_content_height(row_count, None)
}
pub(crate) fn capped_library_table_content_height(
    row_count: usize,
    max_visible_rows: Option<usize>,
) -> i32 {
    let max_rows = ((i32::MAX - LIBRARY_TABLE_HEADER_HEIGHT) / LIBRARY_TABLE_ROW_HEIGHT) as usize;
    let visible_rows = row_count.max(1).min(max_visible_rows.unwrap_or(max_rows));
    LIBRARY_TABLE_HEADER_HEIGHT + visible_rows as i32 * LIBRARY_TABLE_ROW_HEIGHT
}
pub(crate) fn smart_playlist_drag_handle(playlist_id: &SmartPlaylistId) -> gtk::Image {
    let drag = gtk::Image::from_icon_name("rufin-list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(SMART_PLAYLIST_REORDER_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let drag_id = playlist_id.as_str().to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&drag_id.to_value()))
    });
    drag.add_controller(source);
    drag
}

pub(crate) fn install_smart_playlist_drop_target(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    target_id: &SmartPlaylistId,
) {
    let widget = target.as_ref().downgrade();
    let library = shell.products.library.clone();
    let target_id = target_id.clone();
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(dragged_id) = value.get::<String>() else {
            return false;
        };
        let dragged_id = SmartPlaylistId::new(dragged_id);
        if dragged_id == target_id {
            return false;
        }
        let Some(widget) = widget.upgrade() else {
            return false;
        };
        let after = y > f64::from(widget.height()) / 2.0;
        library.move_smart_playlist(dragged_id, target_id.clone(), after);
        true
    });
    target.add_controller(drop_target);
}
fn collection_grid_field_class(field: LibraryField) -> &'static str {
    match field {
        LibraryField::Artist | LibraryField::AlbumArtist => "artist-label",
        _ => "muted",
    }
}

pub(super) fn collection_grid_field_label(
    value: &str,
    field: LibraryField,
) -> (gtk::Widget, gtk::Label) {
    grid_label_with_label(value, collection_grid_field_class(field))
}

pub(super) fn track_grid_field_route(track: &Track, field: LibraryField) -> Option<Route> {
    match field {
        LibraryField::Artist => track_artist_route(track),
        LibraryField::AlbumArtist => track_album_artist_route(track),
        LibraryField::Album => Some(Route::AlbumDetail(track.album_id.clone())),
        _ => None,
    }
}

fn track_album_artist_route(track: &Track) -> Option<Route> {
    track
        .album_artist_credits
        .first()
        .map(|artist| Route::ArtistDetail(artist.id.clone()))
}

pub(super) fn collection_grid_card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, COLLECTION_GRID_CARD_GAP);
    card.set_hexpand(true);
    card.set_vexpand(false);
    card.set_halign(gtk::Align::Fill);
    card.set_valign(gtk::Align::Start);
    card
}

#[cfg(test)]
mod scroll_host_tests {
    use super::super::grid_cells::ReusableCollectionGridCell;
    use super::super::library_fields::{
        ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH, COLLECTION_GRID_MIN_CARD_WIDTH,
    };
    use super::super::route_layout::{HOME_ALBUM_GAP, home_album_page_size};
    use super::*;
    use crate::layout::width_allocation_owner;

    struct FixedRowCell {
        card: gtk::Box,
    }

    impl FixedRowCell {
        fn new() -> Self {
            let card = collection_grid_card();
            let cover_child = gtk::Box::new(gtk::Orientation::Vertical, 0);
            cover_child.set_hexpand(true);
            cover_child.set_vexpand(true);
            card.append(&super::super::cards::square_cover_frame(&cover_child));
            for class in ["track-title", "artist-label", "muted"] {
                card.append(&grid_label_with_label(class, class).0);
            }
            Self { card }
        }
    }

    impl ReusableCollectionGridCell<u8> for FixedRowCell {
        fn widget(&self) -> gtk::Widget {
            self.card.clone().upcast()
        }

        fn bind(&self, _: u32, _: u8) {}

        fn clear(&self) {}

        fn apply_fields(&self, _: &[LibraryField]) {}
    }

    fn test_row_projection(
        table: gtk::ColumnView,
        columns: Vec<(gtk::ColumnViewColumn, i32)>,
    ) -> LibraryPresentationProjection {
        let width_fit = install_column_view_width_fit(&table, columns.clone(), 600);
        LibraryPresentationProjection::Row(CollectionTableProjection {
            table,
            fixed_columns: Rc::new(columns),
            column_for_field: Rc::new(|field| {
                (
                    gtk::ColumnViewColumn::new(Some(field.title()), None::<gtk::ListItemFactory>),
                    80,
                )
            }),
            fields: Rc::new(RefCell::new(Vec::new())),
            width_fit,
        })
    }

    #[test]
    fn collection_hosts_keep_native_layout_contracts() {
        gtk::init().expect("initialize GTK");
        crate::application::style::install_css();
        let home_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        home_model.append(&glib::BoxedAnyObject::new(7_u8));
        let activated = Rc::new(Cell::new(None::<u8>));
        let activated_from_row = Rc::clone(&activated);
        let home_row = fixed_page_collection_row(
            home_model.clone(),
            3,
            &[],
            |_| FixedRowCell::new(),
            move |_, value: u8| activated_from_row.set(Some(value)),
        );
        let home_row_widget = home_row.widget();
        let stable_row = home_row_widget.as_ptr();
        let natural = home_row_widget.measure(gtk::Orientation::Vertical, -1).0;
        let allocated_width = home_row_widget.measure(gtk::Orientation::Vertical, 600).0;
        assert_eq!(
            home_row_widget.request_mode(),
            gtk::SizeRequestMode::HeightForWidth
        );
        assert!(allocated_width > natural);
        assert_eq!(home_row_widget.height_request(), -1);
        let first = home_row_widget
            .first_child()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("first bound Home card");
        let second = first
            .next_sibling()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("first empty Home slot");
        let third = second
            .next_sibling()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("second empty Home slot");
        assert!(third.next_sibling().is_none());
        let home_height = home_row_widget.measure(gtk::Orientation::Vertical, 600).1;
        home_row_widget.allocate(600, home_height, -1, None);
        let slot_widths = [first.width(), second.width(), third.width()];
        let smallest_slot = *slot_widths.iter().min().expect("Home slot width");
        let largest_slot = *slot_widths.iter().max().expect("Home slot width");
        assert!(largest_slot - smallest_slot <= 1);
        assert_eq!(slot_widths.iter().sum::<i32>() + HOME_ALBUM_GAP * 2, 600);

        for value in [8_u8, 9_u8] {
            home_model.append(&glib::BoxedAnyObject::new(value));
        }
        let first = home_row_widget
            .first_child()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("first filled Home card");
        let second = first
            .next_sibling()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("second filled Home card");
        let third = second
            .next_sibling()
            .and_then(|child| child.downcast::<gtk::FlowBoxChild>().ok())
            .expect("third filled Home card");
        home_row_widget.allocate(600, home_height, -1, None);
        assert_eq!([first.width(), second.width(), third.width()], slot_widths);
        let first_card = first.first_child().expect("first Home card body");
        let second_card = second.first_child().expect("second Home card body");
        let first_bounds = first_card
            .compute_bounds(&home_row_widget)
            .expect("first Home card bounds");
        let second_bounds = second_card
            .compute_bounds(&home_row_widget)
            .expect("second Home card bounds");
        assert_eq!(
            (second_bounds.x() - first_bounds.x() - first_bounds.width()).round() as i32,
            HOME_ALBUM_GAP
        );
        home_row_widget.emit_by_name::<()>("child-activated", &[&second]);
        assert_eq!(activated.get(), Some(8));
        home_row.set_page_size(2);
        assert_eq!(home_row.widget().as_ptr(), stable_row);
        assert_eq!(home_row.widget().min_children_per_line(), 2);
        assert_eq!(home_row.widget().max_children_per_line(), 2);

        let generic_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let generic_inset = super::super::cards::collection_grid_card_inset(
            &generic_body,
            COLLECTION_GRID_MIN_CARD_WIDTH,
        );
        let album_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let album_inset = super::super::cards::collection_grid_card_inset(
            &album_body,
            ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH,
        );
        let generic_minimum = generic_inset.measure(gtk::Orientation::Horizontal, -1).0;
        let album_minimum = album_inset.measure(gtk::Orientation::Horizontal, -1).0;
        assert_eq!(
            album_minimum - generic_minimum,
            ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH - COLLECTION_GRID_MIN_CARD_WIDTH
        );
        generic_inset.allocate(generic_minimum, generic_minimum, -1, None);
        assert_eq!(generic_body.width(), COLLECTION_GRID_MIN_CARD_WIDTH);

        let generic_projection = collection_grid(
            gio::ListStore::new::<glib::BoxedAnyObject>(),
            &[],
            |_| FixedRowCell::new(),
            |_, _: u8| {},
        );
        generic_projection.fit_allocation(600);
        let generic_grid = generic_projection
            .widget()
            .downcast::<gtk::GridView>()
            .expect("generic virtualized collection grid");
        let album_projection = collection_grid_with_minimum_card_width(
            gio::ListStore::new::<glib::BoxedAnyObject>(),
            ALBUM_COLLECTION_GRID_MIN_CARD_WIDTH,
            &[],
            |_| FixedRowCell::new(),
            |_, _: u8| {},
        );
        album_projection.fit_allocation(600);
        let album_grid = album_projection
            .widget()
            .downcast::<gtk::GridView>()
            .expect("Album virtualized collection grid");
        assert_eq!(generic_grid.min_columns(), 1);
        assert_eq!(generic_grid.max_columns(), 3);
        assert_eq!(album_grid.min_columns(), 1);
        assert_eq!(album_grid.max_columns(), 2);

        let cover_child = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let cover = super::super::cards::square_cover_frame(&cover_child);
        cover_child.set_visible(true);
        cover.set_visible(true);
        assert_eq!(cover.measure(gtk::Orientation::Vertical, 360).0, 360);
        assert_eq!(cover.measure(gtk::Orientation::Vertical, 180).0, 180);
        assert_eq!(cover.height_request(), -1);

        let builds = Rc::new(Cell::new(0));
        let count_builds = Rc::clone(&builds);
        let mut settings = crate::LibraryListSettings {
            layout: LibraryLayout::Row,
            ..crate::LibraryListSettings::for_key(LibraryListKey::Albums)
        };
        let projection = LibraryCollectionProjection::new(
            settings.clone(),
            Rc::new(move |layout| {
                count_builds.set(count_builds.get() + 1);
                match layout {
                    LibraryLayout::Grid => LibraryPresentationProjection::Grid(collection_grid(
                        gio::ListStore::new::<glib::BoxedAnyObject>(),
                        &[],
                        |_| FixedRowCell::new(),
                        |_, _: u8| {},
                    )),
                    LibraryLayout::Row | LibraryLayout::Detail => test_row_projection(
                        gtk::ColumnView::new(None::<gtk::SelectionModel>),
                        Vec::new(),
                    ),
                }
            }),
        );
        assert_eq!(builds.get(), 1);

        let scroller = projection.scrolling_scroller();
        let row_child = scroller.child().expect("row presentation");
        assert!(row_child.is::<gtk::ColumnView>());
        assert_eq!(row_child.parent(), Some(scroller.clone().upcast()));
        assert_eq!(row_child.margin_start(), PRIMARY_ROUTE_MARGIN_START);
        assert_eq!(row_child.margin_end(), PRIMARY_ROUTE_MARGIN_END);
        let scrolling_surface = projection.scrolling_widget();
        assert_eq!(scroller.parent(), Some(scrolling_surface.clone()));
        assert!(scrolling_surface.vexpands());

        projection.apply_settings(&settings);
        assert_eq!(
            scroller.child().expect("retained row presentation"),
            row_child
        );
        assert_eq!(builds.get(), 1);
        assert_eq!(projection.scrolling_scroller(), scroller);

        settings.layout = LibraryLayout::Grid;
        projection.apply_settings(&settings);
        let grid_child = scroller.child().expect("grid presentation");
        assert!(grid_child.is::<gtk::GridView>());
        assert_eq!(grid_child.parent(), Some(scroller.clone().upcast()));
        assert!(row_child.parent().is_none());
        assert_eq!(builds.get(), 2);
        let grid = grid_child
            .clone()
            .downcast::<gtk::GridView>()
            .expect("grid view");
        let selection = grid.model().expect("grid selection");
        let factory = grid.factory().expect("grid factory");
        let grid_weak = grid.downgrade();
        let selection_weak = selection.downgrade();
        let factory_weak = factory.downgrade();

        settings.layout = LibraryLayout::Row;
        projection.apply_settings(&settings);
        let replacement_row = scroller.child().expect("rebuilt row presentation");
        assert!(replacement_row.is::<gtk::ColumnView>());
        assert_ne!(replacement_row, row_child);
        assert!(grid_child.parent().is_none());
        assert_eq!(builds.get(), 3);
        drop(factory);
        drop(selection);
        drop(grid);
        drop(grid_child);
        while glib::MainContext::default().iteration(false) {}
        assert!(grid_weak.upgrade().is_none());
        assert!(selection_weak.upgrade().is_none());
        assert!(factory_weak.upgrade().is_none());
    }

    #[test]
    fn compact_track_host_shows_up_to_four_complete_rows() {
        gtk::init().expect("initialize GTK");
        crate::application::style::install_css();
        let mut four_row_height = None;
        for row_count in [1_usize, 2, 3, 4, 5] {
            let model = gio::ListStore::new::<glib::BoxedAnyObject>();
            for value in 0..row_count {
                model.append(&glib::BoxedAnyObject::new(value));
            }
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(|_, item| {
                let item = item
                    .downcast_ref::<gtk::ListItem>()
                    .expect("column list item");
                let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                cell.set_height_request(48);
                item.set_child(Some(&cell));
            });
            let selection = gtk::NoSelection::new(Some(model));
            let table = gtk::ColumnView::new(Some(selection));
            table.add_css_class("track-table");
            table.add_css_class("track-list");
            let column = gtk::ColumnViewColumn::new(Some("Title"), Some(factory));
            table.append_column(&column);
            table.set_vscroll_policy(gtk::ScrollablePolicy::Minimum);
            table.set_hexpand(true);
            table.set_vexpand(true);
            let presentation_table = table.clone();
            let projection = LibraryCollectionProjection::new(
                crate::LibraryListSettings::for_key(LibraryListKey::ArtistTracks),
                Rc::new(move |_| {
                    test_row_projection(presentation_table.clone(), vec![(column.clone(), 220)])
                }),
            );
            let scroller = gtk::ScrolledWindow::new();
            configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
            configure_compact_track_table_scroller(&scroller, row_count);
            let host = projection.mount_in_scroller(&scroller, 0, 0);
            let next_heading = gtk::Label::new(Some("Albums"));
            let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
            content.append(&host);
            content.append(&next_heading);
            let window = gtk::Window::new();
            window.set_child(Some(&content));
            gtk::prelude::WidgetExt::realize(&window);
            let width = 600;
            let content_height = content.measure(gtk::Orientation::Vertical, width).1;
            content.allocate(width, content_height, -1, None);
            while glib::MainContext::default().pending() {
                glib::MainContext::default().iteration(false);
            }
            let table_height = table.measure(gtk::Orientation::Vertical, width).1;
            let host_bounds = host
                .compute_bounds(&content)
                .expect("compact track host bounds");
            let heading_bounds = next_heading
                .compute_bounds(&content)
                .expect("following heading bounds");
            let host_height = host_bounds.height().round() as i32;
            if row_count < 4 {
                assert!(host_height >= table_height);
            } else if row_count == 4 {
                assert!(host_height >= table_height);
                four_row_height = Some(host_height);
            } else {
                assert_eq!(
                    host_height,
                    four_row_height.expect("four-row compact height")
                );
            }
            assert_eq!(
                (heading_bounds.y() - host_bounds.y()).round() as i32,
                host_height + 18
            );
        }
    }

    #[test]
    fn home_row_host_measures_the_columns_it_will_allocate() {
        gtk::init().expect("initialize GTK");
        crate::application::style::install_css();
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        for value in 0_u8..3 {
            model.append(&glib::BoxedAnyObject::new(value));
        }
        let row = fixed_page_collection_row(model, 2, &[], |_| FixedRowCell::new(), |_, _: u8| {});
        let stack = gtk::Stack::new();
        stack.add_named(&row.widget(), Some("row"));
        let current_columns = Rc::new(Cell::new(2_usize));
        let fit_columns = Rc::clone(&current_columns);
        let fit_row = row.clone();
        let host = width_allocation_owner(&stack, move |width| {
            let columns = home_album_page_size(width, Some(fit_columns.get()));
            fit_columns.set(columns);
            fit_row.set_page_size(columns);
        });

        let wide_height = host.measure(gtk::Orientation::Vertical, 600).1;
        assert_eq!(current_columns.get(), 3);
        assert_eq!(row.widget().min_children_per_line(), 3);
        assert_eq!(
            wide_height,
            row.widget().measure(gtk::Orientation::Vertical, 600).1
        );

        let narrow_height = host.measure(gtk::Orientation::Vertical, 450).1;
        assert_eq!(current_columns.get(), 2);
        assert_eq!(row.widget().min_children_per_line(), 2);
        assert_eq!(
            narrow_height,
            row.widget().measure(gtk::Orientation::Vertical, 450).1
        );
    }
}
