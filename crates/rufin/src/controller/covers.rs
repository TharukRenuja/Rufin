use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use domain::ImageRef;
use library::image_cache_key;
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
    ExternalCoverPrefetchRequest, ProviderCoverPrefetchRequest,
    prefetch_initial_provider_cover_cache, start_cover_prefetch,
};

const EXTERNAL_PREFETCH_PAGE_SIZE: usize = 500;
const EXTERNAL_PREFETCH_COVER_SIZE: u32 = 256;
const EXTERNAL_THUMB_COVER_SIZE: u32 = 96;
const EXTERNAL_DETAIL_COVER_SIZE: u32 = 512;
const EXTERNAL_PREFETCH_DELAY: Duration = Duration::from_secs(1);

enum CoverRequestOutcome {
    Ready(PathBuf),
    TerminalMissing {
        external_retry_generation: Option<u64>,
    },
    Deferred,
}

pub(super) fn mark_cover_in_flight(
    cover_in_flight: &Arc<Mutex<HashMap<String, u64>>>,
    key: &str,
    generation: u64,
) -> bool {
    cover_in_flight
        .lock()
        .map(|mut in_flight| {
            if in_flight
                .get(key)
                .is_some_and(|existing_generation| *existing_generation >= generation)
            {
                return false;
            }
            in_flight.insert(key.to_string(), generation);
            true
        })
        .unwrap_or(false)
}

#[cfg(test)]
pub(super) fn clear_cover_in_flight(cover_in_flight: &Arc<Mutex<HashMap<String, u64>>>) {
    if let Ok(mut in_flight) = cover_in_flight.lock() {
        in_flight.clear();
    }
}

pub(super) fn unmark_cover_in_flight_generation(
    cover_in_flight: &Arc<Mutex<HashMap<String, u64>>>,
    key: &str,
    generation: u64,
) {
    if let Ok(mut in_flight) = cover_in_flight.lock()
        && in_flight.get(key).copied() == Some(generation)
    {
        in_flight.remove(key);
    }
}

fn cover_retry_check(external_cover_retry_generation: &AtomicU64, generation: u64) -> bool {
    external_cover_retry_generation.load(Ordering::SeqCst) == generation
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

    pub fn cover_retry_status(&self, generation: u64) -> bool {
        cover_retry_check(&self.external_cover_retry_generation, generation)
    }

    #[cfg(test)]
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
        let retry_generation = {
            let mut in_flight = self
                .cover_in_flight
                .lock()
                .map_err(|_| "cover in-flight lock was poisoned.".to_string())?;
            let retry_generation = self
                .external_cover_retry_generation
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            in_flight.clear();
            retry_generation
        };
        self.store
            .with_store(|store| store.clear_external_image_lookup_misses(&saved.server.id))?;
        start_cover_prefetch(ExternalCoverPrefetchRequest {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
            external_cover_retry_generation: Arc::clone(&self.external_cover_retry_generation),
            retry_generation,
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
        let retry_generation = self.external_cover_retry_generation.load(Ordering::SeqCst);
        if !mark_cover_in_flight(&self.cover_in_flight, &key, retry_generation) {
            return;
        }

        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        thread::spawn(move || {
            if !acquire_cover_slot(&cover_slots) {
                unmark_cover_in_flight_generation(&cover_in_flight, &key, retry_generation);
                return;
            }
            let result = fetch_and_cache_cover(&store, &runtime, &secrets, &saved, image_ref, size);
            release_cover_slot(&cover_slots);
            unmark_cover_in_flight_generation(&cover_in_flight, &key, retry_generation);
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

    pub fn request_cover_for_key(&self, key: String, image_ref: ImageRef, size: u32) -> bool {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let secrets = Arc::clone(&self.secrets);
        let events = self.events.clone();
        let cover_in_flight = Arc::clone(&self.cover_in_flight);
        let cover_slots = Arc::clone(&self.cover_slots);
        let external_cover_retry_generation = Arc::clone(&self.external_cover_retry_generation);
        let retry_generation = external_cover_retry_generation.load(Ordering::SeqCst);
        if !mark_cover_in_flight(&self.cover_in_flight, &key, retry_generation) {
            return false;
        }
        thread::spawn(move || {
            let is_external_cover = external_metadata::is_external_image_ref(&image_ref);
            let miss_item_id = image_ref.item_id.clone();
            let miss_image_tag = image_ref
                .tag
                .clone()
                .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
            let result = defer_locked_cover_prepare((|| -> Result<CoverRequestOutcome, String> {
                if let Some(path) = cached_cover_path_for_key(&key) {
                    return Ok(CoverRequestOutcome::Ready(path));
                }

                let settings = load_settings_from_store(&store);
                if is_external_cover && !external_metadata::enabled(&settings) {
                    return Ok(CoverRequestOutcome::TerminalMissing {
                        external_retry_generation: Some(retry_generation),
                    });
                }

                let Some(saved) = store.with_store(|store| store.active_server())? else {
                    return Ok(CoverRequestOutcome::Deferred);
                };
                if saved.server.provider == "fake" {
                    return Ok(CoverRequestOutcome::TerminalMissing {
                        external_retry_generation: None,
                    });
                }

                let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
                let expected_key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
                if expected_key != key {
                    return Ok(CoverRequestOutcome::Deferred);
                }

                if let Some(path) = cached_cover_path_for_saved(&store, &saved, &image_ref, size)? {
                    return Ok(CoverRequestOutcome::Ready(path));
                }
                if is_external_cover
                    && external_lookup_miss_cached(&store, &saved, &image_ref, size)?
                {
                    if !cover_retry_check(&external_cover_retry_generation, retry_generation) {
                        return Ok(CoverRequestOutcome::Deferred);
                    }
                    return Ok(CoverRequestOutcome::TerminalMissing {
                        external_retry_generation: Some(retry_generation),
                    });
                }

                if !acquire_cover_slot(&cover_slots) {
                    return Ok(CoverRequestOutcome::Deferred);
                }
                let result = (|| -> Result<CoverRequestOutcome, String> {
                    match fetch_and_cache_cover(
                        &store,
                        &runtime,
                        &secrets,
                        &saved,
                        image_ref.clone(),
                        size,
                    ) {
                        Ok(path) => Ok(CoverRequestOutcome::Ready(path)),
                        Err(error)
                            if cover_error_is_terminal(
                                &saved.server.provider,
                                is_external_cover,
                                &error,
                            ) =>
                        {
                            if is_external_cover
                                && external_metadata::is_expected_lookup_miss(&error)
                            {
                                let _in_flight = cover_in_flight.lock().map_err(|_| {
                                    "cover in-flight lock was poisoned.".to_string()
                                })?;
                                if !cover_retry_check(
                                    &external_cover_retry_generation,
                                    retry_generation,
                                ) {
                                    return Ok(CoverRequestOutcome::Deferred);
                                }
                                let _saved_miss = store.with_store(|store| {
                                    store.save_external_image_lookup_miss(
                                        &saved.server.id,
                                        &miss_item_id,
                                        &miss_image_tag,
                                        size,
                                        &error,
                                    )
                                });
                            }
                            if is_external_cover {
                                debug!(%error, "external metadata cover was not available");
                            } else if is_provider_not_found_error(&error) {
                                debug!(%error, "cached cover source item is no longer available");
                            } else {
                                warn!(%error, "cover is not available");
                            }
                            Ok(CoverRequestOutcome::TerminalMissing {
                                external_retry_generation: is_external_cover
                                    .then_some(retry_generation),
                            })
                        }
                        Err(error) => Err(error),
                    }
                })();
                release_cover_slot(&cover_slots);
                result
            })());

            unmark_cover_in_flight_generation(&cover_in_flight, &key, retry_generation);
            emit_cover_outcome(&events, &external_cover_retry_generation, key, result);
        });
        true
    }
}

fn defer_locked_cover_prepare(
    result: Result<CoverRequestOutcome, String>,
) -> Result<CoverRequestOutcome, String> {
    if let Err(error) = &result
        && cover_error_is_transient(error)
    {
        debug!(%error, "deferred cover preparation while store is busy");
        return Ok(CoverRequestOutcome::Deferred);
    }
    result
}

fn cover_error_is_transient(error: &str) -> bool {
    error.contains("database is locked") || error.contains("database table is locked")
}

fn emit_cover_outcome(
    events: &Sender<ControllerEvent>,
    external_cover_retry_generation: &AtomicU64,
    key: String,
    result: Result<CoverRequestOutcome, String>,
) {
    match result {
        Ok(CoverRequestOutcome::Ready(path)) => {
            let _sent = events.send(ControllerEvent::CoverReady { key, path });
        }
        Ok(CoverRequestOutcome::TerminalMissing {
            external_retry_generation,
        }) => {
            if external_retry_generation.is_some_and(|generation| {
                !cover_retry_check(external_cover_retry_generation, generation)
            }) {
                return;
            }
            let _sent = events.send(ControllerEvent::CoverUnavailable {
                key,
                external_retry_generation,
            });
        }
        Ok(CoverRequestOutcome::Deferred) => {
            let _sent = events.send(ControllerEvent::CoverDeferred { key });
        }
        Err(error) => {
            warn!(%error, "failed to prepare cover");
        }
    }
}

fn cover_error_is_terminal(provider: &str, is_external_cover: bool, error: &str) -> bool {
    is_provider_not_found_error(error)
        || error == "cover response was empty"
        || error.contains("No such file or directory")
        || error.contains("os error 2")
        || (provider == source_local::LOCAL_PROVIDER_ID
            && (error.contains("local cover exceeded")
                || error.contains("embedded cover exceeded")
                || error.contains("no pictures")
                || error.contains("no tag found")))
        || (is_external_cover && external_metadata::is_expected_lookup_miss(error))
}

#[cfg(test)]
mod tests {
    use super::{
        CoverRequestOutcome, clear_cover_in_flight, cover_error_is_terminal,
        cover_error_is_transient, emit_cover_outcome, mark_cover_in_flight,
        unmark_cover_in_flight_generation,
    };
    use crate::controller::ControllerEvent;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    #[test]
    fn cover_exclude_failure() {
        assert!(cover_error_is_terminal(
            "jellyfin",
            true,
            "404 Not Found: did not return album art"
        ));
        assert!(!cover_error_is_terminal(
            "jellyfin",
            true,
            "error sending request for url"
        ));
        assert!(!cover_error_is_terminal(
            "jellyfin",
            true,
            "provider network failed: timed out"
        ));
    }

    #[test]
    fn cover_local_unavailable() {
        assert!(cover_error_is_terminal(
            source_local::LOCAL_PROVIDER_ID,
            false,
            "No such file or directory (os error 2)"
        ));
    }

    #[test]
    fn cover_transient_error() {
        assert!(cover_error_is_transient(
            "sqlite failed: database is locked"
        ));
        assert!(cover_error_is_transient(
            "sqlite failed: database table is locked"
        ));
        assert!(!cover_error_is_transient("cover response was empty"));
    }

    #[test]
    fn cover_emit_deferred() {
        let (events, receiver) = channel();
        let key = "server::cover::256".to_string();

        emit_cover_outcome(
            &events,
            &AtomicU64::new(0),
            key.clone(),
            Ok(CoverRequestOutcome::Deferred),
        );

        assert!(matches!(
            receiver.recv().expect("cover event"),
            ControllerEvent::CoverDeferred { key: event_key } if event_key == key
        ));
    }

    #[test]
    fn cover_keep_generation() {
        let in_flight = Arc::new(Mutex::new(HashMap::new()));
        let key = "server::cover::256";

        assert!(mark_cover_in_flight(&in_flight, key, 1));
        assert!(mark_cover_in_flight(&in_flight, key, 2));
        assert!(!mark_cover_in_flight(&in_flight, key, 1));
        clear_cover_in_flight(&in_flight);
        assert!(mark_cover_in_flight(&in_flight, key, 1));
        clear_cover_in_flight(&in_flight);
        assert!(mark_cover_in_flight(&in_flight, key, 2));

        unmark_cover_in_flight_generation(&in_flight, key, 1);
        assert_eq!(
            in_flight
                .lock()
                .expect("cover in-flight lock")
                .get(key)
                .copied(),
            Some(2)
        );

        unmark_cover_in_flight_generation(&in_flight, key, 2);
        assert!(in_flight.lock().expect("cover in-flight lock").is_empty());
    }
}
