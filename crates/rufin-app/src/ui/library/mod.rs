use super::{
    ArtworkTile, COVER_PATH_LOOKUP_MAX_IN_FLIGHT, GRID_COVER_SIZE, GRID_ROUTE_PAGE_SIZE,
    PRIMARY_ROUTE_MARGIN_START, PagedGridCursor, Route, Shell, THUMB_COVER_SIZE,
    TRACK_ROUTE_PAGE_SIZE, UiPerfRouteModelContract, UiPerfTrackRowContract, album_favorite_key,
    append_albums_to_model, append_artists_to_model, append_genres_to_model,
    append_playlists_to_model, append_tracks_to_model, artist_favorite_key, cards,
    connect_paged_grid_loader, favorite_button_is_active, favorite_icon_button, finish_grid_page,
    icon_button, install_album_context_menu, install_artist_context_menu,
    install_dynamic_album_context_menu, install_dynamic_track_context_menu,
    install_playlist_context_menu, install_smart_playlist_context_menu, install_track_context_menu,
    layout::{large_popup_content_height, large_popup_content_width, route_content_width},
    replace_albums_in_model, replace_artists_in_model, replace_genres_in_model,
    replace_playlists_in_model, set_favorite_button_active, stable_seed, text_button,
};
use crate::i18n::tr;
use adw::prelude::*;
use gtk::{gio, glib};
use rufin_core::{
    Album, AlbumId, Artist, Genre, ImageRef, LibraryField, LibraryLayout, LibraryListKey,
    LibraryListSettings, Playlist, SmartPlaylist, Track, TrackId, available_sort_fields,
    format_duration,
};
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

mod album_detail;
mod collections;
#[path = "cards.rs"]
mod field_cards;
mod route_shell;
mod routes;

pub(super) use album_detail::*;
pub(super) use collections::*;
pub(super) use field_cards::*;
pub(super) use route_shell::*;

#[cfg(test)]
mod route_tests;

const LIBRARY_CONFIG_DIALOG_WIDTH: i32 = 620;
const LIBRARY_CONFIG_DIALOG_HEIGHT: i32 = 560;
const LIBRARY_TABLE_HEADER_HEIGHT: i32 = 92;
pub(in crate::ui) const LIBRARY_TABLE_ROW_HEIGHT: i32 = 58;
const LIBRARY_ROUTE_BOTTOM_MARGIN: i32 = 8;
const ALBUM_DETAIL_FIXED_TRAILING_INSET: i32 = 64;
const ALBUM_DETAIL_INLINE_TRACK_ROWS: usize = 8;
const ALBUM_DETAIL_TRACK_HEADER_HEIGHT: i32 = 34;
const ALBUM_DETAIL_META_SPACING: i32 = 6;
const ALBUM_DETAIL_META_LABEL_HEIGHT: i32 = 20;
const INITIAL_ROUTE_COVER_WARM_ITEMS: usize = 16;
const TRACK_ROW_CONTRACT_SCROLL_DELAY_MS: u64 = 250;
const TRACK_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 0;
const TRACK_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 0;
const TRACK_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 96;
const TRACK_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 48;
const TRACK_VIEWPORT_COVER_WARM_AHEAD_ROWS: usize = 32;
const TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS: usize = 16;
const TRACK_VIEWPORT_COVER_WARM_DELAY_MS: u64 = 32;
const ALBUM_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 0;
const ALBUM_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 0;
const ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 48;
const ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 24;
const ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS: usize = 32;
const ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS: usize = 16;
const ALBUM_VIEWPORT_COVER_WARM_DELAY_MS: u64 = 32;
const GRID_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 0;
const GRID_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 0;
const GRID_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS: usize = 24;
const GRID_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS: usize = 8;
const GRID_VIEWPORT_COVER_WARM_AHEAD_ROWS: usize = 6;
const GRID_VIEWPORT_COVER_WARM_BEHIND_ROWS: usize = 2;
const GRID_VIEWPORT_COVER_WARM_DELAY_MS: u64 = 64;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TrackViewportCoverRanges {
    visible_start: usize,
    visible_end: usize,
    priority_start: usize,
    priority_end: usize,
    warm_before_start: usize,
    warm_before_end: usize,
    warm_after_start: usize,
    warm_after_end: usize,
}
fn track_viewport_cover_ranges(
    total: usize,
    visible_start: usize,
    visible_rows: usize,
) -> Option<TrackViewportCoverRanges> {
    viewport_cover_ranges(
        total,
        visible_start,
        visible_rows,
        TRACK_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        TRACK_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS,
        TRACK_VIEWPORT_COVER_WARM_AHEAD_ROWS,
    )
}
fn track_interaction_viewport_cover_ranges(
    total: usize,
    visible_start: usize,
    visible_rows: usize,
) -> Option<TrackViewportCoverRanges> {
    viewport_cover_ranges(
        total,
        visible_start,
        visible_rows,
        TRACK_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        TRACK_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        TRACK_VIEWPORT_COVER_WARM_BEHIND_ROWS,
        TRACK_VIEWPORT_COVER_WARM_AHEAD_ROWS,
    )
}
fn album_viewport_cover_ranges(
    total: usize,
    visible_start: usize,
    visible_rows: usize,
) -> Option<TrackViewportCoverRanges> {
    viewport_cover_ranges(
        total,
        visible_start,
        visible_rows,
        ALBUM_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        ALBUM_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS,
        ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS,
    )
}
fn album_interaction_viewport_cover_ranges(
    total: usize,
    visible_start: usize,
    visible_rows: usize,
) -> Option<TrackViewportCoverRanges> {
    viewport_cover_ranges(
        total,
        visible_start,
        visible_rows,
        ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS,
        ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS,
    )
}
fn viewport_cover_ranges(
    total: usize,
    visible_start: usize,
    visible_count: usize,
    priority_behind: usize,
    priority_ahead: usize,
    warm_behind: usize,
    warm_ahead: usize,
) -> Option<TrackViewportCoverRanges> {
    if total == 0 {
        return None;
    }

    let visible_count = visible_count.max(1).min(total);
    let visible_start = visible_start.min(total.saturating_sub(visible_count));
    let priority_start = visible_start.saturating_sub(priority_behind);
    let priority_end = visible_start
        .saturating_add(visible_count)
        .saturating_add(priority_ahead)
        .min(total);
    if priority_start >= priority_end {
        return None;
    }

    Some(TrackViewportCoverRanges {
        visible_start,
        visible_end: visible_start.saturating_add(visible_count).min(total),
        priority_start,
        priority_end,
        warm_before_start: priority_start.saturating_sub(warm_behind),
        warm_before_end: priority_start,
        warm_after_start: priority_end,
        warm_after_end: priority_end.saturating_add(warm_ahead).min(total),
    })
}
#[derive(Debug, Default, Eq, PartialEq)]
struct ViewportCoverRefBatches {
    visible_priority_len: usize,
    priority_refs: Vec<ImageRef>,
    warm_refs: Vec<ImageRef>,
}
fn viewport_cover_ref_batches_for_ranges(
    ranges: TrackViewportCoverRanges,
    mut refs_for_range: impl FnMut(usize, usize) -> Vec<ImageRef>,
) -> ViewportCoverRefBatches {
    let mut priority_refs = refs_for_range(ranges.visible_start, ranges.visible_end);
    dedupe_image_refs(&mut priority_refs, &[]);
    let visible_priority_len = priority_refs.len();
    priority_refs.extend(refs_for_range(ranges.priority_start, ranges.visible_start));
    priority_refs.extend(refs_for_range(ranges.visible_end, ranges.priority_end));
    dedupe_image_refs(&mut priority_refs, &[]);

    let mut warm_refs = refs_for_range(ranges.warm_before_start, ranges.warm_before_end);
    warm_refs.extend(refs_for_range(
        ranges.warm_after_start,
        ranges.warm_after_end,
    ));
    dedupe_image_refs(&mut warm_refs, &priority_refs);

    ViewportCoverRefBatches {
        visible_priority_len,
        priority_refs,
        warm_refs,
    }
}
fn dedupe_image_refs(refs: &mut Vec<ImageRef>, excluded: &[ImageRef]) {
    let mut deduped = Vec::with_capacity(refs.len());
    for image_ref in refs.drain(..) {
        if excluded.iter().any(|existing| existing == &image_ref)
            || deduped.iter().any(|existing| existing == &image_ref)
        {
            continue;
        }
        deduped.push(image_ref);
    }
    *refs = deduped;
}
fn cap_viewport_priority_cover_refs(
    mut batches: ViewportCoverRefBatches,
    priority_limit: usize,
) -> (ViewportCoverRefBatches, bool) {
    let priority_limit = priority_limit.max(batches.visible_priority_len).max(1);
    if batches.priority_refs.len() <= priority_limit {
        return (batches, false);
    }
    let mut deferred_refs = batches.priority_refs.split_off(priority_limit);
    deferred_refs.extend(batches.warm_refs);
    batches.warm_refs = deferred_refs;
    (batches, true)
}
fn prepare_viewport_cover_refs(
    shell: &Rc<Shell>,
    batches: ViewportCoverRefBatches,
    fetch_size: u32,
    size: i32,
    include_warm: bool,
) {
    let (batches, priority_overflowed) =
        cap_viewport_priority_cover_refs(batches, COVER_PATH_LOOKUP_MAX_IN_FLIGHT);
    if !batches.priority_refs.is_empty() {
        shell.prime_cover_refs_now(batches.priority_refs, fetch_size, size);
    }
    if (include_warm || priority_overflowed) && !batches.warm_refs.is_empty() {
        shell.warm_cover_refs_now(batches.warm_refs, fetch_size, size);
    }
}
fn route_viewport_page_size(shell: &Shell, adjustment: &gtk::Adjustment) -> f64 {
    route_viewport_page_size_from_metrics(
        adjustment.page_size(),
        shell.route_host.height(),
        shell.app_root.height(),
    )
}
fn route_viewport_page_size_from_metrics(
    adjustment_page_size: f64,
    route_host_height: i32,
    app_root_height: i32,
) -> f64 {
    adjustment_page_size
        .max(f64::from(route_host_height))
        .max(f64::from(app_root_height))
        .max(1.0)
}
fn library_layout_loads_complete_page(key: LibraryListKey, settings: &LibraryListSettings) -> bool {
    key.supports_layout(settings.layout)
}
fn record_library_route_model_contract(
    shell: &Shell,
    route: &'static str,
    settings: &LibraryListSettings,
    loaded: usize,
    total: usize,
    paginated: bool,
) {
    let Some(perf) = &shell.state.perf else {
        return;
    };
    perf.record_route_model_contract(UiPerfRouteModelContract {
        route,
        layout: library_layout_name(settings.layout),
        loaded,
        total,
        complete: loaded >= total,
        paginated,
    });
}
fn library_layout_name(layout: LibraryLayout) -> &'static str {
    match layout {
        LibraryLayout::Row => "row",
        LibraryLayout::Grid => "grid",
        LibraryLayout::Detail => "detail",
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
    warm_track_covers_for_settings_now(shell, tracks, settings);
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
        Some(INITIAL_ROUTE_COVER_WARM_ITEMS),
    );
    shell.prime_cover_refs_now(image_refs, fetch_size, size);
}
#[cfg(test)]
fn track_cover_refs_for_settings(
    tracks: &[Track],
    settings: &LibraryListSettings,
) -> Vec<ImageRef> {
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
    let generation = Rc::new(Cell::new(0_u64));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_track_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let generation = Rc::clone(&generation);
        glib::idle_add_local_once({
            let shell = Rc::clone(&shell);
            let model = model.clone();
            let adjustment = adjustment.clone();
            let generation = Rc::clone(&generation);
            move || {
                if generation.get() == next_generation {
                    prime_track_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
                }
            }
        });
        glib::timeout_add_local_once(
            Duration::from_millis(TRACK_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                if generation.get() != next_generation {
                    return;
                }
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
    let generation = Rc::new(Cell::new(0_u64));

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
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let generation = Rc::clone(&generation);
        glib::timeout_add_local_once(
            Duration::from_millis(TRACK_ROW_CONTRACT_SCROLL_DELAY_MS),
            move || {
                if generation.get() != next_generation {
                    return;
                }
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
    let page_size = route_viewport_page_size(shell, adjustment);
    let visible_rows = (page_size.max(row_height) / row_height).ceil().max(1.0) as usize;
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
        perf.record_tracks_row_contract(UiPerfTrackRowContract {
            scenario,
            visible_start,
            visible_end,
            ready,
            coverless,
            pending,
            missing,
        });
    }
}
fn warm_track_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
) {
    prepare_track_cover_model_viewport(shell, model, adjustment, fetch_size, size, true, false);
}
fn prime_track_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
) {
    prepare_track_cover_model_viewport(shell, model, adjustment, fetch_size, size, false, true);
}
fn prepare_track_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
    include_warm: bool,
    interaction: bool,
) {
    let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
    let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
    let page_size = route_viewport_page_size(shell, adjustment);
    let visible_rows = (page_size.max(row_height) / row_height).ceil() as usize;
    let ranges = if interaction {
        track_interaction_viewport_cover_ranges(
            model.n_items() as usize,
            visible_start,
            visible_rows,
        )
    } else {
        track_viewport_cover_ranges(model.n_items() as usize, visible_start, visible_rows)
    };
    let Some(ranges) = ranges else {
        return;
    };

    let batches = viewport_cover_ref_batches_for_ranges(ranges, |start, end| {
        track_cover_refs_for_model_range(model, start, end)
    });
    prepare_viewport_cover_refs(shell, batches, fetch_size, size, include_warm);
}
fn track_cover_refs_for_model_range(
    model: &gio::ListStore,
    start: usize,
    end: usize,
) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for index in start..end.min(model.n_items() as usize) {
        let Some(track) = item_at::<Track>(model, index as u32) else {
            continue;
        };
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
fn album_cover_refs_for_model_range(
    model: &gio::ListStore,
    start: usize,
    end: usize,
) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for index in start..end.min(model.n_items() as usize) {
        let Some(album) = item_at::<Album>(model, index as u32) else {
            continue;
        };
        let Some(image_ref) = album.image_ref else {
            continue;
        };
        if refs.iter().any(|existing| existing == &image_ref) {
            continue;
        }
        refs.push(image_ref);
    }
    refs
}
fn artist_cover_refs_for_model_range(
    model: &gio::ListStore,
    start: usize,
    end: usize,
) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for index in start..end.min(model.n_items() as usize) {
        let Some(artist) = item_at::<Artist>(model, index as u32) else {
            continue;
        };
        let Some(image_ref) = artist.image_ref else {
            continue;
        };
        if refs.iter().any(|existing| existing == &image_ref) {
            continue;
        }
        refs.push(image_ref);
    }
    refs
}
fn genre_cover_refs_for_model_range(
    model: &gio::ListStore,
    start: usize,
    end: usize,
) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for index in start..end.min(model.n_items() as usize) {
        let Some(genre) = item_at::<Genre>(model, index as u32) else {
            continue;
        };
        let mut genre_refs = genre.image_refs;
        genre_refs.extend(genre.image_ref);
        for image_ref in genre_refs {
            if !refs.iter().any(|existing| existing == &image_ref) {
                refs.push(image_ref);
            }
        }
    }
    refs
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
    let generation = Rc::new(Cell::new(0_u64));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_album_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let adjustment = adjustment.clone();
        let generation = Rc::clone(&generation);
        glib::idle_add_local_once({
            let shell = Rc::clone(&shell);
            let model = model.clone();
            let adjustment = adjustment.clone();
            let generation = Rc::clone(&generation);
            move || {
                if generation.get() == next_generation {
                    prime_album_cover_model_viewport(&shell, &model, &adjustment, fetch_size, size);
                }
            }
        });
        glib::timeout_add_local_once(
            Duration::from_millis(ALBUM_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                if generation.get() != next_generation {
                    return;
                }
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
    prepare_album_cover_model_viewport(shell, model, adjustment, fetch_size, size, true, false);
}
fn prime_album_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
) {
    prepare_album_cover_model_viewport(shell, model, adjustment, fetch_size, size, false, true);
}
fn prepare_album_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    fetch_size: u32,
    size: i32,
    include_warm: bool,
    interaction: bool,
) {
    let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
    let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
    let page_size = route_viewport_page_size(shell, adjustment);
    let visible_rows = (page_size.max(row_height) / row_height).ceil() as usize;
    let ranges = if interaction {
        album_interaction_viewport_cover_ranges(
            model.n_items() as usize,
            visible_start,
            visible_rows,
        )
    } else {
        album_viewport_cover_ranges(model.n_items() as usize, visible_start, visible_rows)
    };
    let Some(ranges) = ranges else {
        return;
    };

    let batches = viewport_cover_ref_batches_for_ranges(ranges, |start, end| {
        album_cover_refs_for_model_range(model, start, end)
    });
    prepare_viewport_cover_refs(shell, batches, fetch_size, size, include_warm);
}
fn connect_artist_viewport_cover_warm(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = grid_or_row_cover_warm_sizes(shell, settings) else {
        return;
    };

    let shell = Rc::clone(shell);
    let model = model.clone();
    let settings = settings.clone();
    let adjustment = scroller.vadjustment();
    let generation = Rc::new(Cell::new(0_u64));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let settings = settings.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_artist_cover_model_viewport(
                &shell,
                &model,
                &adjustment,
                &settings,
                fetch_size,
                size,
            );
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let settings = settings.clone();
        let adjustment = adjustment.clone();
        let generation = Rc::clone(&generation);
        glib::idle_add_local_once({
            let shell = Rc::clone(&shell);
            let model = model.clone();
            let settings = settings.clone();
            let adjustment = adjustment.clone();
            let generation = Rc::clone(&generation);
            move || {
                if generation.get() == next_generation {
                    prime_artist_cover_model_viewport(
                        &shell,
                        &model,
                        &adjustment,
                        &settings,
                        fetch_size,
                        size,
                    );
                }
            }
        });
        glib::timeout_add_local_once(
            Duration::from_millis(GRID_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                if generation.get() != next_generation {
                    return;
                }
                warm_artist_cover_model_viewport(
                    &shell,
                    &model,
                    &adjustment,
                    &settings,
                    fetch_size,
                    size,
                );
            },
        );
    });
}
fn warm_artist_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
) {
    prepare_artist_cover_model_viewport(
        shell, model, adjustment, settings, fetch_size, size, true, false,
    );
}
fn prime_artist_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
) {
    prepare_artist_cover_model_viewport(
        shell, model, adjustment, settings, fetch_size, size, false, true,
    );
}
#[allow(clippy::too_many_arguments)]
fn prepare_artist_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
    include_warm: bool,
    interaction: bool,
) {
    let ranges = if interaction {
        library_interaction_viewport_cover_ranges(
            shell,
            adjustment,
            model.n_items() as usize,
            settings.layout,
        )
    } else {
        library_viewport_cover_ranges(shell, adjustment, model.n_items() as usize, settings.layout)
    };
    let Some(ranges) = ranges else {
        return;
    };

    let batches = viewport_cover_ref_batches_for_ranges(ranges, |start, end| {
        artist_cover_refs_for_model_range(model, start, end)
    });
    prepare_viewport_cover_refs(shell, batches, fetch_size, size, include_warm);
}
fn connect_genre_viewport_cover_warm(
    shell: &Rc<Shell>,
    scroller: &gtk::ScrolledWindow,
    model: &gio::ListStore,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = grid_or_row_cover_warm_sizes(shell, settings) else {
        return;
    };

    let shell = Rc::clone(shell);
    let model = model.clone();
    let settings = settings.clone();
    let adjustment = scroller.vadjustment();
    let generation = Rc::new(Cell::new(0_u64));

    {
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let settings = settings.clone();
        let adjustment = adjustment.clone();
        glib::idle_add_local_once(move || {
            warm_genre_cover_model_viewport(
                &shell,
                &model,
                &adjustment,
                &settings,
                fetch_size,
                size,
            );
        });
    }

    adjustment.connect_value_changed(move |adjustment| {
        let next_generation = generation.get().saturating_add(1);
        generation.set(next_generation);
        let shell = Rc::clone(&shell);
        let model = model.clone();
        let settings = settings.clone();
        let adjustment = adjustment.clone();
        let generation = Rc::clone(&generation);
        glib::idle_add_local_once({
            let shell = Rc::clone(&shell);
            let model = model.clone();
            let settings = settings.clone();
            let adjustment = adjustment.clone();
            let generation = Rc::clone(&generation);
            move || {
                if generation.get() == next_generation {
                    prime_genre_cover_model_viewport(
                        &shell,
                        &model,
                        &adjustment,
                        &settings,
                        fetch_size,
                        size,
                    );
                }
            }
        });
        glib::timeout_add_local_once(
            Duration::from_millis(GRID_VIEWPORT_COVER_WARM_DELAY_MS),
            move || {
                if generation.get() != next_generation {
                    return;
                }
                warm_genre_cover_model_viewport(
                    &shell,
                    &model,
                    &adjustment,
                    &settings,
                    fetch_size,
                    size,
                );
            },
        );
    });
}
fn warm_genre_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
) {
    prepare_genre_cover_model_viewport(
        shell, model, adjustment, settings, fetch_size, size, true, false,
    );
}
fn prime_genre_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
) {
    prepare_genre_cover_model_viewport(
        shell, model, adjustment, settings, fetch_size, size, false, true,
    );
}
#[allow(clippy::too_many_arguments)]
fn prepare_genre_cover_model_viewport(
    shell: &Rc<Shell>,
    model: &gio::ListStore,
    adjustment: &gtk::Adjustment,
    settings: &LibraryListSettings,
    fetch_size: u32,
    size: i32,
    include_warm: bool,
    interaction: bool,
) {
    let ranges = if interaction {
        library_interaction_viewport_cover_ranges(
            shell,
            adjustment,
            model.n_items() as usize,
            settings.layout,
        )
    } else {
        library_viewport_cover_ranges(shell, adjustment, model.n_items() as usize, settings.layout)
    };
    let Some(ranges) = ranges else {
        return;
    };

    let batches = viewport_cover_ref_batches_for_ranges(ranges, |start, end| {
        genre_cover_refs_for_model_range(model, start, end)
    });
    prepare_viewport_cover_refs(shell, batches, fetch_size, size, include_warm);
}
fn library_viewport_cover_ranges(
    shell: &Rc<Shell>,
    adjustment: &gtk::Adjustment,
    total: usize,
    layout: LibraryLayout,
) -> Option<TrackViewportCoverRanges> {
    library_viewport_cover_ranges_with_priority(
        shell,
        adjustment,
        total,
        layout,
        ALBUM_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        ALBUM_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        GRID_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        GRID_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
    )
}
fn library_interaction_viewport_cover_ranges(
    shell: &Rc<Shell>,
    adjustment: &gtk::Adjustment,
    total: usize,
    layout: LibraryLayout,
) -> Option<TrackViewportCoverRanges> {
    library_viewport_cover_ranges_with_priority(
        shell,
        adjustment,
        total,
        layout,
        ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        ALBUM_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
        GRID_INTERACTION_VIEWPORT_COVER_PRIORITY_BEHIND_ROWS,
        GRID_INTERACTION_VIEWPORT_COVER_PRIORITY_AHEAD_ROWS,
    )
}
#[allow(clippy::too_many_arguments)]
fn library_viewport_cover_ranges_with_priority(
    shell: &Rc<Shell>,
    adjustment: &gtk::Adjustment,
    total: usize,
    layout: LibraryLayout,
    row_priority_behind: usize,
    row_priority_ahead: usize,
    grid_priority_behind_rows: usize,
    grid_priority_ahead_rows: usize,
) -> Option<TrackViewportCoverRanges> {
    if total == 0 {
        return None;
    }
    match layout {
        LibraryLayout::Row => {
            let row_height = f64::from(LIBRARY_TABLE_ROW_HEIGHT.max(1));
            let visible_start = (adjustment.value().max(0.0) / row_height).floor() as usize;
            let page_size = route_viewport_page_size(shell, adjustment);
            let visible_rows = (page_size.max(row_height) / row_height).ceil() as usize;
            viewport_cover_ranges(
                total,
                visible_start,
                visible_rows,
                row_priority_behind,
                row_priority_ahead,
                ALBUM_VIEWPORT_COVER_WARM_BEHIND_ROWS,
                ALBUM_VIEWPORT_COVER_WARM_AHEAD_ROWS,
            )
        }
        LibraryLayout::Grid | LibraryLayout::Detail => {
            let (columns, card_size) = shell.responsive_card_grid_metrics();
            let columns = columns.max(1);
            let item_extent = f64::from(card_size.saturating_add(88).max(1));
            let first_row = (adjustment.value().max(0.0) / item_extent).floor() as usize;
            let page_size = route_viewport_page_size(shell, adjustment);
            let rows = (page_size.max(1.0) / item_extent).ceil().max(1.0) as usize + 1;
            let visible_count = rows.saturating_mul(columns).max(columns).min(total);
            let raw_start = first_row.saturating_mul(columns);
            let visible_start = raw_start.min(total.saturating_sub(visible_count));
            viewport_cover_ranges(
                total,
                visible_start,
                visible_count,
                columns.saturating_mul(grid_priority_behind_rows),
                columns.saturating_mul(grid_priority_ahead_rows),
                columns.saturating_mul(GRID_VIEWPORT_COVER_WARM_BEHIND_ROWS),
                columns.saturating_mul(GRID_VIEWPORT_COVER_WARM_AHEAD_ROWS),
            )
        }
    }
}
fn warm_album_covers_for_settings(
    shell: &Rc<Shell>,
    albums: &[Album],
    settings: &LibraryListSettings,
) {
    warm_album_covers_for_settings_now(shell, albums, settings);
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
        .take(INITIAL_ROUTE_COVER_WARM_ITEMS)
        .filter_map(|album| album.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.prime_cover_refs_now(image_refs, fetch_size, size);
}
fn warm_artist_covers_for_settings(
    shell: &Rc<Shell>,
    artists: &[Artist],
    settings: &LibraryListSettings,
) {
    warm_artist_covers_for_settings_now(shell, artists, settings);
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
        .take(INITIAL_ROUTE_COVER_WARM_ITEMS)
        .filter_map(|artist| artist.image_ref.clone())
        .collect::<Vec<ImageRef>>();
    shell.prime_cover_refs_now(image_refs, fetch_size, size);
}
fn warm_genre_covers_for_settings(
    shell: &Rc<Shell>,
    genres: &[Genre],
    settings: &LibraryListSettings,
) {
    warm_genre_covers_for_settings_now(shell, genres, settings);
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
        .take(INITIAL_ROUTE_COVER_WARM_ITEMS)
        .flat_map(|genre| {
            let mut refs = genre.image_refs.clone();
            refs.extend(genre.image_ref.iter().cloned());
            refs
        })
        .collect::<Vec<ImageRef>>();
    shell.prime_cover_refs_now(image_refs, fetch_size, size);
}
fn warm_playlist_covers_for_settings(
    shell: &Rc<Shell>,
    playlists: &[Playlist],
    settings: &LibraryListSettings,
) {
    warm_playlist_covers_for_settings_now(shell, playlists, settings);
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
        .take(GRID_ROUTE_PAGE_SIZE)
        .flat_map(|playlist| {
            let mut refs = playlist.image_refs.clone();
            refs.extend(playlist.image_ref.iter().cloned());
            refs
        })
        .collect::<Vec<ImageRef>>();
    shell.prime_cover_refs_now(image_refs, fetch_size, size);
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
        LibraryLayout::Detail => Some((
            GRID_COVER_SIZE,
            if compact_detail_layout(shell) {
                148
            } else {
                220
            },
        )),
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
