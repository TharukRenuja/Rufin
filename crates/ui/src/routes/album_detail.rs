use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use ::library::{ActiveLibraryQuery, Album, AlbumId, Track, TrackId};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::AlbumPlayRequest;

use super::collection_context::{install_album_context_menu, install_track_context_menu};
use crate::favorites::{
    album_favorite_key, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active, track_favorite_key,
};
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{GRID_COVER_SIZE, THUMB_COVER_SIZE};
use crate::shell::layout::route_content_width;
use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};
use localization::tr;

use super::cards;
use super::columns::track_column_width;
use super::library_fields::{
    album_detail_meta_label, album_fact_text, compare_track, item_at, play_count_column_width,
    replace_album_items, track_field,
};
use super::models::{populate_album_model, sort_albums};
use super::route::Route;
use super::route_layout::{PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START};
use super::table_sizing::fitted_column_widths;

const ALBUM_DETAIL_ROW_HORIZONTAL_INSET: i32 = 8;
const ALBUM_DETAIL_TRACK_COLUMN_GAP: i32 = 8;
const ALBUM_DETAIL_MAX_COVER: i32 = 168;
const ALBUM_DETAIL_MIN_COVER: i32 = 102;
const ALBUM_DETAIL_NARROW_WIDTH: i32 = 360;
const ALBUM_DETAIL_SEPARATOR_WIDTH: i32 = 1;
const ALBUM_DETAIL_INLINE_TRACK_ROWS: usize = 8;
const ALBUM_DETAIL_TRACK_ROW_HEIGHT: i32 = 36;
const ALBUM_DETAIL_TRACK_HEADER_HEIGHT: i32 = 26;
const ALBUM_DETAIL_META_SPACING: i32 = 6;
pub(super) const ALBUM_DETAIL_META_LABEL_HEIGHT: i32 = 20;

#[derive(Clone)]
pub(crate) struct AlbumCollectionModels {
    albums: gio::ListStore,
    detail: gio::ListStore,
}

impl AlbumCollectionModels {
    pub(crate) fn new() -> Self {
        Self {
            albums: gio::ListStore::new::<glib::BoxedAnyObject>(),
            detail: gio::ListStore::new::<glib::BoxedAnyObject>(),
        }
    }

    pub(crate) fn albums(&self) -> gio::ListStore {
        self.albums.clone()
    }

    pub(crate) fn detail(&self) -> gio::ListStore {
        self.detail.clone()
    }

    pub(crate) fn clear_inactive(&self, layout: LibraryLayout) {
        if layout == LibraryLayout::Detail {
            self.albums.remove_all();
        } else {
            self.detail.remove_all();
        }
    }
}

pub(crate) fn populate_album_collection_model(
    models: &AlbumCollectionModels,
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) {
    if settings.layout == LibraryLayout::Detail {
        replace_album_items(
            &models.detail,
            album_detail_items_for(albums, settings, album_tracks),
        );
    } else {
        populate_album_model(&models.albums, albums, settings);
    }
}

pub(crate) fn album_detail_items_for(
    albums: &[Album],
    settings: &LibraryListSettings,
    album_tracks: &HashMap<AlbumId, Vec<Track>>,
) -> Vec<AlbumDetailItem> {
    let mut albums = albums.to_vec();
    sort_albums(&mut albums, settings);
    let mut rows = Vec::new();

    for album in albums {
        let mut tracks = album_tracks.get(&album.id).cloned().unwrap_or_default();
        sort_album_detail_tracks(&mut tracks);
        let inline_count = tracks.len().min(ALBUM_DETAIL_INLINE_TRACK_ROWS);
        let remaining_tracks = tracks.split_off(inline_count);
        rows.push(AlbumDetailItem::Lead {
            album,
            inline_tracks: tracks,
            last_in_album: remaining_tracks.is_empty(),
        });
        let remaining_count = remaining_tracks.len();
        for (offset, track) in remaining_tracks.into_iter().enumerate() {
            rows.push(AlbumDetailItem::Track {
                track,
                index: inline_count + offset,
                last_in_album: offset + 1 == remaining_count,
            });
        }
    }

    rows
}

pub(crate) fn album_detail_list(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
) -> AlbumDetailVirtualList {
    AlbumDetailVirtualList::new(shell, model, key, query)
}

#[derive(Clone)]
pub(crate) struct AlbumDetailVirtualList {
    inner: Rc<AlbumDetailVirtualListInner>,
}

struct AlbumDetailVirtualListInner {
    shell: Weak<Shell>,
    model: gio::ListStore,
    key: LibraryListKey,
    query: ActiveLibraryQuery,
    widget: gtk::Box,
    top_spacer: gtk::Box,
    rows_widget: gtk::Box,
    bottom_spacer: gtk::Box,
    rows: RefCell<Vec<AlbumDetailVirtualRow>>,
    rendered: RefCell<Option<(usize, usize)>>,
    selection: AlbumDetailTrackSelection,
    width: Cell<i32>,
    scheduled: Cell<bool>,
    model_handler: RefCell<Option<glib::SignalHandlerId>>,
    adjustment_handlers: RefCell<Option<AlbumDetailAdjustmentHandlers>>,
}

struct AlbumDetailAdjustmentHandlers {
    adjustment: glib::WeakRef<gtk::Adjustment>,
    value_changed: glib::SignalHandlerId,
    changed: glib::SignalHandlerId,
}

#[derive(Clone)]
struct AlbumDetailVirtualRow {
    item: AlbumDetailItem,
    top: i32,
    height: i32,
}

impl AlbumDetailVirtualRow {
    fn bottom(&self) -> i32 {
        self.top.saturating_add(self.height)
    }
}

impl AlbumDetailVirtualList {
    fn new(
        shell: &Rc<Shell>,
        model: gio::ListStore,
        key: LibraryListKey,
        query: ActiveLibraryQuery,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("track-table");
        widget.add_css_class("album-detail-list");
        widget.set_hexpand(true);
        widget.set_halign(gtk::Align::Fill);
        widget.set_vexpand(false);

        let top_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let rows_widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rows_widget.set_hexpand(true);
        rows_widget.set_halign(gtk::Align::Fill);
        let bottom_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&top_spacer);
        widget.append(&rows_widget);
        widget.append(&bottom_spacer);

        let width = route_content_width(shell).max(1);
        let inner = Rc::new(AlbumDetailVirtualListInner {
            shell: Rc::downgrade(shell),
            model: model.clone(),
            key,
            query,
            widget,
            top_spacer,
            rows_widget,
            bottom_spacer,
            rows: RefCell::new(album_detail_virtual_rows(&model, key, width)),
            rendered: RefCell::new(None),
            selection: AlbumDetailTrackSelection::default(),
            width: Cell::new(width),
            scheduled: Cell::new(false),
            model_handler: RefCell::new(None),
            adjustment_handlers: RefCell::new(None),
        });
        inner.apply_extent();

        let weak = Rc::downgrade(&inner);
        let handler = model.connect_items_changed(move |_, _, _, _| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.rebuild_rows();
            AlbumDetailVirtualListInner::queue_render(&inner);
        });
        inner.model_handler.replace(Some(handler));
        let current_track_list = Rc::downgrade(&inner);
        shell.register_current_route_track_selection(Rc::new(move |track_id| {
            let Some(inner) = current_track_list.upgrade() else {
                return false;
            };
            let track_id = track_id
                .filter(|current| &current.source_id == inner.query.source_id())
                .map(|current| &current.track_id);
            inner.selection.select_now_playing_track(track_id);
            true
        }));
        Self { inner }
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.inner.widget.clone().upcast()
    }

    pub(crate) fn attach_scroller(&self, scroller: &gtk::ScrolledWindow) {
        self.inner.disconnect_adjustment();
        let adjustment = scroller.vadjustment();

        let weak = Rc::downgrade(&self.inner);
        let value_changed = adjustment.connect_value_changed(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            AlbumDetailVirtualListInner::queue_render(&inner);
        });
        let weak = Rc::downgrade(&self.inner);
        let changed = adjustment.connect_changed(move |_| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            AlbumDetailVirtualListInner::queue_render(&inner);
        });
        self.inner
            .adjustment_handlers
            .replace(Some(AlbumDetailAdjustmentHandlers {
                adjustment: adjustment.downgrade(),
                value_changed,
                changed,
            }));
        self.inner.render();
        AlbumDetailVirtualListInner::queue_render(&self.inner);
    }

    pub(crate) fn fit_allocation(&self, width: i32) {
        if width <= 1 {
            return;
        }
        let previous_width = self.inner.width.replace(width);
        if previous_width == width {
            return;
        }

        let previous_cover =
            album_detail_row_metrics_for_width(self.inner.key, previous_width).cover_size;
        let current_cover = album_detail_row_metrics_for_width(self.inner.key, width).cover_size;
        if previous_cover != current_cover {
            self.inner.rebuild_rows();
            self.inner.render();
        } else {
            self.inner.resize_rendered_rows();
        }
    }
}

impl AlbumDetailVirtualListInner {
    fn rebuild_rows(&self) {
        self.rows.replace(album_detail_virtual_rows(
            &self.model,
            self.key,
            self.width.get(),
        ));
        self.rendered.replace(None);
        self.apply_extent();
    }

    fn apply_extent(&self) {
        self.widget
            .set_height_request(album_detail_virtual_total_height(&self.rows.borrow()));
    }

    fn adjustment(&self) -> Option<gtk::Adjustment> {
        self.adjustment_handlers
            .borrow()
            .as_ref()
            .and_then(|handlers| handlers.adjustment.upgrade())
    }

    fn queue_render(inner: &Rc<Self>) {
        if inner.scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(inner);
        glib::idle_add_local_once(move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.scheduled.set(false);
            inner.render();
        });
    }

    fn render(&self) {
        let Some(adjustment) = self.adjustment() else {
            return;
        };
        let rows = self.rows.borrow();
        let total_height = album_detail_virtual_total_height(&rows);
        let overscan = f64::from(album_detail_virtual_overscan_height());
        let visible_top = (adjustment.value() - overscan).max(0.0);
        let visible_bottom = (adjustment.value() + adjustment.page_size() + overscan)
            .min(f64::from(total_height))
            .max(visible_top);
        let (start, end) = album_detail_virtual_range(&rows, visible_top, visible_bottom);
        if self.rendered.borrow().as_ref() == Some(&(start, end)) {
            return;
        }
        self.rendered.replace(Some((start, end)));

        self.selection.clear_bound_rows();
        while let Some(child) = self.rows_widget.first_child() {
            self.rows_widget.remove(&child);
        }

        let top_height = rows.get(start).map(|row| row.top).unwrap_or(0).max(0);
        let bottom_start = end
            .checked_sub(1)
            .and_then(|index| rows.get(index))
            .map(AlbumDetailVirtualRow::bottom)
            .unwrap_or(top_height);
        self.top_spacer.set_height_request(top_height);
        self.bottom_spacer
            .set_height_request(total_height.saturating_sub(bottom_start).max(0));

        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let fields = shell
            .settings
            .current
            .borrow()
            .library_list(self.key)
            .detail_track_fields;
        for row in &rows[start..end] {
            let widget = album_detail_item_row(
                &shell,
                row.item.clone(),
                self.key,
                &self.query,
                &self.selection,
            );
            resize_album_detail_item_row(&widget, &row.item, self.key, &fields, self.width.get());
            self.rows_widget.append(&widget);
        }
    }

    fn resize_rendered_rows(&self) {
        let Some((start, end)) = *self.rendered.borrow() else {
            return;
        };
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let fields = shell
            .settings
            .current
            .borrow()
            .library_list(self.key)
            .detail_track_fields;
        let rows = self.rows.borrow();
        let mut child = self.rows_widget.first_child();
        for row in &rows[start..end] {
            let Some(widget) = child else {
                break;
            };
            child = widget.next_sibling();
            resize_album_detail_item_row(&widget, &row.item, self.key, &fields, self.width.get());
        }
    }

    fn disconnect_adjustment(&self) {
        let Some(handlers) = self.adjustment_handlers.borrow_mut().take() else {
            return;
        };
        let Some(adjustment) = handlers.adjustment.upgrade() else {
            return;
        };
        adjustment.disconnect(handlers.value_changed);
        adjustment.disconnect(handlers.changed);
    }
}

impl Drop for AlbumDetailVirtualListInner {
    fn drop(&mut self) {
        if let Some(handler) = self.model_handler.get_mut().take() {
            self.model.disconnect(handler);
        }
        self.disconnect_adjustment();
    }
}

fn album_detail_virtual_rows(
    model: &gio::ListStore,
    key: LibraryListKey,
    width: i32,
) -> Vec<AlbumDetailVirtualRow> {
    let cover_size = album_detail_row_metrics_for_width(key, width).cover_size;
    let mut rows = Vec::with_capacity(model.n_items() as usize);
    let mut top = 0_i32;
    for index in 0..model.n_items() {
        let Some(item) = item_at::<AlbumDetailItem>(model, index) else {
            continue;
        };
        let height = album_detail_item_total_height(&item, cover_size);
        rows.push(AlbumDetailVirtualRow { item, top, height });
        top = top.saturating_add(height);
    }
    rows
}

fn album_detail_virtual_total_height(rows: &[AlbumDetailVirtualRow]) -> i32 {
    rows.last()
        .map(AlbumDetailVirtualRow::bottom)
        .unwrap_or(1)
        .max(1)
}

fn album_detail_virtual_range(
    rows: &[AlbumDetailVirtualRow],
    visible_top: f64,
    visible_bottom: f64,
) -> (usize, usize) {
    let start = rows.partition_point(|row| f64::from(row.bottom()) < visible_top);
    let end = rows
        .partition_point(|row| f64::from(row.top) <= visible_bottom)
        .max(start);
    (start, end)
}

fn album_detail_virtual_overscan_height() -> i32 {
    ALBUM_DETAIL_TRACK_ROW_HEIGHT * 8
}

#[derive(Clone, Default)]
pub(crate) struct AlbumDetailTrackSelection {
    selected_track_id: Rc<RefCell<Option<TrackId>>>,
    bound_rows: Rc<RefCell<Vec<AlbumDetailTrackRowBinding>>>,
}

struct AlbumDetailTrackRowBinding {
    track_id: TrackId,
    row: glib::WeakRef<gtk::Widget>,
}

impl AlbumDetailTrackSelection {
    pub(crate) fn bind_row(&self, row: &gtk::Widget, track_id: &TrackId) {
        if self.selected_track_id.borrow().as_ref() == Some(track_id) {
            row.add_css_class("album-detail-track-selected");
        }
        self.bound_rows
            .borrow_mut()
            .push(AlbumDetailTrackRowBinding {
                track_id: track_id.clone(),
                row: row.downgrade(),
            });
    }

    fn select_now_playing_track(&self, track_id: Option<&TrackId>) {
        *self.selected_track_id.borrow_mut() = track_id.cloned();
        self.bound_rows.borrow_mut().retain(|binding| {
            let Some(row) = binding.row.upgrade() else {
                return false;
            };
            if track_id == Some(&binding.track_id) {
                row.add_css_class("album-detail-track-selected");
            } else {
                row.remove_css_class("album-detail-track-selected");
            }
            true
        });
    }

    fn clear_bound_rows(&self) {
        self.bound_rows.borrow_mut().clear();
    }
}
#[derive(Clone, Debug, PartialEq)]
#[expect(clippy::large_enum_variant)]
pub(crate) enum AlbumDetailItem {
    Lead {
        album: Album,
        inline_tracks: Vec<Track>,
        last_in_album: bool,
    },
    Track {
        track: Track,
        index: usize,
        last_in_album: bool,
    },
}
pub(crate) fn album_detail_item_row(
    shell: &Rc<Shell>,
    row: AlbumDetailItem,
    key: LibraryListKey,
    query: &ActiveLibraryQuery,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    match row {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album,
        } => album_detail_lead_row(
            shell,
            &album,
            &inline_tracks,
            last_in_album,
            key,
            query,
            selection,
        ),
        AlbumDetailItem::Track {
            track,
            index,
            last_in_album,
        } => album_detail_track_row(shell, &track, index, last_in_album, key, query, selection),
    }
}

fn resize_album_detail_item_row(
    widget: &gtk::Widget,
    item: &AlbumDetailItem,
    key: LibraryListKey,
    fields: &[LibraryField],
    route_width: i32,
) {
    let Ok(row) = widget.clone().downcast::<gtk::Box>() else {
        return;
    };
    let metrics = album_detail_row_metrics_for_width(key, route_width);
    row.set_spacing(metrics.spacing);

    match item {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            ..
        } => {
            let content_height =
                album_detail_lead_content_height(album, metrics.cover_size, inline_tracks.len());
            row.set_height_request(content_height);
            let Some(meta) = row.first_child() else {
                return;
            };
            resize_album_detail_meta(&meta, album, metrics.cover_size, metrics.meta_width);
            if inline_tracks.is_empty() {
                return;
            }

            let track_width = album_detail_track_area_width_for(
                key,
                route_width,
                metrics.meta_width,
                metrics.spacing,
                fields,
            );
            let field_widths = album_detail_track_field_widths(key, fields, track_width);
            if let Some(separator) = meta.next_sibling() {
                separator.set_height_request(content_height);
            }
            let Some(track_area) = row.last_child() else {
                return;
            };
            track_area.set_width_request(track_width);
            let mut track_row = track_area.first_child();
            while let Some(current) = track_row {
                track_row = current.next_sibling();
                resize_album_detail_track_cells(&current, &field_widths);
            }
        }
        AlbumDetailItem::Track { .. } => {
            let Some(spacer) = row.first_child() else {
                return;
            };
            spacer.set_width_request(metrics.meta_width);
            let track_width = album_detail_track_area_width_for(
                key,
                route_width,
                metrics.meta_width,
                metrics.spacing,
                fields,
            );
            let field_widths = album_detail_track_field_widths(key, fields, track_width);
            if let Some(cells) = row.last_child() {
                resize_album_detail_track_cells(&cells, &field_widths);
            }
        }
    }
}

fn resize_album_detail_meta(widget: &gtk::Widget, album: &Album, cover_size: i32, width: i32) {
    let Ok(meta) = widget.clone().downcast::<gtk::Box>() else {
        return;
    };
    meta.set_width_request(width);
    meta.set_height_request(album_detail_meta_height(album, cover_size));

    let Some(cover) = meta.first_child() else {
        return;
    };
    cards::constrain_cover_widget(&cover, cover_size);
    if let Ok(overlay) = cover.clone().downcast::<gtk::Overlay>() {
        if let Some(button) = overlay.child() {
            cards::constrain_cover_widget(&button, cover_size);
            if let Ok(button) = button.downcast::<gtk::Button>()
                && let Some(tile) = button.child()
            {
                tile.set_size_request(cover_size, cover_size);
            }
        }
        let mut child = overlay.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            if current.has_css_class("cover-hover-layer") {
                cards::constrain_cover_widget(&current, cover_size);
            }
        }
    }

    let mut label = cover.next_sibling();
    while let Some(current) = label {
        label = current.next_sibling();
        let Ok(clip) = current.downcast::<gtk::ScrolledWindow>() else {
            continue;
        };
        clip.set_width_request(width);
        clip.set_min_content_width(width);
        clip.set_max_content_width(width);
    }
}

fn resize_album_detail_track_cells(widget: &gtk::Widget, field_widths: &[(LibraryField, i32)]) {
    let Ok(row) = widget.clone().downcast::<gtk::Box>() else {
        return;
    };
    row.set_width_request(album_detail_track_cells_width(field_widths));
    let data_row = row.has_css_class("album-detail-track-row");
    let mut cell = row.first_child();
    for (field, width) in field_widths {
        let Some(current) = cell else {
            break;
        };
        cell = current.next_sibling();
        current.set_width_request(*width);
        if let Some(content) = current.first_child()
            && (!data_row || !matches!(field, LibraryField::Favorite | LibraryField::Image))
        {
            content.set_width_request(*width);
        }
    }
}

pub(crate) fn album_detail_lead_row(
    shell: &Rc<Shell>,
    album: &Album,
    inline_tracks: &[Track],
    last_in_album: bool,
    key: LibraryListKey,
    query: &ActiveLibraryQuery,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let metrics = album_detail_row_metrics(shell, key);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, metrics.spacing);
    let content_height =
        album_detail_lead_content_height(album, metrics.cover_size, inline_tracks.len());
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_top(12);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_height_request(content_height);

    row.append(&album_detail_meta(
        shell,
        album,
        metrics.cover_size,
        metrics.meta_width,
        query,
    ));

    let track_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    track_area.set_hexpand(true);
    track_area.set_halign(gtk::Align::Fill);
    if !inline_tracks.is_empty() {
        let fields = shell
            .settings
            .current
            .borrow()
            .library_list(key)
            .detail_track_fields;
        let track_width =
            album_detail_track_area_width(shell, key, metrics.meta_width, metrics.spacing, &fields);
        let field_widths = album_detail_track_field_widths(key, &fields, track_width);
        track_area.set_width_request(track_width);
        track_area.set_height_request(album_detail_track_area_height(inline_tracks.len()));
        track_area.append(&album_detail_track_header(&field_widths));
        for (index, track) in inline_tracks.iter().enumerate() {
            track_area.append(&album_detail_track_cells(
                shell,
                track,
                index,
                &field_widths,
                query,
                selection,
            ));
        }
        row.append(&album_detail_separator(content_height));
    }
    row.append(&track_area);
    row.upcast()
}
pub(crate) fn album_detail_track_row(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    last_in_album: bool,
    key: LibraryListKey,
    query: &ActiveLibraryQuery,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let metrics = album_detail_row_metrics(shell, key);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, metrics.spacing);
    row.add_css_class("album-detail-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_margin_bottom(if last_in_album { 16 } else { 0 });
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_height_request(ALBUM_DETAIL_TRACK_ROW_HEIGHT);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_width_request(metrics.meta_width);
    spacer.set_hexpand(false);
    row.append(&spacer);
    row.append(&album_detail_separator(ALBUM_DETAIL_TRACK_ROW_HEIGHT));

    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(key)
        .detail_track_fields;
    let track_width =
        album_detail_track_area_width(shell, key, metrics.meta_width, metrics.spacing, &fields);
    let field_widths = album_detail_track_field_widths(key, &fields, track_width);
    row.append(&album_detail_track_cells(
        shell,
        track,
        index,
        &field_widths,
        query,
        selection,
    ));
    row.upcast()
}
pub(crate) fn album_detail_separator(height: i32) -> gtk::Widget {
    let separator = gtk::Separator::new(gtk::Orientation::Vertical);
    separator.add_css_class("album-detail-separator");
    separator.set_width_request(ALBUM_DETAIL_SEPARATOR_WIDTH);
    separator.set_height_request(height.max(1));
    separator.set_valign(gtk::Align::Fill);
    separator.upcast()
}
pub(crate) fn album_detail_meta(
    shell: &Rc<Shell>,
    album: &Album,
    cover_size: i32,
    meta_width: i32,
    query: &ActiveLibraryQuery,
) -> gtk::Widget {
    let meta = gtk::Box::new(gtk::Orientation::Vertical, ALBUM_DETAIL_META_SPACING);
    meta.set_width_request(meta_width);
    meta.set_height_request(album_detail_meta_height(album, cover_size));
    meta.set_hexpand(false);
    meta.append(&album_detail_cover_tile(shell, album, cover_size, query));
    meta.append(&album_detail_meta_label(
        &album.title,
        "track-title",
        meta_width,
    ));
    meta.append(&album_detail_meta_label(&album.artist, "muted", meta_width));
    meta.append(&album_detail_meta_label(
        &album_fact_text(album),
        "muted",
        meta_width,
    ));
    if !album.genres.is_empty() {
        meta.append(&album_detail_meta_label(
            &album.genres.join(", "),
            "muted",
            meta_width,
        ));
    }
    meta.upcast()
}
pub(crate) fn album_detail_cover_tile(
    shell: &Rc<Shell>,
    album: &Album,
    cover_size: i32,
    query: &ActiveLibraryQuery,
) -> gtk::Widget {
    let overlay = cards::cover_overlay(cover_size);
    let cover_button = gtk::Button::new();
    cover_button.add_css_class("flat");
    cover_button.add_css_class("album-cover-button");
    cards::constrain_cover_widget(&cover_button, cover_size);
    cards::clip_cover(&cover_button);
    cover_button.set_child(Some(&shell.cover_tile_for_candidates(
        ArtworkBinding::album(album),
        album.color_seed,
        cover_size,
        GRID_COVER_SIZE,
    )));
    let shell_for_open = Rc::clone(shell);
    let album_id = album.id.clone();
    cover_button.connect_clicked(move |_| {
        shell_for_open.navigate(Route::AlbumDetail(album_id.clone()));
    });
    overlay.set_child(Some(&cover_button));

    let controls = cards::cover_hover_controls(cover_size, "Play album", album.favorite);
    let controller = shell.products.playback.queue.clone();
    let play_query = query.clone();
    let album_id = album.id.clone();
    controls.play.connect_clicked(move |_| {
        if let Ok(Some((album, tracks))) = play_query.album_detail(&album_id) {
            controller.play_album(AlbumPlayRequest {
                album_id: album.id,
                tracks,
                anchor_index: 0,
                shuffled_start: true,
            });
        }
    });

    let controller = shell.products.playback.queue.clone();
    let next_query = query.clone();
    let album_id = album.id.clone();
    controls.play_next.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = next_query.album_detail(&album_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });

    let controller = shell.products.playback.queue.clone();
    let last_query = query.clone();
    let album_id = album.id.clone();
    controls.play_last.connect_clicked(move |_| {
        if let Ok(Some((_, tracks))) = last_query.album_detail(&album_id) {
            controller.play_last(tracks);
        }
    });

    if let Some(favorite) = controls.favorite.as_ref() {
        shell
            .favorites
            .register_button(album_favorite_key(&album.id), favorite);
        let favorite_shell = Rc::clone(shell);
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Album(album_id.clone()),
                favorite,
                Some(button),
            );
        });
    }

    controls.add_to_overlay(&overlay);
    controls.connect_hover(&overlay);
    install_album_context_menu(&overlay, shell, album.clone());
    overlay.upcast()
}
pub(crate) fn album_detail_track_header(field_widths: &[(LibraryField, i32)]) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, ALBUM_DETAIL_TRACK_COLUMN_GAP);
    row.add_css_class("album-detail-track-cells");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_width_request(album_detail_track_cells_width(field_widths));
    row.set_height_request(ALBUM_DETAIL_TRACK_HEADER_HEIGHT);
    row.set_margin_end(album_detail_track_trailing_inset(field_widths));
    for (field, width) in field_widths {
        row.append(&album_detail_track_cell_box(
            *field,
            *width,
            ALBUM_DETAIL_TRACK_HEADER_HEIGHT,
            album_detail_track_header_cell(*field, *width),
        ));
    }
    row.upcast()
}
pub(crate) fn album_detail_track_header_cell(field: LibraryField, width: i32) -> gtk::Widget {
    if field == LibraryField::Duration {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_width_request(width);
        row.set_halign(gtk::Align::Fill);
        let image = gtk::Image::from_icon_name("appointment-soon-symbolic");
        let label = tr("Duration");
        image.add_css_class("muted");
        image.set_halign(gtk::Align::Start);
        image.set_tooltip_text(Some(&label));
        image.update_property(&[gtk::accessible::Property::Label(&label)]);
        row.append(&image);
        return row.upcast();
    }

    let label = album_detail_track_label(&tr(field.title()), field, width, false);
    label.add_css_class("muted");
    label.upcast()
}
pub(crate) fn album_detail_track_cells(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    field_widths: &[(LibraryField, i32)],
    query: &ActiveLibraryQuery,
    selection: &AlbumDetailTrackSelection,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, ALBUM_DETAIL_TRACK_COLUMN_GAP);
    row.add_css_class("album-detail-track-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_width_request(album_detail_track_cells_width(field_widths));
    row.set_height_request(ALBUM_DETAIL_TRACK_ROW_HEIGHT);
    row.set_valign(gtk::Align::Center);
    row.set_margin_end(album_detail_track_trailing_inset(field_widths));
    row.set_focusable(false);
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} {}",
        track.title, track.artist
    ))]);
    selection.bind_row(row.upcast_ref(), &track.id);
    for (field, width) in field_widths {
        row.append(&album_detail_track_cell(
            shell, track, index, *field, *width,
        ));
    }
    install_track_context_menu(&row, shell, track.clone());

    let controller = shell.products.playback.queue.clone();
    let query = query.clone();
    let track = track.clone();
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.set_button(1);
    gesture.connect_pressed(move |gesture, n_press, _, _| {
        if album_detail_play_click(n_press) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let controller = controller.clone();
            let query = query.clone();
            let track = track.clone();
            glib::idle_add_local_once(move || play_album_track(&query, &controller, track));
        }
    });
    row.add_controller(gesture);
    row.upcast()
}
fn album_detail_play_click(n_press: i32) -> bool {
    n_press >= 2 && n_press % 2 == 0
}

fn play_album_track(query: &ActiveLibraryQuery, controller: &playback::QueueHandle, track: Track) {
    let Ok(Some((album, tracks))) = query.album_detail(&track.album_id) else {
        controller.play_now(track);
        return;
    };
    let Some(anchor_index) = tracks.iter().position(|candidate| candidate.id == track.id) else {
        controller.play_now(track);
        return;
    };
    controller.play_album(AlbumPlayRequest {
        album_id: album.id,
        tracks,
        anchor_index,
        shuffled_start: false,
    });
}
pub(crate) fn album_detail_track_cell(
    shell: &Rc<Shell>,
    track: &Track,
    index: usize,
    field: LibraryField,
    width: i32,
) -> gtk::Widget {
    match field {
        LibraryField::Favorite => {
            let button = favorite_icon_button("Favorite track");
            set_favorite_button_active(&button, track.favorite);
            shell
                .favorites
                .register_button(track_favorite_key(&track.id), &button);
            let favorite_shell = Rc::clone(shell);
            let track_id = track.id.clone();
            button.connect_clicked(move |button| {
                let favorite = !favorite_button_is_active(button);
                favorite_shell.set_favorite_with_feedback(
                    library::FavoriteItemId::Track(track_id.clone()),
                    favorite,
                    Some(button),
                );
            });
            button.set_halign(gtk::Align::Center);
            album_detail_fixed_cell(width, button.upcast())
        }
        LibraryField::Image => {
            let cover = shell.cover_tile_for_candidates(
                ArtworkBinding::track(track),
                stable_seed(track.id.as_str()),
                48,
                THUMB_COVER_SIZE,
            );
            cover.set_halign(gtk::Align::Center);
            album_detail_fixed_cell(width, cover)
        }
        _ => album_detail_track_cell_box(
            field,
            width,
            ALBUM_DETAIL_TRACK_ROW_HEIGHT,
            album_detail_track_label(
                &album_detail_track_text(track, index, field),
                field,
                width,
                true,
            )
            .upcast(),
        ),
    }
}
pub(crate) fn album_detail_fixed_cell(width: i32, child: gtk::Widget) -> gtk::Widget {
    album_detail_fixed_cell_height(width, ALBUM_DETAIL_TRACK_ROW_HEIGHT, child)
}
pub(crate) fn album_detail_track_cell_box(
    field: LibraryField,
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    if field == LibraryField::Title {
        return album_detail_expanding_cell_height(width, height, child);
    }

    album_detail_fixed_cell_height(width, height, child)
}
pub(crate) fn album_detail_expanding_cell_height(
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    let width = width.max(1);
    let height = height.max(1);
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    cell.set_width_request(width);
    cell.set_height_request(height);
    cell.set_size_request(width, height);
    cell.set_hexpand(true);
    cell.set_halign(gtk::Align::Fill);
    cell.set_valign(gtk::Align::Fill);
    cell.set_overflow(gtk::Overflow::Hidden);
    child.set_hexpand(true);
    child.set_halign(gtk::Align::Fill);
    cell.append(&child);
    cell.upcast()
}
pub(crate) fn album_detail_fixed_cell_height(
    width: i32,
    height: i32,
    child: gtk::Widget,
) -> gtk::Widget {
    let width = width.max(1);
    let height = height.max(1);
    let clip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    clip.set_overflow(gtk::Overflow::Hidden);
    clip.set_width_request(width);
    clip.set_size_request(width, height);
    clip.set_height_request(height);
    clip.set_hexpand(false);
    clip.set_halign(gtk::Align::Fill);
    clip.set_valign(gtk::Align::Fill);
    child.set_hexpand(true);
    child.set_halign(gtk::Align::Fill);
    clip.append(&child);
    clip.upcast()
}
pub(crate) fn album_detail_track_label(
    text: &str,
    field: LibraryField,
    width: i32,
    ellipsize: bool,
) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    if ellipsize && matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
        label.add_css_class("track-list-title");
    }
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Fill);
    label.set_valign(gtk::Align::Center);
    label.set_wrap(false);
    label.set_lines(1);
    label.set_single_line_mode(true);
    label.set_width_request(width);
    label.set_size_request(width, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(1);
    label.set_hexpand(false);
    if ellipsize {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}
pub(crate) fn album_detail_track_trailing_inset(field_widths: &[(LibraryField, i32)]) -> i32 {
    let _ = field_widths;
    0
}
pub(crate) fn album_detail_track_field_widths(
    key: LibraryListKey,
    fields: &[LibraryField],
    available_width: i32,
) -> Vec<(LibraryField, i32)> {
    let gap_count = fields.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    let gap_total = ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count);
    let available_width = available_width
        .saturating_sub(gap_total)
        .max(fields.len() as i32);
    fields
        .iter()
        .copied()
        .zip(album_detail_text_column_widths(
            key,
            fields,
            available_width,
        ))
        .collect()
}
fn album_detail_text_column_widths(
    key: LibraryListKey,
    fields: &[LibraryField],
    available_width: i32,
) -> Vec<i32> {
    if fields.is_empty() {
        return Vec::new();
    }

    let base_widths = fields
        .iter()
        .map(|field| album_detail_track_column_width(key, *field))
        .collect::<Vec<_>>();
    let fixed_total = fields
        .iter()
        .zip(base_widths.iter())
        .filter_map(|(field, width)| (*field != LibraryField::Title).then_some(*width))
        .sum::<i32>();
    let title_count = fields
        .iter()
        .filter(|field| **field == LibraryField::Title)
        .count()
        .min(i32::MAX as usize) as i32;
    if title_count == 0 {
        return fitted_column_widths(&base_widths, available_width);
    }
    if fixed_total.saturating_add(title_count) > available_width {
        return fitted_column_widths(&base_widths, available_width);
    }

    let remaining_for_title = available_width.saturating_sub(fixed_total).max(title_count);
    fields
        .iter()
        .zip(base_widths.iter())
        .map(|(field, width)| {
            if *field == LibraryField::Title {
                remaining_for_title / title_count
            } else {
                *width
            }
        })
        .collect()
}
pub(crate) fn album_detail_track_column_width(key: LibraryListKey, field: LibraryField) -> i32 {
    match field {
        LibraryField::RowIndex => 40,
        LibraryField::TrackNumber => 52,
        LibraryField::DiscNumber => 44,
        LibraryField::Duration => 48,
        LibraryField::Year | LibraryField::Bpm => 52,
        LibraryField::PlayCount => play_count_column_width().min(56),
        LibraryField::UserRating | LibraryField::SongCount | LibraryField::AlbumCount => 64,
        LibraryField::Favorite => 40,
        LibraryField::Image => 56,
        LibraryField::Title | LibraryField::TitleMerged => track_column_width(key, field),
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 160,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 96,
    }
}
pub(crate) fn album_detail_track_cells_width(field_widths: &[(LibraryField, i32)]) -> i32 {
    let fields_width = field_widths.iter().map(|(_, width)| *width).sum::<i32>();
    let gap_count = field_widths.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    let gap_total = ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count);
    fields_width.saturating_add(gap_total).max(1)
}
pub(crate) fn album_detail_track_area_width(
    shell: &Shell,
    key: LibraryListKey,
    meta_width: i32,
    spacing: i32,
    fields: &[LibraryField],
) -> i32 {
    album_detail_track_area_width_for(key, route_content_width(shell), meta_width, spacing, fields)
}

fn album_detail_track_area_width_for(
    key: LibraryListKey,
    route_width: i32,
    meta_width: i32,
    spacing: i32,
    fields: &[LibraryField],
) -> i32 {
    album_detail_row_content_width(key, route_width)
        .saturating_sub(meta_width)
        .saturating_sub(ALBUM_DETAIL_SEPARATOR_WIDTH)
        .saturating_sub(spacing.saturating_mul(2))
        .max(album_detail_min_track_area_width(fields))
}
pub(crate) fn album_detail_track_text(track: &Track, index: usize, field: LibraryField) -> String {
    if field == LibraryField::RowIndex {
        (index + 1).to_string()
    } else {
        track_field(track, field)
    }
}
pub(crate) fn album_detail_lead_content_height(
    album: &Album,
    cover_size: i32,
    inline_count: usize,
) -> i32 {
    album_detail_meta_height(album, cover_size).max(album_detail_track_area_height(inline_count))
}
fn album_detail_item_total_height(row: &AlbumDetailItem, cover_size: i32) -> i32 {
    match row {
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album,
        } => {
            album_detail_lead_content_height(album, cover_size, inline_tracks.len())
                + 12
                + if *last_in_album { 16 } else { 0 }
        }
        AlbumDetailItem::Track { last_in_album, .. } => {
            ALBUM_DETAIL_TRACK_ROW_HEIGHT + if *last_in_album { 16 } else { 0 }
        }
    }
}
pub(crate) fn album_detail_meta_height(album: &Album, cover_size: i32) -> i32 {
    cover_size
        + album_detail_meta_label_count(album) as i32
            * (ALBUM_DETAIL_META_LABEL_HEIGHT + ALBUM_DETAIL_META_SPACING)
        + ALBUM_DETAIL_META_LABEL_HEIGHT
}
pub(crate) fn album_detail_meta_label_count(album: &Album) -> usize {
    3 + usize::from(!album.genres.is_empty())
}
pub(crate) fn album_detail_track_area_height(inline_count: usize) -> i32 {
    if inline_count == 0 {
        0
    } else {
        ALBUM_DETAIL_TRACK_HEADER_HEIGHT
            + inline_count.min(ALBUM_DETAIL_INLINE_TRACK_ROWS) as i32
                * ALBUM_DETAIL_TRACK_ROW_HEIGHT
    }
}
pub(crate) fn sort_album_detail_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|left, right| compare_track(left, right, LibraryField::TrackNumber));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AlbumDetailRowMetrics {
    cover_size: i32,
    meta_width: i32,
    spacing: i32,
}

fn album_detail_row_metrics(shell: &Shell, key: LibraryListKey) -> AlbumDetailRowMetrics {
    album_detail_row_metrics_for_width(key, route_content_width(shell))
}

fn album_detail_row_metrics_for_width(
    key: LibraryListKey,
    route_width: i32,
) -> AlbumDetailRowMetrics {
    let row_width = album_detail_row_content_width(key, route_width);
    let spacing = if row_width < ALBUM_DETAIL_NARROW_WIDTH {
        8
    } else {
        12
    };
    let target_cover = if row_width < ALBUM_DETAIL_NARROW_WIDTH {
        row_width * 38 / 100
    } else {
        row_width * 42 / 100
    };
    let cover_size = if row_width < ALBUM_DETAIL_MIN_COVER {
        row_width
    } else {
        target_cover.clamp(ALBUM_DETAIL_MIN_COVER, ALBUM_DETAIL_MAX_COVER)
    }
    .min(row_width)
    .max(1);

    AlbumDetailRowMetrics {
        cover_size,
        meta_width: cover_size,
        spacing,
    }
}

fn album_detail_row_content_width(key: LibraryListKey, route_width: i32) -> i32 {
    route_width
        .saturating_sub(album_detail_route_inset(key))
        .saturating_sub(ALBUM_DETAIL_ROW_HORIZONTAL_INSET)
        .max(1)
}

fn album_detail_route_inset(key: LibraryListKey) -> i32 {
    let _ = key;
    PRIMARY_ROUTE_MARGIN_START.saturating_add(PRIMARY_ROUTE_MARGIN_END)
}

fn album_detail_min_track_area_width(fields: &[LibraryField]) -> i32 {
    let field_count = fields.len().min(i32::MAX as usize) as i32;
    let gap_count = fields.len().saturating_sub(1).min(i32::MAX as usize) as i32;
    field_count + ALBUM_DETAIL_TRACK_COLUMN_GAP.saturating_mul(gap_count)
}

#[cfg(test)]
mod album_detail_width_tests {
    use crate::{LibraryField, LibraryListKey};
    use ::library::TrackId;
    use adw::prelude::*;

    use super::{
        ALBUM_DETAIL_MAX_COVER, ALBUM_DETAIL_ROW_HORIZONTAL_INSET, ALBUM_DETAIL_SEPARATOR_WIDTH,
        AlbumDetailRowMetrics, AlbumDetailTrackSelection, album_detail_play_click,
        album_detail_row_content_width, album_detail_row_metrics_for_width,
        album_detail_track_area_width_for, album_detail_track_cells_width,
        album_detail_track_field_widths,
    };
    use crate::routes::route_layout::{PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START};

    #[test]
    fn album_detail_meta_shrinks_with_route() {
        let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, 260);

        assert_eq!(metrics.spacing, 8);
        assert!(metrics.meta_width < ALBUM_DETAIL_MAX_COVER);
        assert_eq!(metrics.cover_size, metrics.meta_width);
    }

    #[test]
    fn album_detail_track_width_stays_in_narrow_budget() {
        let fields = [
            LibraryField::RowIndex,
            LibraryField::Title,
            LibraryField::Duration,
        ];
        let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, 260);
        let track_width = album_detail_track_area_width_for(
            LibraryListKey::Albums,
            260,
            metrics.meta_width,
            metrics.spacing,
            &fields,
        );

        let row_width = album_detail_rendered_width(
            metrics,
            album_detail_track_cells_width(&album_detail_track_field_widths(
                LibraryListKey::Albums,
                &fields,
                track_width,
            )),
        );
        assert!(row_width <= album_detail_row_content_width(LibraryListKey::Albums, 260));
    }

    #[test]
    fn album_detail_width_respects_route_side_insets() {
        let fields = [
            LibraryField::Image,
            LibraryField::Title,
            LibraryField::Year,
            LibraryField::Duration,
        ];

        for route_width in [360, 562, 1145] {
            let metrics = album_detail_row_metrics_for_width(LibraryListKey::Albums, route_width);
            let track_width = album_detail_track_area_width_for(
                LibraryListKey::Albums,
                route_width,
                metrics.meta_width,
                metrics.spacing,
                &fields,
            );
            let row_width = album_detail_rendered_width(
                metrics,
                album_detail_track_cells_width(&album_detail_track_field_widths(
                    LibraryListKey::Albums,
                    &fields,
                    track_width,
                )),
            );
            let route_budget = route_width
                .saturating_sub(PRIMARY_ROUTE_MARGIN_START)
                .saturating_sub(PRIMARY_ROUTE_MARGIN_END)
                .saturating_sub(ALBUM_DETAIL_ROW_HORIZONTAL_INSET)
                .max(1);
            assert!(row_width <= route_budget);
        }
    }

    fn album_detail_rendered_width(metrics: AlbumDetailRowMetrics, track_width: i32) -> i32 {
        metrics
            .meta_width
            .saturating_add(ALBUM_DETAIL_SEPARATOR_WIDTH)
            .saturating_add(metrics.spacing.saturating_mul(2))
            .saturating_add(track_width)
    }

    #[test]
    fn album_detail_repeated_double_clicks_activate() {
        assert!(!album_detail_play_click(1));
        assert!(album_detail_play_click(2));
        assert!(!album_detail_play_click(3));
        assert!(album_detail_play_click(4));
    }

    #[test]
    fn album_detail_now_playing_follows_track_changes_and_virtual_rebinds() {
        gtk::init().expect("initialize GTK");
        let first_id = TrackId::fake(1);
        let second_id = TrackId::fake(2);
        let selection = AlbumDetailTrackSelection::default();
        let first = gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast::<gtk::Widget>();
        let second = gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast::<gtk::Widget>();
        selection.bind_row(&first, &first_id);
        selection.bind_row(&second, &second_id);

        selection.select_now_playing_track(Some(&first_id));
        assert!(first.has_css_class("album-detail-track-selected"));
        assert!(!second.has_css_class("album-detail-track-selected"));

        selection.select_now_playing_track(Some(&second_id));
        assert!(!first.has_css_class("album-detail-track-selected"));
        assert!(second.has_css_class("album-detail-track-selected"));

        selection.clear_bound_rows();
        let rebound = gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast::<gtk::Widget>();
        selection.bind_row(&rebound, &second_id);
        assert!(rebound.has_css_class("album-detail-track-selected"));

        selection.select_now_playing_track(None);
        assert!(!rebound.has_css_class("album-detail-track-selected"));
    }
}
