use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use ::library::AlbumSummary;
use adw::prelude::*;
use gtk::{gio, glib};

use crate::layout::width_allocation_owner;
use crate::localization::{bind_search_placeholder, localized_label};
use crate::shell::Shell;
use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};
use localization::msgid;

use super::cards;
use super::collections::{CollectionTableProjection, album_table};
use super::grid_cells::{AlbumGridCell, COLLECTION_GRID_MAX_COLUMNS, ReusableCollectionGridCell};
use super::library_fields::{
    COLLECTION_GRID_CARD_MARGIN, COLLECTION_GRID_MIN_CARD_WIDTH, album_matches_query,
};
use super::models::{replace_albums_in_model, sort_albums};
use super::release_kind::{AlbumReleaseKind, album_release_kind};
use super::route_layout::{ROUTE_TOP_MARGIN, detail_route_scroller};
use super::route_shell::{LibraryToolbarProjection, non_propagating_width_scroller};

const ARTIST_RELEASE_SECTION_GAP: i32 = 18;
const ARTIST_RELEASE_HEADER_GAP: i32 = 10;
const ARTIST_ROUTE_BOTTOM_MARGIN: i32 = 36;
const ARTIST_RELEASE_SECTION_COUNT: usize = 6;

#[derive(Clone)]
pub(super) struct ArtistRouteSearchTarget {
    pub(super) search: gtk::SearchEntry,
    pub(super) focus: Rc<dyn Fn()>,
}

pub(super) struct ArtistReleaseRoutePreamble {
    pub(super) header: gtk::Widget,
    pub(super) favorite: Option<(gtk::Widget, gtk::SearchEntry)>,
    pub(super) favorite_present: bool,
    pub(super) empty: gtk::Widget,
}

#[derive(Clone)]
pub(super) struct ArtistReleaseProjections {
    sections: Rc<Vec<ArtistAlbumProjection>>,
    surface: gtk::Widget,
    layout: Rc<Cell<LibraryLayout>>,
    favorite_present: Rc<Cell<bool>>,
    update_gate: Rc<ArtistReleaseUpdateGate>,
    search_targets: Rc<HashMap<ArtistRouteTarget, ArtistRouteSearchTarget>>,
    apply_grid_fields: Rc<dyn Fn(&[LibraryField])>,
    grid_fields: Rc<RefCell<Vec<LibraryField>>>,
}

#[derive(Clone)]
struct ArtistAlbumProjection {
    source: Rc<RefCell<Arc<Vec<AlbumSummary>>>>,
    visible: Rc<RefCell<Arc<Vec<AlbumSummary>>>>,
    search: gtk::SearchEntry,
    header: gtk::Widget,
    toolbar: LibraryToolbarProjection,
    row_model: gio::ListStore,
    row_table: Rc<RefCell<Option<CollectionTableProjection>>>,
    row_surface: Rc<RefCell<Option<gtk::Widget>>>,
    applied_settings: Rc<RefCell<LibraryListSettings>>,
    shell: Rc<Shell>,
    recompute: Rc<dyn Fn(String, bool)>,
}

#[derive(Default)]
struct ArtistReleaseUpdateGate {
    suspended: Cell<bool>,
    dirty: Cell<bool>,
    rebuild: RefCell<Option<Rc<dyn Fn()>>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ArtistRouteTarget {
    Favorite,
    Release(usize),
}

#[derive(Clone)]
enum ArtistRouteRow {
    Static {
        widget: gtk::Widget,
        margin_bottom: i32,
    },
    AlbumGrid {
        albums: Arc<Vec<AlbumSummary>>,
        start: usize,
        len: usize,
        columns: usize,
        margin_bottom: i32,
    },
}

struct ArtistRouteListCell<Cell> {
    root: gtk::Box,
    grid_cells: Vec<ArtistGridSlot<Cell>>,
    grid_columns: usize,
    grid_mode: bool,
}

struct ArtistGridSlot<Cell> {
    cell: Cell,
    widget: gtk::Widget,
}

impl ArtistReleaseUpdateGate {
    fn changed(&self) {
        if self.suspended.get() {
            self.dirty.set(true);
            return;
        }
        if let Some(rebuild) = self.rebuild.borrow().as_ref().cloned() {
            rebuild();
        } else {
            self.dirty.set(true);
        }
    }

    fn batch(&self, update: impl FnOnce()) {
        let was_suspended = self.suspended.replace(true);
        update();
        self.suspended.set(was_suspended);
        if !was_suspended && self.dirty.replace(false) {
            self.changed();
        }
    }

    fn install(&self, rebuild: Rc<dyn Fn()>) {
        self.rebuild.replace(Some(rebuild));
        self.dirty.set(false);
    }
}

impl ArtistAlbumProjection {
    fn new(
        shell: &Rc<Shell>,
        title: &'static str,
        albums: Vec<AlbumSummary>,
        update_gate: Rc<ArtistReleaseUpdateGate>,
    ) -> Self {
        let key = LibraryListKey::ArtistAlbums;
        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let toolbar = shell.library_toolbar_projection(key, search.clone());
        let header = gtk::Box::new(gtk::Orientation::Vertical, ARTIST_RELEASE_HEADER_GAP);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        let heading = localized_label(title);
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        header.append(&heading);
        header.append(&toolbar.widget());

        let settings = shell.settings.current.borrow().library_list(key);
        let albums = Arc::new(albums);
        let source = Rc::new(RefCell::new(Arc::clone(&albums)));
        let visible = Rc::new(RefCell::new(albums));
        let row_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let row_table = Rc::new(RefCell::new(None::<CollectionTableProjection>));
        let weak_update_gate = Rc::downgrade(&update_gate);
        let recompute = {
            let source = Rc::clone(&source);
            let visible = Rc::clone(&visible);
            let row_model = row_model.clone();
            let row_table = Rc::clone(&row_table);
            Rc::new(move |query: String, notify: bool| {
                let query = query.trim().to_lowercase();
                let next = {
                    let source = source.borrow();
                    if query.is_empty() {
                        Arc::clone(&source)
                    } else {
                        Arc::new(
                            source
                                .iter()
                                .filter(|album| album_matches_query(album, &query))
                                .cloned()
                                .collect::<Vec<_>>(),
                        )
                    }
                };
                visible.replace(Arc::clone(&next));
                if row_table.borrow().is_some() {
                    replace_albums_in_model(&row_model, next.iter().cloned());
                }
                if notify && let Some(update_gate) = weak_update_gate.upgrade() {
                    update_gate.changed();
                }
            }) as Rc<dyn Fn(String, bool)>
        };
        let changed_recompute = Rc::clone(&recompute);
        search.connect_search_changed(move |entry| {
            changed_recompute(entry.text().to_string(), true);
        });

        Self {
            source,
            visible,
            search,
            header: header.upcast(),
            toolbar,
            row_model,
            row_table,
            row_surface: Rc::new(RefCell::new(None)),
            applied_settings: Rc::new(RefCell::new(settings)),
            shell: Rc::clone(shell),
            recompute,
        }
    }

    fn recompute(&self, notify: bool) {
        (self.recompute)(self.search.text().to_string(), notify);
    }

    fn source_is_empty(&self) -> bool {
        self.source.borrow().is_empty()
    }

    fn replace_prepared(&self, albums: Vec<AlbumSummary>) {
        self.source.replace(Arc::new(albums));
        self.recompute(true);
    }

    fn visible(&self) -> Arc<Vec<AlbumSummary>> {
        Arc::clone(&self.visible.borrow())
    }

    fn row_widget(&self, settings: &LibraryListSettings) -> gtk::Widget {
        if let Some(surface) = self.row_surface.borrow().as_ref() {
            if let Some(table) = self.row_table.borrow().as_ref() {
                table.apply_fields(&settings.row_fields);
            }
            return surface.clone();
        }

        let visible = self.visible();
        replace_albums_in_model(&self.row_model, visible.iter().cloned());
        let table = album_table(
            &self.shell,
            self.row_model.clone(),
            LibraryListKey::ArtistAlbums,
        );
        table.apply_fields(&settings.row_fields);
        let clip = non_propagating_width_scroller();
        clip.set_child(Some(&table.widget()));
        let resize_table = table.clone();
        let resize_clip = clip.clone();
        let surface = width_allocation_owner(&clip, move |width| {
            resize_table.fit_scroller_allocation(&resize_clip, width);
        })
        .upcast::<gtk::Widget>();
        self.row_table.replace(Some(table));
        self.row_surface.replace(Some(surface.clone()));
        surface
    }

    fn clear_row_projection(&self) {
        self.row_surface.borrow_mut().take();
        self.row_table.borrow_mut().take();
        self.row_model.remove_all();
    }

    fn apply_settings(&self, settings: &LibraryListSettings) {
        let previous = self.applied_settings.borrow().clone();
        if previous.sort_key != settings.sort_key || previous.descending != settings.descending {
            let mut source = self.source.borrow_mut();
            let source_items: &mut Vec<AlbumSummary> = Arc::make_mut(&mut *source);
            sort_albums(source_items, settings);
            drop(source);
            self.recompute(true);
        }
        if previous.row_fields != settings.row_fields
            && let Some(table) = self.row_table.borrow().as_ref()
        {
            table.apply_fields(&settings.row_fields);
        }
        self.toolbar.apply(LibraryListKey::ArtistAlbums, settings);
        self.applied_settings.replace(settings.clone());
    }
}

impl ArtistReleaseProjections {
    pub(super) fn new(
        shell: &Rc<Shell>,
        preamble: ArtistReleaseRoutePreamble,
        albums: Arc<[AlbumSummary]>,
        appears_on: Arc<[AlbumSummary]>,
    ) -> Self {
        let update_gate = Rc::new(ArtistReleaseUpdateGate::default());
        let partitioned = partition_artist_releases(albums, appears_on);
        let titles = [
            AlbumReleaseKind::Album.section_title(),
            AlbumReleaseKind::Ep.section_title(),
            AlbumReleaseKind::Single.section_title(),
            AlbumReleaseKind::Collection.section_title(),
            AlbumReleaseKind::Other.section_title(),
            msgid("Appears On"),
        ];
        let sections = Rc::new(
            titles
                .into_iter()
                .zip(partitioned)
                .map(|(title, albums)| {
                    ArtistAlbumProjection::new(shell, title, albums, Rc::clone(&update_gate))
                })
                .collect::<Vec<_>>(),
        );

        let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        let settings = shell
            .settings
            .current
            .borrow()
            .library_list(LibraryListKey::ArtistAlbums);
        let grid_fields = Rc::new(RefCell::new(settings.grid_fields.clone()));
        let cell_shell = Rc::clone(shell);
        let (list, apply_grid_fields) =
            artist_route_list(rows.clone(), Rc::clone(&grid_fields), move |fields| {
                AlbumGridCell::new(
                    Rc::clone(&cell_shell),
                    fields,
                    COLLECTION_GRID_MIN_CARD_WIDTH,
                )
            });
        list.set_margin_top(ROUTE_TOP_MARGIN);
        list.set_margin_bottom(ARTIST_ROUTE_BOTTOM_MARGIN);

        let layout = Rc::new(Cell::new(normalized_artist_layout(settings.layout)));
        let columns = Rc::new(Cell::new(1));
        let favorite_present = Rc::new(Cell::new(preamble.favorite_present));
        let positions = Rc::new(RefCell::new(HashMap::<ArtistRouteTarget, u32>::new()));
        let favorite_widget = preamble.favorite.as_ref().map(|(widget, _)| widget.clone());
        let header = preamble.header;
        let empty = preamble.empty;

        let rebuild: Rc<dyn Fn()> = {
            let rows = rows.clone();
            let sections = Rc::clone(&sections);
            let layout = Rc::clone(&layout);
            let columns = Rc::clone(&columns);
            let favorite_present = Rc::clone(&favorite_present);
            let positions = Rc::clone(&positions);
            let header = header.clone();
            let favorite_widget = favorite_widget.clone();
            let empty = empty.clone();
            let settings_shell = Rc::clone(shell);
            Rc::new(move || {
                let settings = settings_shell
                    .settings
                    .current
                    .borrow()
                    .library_list(LibraryListKey::ArtistAlbums);
                let mut next = Vec::<ArtistRouteRow>::new();
                let mut next_positions = HashMap::new();
                next.push(ArtistRouteRow::Static {
                    widget: header.clone(),
                    margin_bottom: ARTIST_RELEASE_SECTION_GAP,
                });
                if favorite_present.get()
                    && let Some(favorite_widget) = favorite_widget.as_ref()
                {
                    next_positions.insert(ArtistRouteTarget::Favorite, next.len() as u32);
                    next.push(ArtistRouteRow::Static {
                        widget: favorite_widget.clone(),
                        margin_bottom: ARTIST_RELEASE_SECTION_GAP,
                    });
                }

                for (section_index, section) in sections.iter().enumerate() {
                    if section.source_is_empty() {
                        continue;
                    }
                    next_positions
                        .insert(ArtistRouteTarget::Release(section_index), next.len() as u32);
                    let visible = section.visible();
                    let uses_rows = layout.get() == LibraryLayout::Row;
                    next.push(ArtistRouteRow::Static {
                        widget: section.header.clone(),
                        margin_bottom: if uses_rows || !visible.is_empty() {
                            ARTIST_RELEASE_HEADER_GAP
                        } else {
                            ARTIST_RELEASE_SECTION_GAP
                        },
                    });
                    if uses_rows {
                        next.push(ArtistRouteRow::Static {
                            widget: section.row_widget(&settings),
                            margin_bottom: ARTIST_RELEASE_SECTION_GAP,
                        });
                    } else {
                        append_grid_rows(&mut next, visible, columns.get());
                    }
                }

                if !favorite_present.get()
                    && sections.iter().all(|section| section.source_is_empty())
                {
                    next.push(ArtistRouteRow::Static {
                        widget: empty.clone(),
                        margin_bottom: 0,
                    });
                }
                let additions = next
                    .iter()
                    .cloned()
                    .map(glib::BoxedAnyObject::new)
                    .collect::<Vec<_>>();
                positions.replace(next_positions);
                rows.splice(0, rows.n_items(), &additions);
            })
        };
        update_gate.install(Rc::clone(&rebuild));
        rebuild();

        let resize_columns = Rc::clone(&columns);
        let resize_layout = Rc::clone(&layout);
        let resize_rebuild = Rc::clone(&rebuild);
        let owner = width_allocation_owner(&list, move |width| {
            if resize_layout.get() == LibraryLayout::Row {
                return;
            }
            let next = artist_release_column_count(width);
            if resize_columns.replace(next) != next {
                resize_rebuild();
            }
        });
        let surface = detail_route_scroller(
            shell,
            super::collections::library_route_inset(owner.upcast()),
        );

        let mut search_targets = HashMap::new();
        if let Some((_, favorite_search)) = preamble.favorite {
            search_targets.insert(
                ArtistRouteTarget::Favorite,
                virtual_search_target(
                    &list,
                    Rc::clone(&positions),
                    ArtistRouteTarget::Favorite,
                    favorite_search,
                ),
            );
        }
        for (index, section) in sections.iter().enumerate() {
            let target = ArtistRouteTarget::Release(index);
            search_targets.insert(
                target,
                virtual_search_target(&list, Rc::clone(&positions), target, section.search.clone()),
            );
        }

        Self {
            sections,
            surface,
            layout,
            favorite_present,
            update_gate,
            search_targets: Rc::new(search_targets),
            apply_grid_fields,
            grid_fields,
        }
    }

    pub(super) fn widget(&self) -> gtk::Widget {
        self.surface.clone()
    }

    pub(super) fn replace_prepared(
        &self,
        albums: Arc<[AlbumSummary]>,
        appears_on: Arc<[AlbumSummary]>,
        favorite_present: bool,
    ) {
        let partitioned = partition_artist_releases(albums, appears_on);
        self.update_gate.batch(|| {
            self.favorite_present.set(favorite_present);
            for (section, albums) in self.sections.iter().zip(partitioned) {
                section.replace_prepared(albums);
            }
        });
    }

    pub(super) fn primary_search(&self) -> Option<ArtistRouteSearchTarget> {
        let target = if self.favorite_present.get() {
            Some(ArtistRouteTarget::Favorite)
        } else {
            self.sections
                .iter()
                .position(|section| !section.source_is_empty())
                .map(ArtistRouteTarget::Release)
        }?;
        self.search_targets.get(&target).cloned()
    }

    pub(super) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &LibraryListSettings,
    ) {
        if key != LibraryListKey::ArtistAlbums {
            return;
        }
        let previous_layout = self.layout.get();
        let next_layout = normalized_artist_layout(settings.layout);
        let previous_fields = self.grid_fields.borrow().clone();
        self.update_gate.batch(|| {
            for section in self.sections.iter() {
                section.apply_settings(settings);
            }
            if previous_layout != next_layout {
                self.layout.set(next_layout);
                if previous_layout == LibraryLayout::Row {
                    for section in self.sections.iter() {
                        section.clear_row_projection();
                    }
                }
                self.update_gate.dirty.set(true);
            }
        });
        if previous_fields != settings.grid_fields {
            self.grid_fields.replace(settings.grid_fields.clone());
            (self.apply_grid_fields)(&settings.grid_fields);
        }
    }
}

fn normalized_artist_layout(layout: LibraryLayout) -> LibraryLayout {
    match layout {
        LibraryLayout::Row => LibraryLayout::Row,
        LibraryLayout::Grid | LibraryLayout::Detail => LibraryLayout::Grid,
    }
}

fn release_section_index(kind: AlbumReleaseKind) -> usize {
    match kind {
        AlbumReleaseKind::Album => 0,
        AlbumReleaseKind::Ep => 1,
        AlbumReleaseKind::Single => 2,
        AlbumReleaseKind::Collection => 3,
        AlbumReleaseKind::Other => 4,
    }
}

fn partition_artist_releases(
    albums: Arc<[AlbumSummary]>,
    appears_on: Arc<[AlbumSummary]>,
) -> [Vec<AlbumSummary>; 6] {
    let mut sections: [Vec<AlbumSummary>; ARTIST_RELEASE_SECTION_COUNT] =
        std::array::from_fn(|_| Vec::new());
    for album in albums.iter().cloned() {
        sections[release_section_index(album_release_kind(&album.album))].push(album);
    }
    sections[5].extend(appears_on.iter().cloned());
    sections
}

fn artist_release_column_count(width: i32) -> usize {
    let slot_width = COLLECTION_GRID_MIN_CARD_WIDTH + COLLECTION_GRID_CARD_MARGIN * 2;
    (width.max(1) / slot_width.max(1)).clamp(1, COLLECTION_GRID_MAX_COLUMNS as i32) as usize
}

fn append_grid_rows(
    rows: &mut Vec<ArtistRouteRow>,
    albums: Arc<Vec<AlbumSummary>>,
    columns: usize,
) {
    let columns = columns.max(1);
    let row_count = albums.len().div_ceil(columns);
    for row in 0..row_count {
        let start = row * columns;
        let len = (albums.len() - start).min(columns);
        rows.push(ArtistRouteRow::AlbumGrid {
            albums: Arc::clone(&albums),
            start,
            len,
            columns,
            margin_bottom: if row + 1 == row_count {
                ARTIST_RELEASE_SECTION_GAP
            } else {
                0
            },
        });
    }
}

fn artist_route_list<Cell, Make>(
    model: gio::ListStore,
    fields: Rc<RefCell<Vec<LibraryField>>>,
    make_cell: Make,
) -> (gtk::ListView, Rc<dyn Fn(&[LibraryField])>)
where
    Cell: ReusableCollectionGridCell<AlbumSummary>,
    Make: Fn(&[LibraryField]) -> Cell + 'static,
{
    let selection = gtk::NoSelection::new(Some(model));
    let factory = gtk::SignalListItemFactory::new();
    let cells = Rc::new(RefCell::new(
        HashMap::<usize, ArtistRouteListCell<Cell>>::new(),
    ));
    let make_cell = Rc::new(make_cell);

    let setup_cells = Rc::clone(&cells);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_hexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_vexpand(false);
        root.set_valign(gtk::Align::Start);
        item.set_child(Some(&root));
        setup_cells.borrow_mut().insert(
            item.as_ptr() as usize,
            ArtistRouteListCell {
                root,
                grid_cells: Vec::new(),
                grid_columns: 0,
                grid_mode: false,
            },
        );
    });

    let bind_cells = Rc::clone(&cells);
    let bind_fields = Rc::clone(&fields);
    let bind_make_cell = Rc::clone(&make_cell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
            .map(|item| item.borrow::<ArtistRouteRow>().clone())
        else {
            return;
        };
        let mut states = bind_cells.borrow_mut();
        let Some(state) = states.get_mut(&(item.as_ptr() as usize)) else {
            return;
        };
        state.root.set_margin_bottom(match &row {
            ArtistRouteRow::Static { margin_bottom, .. }
            | ArtistRouteRow::AlbumGrid { margin_bottom, .. } => *margin_bottom,
        });
        match row {
            ArtistRouteRow::Static { widget, .. } => {
                clear_grid_state(state, true);
                detach_static_widget(&widget);
                state.root.set_homogeneous(false);
                state.root.append(&widget);
            }
            ArtistRouteRow::AlbumGrid {
                albums,
                start,
                len,
                columns,
                ..
            } => {
                if !state.grid_mode || state.grid_columns != columns {
                    clear_grid_state(state, true);
                    state.root.set_homogeneous(true);
                    state.root.add_css_class("album-grid");
                    for _ in 0..columns {
                        let cell = bind_make_cell(&bind_fields.borrow());
                        let widget = cell.widget();
                        state.root.append(&cards::collection_grid_card_inset(
                            &widget,
                            COLLECTION_GRID_MIN_CARD_WIDTH,
                        ));
                        state.grid_cells.push(ArtistGridSlot { cell, widget });
                    }
                    state.grid_columns = columns;
                    state.grid_mode = true;
                }
                for (offset, slot) in state.grid_cells.iter().enumerate() {
                    if offset < len {
                        slot.widget.set_visible(true);
                        slot.cell.bind(
                            start.saturating_add(offset) as u32,
                            albums[start + offset].clone(),
                        );
                    } else {
                        slot.cell.clear();
                        slot.widget.set_visible(false);
                    }
                }
            }
        }
    });

    let unbind_cells = Rc::clone(&cells);
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(state) = unbind_cells.borrow_mut().get_mut(&(item.as_ptr() as usize)) {
            if state.grid_mode {
                clear_grid_state(state, false);
            } else {
                remove_box_children(&state.root);
            }
        }
    });

    let teardown_cells = Rc::clone(&cells);
    factory.connect_teardown(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(mut state) = teardown_cells
            .borrow_mut()
            .remove(&(item.as_ptr() as usize))
        {
            clear_grid_state(&mut state, true);
        }
        item.set_child(None::<&gtk::Widget>);
    });

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("artist-release-list");
    list.set_single_click_activate(false);
    list.set_hexpand(true);
    list.set_halign(gtk::Align::Fill);
    list.set_vexpand(true);

    let apply_cells = Rc::clone(&cells);
    let apply_fields = Rc::new(move |fields: &[LibraryField]| {
        for state in apply_cells.borrow().values() {
            for slot in &state.grid_cells {
                slot.cell.apply_fields(fields);
            }
        }
    }) as Rc<dyn Fn(&[LibraryField])>;
    (list, apply_fields)
}

fn clear_grid_state<Cell: ReusableCollectionGridCell<AlbumSummary>>(
    state: &mut ArtistRouteListCell<Cell>,
    remove: bool,
) {
    for slot in &state.grid_cells {
        slot.cell.clear();
        slot.widget.set_visible(false);
    }
    if remove {
        remove_box_children(&state.root);
        state.grid_cells.clear();
        state.grid_columns = 0;
        state.grid_mode = false;
        state.root.remove_css_class("album-grid");
    }
}

fn remove_box_children(root: &gtk::Box) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }
}

fn detach_static_widget(widget: &gtk::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Ok(parent) = parent.downcast::<gtk::Box>() {
        parent.remove(widget);
    }
}

fn virtual_search_target(
    list: &gtk::ListView,
    positions: Rc<RefCell<HashMap<ArtistRouteTarget, u32>>>,
    target: ArtistRouteTarget,
    search: gtk::SearchEntry,
) -> ArtistRouteSearchTarget {
    let pending = Rc::new(Cell::new(false));
    let mapped_pending = Rc::clone(&pending);
    search.connect_map(move |search| {
        if mapped_pending.replace(false) {
            search.grab_focus();
        }
    });
    let weak_list = list.downgrade();
    let weak_search = search.downgrade();
    let focus = Rc::new(move || {
        let Some(search) = weak_search.upgrade() else {
            return;
        };
        if search.is_mapped() {
            search.grab_focus();
            return;
        }
        pending.set(true);
        let Some(position) = positions.borrow().get(&target).copied() else {
            pending.set(false);
            return;
        };
        if let Some(list) = weak_list.upgrade() {
            list.scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
        }
    }) as Rc<dyn Fn()>;
    ArtistRouteSearchTarget { search, focus }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::library::{Album, AlbumArtwork, AlbumId, AlbumRelations};

    fn album(index: u64) -> AlbumSummary {
        let album = Arc::new(Album {
            id: AlbumId::fake(index),
            title: format!("Album {index}"),
            artist: "Artist".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            favorite: false,
            color_seed: index as u32,
            image_ref: None,
            local_artwork: None,
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
            relations: AlbumRelations::default(),
        });
        AlbumSummary {
            artwork: AlbumArtwork {
                album: Arc::clone(&album),
                representative_track: None,
            },
            album,
            track_count: 1,
            duration_seconds: 60,
        }
    }

    #[test]
    fn grid_rows_preserve_every_album_once_at_each_column_count() {
        let albums = Arc::new((0..37).map(album).collect::<Vec<_>>());
        for columns in 1..=8 {
            let mut rows = Vec::new();
            append_grid_rows(&mut rows, Arc::clone(&albums), columns);
            let projected = rows
                .into_iter()
                .flat_map(|row| match row {
                    ArtistRouteRow::AlbumGrid {
                        albums, start, len, ..
                    } => albums[start..start + len]
                        .iter()
                        .map(|album| album.album.id.clone())
                        .collect::<Vec<_>>(),
                    ArtistRouteRow::Static { .. } => Vec::new(),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                projected,
                albums
                    .iter()
                    .map(|album| album.album.id.clone())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn column_count_changes_only_at_complete_card_thresholds() {
        let slot = COLLECTION_GRID_MIN_CARD_WIDTH + COLLECTION_GRID_CARD_MARGIN * 2;
        assert_eq!(artist_release_column_count(slot - 1), 1);
        assert_eq!(artist_release_column_count(slot), 1);
        assert_eq!(artist_release_column_count(slot * 3 - 1), 2);
        assert_eq!(artist_release_column_count(slot * 3), 3);
        assert_eq!(artist_release_column_count(slot * 4 - 1), 3);
        assert_eq!(artist_release_column_count(slot * 4), 4);
    }
}
