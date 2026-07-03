use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use domain::{ImageRef, ServerId};
use library::{SavedServer, Store, image_cache_key};
use secrets::SecretStore;
use source::MusicSource;
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

use crate::controller::{
    CancellationToken, ControllerEvent, IMAGE_TAG_UNTAGGED, StoreHandle, acquire_cover_slot,
    load_settings_from_store, release_cover_slot,
};
use crate::external_metadata;

use super::cache::{
    cached_cover_path_for_key, cached_cover_path_for_saved, cached_cover_path_for_saved_in_store,
    external_lookup_miss_cached, save_external_lookup_miss,
};
use super::candidates::{
    external_album_refs, push_source_album_image_refs, push_source_artist_image_refs,
    push_source_genre_image_refs, push_source_playlist_image_refs, push_source_track_image_refs,
    source_artist_refs,
};
use super::fetch::{
    CoverFetchTiming, fetch_and_cache_cover, fetch_and_cache_source_cover_timed_with_store,
    is_source_not_found_error,
};
use super::{
    EXTERNAL_PREFETCH_COVER_SIZE, EXTERNAL_PREFETCH_DELAY, EXTERNAL_PREFETCH_PAGE_SIZE,
    mark_cover_in_flight, unmark_cover_in_flight_generation,
};

const SLOW_SOURCE_COVER_STAGE_MS: u64 = 500;
const SLOW_SOURCE_COVER_TOTAL_MS: u64 = 1_000;

#[derive(Default)]
struct SyncedImagePrefetchStats {
    album_rows: usize,
    album_image_refs: usize,
    artist_rows: usize,
    artist_image_refs: usize,
    album_artist_rows: usize,
    album_artist_image_refs: usize,
    cache_hits: usize,
    known_misses: usize,
    skipped: usize,
    fetched: usize,
    misses: usize,
    errors: usize,
}

#[derive(Default)]
struct SourceCoverPrefetchStats {
    album_rows: usize,
    album_image_refs: usize,
    track_rows: usize,
    track_image_refs: usize,
    artist_rows: usize,
    artist_image_refs: usize,
    album_artist_rows: usize,
    album_artist_image_refs: usize,
    genre_rows: usize,
    genre_image_refs: usize,
    playlist_rows: usize,
    playlist_image_refs: usize,
    image_refs: usize,
    cache_hits: usize,
    skipped: usize,
    fetched: usize,
    misses: usize,
    errors: usize,
    select_refs_elapsed_ms: u64,
    cover_loop_elapsed_ms: u64,
    total_elapsed_ms: u64,
    fetched_bytes: usize,
    fetch_elapsed_ms: u64,
    normalize_elapsed_ms: u64,
    write_elapsed_ms: u64,
    store_elapsed_ms: u64,
    cover_elapsed_ms: u64,
    error_elapsed_ms: u64,
    max_fetch_ms: u64,
    max_normalize_ms: u64,
    max_write_ms: u64,
    max_store_ms: u64,
    max_cover_ms: u64,
    max_error_ms: u64,
    slow_fetches: usize,
    slow_normalizes: usize,
    slow_writes: usize,
    slow_store_saves: usize,
    slow_covers: usize,
    slow_errors: usize,
}

impl SourceCoverPrefetchStats {
    fn record_timing(&mut self, timing: CoverFetchTiming) {
        self.fetched_bytes = self.fetched_bytes.saturating_add(timing.bytes);
        self.fetch_elapsed_ms = self.fetch_elapsed_ms.saturating_add(timing.fetch_ms);
        self.normalize_elapsed_ms = self
            .normalize_elapsed_ms
            .saturating_add(timing.normalize_ms);
        self.write_elapsed_ms = self.write_elapsed_ms.saturating_add(timing.write_ms);
        self.store_elapsed_ms = self.store_elapsed_ms.saturating_add(timing.store_ms);
        self.cover_elapsed_ms = self.cover_elapsed_ms.saturating_add(timing.total_ms);
        self.max_fetch_ms = self.max_fetch_ms.max(timing.fetch_ms);
        self.max_normalize_ms = self.max_normalize_ms.max(timing.normalize_ms);
        self.max_write_ms = self.max_write_ms.max(timing.write_ms);
        self.max_store_ms = self.max_store_ms.max(timing.store_ms);
        self.max_cover_ms = self.max_cover_ms.max(timing.total_ms);
        self.slow_fetches += usize::from(timing.fetch_ms >= SLOW_SOURCE_COVER_STAGE_MS);
        self.slow_normalizes += usize::from(timing.normalize_ms >= SLOW_SOURCE_COVER_STAGE_MS);
        self.slow_writes += usize::from(timing.write_ms >= SLOW_SOURCE_COVER_STAGE_MS);
        self.slow_store_saves += usize::from(timing.store_ms >= SLOW_SOURCE_COVER_STAGE_MS);
        self.slow_covers += usize::from(timing.total_ms >= SLOW_SOURCE_COVER_TOTAL_MS);
    }

    fn record_error_elapsed(&mut self, elapsed_ms: u64) {
        self.error_elapsed_ms = self.error_elapsed_ms.saturating_add(elapsed_ms);
        self.max_error_ms = self.max_error_ms.max(elapsed_ms);
        self.slow_errors += usize::from(elapsed_ms >= SLOW_SOURCE_COVER_TOTAL_MS);
    }
}

pub(in crate::controller) struct ExternalCoverPrefetchRequest {
    pub(in crate::controller) store: StoreHandle,
    pub(in crate::controller) runtime: Arc<Runtime>,
    pub(in crate::controller) secrets: Arc<dyn SecretStore>,
    pub(in crate::controller) events: Sender<ControllerEvent>,
    pub(in crate::controller) cover_in_flight: Arc<Mutex<HashMap<String, u64>>>,
    pub(in crate::controller) external_cover_retry_generation: Arc<AtomicU64>,
    pub(in crate::controller) retry_generation: u64,
    pub(in crate::controller) external_cover_prefetch_in_flight: Arc<Mutex<HashMap<ServerId, u64>>>,
    pub(in crate::controller) cover_slots: Arc<(Mutex<usize>, Condvar)>,
    pub(in crate::controller) saved: SavedServer,
}

struct CoverPrefetchContext<'a> {
    store: &'a StoreHandle,
    runtime: &'a Runtime,
    secrets: &'a Arc<dyn SecretStore>,
    events: &'a Sender<ControllerEvent>,
    emit_status: bool,
    cover_in_flight: &'a Arc<Mutex<HashMap<String, u64>>>,
    external_cover_retry_generation: &'a Arc<AtomicU64>,
    retry_generation: u64,
    cover_slots: &'a Arc<(Mutex<usize>, Condvar)>,
    saved: &'a SavedServer,
    cancellation: Option<&'a CancellationToken>,
}

pub(in crate::controller) struct SourceCoverPrefetchRequest<'a> {
    pub(in crate::controller) store: &'a StoreHandle,
    pub(in crate::controller) runtime: &'a Runtime,
    pub(in crate::controller) secrets: &'a Arc<dyn SecretStore>,
    pub(in crate::controller) events: &'a Sender<ControllerEvent>,
    pub(in crate::controller) cover_in_flight: &'a Arc<Mutex<HashMap<String, u64>>>,
    pub(in crate::controller) external_cover_retry_generation: &'a Arc<AtomicU64>,
    pub(in crate::controller) retry_generation: u64,
    pub(in crate::controller) cover_slots: &'a Arc<(Mutex<usize>, Condvar)>,
    pub(in crate::controller) saved: &'a SavedServer,
    pub(in crate::controller) source: &'a dyn MusicSource,
    pub(in crate::controller) cancellation: Option<&'a CancellationToken>,
    pub(in crate::controller) emit_status: bool,
}

fn mark_prefetch_flight(
    external_cover_prefetch_in_flight: &Arc<Mutex<HashMap<ServerId, u64>>>,
    server_id: &ServerId,
    generation: u64,
) -> bool {
    external_cover_prefetch_in_flight
        .lock()
        .map(|mut running| {
            if running
                .get(server_id)
                .is_some_and(|existing_generation| *existing_generation >= generation)
            {
                return false;
            }
            running.insert(server_id.clone(), generation);
            true
        })
        .unwrap_or(false)
}

fn clear_prefetch_generation(
    external_cover_prefetch_in_flight: &Arc<Mutex<HashMap<ServerId, u64>>>,
    server_id: &ServerId,
    generation: u64,
) {
    if let Ok(mut running) = external_cover_prefetch_in_flight.lock()
        && running.get(server_id).copied() == Some(generation)
    {
        running.remove(server_id);
    }
}

#[derive(Clone, Copy)]
enum SyncedImagePrefetchOutcome {
    CacheHit,
    KnownMiss,
    Skipped,
    Fetched,
    Miss,
    Error,
}

impl SyncedImagePrefetchOutcome {
    fn used_network(self) -> bool {
        matches!(self, Self::Fetched | Self::Miss | Self::Error)
    }
}

pub(in crate::controller) fn start_cover_prefetch(request: ExternalCoverPrefetchRequest) {
    let ExternalCoverPrefetchRequest {
        store,
        runtime,
        secrets,
        events,
        cover_in_flight,
        external_cover_retry_generation,
        retry_generation,
        external_cover_prefetch_in_flight,
        cover_slots,
        saved,
    } = request;
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    if !mark_prefetch_flight(
        &external_cover_prefetch_in_flight,
        &server_id,
        retry_generation,
    ) {
        return;
    }

    thread::spawn(move || {
        info!(
            server_id = %saved.server.id,
            "started synced image prefetch"
        );
        let mut stats = SyncedImagePrefetchStats::default();
        let context = CoverPrefetchContext {
            store: &store,
            runtime: &runtime,
            secrets: &secrets,
            events: &events,
            emit_status: true,
            cover_in_flight: &cover_in_flight,
            external_cover_retry_generation: &external_cover_retry_generation,
            retry_generation,
            cover_slots: &cover_slots,
            saved: &saved,
            cancellation: None,
        };
        let result = prefetch_synced_images(&context, &mut stats);
        match result {
            Ok(()) => {
                info!(
                    server_id = %saved.server.id,
                    album_rows = stats.album_rows,
                    album_image_refs = stats.album_image_refs,
                    artist_rows = stats.artist_rows,
                    artist_image_refs = stats.artist_image_refs,
                    album_artist_rows = stats.album_artist_rows,
                    album_artist_image_refs = stats.album_artist_image_refs,
                    cache_hits = stats.cache_hits,
                    known_misses = stats.known_misses,
                    skipped = stats.skipped,
                    fetched = stats.fetched,
                    misses = stats.misses,
                    errors = stats.errors,
                    "completed synced image prefetch"
                );
            }
            Err(error) => {
                warn!(
                    %error,
                    server_id = %saved.server.id,
                    album_rows = stats.album_rows,
                    album_image_refs = stats.album_image_refs,
                    artist_rows = stats.artist_rows,
                    artist_image_refs = stats.artist_image_refs,
                    album_artist_rows = stats.album_artist_rows,
                    album_artist_image_refs = stats.album_artist_image_refs,
                    cache_hits = stats.cache_hits,
                    known_misses = stats.known_misses,
                    skipped = stats.skipped,
                    fetched = stats.fetched,
                    misses = stats.misses,
                    errors = stats.errors,
                    "failed to prefetch synced images"
                );
            }
        }
        clear_prefetch_generation(
            &external_cover_prefetch_in_flight,
            &server_id,
            retry_generation,
        );
    });
}

pub(in crate::controller) fn prefetch_initial_source_cover_cache(
    request: SourceCoverPrefetchRequest<'_>,
) -> Result<(), String> {
    let saved = request.saved;
    if saved.server.provider == "fake" {
        return Ok(());
    }

    let started = Instant::now();
    let mut source_stats = SourceCoverPrefetchStats::default();
    let context = CoverPrefetchContext {
        store: request.store,
        runtime: request.runtime,
        events: request.events,
        emit_status: request.emit_status,
        secrets: request.secrets,
        cover_in_flight: request.cover_in_flight,
        external_cover_retry_generation: request.external_cover_retry_generation,
        retry_generation: request.retry_generation,
        cover_slots: request.cover_slots,
        saved,
        cancellation: request.cancellation,
    };
    context.store.with_store_session(|cache_store| {
        prefetch_synced_source_covers(&context, request.source, cache_store, &mut source_stats)
    })?;
    source_stats.total_elapsed_ms = elapsed_ms(started);
    info!(
        server_id = %saved.server.id,
        album_rows = source_stats.album_rows,
        album_image_refs = source_stats.album_image_refs,
        track_rows = source_stats.track_rows,
        track_image_refs = source_stats.track_image_refs,
        artist_rows = source_stats.artist_rows,
        artist_image_refs = source_stats.artist_image_refs,
        album_artist_rows = source_stats.album_artist_rows,
        album_artist_image_refs = source_stats.album_artist_image_refs,
        genre_rows = source_stats.genre_rows,
        genre_image_refs = source_stats.genre_image_refs,
        playlist_rows = source_stats.playlist_rows,
        playlist_image_refs = source_stats.playlist_image_refs,
        image_refs = source_stats.image_refs,
        cache_hits = source_stats.cache_hits,
        skipped = source_stats.skipped,
        fetched = source_stats.fetched,
        misses = source_stats.misses,
        errors = source_stats.errors,
        select_refs_elapsed_ms = source_stats.select_refs_elapsed_ms,
        cover_loop_elapsed_ms = source_stats.cover_loop_elapsed_ms,
        total_elapsed_ms = source_stats.total_elapsed_ms,
        fetched_bytes = source_stats.fetched_bytes,
        fetch_elapsed_ms = source_stats.fetch_elapsed_ms,
        normalize_elapsed_ms = source_stats.normalize_elapsed_ms,
        write_elapsed_ms = source_stats.write_elapsed_ms,
        store_elapsed_ms = source_stats.store_elapsed_ms,
        cover_elapsed_ms = source_stats.cover_elapsed_ms,
        error_elapsed_ms = source_stats.error_elapsed_ms,
        max_fetch_ms = source_stats.max_fetch_ms,
        max_normalize_ms = source_stats.max_normalize_ms,
        max_write_ms = source_stats.max_write_ms,
        max_store_ms = source_stats.max_store_ms,
        max_cover_ms = source_stats.max_cover_ms,
        max_error_ms = source_stats.max_error_ms,
        slow_fetches = source_stats.slow_fetches,
        slow_normalizes = source_stats.slow_normalizes,
        slow_writes = source_stats.slow_writes,
        slow_store_saves = source_stats.slow_store_saves,
        slow_covers = source_stats.slow_covers,
        slow_errors = source_stats.slow_errors,
        "completed initial source cover cache prefetch"
    );
    Ok(())
}

fn prefetch_synced_images(
    context: &CoverPrefetchContext<'_>,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    prefetch_synced_album_covers(context, stats)?;
    prefetch_synced_artist_covers(context, false, stats)?;
    prefetch_synced_artist_covers(context, true, stats)
}

fn prefetch_synced_source_covers(
    context: &CoverPrefetchContext<'_>,
    source: &dyn MusicSource,
    cache_store: &Store,
    stats: &mut SourceCoverPrefetchStats,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    check_prefetch_cancelled(context.cancellation)?;
    let select_started = Instant::now();
    let image_refs = synced_source_cover_refs(
        cache_store,
        context.saved,
        context.cancellation,
        &mut seen,
        stats,
    )?;
    stats.select_refs_elapsed_ms = elapsed_ms(select_started);
    stats.image_refs = image_refs.len();
    emit_initial_cover_prefetch_status(context, 0, stats.image_refs);
    let loop_started = Instant::now();
    for (index, image_ref) in image_refs.into_iter().enumerate() {
        check_prefetch_cancelled(context.cancellation)?;
        if active_server_changed_in_store(cache_store, context.saved)? {
            stats.cover_loop_elapsed_ms = elapsed_ms(loop_started);
            info!(
                server_id = %context.saved.server.id,
                "stopped initial source cover prefetch because active server changed"
            );
            return Ok(());
        }
        let outcome = prefetch_source_image_ref(context, source, cache_store, image_ref, stats)?;
        record_source_cover_prefetch_outcome(stats, outcome);
        let processed = index + 1;
        if processed == stats.image_refs || processed % 25 == 0 {
            emit_initial_cover_prefetch_status(context, processed, stats.image_refs);
        }
    }
    stats.cover_loop_elapsed_ms = elapsed_ms(loop_started);
    Ok(())
}

fn emit_initial_cover_prefetch_status(
    context: &CoverPrefetchContext<'_>,
    processed: usize,
    total: usize,
) {
    if total == 0 || !context.emit_status {
        return;
    }
    let _sent = context.events.send(ControllerEvent::LoginStatus(format!(
        "Caching library artwork... {processed}/{total} covers checked"
    )));
}

fn synced_source_cover_refs(
    store: &Store,
    saved: &SavedServer,
    cancellation: Option<&CancellationToken>,
    seen: &mut HashSet<(String, String)>,
    stats: &mut SourceCoverPrefetchStats,
) -> Result<Vec<ImageRef>, String> {
    let mut image_refs = Vec::new();
    let mut offset = 0;
    loop {
        check_prefetch_cancelled(cancellation)?;
        let page = store
            .load_albums(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
            .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.album_rows += item_count;
        let before = image_refs.len();
        push_source_album_image_refs(&mut image_refs, seen, page.items);
        stats.album_image_refs += image_refs.len().saturating_sub(before);
        offset += item_count;
    }

    let mut offset = 0;
    loop {
        check_prefetch_cancelled(cancellation)?;
        let page = store
            .load_tracks(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
            .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.track_rows += item_count;
        let before = image_refs.len();
        push_source_track_image_refs(&mut image_refs, seen, page.items);
        stats.track_image_refs += image_refs.len().saturating_sub(before);
        offset += item_count;
    }

    for album_artist in [false, true] {
        let mut offset = 0;
        loop {
            check_prefetch_cancelled(cancellation)?;
            let page = store
                .load_artists(
                    &saved.server.id,
                    album_artist,
                    offset,
                    EXTERNAL_PREFETCH_PAGE_SIZE,
                )
                .map_err(|error| error.to_string())?;
            let item_count = page.items.len();
            if item_count == 0 {
                break;
            }
            if album_artist {
                stats.album_artist_rows += item_count;
            } else {
                stats.artist_rows += item_count;
            }
            let before = image_refs.len();
            push_source_artist_image_refs(&mut image_refs, seen, page.items);
            if album_artist {
                stats.album_artist_image_refs += image_refs.len().saturating_sub(before);
            } else {
                stats.artist_image_refs += image_refs.len().saturating_sub(before);
            }
            offset += item_count;
        }
    }

    let mut offset = 0;
    loop {
        check_prefetch_cancelled(cancellation)?;
        let page = store
            .load_genres(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
            .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.genre_rows += item_count;
        let before = image_refs.len();
        push_source_genre_image_refs(&mut image_refs, seen, page.items);
        stats.genre_image_refs += image_refs.len().saturating_sub(before);
        offset += item_count;
    }

    let mut offset = 0;
    loop {
        check_prefetch_cancelled(cancellation)?;
        let page = store
            .load_playlists(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
            .map_err(|error| error.to_string())?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.playlist_rows += item_count;
        let before = image_refs.len();
        push_source_playlist_image_refs(&mut image_refs, seen, page.items);
        stats.playlist_image_refs += image_refs.len().saturating_sub(before);
        offset += item_count;
    }

    Ok(image_refs)
}

fn prefetch_source_image_ref(
    context: &CoverPrefetchContext<'_>,
    source: &dyn MusicSource,
    cache_store: &Store,
    image_ref: ImageRef,
    stats: &mut SourceCoverPrefetchStats,
) -> Result<SyncedImagePrefetchOutcome, String> {
    check_prefetch_cancelled(context.cancellation)?;
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(
        &context.saved.server.id,
        &image_ref.item_id,
        tag,
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    let cache_hit = if cached_cover_path_for_key(&key).is_some() {
        true
    } else {
        cached_cover_path_for_saved_in_store(
            cache_store,
            context.saved,
            &image_ref,
            EXTERNAL_PREFETCH_COVER_SIZE,
        )
        .map_err(|error| error.to_string())?
        .is_some()
    };
    if cache_hit {
        return Ok(SyncedImagePrefetchOutcome::CacheHit);
    }
    if !mark_cover_in_flight(context.cover_in_flight, &key, context.retry_generation) {
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }

    check_prefetch_cancelled(context.cancellation)?;
    if !acquire_cover_slot(context.cover_slots) {
        unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }
    if let Err(error) = check_prefetch_cancelled(context.cancellation) {
        release_cover_slot(context.cover_slots);
        unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);
        return Err(error);
    }
    let request_started = Instant::now();
    let result = fetch_and_cache_source_cover_timed_with_store(
        cache_store,
        context.runtime,
        context.saved,
        source,
        image_ref.clone(),
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    release_cover_slot(context.cover_slots);
    unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);

    match result {
        Ok(result) => {
            stats.record_timing(result.timing);
            log_source_cover_timing(context.saved, &image_ref, result.timing);
            let _sent = context.events.send(ControllerEvent::CoverReady {
                key,
                path: result.path,
            });
            Ok(SyncedImagePrefetchOutcome::Fetched)
        }
        Err(error) => {
            let elapsed_ms = elapsed_ms(request_started);
            stats.record_error_elapsed(elapsed_ms);
            if is_source_not_found_error(&error) {
                debug!(
                    server_id = %context.saved.server.id,
                    item_id = %image_ref.item_id,
                    image_tag = tag,
                    elapsed_ms,
                    %error,
                    "initial source image was not available"
                );
                Ok(SyncedImagePrefetchOutcome::Miss)
            } else {
                warn!(
                    server_id = %context.saved.server.id,
                    item_id = %image_ref.item_id,
                    image_tag = tag,
                    elapsed_ms,
                    %error,
                    "failed to prefetch initial source image"
                );
                Ok(SyncedImagePrefetchOutcome::Error)
            }
        }
    }
}

fn log_source_cover_timing(saved: &SavedServer, image_ref: &ImageRef, timing: CoverFetchTiming) {
    let image_tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    debug!(
        server_id = %saved.server.id,
        source = %saved.server.provider,
        item_id = %image_ref.item_id,
        image_tag,
        bytes = timing.bytes,
        fetch_ms = timing.fetch_ms,
        normalize_ms = timing.normalize_ms,
        write_ms = timing.write_ms,
        store_ms = timing.store_ms,
        total_ms = timing.total_ms,
        "prefetched initial source cover"
    );
    if timing.fetch_ms >= SLOW_SOURCE_COVER_STAGE_MS
        || timing.normalize_ms >= SLOW_SOURCE_COVER_STAGE_MS
        || timing.write_ms >= SLOW_SOURCE_COVER_STAGE_MS
        || timing.store_ms >= SLOW_SOURCE_COVER_STAGE_MS
        || timing.total_ms >= SLOW_SOURCE_COVER_TOTAL_MS
    {
        warn!(
            server_id = %saved.server.id,
            source = %saved.server.provider,
            item_id = %image_ref.item_id,
            image_tag,
            bytes = timing.bytes,
            fetch_ms = timing.fetch_ms,
            normalize_ms = timing.normalize_ms,
            write_ms = timing.write_ms,
            store_ms = timing.store_ms,
            total_ms = timing.total_ms,
            "slow initial source cover prefetch"
        );
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn prefetch_synced_album_covers(
    context: &CoverPrefetchContext<'_>,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let settings = load_settings_from_store(context.store);
        if !external_metadata::enabled(&settings) {
            info!(
                server_id = %context.saved.server.id,
                private_mode = settings.private_mode,
                external_metadata_enabled = settings.external_metadata_enabled,
                "skipped synced external album cover prefetch"
            );
            return Ok(());
        }
        if active_server_changed(context.store, context.saved)? {
            info!(
                server_id = %context.saved.server.id,
                "stopped synced external album cover prefetch because active server changed"
            );
            return Ok(());
        }
        let page = context.store.with_store(|store| {
            store.load_albums(
                &context.saved.server.id,
                offset,
                EXTERNAL_PREFETCH_PAGE_SIZE,
            )
        })?;
        if page.items.is_empty() {
            return Ok(());
        }
        let album_count = page.items.len();
        stats.album_rows += album_count;
        let image_refs = external_album_refs(page.items, &settings);
        stats.album_image_refs += image_refs.len();
        for image_ref in image_refs {
            if !external_metadata::enabled(&load_settings_from_store(context.store))
                || active_server_changed(context.store, context.saved)?
            {
                return Ok(());
            }
            let outcome = prefetch_image_ref(context, image_ref)?;
            record_synced_image_prefetch_outcome(stats, outcome);
            if outcome.used_network() {
                thread::sleep(EXTERNAL_PREFETCH_DELAY);
            }
        }
        offset += album_count;
    }
}

fn prefetch_synced_artist_covers(
    context: &CoverPrefetchContext<'_>,
    album_artist: bool,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        if active_server_changed(context.store, context.saved)? {
            info!(
                server_id = %context.saved.server.id,
                album_artist,
                "stopped synced source artist image prefetch because active server changed"
            );
            return Ok(());
        }
        let page = context.store.with_store(|store| {
            store.load_artists(
                &context.saved.server.id,
                album_artist,
                offset,
                EXTERNAL_PREFETCH_PAGE_SIZE,
            )
        })?;
        let artists = page.items;
        if artists.is_empty() {
            return Ok(());
        }
        let artist_count = artists.len();
        if album_artist {
            stats.album_artist_rows += artist_count;
        } else {
            stats.artist_rows += artist_count;
        }
        let image_refs = source_artist_refs(artists);
        if album_artist {
            stats.album_artist_image_refs += image_refs.len();
        } else {
            stats.artist_image_refs += image_refs.len();
        }
        for image_ref in image_refs {
            if active_server_changed(context.store, context.saved)? {
                return Ok(());
            }
            let outcome = prefetch_image_ref(context, image_ref)?;
            record_synced_image_prefetch_outcome(stats, outcome);
        }
        offset += artist_count;
    }
}

fn prefetch_image_ref(
    context: &CoverPrefetchContext<'_>,
    image_ref: ImageRef,
) -> Result<SyncedImagePrefetchOutcome, String> {
    let is_external_image = external_metadata::is_external_image_ref(&image_ref);
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(
        &context.saved.server.id,
        &image_ref.item_id,
        tag,
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    if cached_cover_path_for_key(&key).is_some()
        || cached_cover_path_for_saved(
            context.store,
            context.saved,
            &image_ref,
            EXTERNAL_PREFETCH_COVER_SIZE,
        )?
        .is_some()
    {
        return Ok(SyncedImagePrefetchOutcome::CacheHit);
    }
    if is_external_image
        && external_lookup_miss_cached(
            context.store,
            context.saved,
            &image_ref,
            EXTERNAL_PREFETCH_COVER_SIZE,
        )?
    {
        return Ok(SyncedImagePrefetchOutcome::KnownMiss);
    }
    if !mark_cover_in_flight(context.cover_in_flight, &key, context.retry_generation) {
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }

    if !acquire_cover_slot(context.cover_slots) {
        unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }
    let result = fetch_and_cache_cover(
        context.store,
        context.runtime,
        context.secrets,
        context.saved,
        image_ref.clone(),
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    release_cover_slot(context.cover_slots);
    unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);

    match result {
        Ok(path) => {
            let _sent = context
                .events
                .send(ControllerEvent::CoverReady { key, path });
            Ok(SyncedImagePrefetchOutcome::Fetched)
        }
        Err(error) => {
            if is_external_image && external_metadata::is_expected_lookup_miss(&error) {
                let _in_flight = context
                    .cover_in_flight
                    .lock()
                    .map_err(|_| "cover in-flight lock was poisoned.".to_string())?;
                if context
                    .external_cover_retry_generation
                    .load(Ordering::SeqCst)
                    != context.retry_generation
                {
                    return Ok(SyncedImagePrefetchOutcome::Skipped);
                }
                save_external_lookup_miss(
                    context.store,
                    context.saved,
                    &image_ref,
                    EXTERNAL_PREFETCH_COVER_SIZE,
                    &error,
                )?;
                debug!(%error, "synced external image was not available");
                Ok(SyncedImagePrefetchOutcome::Miss)
            } else if !is_external_image && is_source_not_found_error(&error) {
                debug!(%error, "synced source image was not available");
                Ok(SyncedImagePrefetchOutcome::Miss)
            } else {
                warn!(%error, "failed to prefetch synced image");
                Ok(SyncedImagePrefetchOutcome::Error)
            }
        }
    }
}

fn record_synced_image_prefetch_outcome(
    stats: &mut SyncedImagePrefetchStats,
    outcome: SyncedImagePrefetchOutcome,
) {
    match outcome {
        SyncedImagePrefetchOutcome::CacheHit => stats.cache_hits += 1,
        SyncedImagePrefetchOutcome::KnownMiss => stats.known_misses += 1,
        SyncedImagePrefetchOutcome::Skipped => stats.skipped += 1,
        SyncedImagePrefetchOutcome::Fetched => stats.fetched += 1,
        SyncedImagePrefetchOutcome::Miss => stats.misses += 1,
        SyncedImagePrefetchOutcome::Error => stats.errors += 1,
    }
}

fn record_source_cover_prefetch_outcome(
    stats: &mut SourceCoverPrefetchStats,
    outcome: SyncedImagePrefetchOutcome,
) {
    match outcome {
        SyncedImagePrefetchOutcome::CacheHit => stats.cache_hits += 1,
        SyncedImagePrefetchOutcome::KnownMiss => stats.misses += 1,
        SyncedImagePrefetchOutcome::Skipped => stats.skipped += 1,
        SyncedImagePrefetchOutcome::Fetched => stats.fetched += 1,
        SyncedImagePrefetchOutcome::Miss => stats.misses += 1,
        SyncedImagePrefetchOutcome::Error => stats.errors += 1,
    }
}

fn active_server_changed(store: &StoreHandle, saved: &SavedServer) -> Result<bool, String> {
    Ok(store
        .with_store(|store| store.active_server())?
        .is_none_or(|active| active.server.id != saved.server.id))
}

fn active_server_changed_in_store(store: &Store, saved: &SavedServer) -> Result<bool, String> {
    Ok(store
        .active_server()
        .map_err(|error| error.to_string())?
        .is_none_or(|active| active.server.id != saved.server.id))
}

fn check_prefetch_cancelled(cancellation: Option<&CancellationToken>) -> Result<(), String> {
    if cancellation.is_some_and(|token| token.cancelled()) {
        return Err("Sync cancelled.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{clear_prefetch_generation, mark_prefetch_flight};
    use domain::ServerId;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn prefetch_replace_generation() {
        let in_flight = Arc::new(Mutex::new(HashMap::new()));
        let server_id = ServerId::new("jellyfin:server:test");

        assert!(mark_prefetch_flight(&in_flight, &server_id, 1));
        assert!(!mark_prefetch_flight(&in_flight, &server_id, 1));
        assert!(mark_prefetch_flight(&in_flight, &server_id, 2));
        assert!(!mark_prefetch_flight(&in_flight, &server_id, 1));

        clear_prefetch_generation(&in_flight, &server_id, 1);
        assert_eq!(
            in_flight
                .lock()
                .expect("external prefetch in-flight lock")
                .get(&server_id)
                .copied(),
            Some(2)
        );

        clear_prefetch_generation(&in_flight, &server_id, 2);
        assert!(
            in_flight
                .lock()
                .expect("external prefetch in-flight lock")
                .is_empty()
        );
    }
}
