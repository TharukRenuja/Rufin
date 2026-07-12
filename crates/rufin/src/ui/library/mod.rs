use super::{
    ArtworkTile, GRID_COVER_SIZE, GRID_ROUTE_PAGE_SIZE, LoadedTrackPlayContext,
    PRIMARY_ROUTE_HORIZONTAL_INSET, PRIMARY_ROUTE_MARGIN_END, PRIMARY_ROUTE_MARGIN_START,
    PagedGridCursor, PlayContextDescriptor, ROUTE_TOP_MARGIN, Route, SLOW_ROUTE_PAGE_LOAD_MS,
    Shell, THUMB_COVER_SIZE, TRACK_ROUTE_PAGE_SIZE, add_dynamic_link_hover, add_label_click,
    album_artist_route, album_count_text, album_favorite_key, append_albums_to_model,
    append_artists_to_model, append_boxed_items_to_model, append_playlists_to_model,
    append_tracks_to_model, artist_favorite_key, cards, connect_paged_grid_loader, context_album,
    context_artist, context_track, favorite_button_is_active, favorite_icon_button,
    finish_grid_page, format_duration_units, icon_button, install_album_context_menu,
    install_artist_context_menu, install_context_menu_openers, install_dynamic_album_context_menu,
    install_dynamic_track_context_menu, install_dynamic_track_context_menu_with_play_handler,
    install_genre_context_menu, install_track_context_menu,
    layout::{
        WINDOW_CHROME_MARGIN_END, configure_fill_width_clip, large_popup_content_height,
        large_popup_content_width, route_content_width,
    },
    present_album_context_menu, present_artist_context_menu, present_genre_context_menu,
    present_light_dismiss_dialog, present_playlist_context_menu,
    present_smart_playlist_context_menu, present_track_context_menu, replace_albums_in_model,
    replace_artists_in_model, replace_playlists_in_model, route_scroller_widget,
    selected_music_folder_id, set_favorite_button_active, smart_playlist_display_name, stable_seed,
    track_artist_route, track_collection_play_context, track_count_text, track_favorite_key,
    track_link_column, track_matches_query,
};
use crate::i18n::tr;
use ::library::{
    Album, AlbumId, Artist, Genre, Mood, Playlist, SmartPlaylist, SmartPlaylistId, Track, TrackId,
};
use adw::prelude::*;
use artwork::CandidateSet;
use domain::{
    LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings, available_sort_fields,
};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

mod album_detail;
mod collection_routes;
mod collections;
mod columns;
#[path = "cards.rs"]
mod field_cards;
mod grid_cells;
mod models;
mod named_collections;
mod route_shell;
mod routes;
mod table_sizing;

pub(super) use album_detail::*;
use collection_routes::*;
pub(super) use collections::*;
pub(super) use columns::*;
pub(super) use field_cards::*;
use grid_cells::*;
pub(super) use models::*;
pub(super) use named_collections::*;
pub(super) use route_shell::*;
pub(super) use table_sizing::*;

#[cfg(test)]
mod route_tests;

const LIBRARY_CONFIG_DIALOG_WIDTH: i32 = 620;
const LIBRARY_CONFIG_DIALOG_HEIGHT: i32 = 560;
const LIBRARY_TABLE_HEADER_HEIGHT: i32 = 92;
pub(in crate::ui) const LIBRARY_TABLE_ROW_HEIGHT: i32 = 58;
pub(in crate::ui) const LIBRARY_ROUTE_BOTTOM_MARGIN: i32 = 8;
const ALBUM_DETAIL_INLINE_TRACK_ROWS: usize = 8;
const ALBUM_DETAIL_TRACK_ROW_HEIGHT: i32 = 36;
const ALBUM_DETAIL_TRACK_HEADER_HEIGHT: i32 = 26;
const ALBUM_DETAIL_META_SPACING: i32 = 6;
const ALBUM_DETAIL_META_LABEL_HEIGHT: i32 = 20;
const SLOW_LIBRARY_ROUTE_SETUP_MS: u64 = 100;
#[derive(Clone)]
pub(in crate::ui) struct TrackTableSelection {
    model: gio::ListStore,
    selection: gtk::SingleSelection,
    selected_position: Rc<Cell<u32>>,
}
pub(in crate::ui) type TrackTableSelectionHandle = Rc<RefCell<Option<TrackTableSelection>>>;

impl TrackTableSelection {
    pub(in crate::ui) fn new(model: &gio::ListStore, selection: &gtk::SingleSelection) -> Self {
        selection.set_selected(gtk::INVALID_LIST_POSITION);
        Self {
            model: model.clone(),
            selection: selection.clone(),
            selected_position: Rc::new(Cell::new(gtk::INVALID_LIST_POSITION)),
        }
    }

    pub(in crate::ui) fn install_guard(&self) {
        let selected_position = Rc::clone(&self.selected_position);
        self.selection
            .connect_selection_changed(move |selection, _, _| {
                let selected = selected_position.get();
                if selection.selected() != selected {
                    selection.set_selected(selected);
                }
            });
    }

    pub(in crate::ui) fn select(&self, position: u32) {
        self.selected_position.set(position);
        self.selection.set_selected(position);
    }

    pub(in crate::ui) fn clear(&self) {
        self.select(gtk::INVALID_LIST_POSITION);
    }

    pub(in crate::ui) fn select_track_id(&self, track_id: &TrackId) -> bool {
        if let Some(position) = (0..self.model.n_items()).find(|position| {
            self.model
                .item(*position)
                .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                .is_some_and(|boxed| boxed.borrow::<Track>().id == *track_id)
        }) {
            self.select(position);
            true
        } else {
            false
        }
    }

    pub(in crate::ui) fn select_now_playing_track(&self, track_id: Option<&TrackId>) {
        if track_id.is_some_and(|track_id| self.select_track_id(track_id)) {
            return;
        }
        self.clear();
    }
}

fn library_layout_loads_complete_page(
    _key: LibraryListKey,
    _settings: &LibraryListSettings,
) -> bool {
    true
}
pub(in crate::ui) fn complete_cached_page<T>(
    page: library::PagedResponse<T>,
    load_complete: bool,
    mut load_all: impl FnMut(usize) -> Result<library::PagedResponse<T>, String>,
    page_name: &str,
) -> library::PagedResponse<T> {
    if !load_complete || page.items.len() >= page.total {
        return page;
    }

    let loaded = page.items.len();
    let total = page.total;
    match load_all(total) {
        Ok(page) => page,
        Err(error) => {
            warn!(
                %error,
                page = page_name,
                loaded,
                total,
                "failed to load complete cached page"
            );
            page
        }
    }
}
fn track_page_is_complete(loaded: usize, total: usize) -> bool {
    loaded >= total
}
fn track_route_has_complete_page(
    loaded: usize,
    total: usize,
    settings: &LibraryListSettings,
) -> bool {
    library_layout_loads_complete_page(LibraryListKey::Tracks, settings)
        && track_page_is_complete(loaded, total)
}
