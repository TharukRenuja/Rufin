use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;
use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{
    Album, AlbumId, Artist, Genre, ImageRef, LibraryField, LibraryLayout, LibraryListKey,
    LibraryListSettings, Playlist, Track, available_sort_fields, format_duration,
};
use tracing::{info, warn};
use super::{
    GRID_COVER_SIZE, GRID_ROUTE_PAGE_SIZE, PRIMARY_ROUTE_MARGIN_START, Route, Shell,
    THUMB_COVER_SIZE, TRACK_ROUTE_PAGE_SIZE, album_favorite_key, append_albums_to_model,
    append_artists_to_model, append_genres_to_model, append_playlists_to_model,
    append_tracks_to_model, artist_favorite_key, cards, connect_paged_grid_loader,
    favorite_button_is_active, favorite_icon_button, finish_grid_page, icon_button,
    install_album_context_menu, install_artist_context_menu, install_track_context_menu,
    layout::{large_popup_content_height, large_popup_content_width, route_content_width},
    replace_albums_in_model, replace_artists_in_model, replace_genres_in_model,
    replace_playlists_in_model, set_favorite_button_active, stable_seed, text_button,
};
use crate::i18n::tr;
const LIBRARY_CONFIG_DIALOG_WIDTH: i32 = 620;
const LIBRARY_CONFIG_DIALOG_HEIGHT: i32 = 560;
const LIBRARY_TABLE_HEADER_HEIGHT: i32 = 92;
const LIBRARY_TABLE_ROW_HEIGHT: i32 = 58;
const LIBRARY_ROUTE_BOTTOM_MARGIN: i32 = 8;
const ALBUM_DETAIL_FIXED_TRAILING_INSET: i32 = 64;
const ALBUM_DETAIL_INLINE_TRACK_ROWS: usize = 4;
const TRACK_INITIAL_COMPLETE_ROWS: usize = TRACK_ROUTE_PAGE_SIZE;
const TRACK_COMPLETE_APPEND_ROWS: usize = 256;
fn library_layout_loads_complete_page(key: LibraryListKey, settings: &LibraryListSettings) -> bool {
    match key {
        LibraryListKey::Tracks => settings.layout == LibraryLayout::Row,
        LibraryListKey::Albums => matches!(
            settings.layout,
            LibraryLayout::Row | LibraryLayout::Grid | LibraryLayout::Detail
        ),
        LibraryListKey::Artists
        | LibraryListKey::AlbumArtists
        | LibraryListKey::Genres
        | LibraryListKey::Playlists => {
            matches!(settings.layout, LibraryLayout::Row | LibraryLayout::Grid)
        }
        _ => false,
    }
}
fn complete_cached_page<T>(
    page: rufin_provider::PagedResponse<T>,
    load_complete: bool,
    mut load_all: impl FnMut(usize) -> Result<rufin_provider::PagedResponse<T>, String>,
    page_name: &str,
) -> rufin_provider::PagedResponse<T> {
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
fn warm_track_covers_for_settings(
    shell: &Rc<Shell>,
    tracks: &[Track],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = track_cover_warm_sizes(shell, settings) else {
        return;
    };
    let image_refs = tracks
        .iter()
        .filter_map(|track| track.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.warm_cover_refs(image_refs, fetch_size, size);
}
fn track_cover_warm_sizes(shell: &Rc<Shell>, settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid => Some((GRID_COVER_SIZE, shell.responsive_card_grid_metrics().1)),
        LibraryLayout::Row => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Detail => None,
    }
}
