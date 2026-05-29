use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rufin_core::ImageRef;
use rufin_store::image_cache_key;
use tracing::{debug, warn};

use crate::external_metadata;

mod cache;
mod candidates;
mod fetch;
mod prefetch;

use super::{
    AppController, ControllerEvent, IMAGE_TAG_UNTAGGED, acquire_cover_slot,
    load_settings_from_store, release_cover_slot,
};
use cache::*;
use fetch::fetch_and_cache_cover;
pub(super) use fetch::is_provider_not_found_error;
pub(super) use prefetch::{
    ExternalCoverPrefetchRequest, prefetch_initial_provider_cover_cache,
    start_external_metadata_cover_prefetch_thread,
};

const EXTERNAL_PREFETCH_PAGE_SIZE: usize = 500;
const EXTERNAL_PREFETCH_COVER_SIZE: u32 = 256;
const EXTERNAL_THUMB_COVER_SIZE: u32 = 96;
const EXTERNAL_DETAIL_COVER_SIZE: u32 = 512;
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

    pub fn external_cover_lookup_known_missing(&self, image_ref: &ImageRef, size: u32) -> bool {
        if !external_metadata::is_external_image_ref(image_ref) {
            return false;
        }
        let Some(saved) = self
            .store
            .with_store(|store| store.active_server())
            .ok()
            .flatten()
        else {
            return false;
        };
        external_lookup_miss_size_candidates(size)
            .into_iter()
            .any(|candidate_size| {
                external_lookup_miss_cached(&self.store, &saved, image_ref, candidate_size)
                    .unwrap_or(false)
            })
    }

    pub fn retry_external_cover_lookups(&self) -> Result<(), String> {
        let Some(saved) = self.store.with_store(|store| store.active_server())? else {
            return Ok(());
        };
        self.store
            .with_store(|store| store.clear_external_image_lookup_misses(&saved.server.id))?;
        start_external_metadata_cover_prefetch_thread(ExternalCoverPrefetchRequest {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
            external_cover_prefetch_in_flight: Arc::clone(&self.external_cover_prefetch_in_flight),
            cover_slots: Arc::clone(&self.cover_slots),
            saved,
        });
        Ok(())
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
