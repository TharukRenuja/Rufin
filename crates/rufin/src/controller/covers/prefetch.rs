use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use domain::{ImageRef, ServerId};
use library::{SavedServer, image_cache_key};
use secrets::SecretStore;
use source::MusicProvider;
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

use crate::controller::{
    ControllerEvent, IMAGE_TAG_UNTAGGED, StoreHandle, acquire_cover_slot, load_settings_from_store,
    release_cover_slot,
};
use crate::external_metadata;

use super::cache::{
    cached_cover_path_for_key, cached_cover_path_for_saved, external_lookup_miss_cached,
    save_external_lookup_miss,
};
use super::candidates::{
    external_album_refs, provider_artist_refs, push_provider_album_image_refs,
    push_provider_artist_image_refs, push_provider_genre_image_refs,
    push_provider_playlist_image_refs, push_provider_track_image_refs,
};
use super::fetch::{
    fetch_and_cache_cover, fetch_and_cache_provider_cover, is_provider_not_found_error,
};
use super::{
    EXTERNAL_PREFETCH_COVER_SIZE, EXTERNAL_PREFETCH_DELAY, EXTERNAL_PREFETCH_PAGE_SIZE,
    mark_cover_in_flight, unmark_cover_in_flight_generation,
};

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
struct ProviderCoverPrefetchStats {
    album_rows: usize,
    track_rows: usize,
    artist_rows: usize,
    album_artist_rows: usize,
    genre_rows: usize,
    playlist_rows: usize,
    image_refs: usize,
    cache_hits: usize,
    skipped: usize,
    fetched: usize,
    misses: usize,
    errors: usize,
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
    cover_in_flight: &'a Arc<Mutex<HashMap<String, u64>>>,
    external_cover_retry_generation: &'a Arc<AtomicU64>,
    retry_generation: u64,
    cover_slots: &'a Arc<(Mutex<usize>, Condvar)>,
    saved: &'a SavedServer,
}

pub(in crate::controller) struct ProviderCoverPrefetchRequest<'a> {
    pub(in crate::controller) store: &'a StoreHandle,
    pub(in crate::controller) runtime: &'a Runtime,
    pub(in crate::controller) secrets: &'a Arc<dyn SecretStore>,
    pub(in crate::controller) events: &'a Sender<ControllerEvent>,
    pub(in crate::controller) cover_in_flight: &'a Arc<Mutex<HashMap<String, u64>>>,
    pub(in crate::controller) external_cover_retry_generation: &'a Arc<AtomicU64>,
    pub(in crate::controller) retry_generation: u64,
    pub(in crate::controller) cover_slots: &'a Arc<(Mutex<usize>, Condvar)>,
    pub(in crate::controller) saved: &'a SavedServer,
    pub(in crate::controller) provider: &'a dyn MusicProvider,
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
            cover_in_flight: &cover_in_flight,
            external_cover_retry_generation: &external_cover_retry_generation,
            retry_generation,
            cover_slots: &cover_slots,
            saved: &saved,
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

pub(in crate::controller) fn prefetch_initial_provider_cover_cache(
    request: ProviderCoverPrefetchRequest<'_>,
) -> Result<(), String> {
    let saved = request.saved;
    if saved.server.provider == "fake" {
        return Ok(());
    }

    let mut provider_stats = ProviderCoverPrefetchStats::default();
    let context = CoverPrefetchContext {
        store: request.store,
        runtime: request.runtime,
        events: request.events,
        secrets: request.secrets,
        cover_in_flight: request.cover_in_flight,
        external_cover_retry_generation: request.external_cover_retry_generation,
        retry_generation: request.retry_generation,
        cover_slots: request.cover_slots,
        saved,
    };
    prefetch_synced_provider_covers(&context, request.provider, &mut provider_stats)?;
    info!(
        server_id = %saved.server.id,
        album_rows = provider_stats.album_rows,
        track_rows = provider_stats.track_rows,
        artist_rows = provider_stats.artist_rows,
        album_artist_rows = provider_stats.album_artist_rows,
        genre_rows = provider_stats.genre_rows,
        playlist_rows = provider_stats.playlist_rows,
        image_refs = provider_stats.image_refs,
        cache_hits = provider_stats.cache_hits,
        skipped = provider_stats.skipped,
        fetched = provider_stats.fetched,
        misses = provider_stats.misses,
        errors = provider_stats.errors,
        "completed initial provider cover cache prefetch"
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

fn prefetch_synced_provider_covers(
    context: &CoverPrefetchContext<'_>,
    provider: &dyn MusicProvider,
    stats: &mut ProviderCoverPrefetchStats,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    let image_refs = synced_provider_cover_refs(context.store, context.saved, &mut seen, stats)?;
    stats.image_refs = image_refs.len();
    emit_initial_cover_prefetch_status(context, 0, stats.image_refs);
    for (index, image_ref) in image_refs.into_iter().enumerate() {
        if active_server_changed(context.store, context.saved)? {
            info!(
                server_id = %context.saved.server.id,
                "stopped initial provider cover prefetch because active server changed"
            );
            return Ok(());
        }
        let outcome = prefetch_provider_image_ref(context, provider, image_ref)?;
        record_provider_cover_prefetch_outcome(stats, outcome);
        let processed = index + 1;
        if processed == stats.image_refs || processed % 25 == 0 {
            emit_initial_cover_prefetch_status(context, processed, stats.image_refs);
        }
    }
    Ok(())
}

fn emit_initial_cover_prefetch_status(
    context: &CoverPrefetchContext<'_>,
    processed: usize,
    total: usize,
) {
    if total == 0 {
        return;
    }
    let _sent = context.events.send(ControllerEvent::LoginStatus(format!(
        "Caching library artwork… {processed}/{total} covers checked"
    )));
}

fn synced_provider_cover_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    seen: &mut HashSet<(String, String)>,
    stats: &mut ProviderCoverPrefetchStats,
) -> Result<Vec<ImageRef>, String> {
    let mut image_refs = Vec::new();
    let mut offset = 0;
    loop {
        let page = store.with_store(|store| {
            store.load_albums(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
        })?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.album_rows += item_count;
        push_provider_album_image_refs(&mut image_refs, seen, page.items);
        offset += item_count;
    }

    let mut offset = 0;
    loop {
        let page = store.with_store(|store| {
            store.load_tracks(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
        })?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.track_rows += item_count;
        push_provider_track_image_refs(&mut image_refs, seen, page.items);
        offset += item_count;
    }

    for album_artist in [false, true] {
        let mut offset = 0;
        loop {
            let page = store.with_store(|store| {
                store.load_artists(
                    &saved.server.id,
                    album_artist,
                    offset,
                    EXTERNAL_PREFETCH_PAGE_SIZE,
                )
            })?;
            let item_count = page.items.len();
            if item_count == 0 {
                break;
            }
            if album_artist {
                stats.album_artist_rows += item_count;
            } else {
                stats.artist_rows += item_count;
            }
            push_provider_artist_image_refs(&mut image_refs, seen, page.items);
            offset += item_count;
        }
    }

    let mut offset = 0;
    loop {
        let page = store.with_store(|store| {
            store.load_genres(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
        })?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.genre_rows += item_count;
        push_provider_genre_image_refs(&mut image_refs, seen, page.items);
        offset += item_count;
    }

    let mut offset = 0;
    loop {
        let page = store.with_store(|store| {
            store.load_playlists(&saved.server.id, offset, EXTERNAL_PREFETCH_PAGE_SIZE)
        })?;
        let item_count = page.items.len();
        if item_count == 0 {
            break;
        }
        stats.playlist_rows += item_count;
        push_provider_playlist_image_refs(&mut image_refs, seen, page.items);
        offset += item_count;
    }

    Ok(image_refs)
}

fn prefetch_provider_image_ref(
    context: &CoverPrefetchContext<'_>,
    provider: &dyn MusicProvider,
    image_ref: ImageRef,
) -> Result<SyncedImagePrefetchOutcome, String> {
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
    if !mark_cover_in_flight(context.cover_in_flight, &key, context.retry_generation) {
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }

    if !acquire_cover_slot(context.cover_slots) {
        unmark_cover_in_flight_generation(context.cover_in_flight, &key, context.retry_generation);
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }
    let result = fetch_and_cache_provider_cover(
        context.store,
        context.runtime,
        context.saved,
        provider,
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
            if is_provider_not_found_error(&error) {
                debug!(%error, "initial provider image was not available");
                Ok(SyncedImagePrefetchOutcome::Miss)
            } else {
                warn!(%error, "failed to prefetch initial provider image");
                Ok(SyncedImagePrefetchOutcome::Error)
            }
        }
    }
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
                "stopped synced provider artist image prefetch because active server changed"
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
        let image_refs = provider_artist_refs(artists);
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
            } else if !is_external_image && is_provider_not_found_error(&error) {
                debug!(%error, "synced provider image was not available");
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

fn record_provider_cover_prefetch_outcome(
    stats: &mut ProviderCoverPrefetchStats,
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
