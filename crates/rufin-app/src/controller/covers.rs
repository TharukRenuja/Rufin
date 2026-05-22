#![allow(clippy::too_many_arguments)]

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use rufin_core::{Album, AppSettings, Artist, ImageRef, ServerId};
use rufin_provider::{ImageKind, ImageRequest};
use rufin_secrets::SecretStore;
use rufin_store::{CoverCacheEntry, SavedServer, image_cache_key};
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

use crate::external_metadata;

use super::{
    AppController, ControllerEvent, IMAGE_TAG_UNTAGGED, StoreHandle, acquire_cover_slot,
    cover_cache_path_for_key, load_settings_from_store, provider_for_saved, release_cover_slot,
};

const EXTERNAL_PREFETCH_PAGE_SIZE: usize = 500;
const EXTERNAL_PREFETCH_COVER_SIZE: u32 = 256;
const EXTERNAL_THUMB_COVER_SIZE: u32 = 96;
const EXTERNAL_DETAIL_COVER_SIZE: u32 = 512;
const EXTERNAL_PREFETCH_DELAY: Duration = Duration::from_secs(1);

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

impl AppController {
    #[cfg(test)]
    pub fn cover_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
        let server = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?
            .server;
        Some(image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            size,
        ))
    }

    pub fn cached_cover_path(&self, image_ref: &ImageRef, size: u32) -> Option<PathBuf> {
        let saved = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?;
        cached_cover_path_for_saved(&self.store, &saved, image_ref, size)
            .ok()
            .flatten()
    }

    pub fn cached_cover_path_for_key(&self, key: &str) -> Option<PathBuf> {
        cached_cover_path_for_key(key)
    }

    #[cfg(test)]
    pub fn request_cover(&self, image_ref: ImageRef, size: u32) {
        let Some(saved) = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None)
        else {
            return;
        };
        if saved.server.provider == "fake" {
            return;
        }
        if let Some(path) = self.cached_cover_path(&image_ref, size) {
            if let Some(key) = self.cover_key(&image_ref, size) {
                let _sent = self.events.send(ControllerEvent::CoverReady { key, path });
            }
            return;
        }
        let tag = image_ref
            .tag
            .clone()
            .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
        let key = image_cache_key(&saved.server.id, &image_ref.item_id, &tag, size);
        match self.cover_in_flight.lock() {
            Ok(mut in_flight) => {
                if !in_flight.insert(key.clone()) {
                    return;
                }
            }
            Err(_) => return,
        }

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            if !acquire_cover_slot(&cover_slots) {
                if let Ok(mut in_flight) = cover_in_flight.lock() {
                    in_flight.remove(&key);
                }
                return;
            }
            let result = fetch_and_cache_cover(&store, &runtime, &secrets, &saved, image_ref, size);
            release_cover_slot(&cover_slots);
            if let Ok(mut in_flight) = cover_in_flight.lock() {
                in_flight.remove(&key);
            }
            match result {
                Ok(path) => {
                    let _sent = events.send(ControllerEvent::CoverReady { key, path });
                }
                Err(error) => {
                    warn!(%error, "failed to fetch cover");
                }
            }
        });
    }

    pub fn request_cover_for_key(&self, key: String, image_ref: ImageRef, size: u32) {
        match self.cover_in_flight.lock() {
            Ok(mut in_flight) => {
                if !in_flight.insert(key.clone()) {
                    return;
                }
            }
            Err(_) => return,
        }

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            let is_external_cover = external_metadata::is_external_image_ref(&image_ref);
            let miss_item_id = image_ref.item_id.clone();
            let miss_image_tag = image_ref
                .tag
                .clone()
                .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
            let result = (|| -> Result<Option<PathBuf>, String> {
                let settings = load_settings_from_store(&store);
                if is_external_cover && !external_metadata::enabled(&settings) {
                    return Ok(None);
                }
                if let Some(path) = cached_cover_path_for_key(&key) {
                    return Ok(Some(path));
                }

                let Some(saved) = store.with_store(|store| store.active_server())? else {
                    return Ok(None);
                };
                if saved.server.provider == "fake" {
                    return Ok(None);
                }

                let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
                let expected_key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
                if expected_key != key {
                    return Ok(None);
                }

                if let Some(path) = cached_cover_path_for_saved(&store, &saved, &image_ref, size)? {
                    return Ok(Some(path));
                }
                if is_external_cover
                    && external_lookup_miss_cached(&store, &saved, &image_ref, size)?
                {
                    return Ok(None);
                }

                if !acquire_cover_slot(&cover_slots) {
                    return Ok(None);
                }
                let result =
                    fetch_and_cache_cover(&store, &runtime, &secrets, &saved, image_ref, size)
                        .map(Some);
                release_cover_slot(&cover_slots);
                result
            })();

            if let Ok(mut in_flight) = cover_in_flight.lock() {
                in_flight.remove(&key);
            }
            match result {
                Ok(Some(path)) => {
                    let _sent = events.send(ControllerEvent::CoverReady { key, path });
                }
                Ok(None) => {}
                Err(error) => {
                    if is_external_cover && external_metadata::is_expected_lookup_miss(&error) {
                        let _saved_miss = store.with_store(|store| {
                            if let Some(saved) = store.active_server()? {
                                store.save_external_image_lookup_miss(
                                    &saved.server.id,
                                    &miss_item_id,
                                    &miss_image_tag,
                                    size,
                                    &error,
                                )?;
                            }
                            Ok(())
                        });
                        debug!(%error, "external metadata cover was not available");
                    } else if is_provider_not_found_error(&error) {
                        debug!(%error, "cached cover source item is no longer available");
                    } else {
                        warn!(%error, "failed to prepare cover");
                    }
                }
            }
        });
    }
}

pub(super) fn start_external_metadata_cover_prefetch_thread(
    store: StoreHandle,
    runtime: Arc<Runtime>,
    secrets: Arc<dyn SecretStore>,
    events: Sender<ControllerEvent>,
    cover_in_flight: Arc<Mutex<HashSet<String>>>,
    external_cover_prefetch_in_flight: Arc<Mutex<HashSet<ServerId>>>,
    cover_slots: Arc<(Mutex<usize>, Condvar)>,
    saved: SavedServer,
) {
    if saved.server.provider == "fake" {
        return;
    }

    let server_id = saved.server.id.clone();
    match external_cover_prefetch_in_flight.lock() {
        Ok(mut running) => {
            if !running.insert(server_id.clone()) {
                return;
            }
        }
        Err(_) => return,
    }

    thread::spawn(move || {
        info!(
            server_id = %saved.server.id,
            "started synced image prefetch"
        );
        let mut stats = SyncedImagePrefetchStats::default();
        let result = prefetch_synced_images(
            &store,
            &runtime,
            &secrets,
            &events,
            &cover_in_flight,
            &cover_slots,
            &saved,
            &mut stats,
        );
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
        if let Ok(mut running) = external_cover_prefetch_in_flight.lock() {
            running.remove(&server_id);
        }
    });
}

fn prefetch_synced_images(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    prefetch_synced_album_covers(
        store,
        runtime,
        secrets,
        events,
        cover_in_flight,
        cover_slots,
        saved,
        stats,
    )?;
    prefetch_synced_artist_covers(
        store,
        runtime,
        secrets,
        events,
        cover_in_flight,
        cover_slots,
        saved,
        false,
        stats,
    )?;
    prefetch_synced_artist_covers(
        store,
        runtime,
        secrets,
        events,
        cover_in_flight,
        cover_slots,
        saved,
        true,
        stats,
    )
}

fn prefetch_synced_album_covers(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings) {
            info!(
                server_id = %saved.server.id,
                private_mode = settings.private_mode,
                external_metadata_enabled = settings.external_metadata_enabled,
                "skipped synced external album cover prefetch"
            );
            return Ok(());
        }
        if active_server_changed(store, saved)? {
            info!(
                server_id = %saved.server.id,
                "stopped synced external album cover prefetch because active server changed"
            );
            return Ok(());
        }
        let albums = store.with_store(|store| {
            store.load_albums_without_image_ref(
                &saved.server.id,
                offset,
                EXTERNAL_PREFETCH_PAGE_SIZE,
            )
        })?;
        if albums.is_empty() {
            return Ok(());
        }
        let album_count = albums.len();
        stats.album_rows += album_count;
        let image_refs = external_album_image_refs_from_albums(albums, &settings);
        stats.album_image_refs += image_refs.len();
        for image_ref in image_refs {
            if !external_metadata::enabled(&load_settings_from_store(store))
                || active_server_changed(store, saved)?
            {
                return Ok(());
            }
            let outcome = prefetch_image_ref(
                store,
                runtime,
                secrets,
                events,
                cover_in_flight,
                cover_slots,
                saved,
                image_ref,
            )?;
            record_synced_image_prefetch_outcome(stats, outcome);
            if outcome.used_network() {
                thread::sleep(EXTERNAL_PREFETCH_DELAY);
            }
        }
        offset += album_count;
    }
}

fn prefetch_synced_artist_covers(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
    album_artist: bool,
    stats: &mut SyncedImagePrefetchStats,
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        if active_server_changed(store, saved)? {
            info!(
                server_id = %saved.server.id,
                album_artist,
                "stopped synced provider artist image prefetch because active server changed"
            );
            return Ok(());
        }
        let page = store.with_store(|store| {
            store.load_artists(
                &saved.server.id,
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
        let image_refs = provider_artist_image_refs_from_artists(artists);
        if album_artist {
            stats.album_artist_image_refs += image_refs.len();
        } else {
            stats.artist_image_refs += image_refs.len();
        }
        for image_ref in image_refs {
            if active_server_changed(store, saved)? {
                return Ok(());
            }
            let outcome = prefetch_image_ref(
                store,
                runtime,
                secrets,
                events,
                cover_in_flight,
                cover_slots,
                saved,
                image_ref,
            )?;
            record_synced_image_prefetch_outcome(stats, outcome);
        }
        offset += artist_count;
    }
}

fn prefetch_image_ref(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
    image_ref: ImageRef,
) -> Result<SyncedImagePrefetchOutcome, String> {
    let is_external_image = external_metadata::is_external_image_ref(&image_ref);
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(
        &saved.server.id,
        &image_ref.item_id,
        tag,
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    if cached_cover_path_for_key(&key).is_some()
        || cached_cover_path_for_saved(store, saved, &image_ref, EXTERNAL_PREFETCH_COVER_SIZE)?
            .is_some()
    {
        return Ok(SyncedImagePrefetchOutcome::CacheHit);
    }
    if is_external_image
        && external_lookup_miss_cached(store, saved, &image_ref, EXTERNAL_PREFETCH_COVER_SIZE)?
    {
        return Ok(SyncedImagePrefetchOutcome::KnownMiss);
    }
    match cover_in_flight.lock() {
        Ok(mut in_flight) => {
            if !in_flight.insert(key.clone()) {
                return Ok(SyncedImagePrefetchOutcome::Skipped);
            }
        }
        Err(_) => return Ok(SyncedImagePrefetchOutcome::Skipped),
    }

    if !acquire_cover_slot(cover_slots) {
        if let Ok(mut in_flight) = cover_in_flight.lock() {
            in_flight.remove(&key);
        }
        return Ok(SyncedImagePrefetchOutcome::Skipped);
    }
    let result = fetch_and_cache_cover(
        store,
        runtime,
        secrets,
        saved,
        image_ref.clone(),
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    release_cover_slot(cover_slots);
    if let Ok(mut in_flight) = cover_in_flight.lock() {
        in_flight.remove(&key);
    }

    match result {
        Ok(path) => {
            let _sent = events.send(ControllerEvent::CoverReady { key, path });
            Ok(SyncedImagePrefetchOutcome::Fetched)
        }
        Err(error) => {
            if is_external_image && external_metadata::is_expected_lookup_miss(&error) {
                save_external_lookup_miss(
                    store,
                    saved,
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

fn active_server_changed(store: &StoreHandle, saved: &SavedServer) -> Result<bool, String> {
    Ok(store
        .with_store(|store| store.active_server())?
        .is_none_or(|active| active.server.id != saved.server.id))
}

fn external_album_image_refs_from_albums(
    mut albums: Vec<Album>,
    settings: &AppSettings,
) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    let mut seen = HashSet::new();
    external_metadata::normalize_albums(&mut albums, settings);
    for image_ref in albums
        .iter()
        .filter_map(|album| album.image_ref.as_ref())
        .filter(|image_ref| external_metadata::is_external_image_ref(image_ref))
    {
        let key = (
            image_ref.item_id.clone(),
            image_ref.tag.clone().unwrap_or_default(),
        );
        if seen.insert(key) {
            image_refs.push(image_ref.clone());
        }
    }
    image_refs
}

fn provider_artist_image_refs_from_artists(artists: Vec<Artist>) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    let mut seen = HashSet::new();
    for image_ref in artists
        .iter()
        .filter_map(|artist| artist.image_ref.as_ref())
        .filter(|image_ref| !external_metadata::is_external_image_ref(image_ref))
    {
        let key = (
            image_ref.item_id.clone(),
            image_ref.tag.clone().unwrap_or_default(),
        );
        if seen.insert(key) {
            image_refs.push(image_ref.clone());
        }
    }
    image_refs
}

fn cached_cover_path_for_saved(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = cached_cover_path_for_saved_size(store, saved, image_ref, size)? {
        return Ok(Some(path));
    }
    for candidate_size in cover_cache_size_candidates(size) {
        if candidate_size == size {
            continue;
        }
        if let Some(path) =
            cached_cover_path_for_saved_size(store, saved, image_ref, candidate_size)?
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn cached_cover_path_for_saved_size(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<Option<PathBuf>, String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
    let Some(entry) = store.with_store(|store| {
        store.load_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?
    else {
        return Ok(cached_cover_path_for_key(&key));
    };
    let path = PathBuf::from(entry.path);
    if path.exists() {
        return Ok(Some(path));
    }
    if let Some(path) = cached_cover_path_for_key(&key) {
        return Ok(Some(path));
    }
    store.with_store(|store| {
        store.delete_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?;
    Ok(None)
}

fn cover_cache_size_candidates(size: u32) -> Vec<u32> {
    if size <= EXTERNAL_THUMB_COVER_SIZE {
        vec![
            EXTERNAL_THUMB_COVER_SIZE,
            EXTERNAL_PREFETCH_COVER_SIZE,
            EXTERNAL_DETAIL_COVER_SIZE,
        ]
    } else if size <= EXTERNAL_PREFETCH_COVER_SIZE {
        vec![EXTERNAL_PREFETCH_COVER_SIZE, EXTERNAL_DETAIL_COVER_SIZE]
    } else {
        vec![EXTERNAL_DETAIL_COVER_SIZE, EXTERNAL_PREFETCH_COVER_SIZE]
    }
}

fn external_lookup_miss_cached(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<bool, String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    store.with_store(|store| {
        store.load_external_image_lookup_miss(&saved.server.id, &image_ref.item_id, tag, size)
    })
}

fn save_external_lookup_miss(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
    reason: &str,
) -> Result<(), String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    store.with_store(|store| {
        store.save_external_image_lookup_miss(
            &saved.server.id,
            &image_ref.item_id,
            tag,
            size,
            reason,
        )
    })
}

fn cached_cover_path_for_key(key: &str) -> Option<PathBuf> {
    let path = cover_cache_path_for_key(key)?;
    path.exists().then_some(path)
}

fn fetch_and_cache_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
    image_ref: ImageRef,
    size: u32,
) -> Result<PathBuf, String> {
    let bytes = if let Some(art) = external_metadata::album_art_from_image_ref(&image_ref) {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings) {
            return Err("external metadata lookup is disabled".to_string());
        }
        external_metadata::fetch_album_cover(&art, size, settings.lastfm_api_key.trim())?
    } else if external_metadata::is_external_artist_image_ref(&image_ref) {
        return Err("external artist image lookup is disabled".to_string());
    } else {
        let provider = provider_for_saved(store, runtime, secrets, saved)?;
        let image = runtime
            .block_on(provider.as_music_provider().image_bytes(ImageRequest {
                item_id: image_ref.item_id.clone(),
                kind: ImageKind::Primary,
                tag: image_ref.tag.clone(),
                size,
            }))
            .map_err(|error| error.to_string())?;
        if image.bytes.is_empty() {
            return Err("cover response was empty".to_string());
        }
        image.bytes
    };

    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
    let path = cover_cache_path_for_key(&key)
        .ok_or_else(|| "cache directory is unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, &path).map_err(|error| error.to_string())?;

    store.with_store(|store| {
        store.save_cover_cache_entry(&CoverCacheEntry {
            server_id: saved.server.id.clone(),
            item_id: image_ref.item_id,
            image_tag: tag.to_string(),
            size,
            path: path.to_string_lossy().to_string(),
        })
    })?;

    Ok(path)
}

pub(super) fn is_provider_not_found_error(error: &str) -> bool {
    error == "provider item was not found"
}

#[cfg(test)]
mod tests {
    use rufin_core::{Album, AlbumId, AppSettings, Artist, ArtistId, ImageRef};

    use super::{external_album_image_refs_from_albums, provider_artist_image_refs_from_artists};
    use crate::external_metadata;

    #[test]
    fn synced_external_cover_candidates_use_only_albums_without_provider_art() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            ..AppSettings::default()
        };
        let refs = external_album_image_refs_from_albums(
            vec![
                album_without_cover(1, "Loveless", "My Bloody Valentine"),
                album_with_cover(2, "Souvlaki", "Slowdive"),
                album_without_cover(3, "Loveless", "My Bloody Valentine"),
            ],
            &settings,
        );

        assert_eq!(refs.len(), 1);
        assert!(external_metadata::is_external_image_ref(&refs[0]));
        assert_eq!(
            external_metadata::album_art_from_image_ref(&refs[0]).map(|art| art.album),
            Some("Loveless".to_string())
        );
    }

    #[test]
    fn synced_external_cover_candidates_respect_private_mode() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            private_mode: true,
            ..AppSettings::default()
        };

        assert!(
            external_album_image_refs_from_albums(
                vec![album_without_cover(1, "Loveless", "My Bloody Valentine")],
                &settings,
            )
            .is_empty()
        );
    }

    #[test]
    fn synced_provider_artist_cover_candidates_use_only_provider_art() {
        let refs = provider_artist_image_refs_from_artists(vec![
            artist_without_cover(1, "Slowdive"),
            artist_with_cover(2, "Ride"),
            artist_with_cover(2, "Ride"),
        ]);

        assert_eq!(refs.len(), 1);
        assert!(!external_metadata::is_external_image_ref(&refs[0]));
        assert_eq!(refs[0].item_id, "provider-artist-2");
    }

    #[test]
    fn synced_provider_artist_cover_candidates_skip_synthetic_external_refs() {
        assert!(
            provider_artist_image_refs_from_artists(vec![Artist {
                image_ref: Some(ImageRef::new(
                    "external:artist:Slowdive",
                    Some("external-artist-v1-old".to_string()),
                )),
                ..artist_without_cover(1, "Slowdive")
            }])
            .is_empty()
        );
    }

    fn album_without_cover(number: u32, title: &str, artist: &str) -> Album {
        Album {
            id: AlbumId::fake(number),
            title: title.to_string(),
            artist: artist.to_string(),
            artist_id: Some(ArtistId::fake(number)),
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 1991,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 60,
            favorite: false,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
        }
    }

    fn album_with_cover(number: u32, title: &str, artist: &str) -> Album {
        Album {
            image_ref: Some(ImageRef::new(
                format!("provider-album-{number}"),
                Some(format!("tag-{number}")),
            )),
            ..album_without_cover(number, title, artist)
        }
    }

    fn artist_without_cover(number: u32, name: &str) -> Artist {
        Artist {
            id: ArtistId::fake(number),
            name: name.to_string(),
            album_count: 1,
            track_count: 1,
            favorite: false,
            last_played: None,
            play_count: None,
            user_rating: None,
            image_ref: None,
        }
    }

    fn artist_with_cover(number: u32, name: &str) -> Artist {
        Artist {
            image_ref: Some(ImageRef::new(
                format!("provider-artist-{number}"),
                Some(format!("tag-{number}")),
            )),
            ..artist_without_cover(number, name)
        }
    }
}
