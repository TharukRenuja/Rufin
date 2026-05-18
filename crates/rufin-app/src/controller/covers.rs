use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rufin_core::ImageRef;
use rufin_provider::{ImageKind, ImageRequest};
use rufin_secrets::SecretStore;
use rufin_store::{CoverCacheEntry, SavedServer, image_cache_key};
use tokio::runtime::Runtime;
use tracing::{debug, warn};

use crate::external_metadata;

use super::{
    AppController, ControllerEvent, IMAGE_TAG_UNTAGGED, LibrarySnapshot, StoreHandle,
    acquire_cover_slot, cache_dir, load_settings_from_store, provider_for_saved,
    release_cover_slot,
};

const EXTERNAL_PREFETCH_COVER_SIZE: u32 = 256;
const EXTERNAL_PREFETCH_DELAY: Duration = Duration::from_secs(1);

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

    #[cfg(test)]
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

    pub fn prefetch_external_metadata_covers(&self, snapshot: &LibrarySnapshot) {
        if snapshot.first_run {
            return;
        }
        let Some(server_id) = snapshot.server.as_ref().map(|server| server.id.clone()) else {
            return;
        };
        let image_refs = external_image_refs_from_snapshot(snapshot);
        if image_refs.is_empty() {
            return;
        }
        match self.external_cover_prefetch_in_flight.lock() {
            Ok(mut running) => {
                if !running.insert(server_id.clone()) {
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
        let external_cover_prefetch_in_flight = Arc::clone(&self.external_cover_prefetch_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            let run = || {
                let settings = load_settings_from_store(&store);
                if !external_metadata::enabled(&settings) {
                    return;
                }
                let Some(saved) = store
                    .with_store(|store| store.active_server())
                    .unwrap_or(None)
                else {
                    return;
                };
                if saved.server.provider == "fake" {
                    return;
                }

                for image_ref in image_refs {
                    let settings = load_settings_from_store(&store);
                    if !external_metadata::enabled(&settings) {
                        return;
                    }
                    if store
                        .with_store(|store| store.active_server())
                        .ok()
                        .flatten()
                        .is_none_or(|active| active.server.id != saved.server.id)
                    {
                        return;
                    }
                    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
                    let key = image_cache_key(
                        &saved.server.id,
                        &image_ref.item_id,
                        tag,
                        EXTERNAL_PREFETCH_COVER_SIZE,
                    );
                    if cached_cover_path_for_key(&key).is_some() {
                        continue;
                    }
                    match cover_in_flight.lock() {
                        Ok(mut in_flight) => {
                            if !in_flight.insert(key.clone()) {
                                continue;
                            }
                        }
                        Err(_) => return,
                    }

                    if !acquire_cover_slot(&cover_slots) {
                        if let Ok(mut in_flight) = cover_in_flight.lock() {
                            in_flight.remove(&key);
                        }
                        return;
                    }
                    let result = fetch_and_cache_cover(
                        &store,
                        &runtime,
                        &secrets,
                        &saved,
                        image_ref,
                        EXTERNAL_PREFETCH_COVER_SIZE,
                    );
                    release_cover_slot(&cover_slots);
                    if let Ok(mut in_flight) = cover_in_flight.lock() {
                        in_flight.remove(&key);
                    }

                    match result {
                        Ok(path) => {
                            let _sent = events.send(ControllerEvent::CoverReady { key, path });
                        }
                        Err(error) => {
                            debug!(%error, "failed to prefetch external metadata cover");
                        }
                    }
                    thread::sleep(EXTERNAL_PREFETCH_DELAY);
                }
            };
            run();
            if let Ok(mut running) = external_cover_prefetch_in_flight.lock() {
                running.remove(&server_id);
            }
        });
    }
}

fn external_image_refs_from_snapshot(snapshot: &LibrarySnapshot) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    let mut seen = HashSet::new();
    for image_ref in snapshot
        .albums
        .iter()
        .filter_map(|album| album.image_ref.as_ref())
        .chain(
            snapshot
                .tracks
                .iter()
                .filter_map(|track| track.image_ref.as_ref()),
        )
        .chain(
            snapshot
                .favorites
                .iter()
                .filter_map(|track| track.image_ref.as_ref()),
        )
        .chain(snapshot.home_sections.iter().flat_map(|section| {
            section
                .albums
                .iter()
                .filter_map(|album| album.image_ref.as_ref())
                .chain(
                    section
                        .tracks
                        .iter()
                        .filter_map(|track| track.image_ref.as_ref()),
                )
        }))
        .chain(snapshot.prefetched_explore.iter().flat_map(|section| {
            section
                .albums
                .iter()
                .filter_map(|album| album.image_ref.as_ref())
                .chain(
                    section
                        .tracks
                        .iter()
                        .filter_map(|track| track.image_ref.as_ref()),
                )
        }))
    {
        if !external_metadata::is_external_image_ref(image_ref) {
            continue;
        }
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
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let Some(entry) = store.with_store(|store| {
        store.load_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?
    else {
        return Ok(None);
    };
    let path = PathBuf::from(entry.path);
    if path.exists() {
        return Ok(Some(path));
    }
    store.with_store(|store| {
        store.delete_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?;
    Ok(None)
}

fn cached_cover_path_for_key(key: &str) -> Option<PathBuf> {
    let path = cache_dir()?.join("covers").join(key);
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
    let path = cache_dir()
        .map(|dir| dir.join("covers").join(&key))
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
