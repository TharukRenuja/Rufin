use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use gtk::{gio, glib};
use rufin_core::{
    Album, AlbumId, Artist, Genre, ImageRef, LibraryField, LibraryLayout, LibraryListKey,
    LibraryListSettings, Playlist, Track, TrackId, available_sort_fields, format_duration,
};
use tracing::{info, warn};
use super::{
    ArtworkTile, CoverDecodePriority, DETAIL_COVER_SIZE, GRID_COVER_SIZE, GRID_ROUTE_PAGE_SIZE,
    PRIMARY_ROUTE_MARGIN_START, Route, Shell, THUMB_COVER_SIZE, TRACK_ROUTE_PAGE_SIZE,
    album_favorite_key, append_albums_to_model, append_artists_to_model, append_genres_to_model,
    append_playlists_to_model, append_tracks_to_model, artist_favorite_key, cards,
    connect_paged_grid_loader, cover_decode_size, favorite_button_is_active, favorite_icon_button,
    finish_grid_page, icon_button, install_album_context_menu, install_artist_context_menu,
    install_dynamic_album_context_menu, install_dynamic_track_context_menu, install_track_context_menu,
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
const ALBUM_DETAIL_INLINE_TRACK_ROWS: usize = 8;
const ALBUM_DETAIL_TRACK_HEADER_HEIGHT: i32 = 34;
const ALBUM_DETAIL_META_SPACING: i32 = 6;
const ALBUM_DETAIL_META_LABEL_HEIGHT: i32 = 20;
const ALBUM_ROUTE_COVER_GATE_ITEMS: usize = 16;
const ROUTE_COVER_GATE_POLL_MS: u64 = 33;
const ROUTE_COVER_GATE_TIMEOUT_MS: u64 = 3_000;
const ROUTE_COVER_GATE_SYNC_DECODE_LIMIT: usize = 0;
const TRACK_ROUTE_COVER_GATE_ROWS: usize = 64;
const TRACK_ROW_CONTRACT_SCROLL_DELAY_MS: u64 = 250;
const TRACK_VIEWPORT_COVER_WARM_AHEAD_ROWS: usize = 128;
const TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS: usize = 16;
const TRACK_VIEWPORT_COVER_WARM_DELAY_MS: u64 = 32;
const ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS: usize = 160;
const ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS: usize = 16;
const ALBUM_VIEWPORT_COVER_WARM_DELAY_MS: u64 = 32;
#[derive(Clone, Copy)]
enum RouteCoverMissingPolicy {
    Any,
}
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
    let shell = Rc::clone(shell);
    let tracks = tracks.to_vec();
    let settings = settings.clone();
    glib::idle_add_local_once(move || {
        warm_track_covers_for_settings_now(&shell, &tracks, &settings);
    });
}
fn warm_track_covers_for_settings_now(
    shell: &Rc<Shell>,
    tracks: &[Track],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = track_cover_warm_sizes(shell, settings) else {
        return;
    };
    let image_refs = track_cover_refs_for_settings_limited(
        tracks,
        settings,
        Some(TRACK_ROUTE_COVER_GATE_ROWS),
    );
    shell.warm_cover_refs(image_refs, fetch_size, size);
}
fn gate_track_route_covers(
    shell: &Rc<Shell>,
    _tracks: &[Track],
    _settings: &LibraryListSettings,
) -> bool {
    shell.clear_route_cover_gate("tracks");
    false
}
#[cfg(test)]
fn track_cover_refs_for_settings(tracks: &[Track], settings: &LibraryListSettings) -> Vec<ImageRef> {
    track_cover_refs_for_settings_limited(tracks, settings, None)
}
fn track_cover_refs_for_settings_limited(
    tracks: &[Track],
    settings: &LibraryListSettings,
    row_limit: Option<usize>,
) -> Vec<ImageRef> {
    let mut values = tracks.to_vec();
    sort_tracks(&mut values, settings, false);
    let mut refs = Vec::new();
    let limit = row_limit.unwrap_or(usize::MAX);
    for track in values.into_iter().take(limit) {
        let Some(image_ref) = track.image_ref else {
            continue;
        };
        if refs.iter().any(|existing| existing == &image_ref) {
            continue;
        }
        refs.push(image_ref);
    }
    refs
}
fn gate_album_route_covers(
    shell: &Rc<Shell>,
    albums: &[Album],
    settings: &LibraryListSettings,
) -> bool {
    if !album_route_cover_gate_enabled(settings) {
        shell.clear_route_cover_gate("albums");
        return false;
    }
    let Some((fetch_size, size)) = album_cover_warm_sizes(shell, settings) else {
        shell.clear_route_cover_gate("albums");
        return false;
    };
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    let image_refs = values
        .iter()
        .take(album_route_cover_gate_items(settings))
        .filter_map(|album| album.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.route_cover_gate_needs_loading(
        "albums",
        image_refs,
        fetch_size,
        size,
        RouteCoverMissingPolicy::Any,
    )
}
fn album_route_cover_gate_enabled(settings: &LibraryListSettings) -> bool {
    settings.layout == LibraryLayout::Detail
}
fn gate_artist_route_covers(
    shell: &Rc<Shell>,
    _artists: &[Artist],
    settings: &LibraryListSettings,
    album_artist: bool,
) -> bool {
    let route_key = artist_route_cover_gate_key(album_artist);
    shell.clear_route_cover_gate(route_key);
    artist_route_cover_gate_enabled(settings)
}
fn artist_route_cover_gate_enabled(_settings: &LibraryListSettings) -> bool {
    false
}
fn artist_route_cover_gate_key(album_artist: bool) -> &'static str {
    if album_artist {
        "album_artists"
    } else {
        "artists"
    }
}
fn album_route_cover_gate_items(_settings: &LibraryListSettings) -> usize {
    ALBUM_ROUTE_COVER_GATE_ITEMS
}
fn route_cover_gate_key_for_current_route(shell: &Shell) -> Option<&'static str> {
    match shell.state.routes.borrow().current() {
        Route::Tracks => Some("tracks"),
        Route::Albums => Some("albums"),
        Route::Artists => Some("artists"),
        Route::AlbumArtists => Some("album_artists"),
        _ => None,
    }
}
impl Shell {
    fn route_cover_gate_needs_loading(
        self: &Rc<Self>,
        route_key: &'static str,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
        missing_policy: RouteCoverMissingPolicy,
    ) -> bool {
        let decode_size = cover_decode_size(size, fetch_size);
        let mut seen = HashSet::new();
        let mut pending = 0_usize;
        let mut requested = 0_usize;
        let mut decoding = 0_usize;
        let use_sync_decode = image_refs.len() <= ROUTE_COVER_GATE_SYNC_DECODE_LIMIT;

        for image_ref in &image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self
                    .decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
                    .is_some()
            {
                continue;
            }
            if let Some((ready_key, path)) =
                self.cached_cover_path_for_startup_prime(&image_ref, fetch_size)
            {
                if self.decoded_cover_has_min_size(&ready_key, decode_size) {
                    continue;
                }
                if use_sync_decode
                    && self.decode_route_gate_cover_from_path(&ready_key, &path, decode_size)
                {
                    continue;
                }
                if self.state.cover_decodes.borrow().contains(&ready_key) {
                    pending = pending.saturating_add(1);
                    decoding = decoding.saturating_add(1);
                    continue;
                }
                self.start_cover_decode_from_path(
                    ready_key,
                    path,
                    decode_size,
                    CoverDecodePriority::Visible,
                );
                pending = pending.saturating_add(1);
                decoding = decoding.saturating_add(1);
            } else if self.route_cover_gate_should_request_missing(
                image_ref,
                fetch_size,
                missing_policy,
            ) {
                self.controller
                    .request_cover_for_key(key, image_ref.clone(), fetch_size);
                pending = pending.saturating_add(1);
                requested = requested.saturating_add(1);
            }
        }

        if pending == 0 {
            self.clear_route_cover_gate(route_key);
            return false;
        }

        if self.route_cover_gate_should_wait(route_key, pending, requested, decoding) {
            self.queue_route_cover_gate_poll(route_key, image_refs, fetch_size, size, missing_policy);
            true
        } else {
            false
        }
    }
    fn route_cover_gate_should_wait(
        &self,
        route_key: &'static str,
        pending: usize,
        requested: usize,
        decoding: usize,
    ) -> bool {
        if self
            .state
            .route_cover_gate_timed_out
            .borrow()
            .contains(route_key)
        {
            return false;
        }

        let now = Instant::now();
        let mut started = self.state.route_cover_gate_started.borrow_mut();
        let is_new = !started.contains_key(route_key);
        let started_at = *started.entry(route_key).or_insert(now);
        let elapsed = now.saturating_duration_since(started_at);
        if elapsed >= Duration::from_millis(ROUTE_COVER_GATE_TIMEOUT_MS) {
            self.state
                .route_cover_gate_timed_out
                .borrow_mut()
                .insert(route_key);
            warn!(
                route = route_key,
                pending,
                requested,
                decoding,
                elapsed_ms = elapsed.as_millis() as u64,
                "revealing route with cover gate still pending"
            );
            if self.state.perf.is_some() {
                println!(
                    "RUFIN_ROUTE_COVER_GATE_TIMEOUT route={} pending={} requested={} decoding={} elapsed_ms={}",
                    route_key,
                    pending,
                    requested,
                    decoding,
                    elapsed.as_millis()
                );
            }
            return false;
        }

        if is_new {
            info!(
                route = route_key,
                pending,
                requested,
                decoding,
                "started route cover gate"
            );
            if self.state.perf.is_some() {
                println!(
                    "RUFIN_ROUTE_COVER_GATE_START route={} pending={} requested={} decoding={}",
                    route_key, pending, requested, decoding
                );
            }
        }
        true
    }
    fn clear_route_cover_gate(&self, route_key: &'static str) {
        let elapsed = self
            .state
            .route_cover_gate_started
            .borrow_mut()
            .remove(route_key)
            .map(|started| started.elapsed().as_millis() as u64);
        self.state.route_cover_gate_queued.borrow_mut().remove(route_key);
        self.state
            .route_cover_gate_timed_out
            .borrow_mut()
            .remove(route_key);
        if let Some(elapsed_ms) = elapsed {
            info!(route = route_key, elapsed_ms, "route cover gate ready");
            if self.state.perf.is_some() {
                println!(
                    "RUFIN_ROUTE_COVER_GATE_READY route={} elapsed_ms={}",
                    route_key, elapsed_ms
                );
            }
        }
    }
    fn queue_route_cover_gate_poll(
        self: &Rc<Self>,
        route_key: &'static str,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
        missing_policy: RouteCoverMissingPolicy,
    ) {
        if !self
            .state
            .route_cover_gate_queued
            .borrow_mut()
            .insert(route_key)
        {
            return;
        }
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(Duration::from_millis(ROUTE_COVER_GATE_POLL_MS), move || {
            shell.state
                .route_cover_gate_queued
                .borrow_mut()
                .remove(route_key);
            if route_cover_gate_key_for_current_route(&shell) == Some(route_key) {
                let waiting = shell.route_cover_gate_needs_loading(
                    route_key,
                    image_refs,
                    fetch_size,
                    size,
                    missing_policy,
                );
                if !waiting {
                    shell.render_current_route();
                }
            }
        });
    }
    fn route_cover_gate_should_request_missing(
        &self,
        image_ref: &ImageRef,
        fetch_size: u32,
        missing_policy: RouteCoverMissingPolicy,
    ) -> bool {
        if self
            .controller
            .external_cover_lookup_known_missing(image_ref, fetch_size)
        {
            return false;
        }
        match missing_policy {
            RouteCoverMissingPolicy::Any => true,
        }
    }
    fn decode_route_gate_cover_from_path(
        &self,
        key: &str,
        path: &std::path::Path,
        size: i32,
    ) -> bool {
        if self.decoded_cover_has_min_size(key, size) {
            return true;
        }
        let decode_size = if size >= DETAIL_COVER_SIZE as i32 {
            size
        } else if size >= GRID_COVER_SIZE as i32 {
            size
        } else {
            size.saturating_mul(2).min(GRID_COVER_SIZE as i32)
        };
        match Pixbuf::from_file_at_scale(path, decode_size, decode_size, true) {
            Ok(pixbuf) => {
                self.remember_decoded_cover(key.to_string(), pixbuf, CoverDecodePriority::Visible);
                self.record_perf_cover_decode_ok(key);
                true
            }
            Err(error) => {
                warn!(
                    %error,
                    path = %path.display(),
                    "failed to synchronously decode route gate cover"
                );
                false
            }
        }
    }
}
fn connect_track_viewport_cover_warm(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    settings: &LibraryListSettings,
) {
    if settings.layout != LibraryLayout::Row {
        return;
    }
    let Some((fetch_size, size)) = track_cover_warm_sizes(shell, settings) else {
        return;
    };

    let shell = Rc::clone(shell);
    let model = model.clone();
    let adjustment = scroller.vadjustment();
    let scheduled = Rc::new(Cell::new(false));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_track_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        if scheduled.replace(true) {
            return;
        }
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let scheduled = Rc::clone(&scheduled);
        glib::timeout_add_local_once(
            Duration::from_millis(TRACK_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                scheduled.set(false);
                warm_track_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
            },
        );
    });
}
fn connect_track_row_contract_observer(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    settings: &LibraryListSettings,
) {
    if shell.state.perf.is_none() || settings.layout != LibraryLayout::Row {
        return;
    }
    let Some((fetch_size, size)) = track_cover_warm_sizes(shell, settings) else {
        return;
    };

    let shell = Rc::clone(shell);
    let model = model.clone();
    let adjustment = scroller.vadjustment();
    let scheduled = Rc::new(Cell::new(false));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            record_track_row_contract_sample(
                &shell,
                &model,
                &adjustment,
                fetch_size,
                size,
                "initial",
            );
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        if scheduled.replace(true) {
            return;
        }
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let scheduled = Rc::clone(&scheduled);
        glib::timeout_add_local_once(
            Duration::from_millis(TRACK_ROW_CONTRACT_SCROLL_DELAY_MS),
            move || {
                scheduled.set(false);
                record_track_row_contract_sample(
                    &shell,
                    &model,
                    &adjustment,
                    fetch_size,
                    size,
                    "scroll",
                );
            },
        );
    });
}
fn record_track_row_contract_sample(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
    scenario: &'static str,
) {
    if !matches!(shell.state.routes.borrow().current(), Route::Tracks) {
        return;
    }
    let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
    let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
    let visible_rows = (adjustment.page_size().max(row_height) / row_height)
        .ceil()
        .max(1.0) as usize;
    let visible_end = visible_start
        .saturating_add(visible_rows)
        .min(model.n_items() as usize);
    if visible_start >= visible_end {
        return;
    }

    let decode_size = size.max(fetch_size as i32).max(1);
    let mut ready = 0_usize;
    let mut coverless = 0_usize;
    let mut pending = 0_usize;
    let missing = 0_usize;
    for index in visible_start..visible_end {
        let Some(track) = item_at::<Track>(model, index as u32) else {
            continue;
        };
        let Some(image_ref) = track.image_ref else {
            coverless = coverless.saturating_add(1);
            continue;
        };
        if shell
            .decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
            .is_some()
        {
            ready = ready.saturating_add(1);
        } else if shell.cover_cache_key(&image_ref, fetch_size).is_none() {
            coverless = coverless.saturating_add(1);
        } else {
            pending = pending.saturating_add(1);
        }
    }
    if let Some(perf) = &shell.state.perf {
        perf.record_tracks_row_contract(
            scenario,
            visible_start,
            visible_end,
            ready,
            coverless,
            pending,
            missing,
        );
    }
}
fn warm_track_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
) {
    let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
    let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
    let visible_rows = (adjustment.page_size().max(row_height) / row_height).ceil() as usize;
    let start = visible_start.saturating_sub(TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS);
    let count = visible_rows
        .saturating_add(TRACK_VIEWPORT_COVER_WARM_AHEAD_ROWS)
        .saturating_add(TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS);
    let end = start.saturating_add(count).min(model.n_items() as usize);
    if start >= end {
        return;
    }

    let image_refs = (start..end)
        .filter_map(|index| item_at::<Track>(model, index as u32))
        .filter_map(|track| track.image_ref)
        .collect::<Vec<_>>();
    if image_refs.is_empty() {
        return;
    }

    shell.warm_cover_refs_now(image_refs, fetch_size, size);
}
fn connect_album_viewport_cover_warm(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    settings: &LibraryListSettings,
) {
    if settings.layout != LibraryLayout::Row || !album_row_layout_uses_cover(settings) {
        return;
    }
    let Some((fetch_size, size)) = album_cover_warm_sizes(shell, settings) else {
        return;
    };

    let shell = Rc::clone(shell);
    let model = model.clone();
    let adjustment = scroller.vadjustment();
    let scheduled = Rc::new(Cell::new(false));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_album_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        if scheduled.replace(true) {
            return;
        }
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let scheduled = Rc::clone(&scheduled);
        glib::timeout_add_local_once(
            Duration::from_millis(ALBUM_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                scheduled.set(false);
                warm_album_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
            },
        );
    });
}
fn warm_album_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
) {
    let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
    let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
    let visible_rows = (adjustment.page_size().max(row_height) / row_height).ceil() as usize;
    let start = visible_start.saturating_sub(ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS);
    let count = visible_rows
        .saturating_add(ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS)
        .saturating_add(ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS);
    let end = start.saturating_add(count).min(model.n_items() as usize);
    if start >= end {
        return;
    }

    let image_refs = (start..end)
        .filter_map(|index| item_at::<Album>(model, index as u32))
        .filter_map(|album| album.image_ref)
        .collect::<Vec<_>>();
    if image_refs.is_empty() {
        return;
    }

    shell.warm_cover_refs_now(image_refs, fetch_size, size);
}
fn warm_album_covers_for_settings(
    shell: &Rc<Shell>,
    albums: &[Album],
    settings: &LibraryListSettings,
) {
    let shell = Rc::clone(shell);
    let albums = albums.to_vec();
    let settings = settings.clone();
    glib::idle_add_local_once(move || {
        warm_album_covers_for_settings_now(&shell, &albums, &settings);
    });
}
fn warm_album_covers_for_settings_now(
    shell: &Rc<Shell>,
    albums: &[Album],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = album_cover_warm_sizes(shell, settings) else {
        return;
    };
    let mut values = albums.to_vec();
    sort_albums(&mut values, settings);
    let image_refs = values
        .iter()
        .filter_map(|album| album.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.warm_cover_refs_now(image_refs, fetch_size, size);
}
fn warm_artist_covers_for_settings(
    shell: &Rc<Shell>,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let shell = Rc::clone(shell);
    let artists = artists.to_vec();
    let settings = settings.clone();
    glib::idle_add_local_once(move || {
        warm_artist_covers_for_settings_now(&shell, &artists, &settings);
    });
}
fn warm_artist_covers_for_settings_now(
    shell: &Rc<Shell>,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = grid_or_row_cover_warm_sizes(shell, settings) else {
        return;
    };
    let mut values = artists.to_vec();
    sort_artists(&mut values, settings);
    let image_refs = values
        .iter()
        .filter_map(|artist| artist.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.warm_cover_refs(image_refs, fetch_size, size);
}
fn warm_genre_covers_for_settings(
    shell: &Rc<Shell>,
    genres: &[Genre],
    settings: &LibraryListSettings,
) {
    let shell = Rc::clone(shell);
    let genres = genres.to_vec();
    let settings = settings.clone();
    glib::idle_add_local_once(move || {
        warm_genre_covers_for_settings_now(&shell, &genres, &settings);
    });
}
fn warm_genre_covers_for_settings_now(
    shell: &Rc<Shell>,
    genres: &[Genre],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = grid_or_row_cover_warm_sizes(shell, settings) else {
        return;
    };
    let mut values = genres.to_vec();
    sort_genres(&mut values, settings);
    let image_refs = values
        .iter()
        .flat_map(|genre| {
            let mut refs = genre_cover_refs(shell, genre);
            if refs.is_empty() {
                refs.extend(genre.image_ref.iter().cloned());
            }
            refs
        })
        .collect::<Vec<ImageRef>>();
    shell.warm_cover_refs(image_refs, fetch_size, size);
}
fn warm_playlist_covers_for_settings(
    shell: &Rc<Shell>,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    let shell = Rc::clone(shell);
    let playlists = playlists.to_vec();
    let settings = settings.clone();
    glib::idle_add_local_once(move || {
        warm_playlist_covers_for_settings_now(&shell, &playlists, &settings);
    });
}
fn warm_playlist_covers_for_settings_now(
    shell: &Rc<Shell>,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = grid_or_row_cover_warm_sizes(shell, settings) else {
        return;
    };
    let mut values = playlists.to_vec();
    sort_playlists(&mut values, settings);
    let image_refs = values
        .iter()
        .filter_map(|playlist| playlist.image_ref.clone())
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
fn album_cover_warm_sizes(shell: &Rc<Shell>, settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid => Some((GRID_COVER_SIZE, shell.responsive_card_grid_metrics().1)),
        LibraryLayout::Detail => Some((GRID_COVER_SIZE, if compact_detail_layout(shell) { 148 } else { 220 })),
        LibraryLayout::Row if album_row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
fn grid_or_row_cover_warm_sizes(
    shell: &Rc<Shell>,
    settings: &LibraryListSettings,
) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid | LibraryLayout::Detail => {
            Some((GRID_COVER_SIZE, shell.responsive_card_grid_metrics().1))
        }
        LibraryLayout::Row if album_row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
fn album_row_layout_uses_cover(settings: &LibraryListSettings) -> bool {
    settings
        .row_fields
        .iter()
        .any(|field| matches!(field, LibraryField::Image | LibraryField::TitleMerged))
}
