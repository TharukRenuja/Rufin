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
use tracing::{debug, warn};

use crate::external_metadata;

use super::{
    AppController, ControllerEvent, IMAGE_TAG_UNTAGGED, StoreHandle, acquire_cover_slot, cache_dir,
    load_settings_from_store, provider_for_saved, release_cover_slot,
};

const EXTERNAL_PREFETCH_PAGE_SIZE: usize = 500;
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
        let result = prefetch_synced_external_images(
            &store,
            &runtime,
            &secrets,
            &events,
            &cover_in_flight,
            &cover_slots,
            &saved,
        );
        if let Err(error) = result {
            warn!(%error, "failed to prefetch synced external images");
        }
        if let Ok(mut running) = external_cover_prefetch_in_flight.lock() {
            running.remove(&server_id);
        }
    });
}

fn prefetch_synced_external_images(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
) -> Result<(), String> {
    prefetch_synced_album_covers(
        store,
        runtime,
        secrets,
        events,
        cover_in_flight,
        cover_slots,
        saved,
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
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings) || active_server_changed(store, saved)? {
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
        for image_ref in external_album_image_refs_from_albums(albums, &settings) {
            if !external_metadata::enabled(&load_settings_from_store(store))
                || active_server_changed(store, saved)?
            {
                return Ok(());
            }
            if prefetch_external_image(
                store,
                runtime,
                secrets,
                events,
                cover_in_flight,
                cover_slots,
                saved,
                image_ref,
            )? {
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
) -> Result<(), String> {
    let mut offset = 0;
    loop {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings)
            || settings.lastfm_api_key.trim().is_empty()
            || active_server_changed(store, saved)?
        {
            return Ok(());
        }
        let artists = store.with_store(|store| {
            store.load_artists_without_image_ref(
                &saved.server.id,
                album_artist,
                offset,
                EXTERNAL_PREFETCH_PAGE_SIZE,
            )
        })?;
        if artists.is_empty() {
            return Ok(());
        }
        let artist_count = artists.len();
        for image_ref in external_artist_image_refs_from_artists(artists, &settings) {
            let settings = load_settings_from_store(store);
            if !external_metadata::enabled(&settings)
                || settings.lastfm_api_key.trim().is_empty()
                || active_server_changed(store, saved)?
            {
                return Ok(());
            }
            if prefetch_external_image(
                store,
                runtime,
                secrets,
                events,
                cover_in_flight,
                cover_slots,
                saved,
                image_ref,
            )? {
                thread::sleep(EXTERNAL_PREFETCH_DELAY);
            }
        }
        offset += artist_count;
    }
}

fn prefetch_external_image(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    events: &Sender<ControllerEvent>,
    cover_in_flight: &Arc<Mutex<HashSet<String>>>,
    cover_slots: &Arc<(Mutex<usize>, Condvar)>,
    saved: &SavedServer,
    image_ref: ImageRef,
) -> Result<bool, String> {
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
        return Ok(false);
    }
    match cover_in_flight.lock() {
        Ok(mut in_flight) => {
            if !in_flight.insert(key.clone()) {
                return Ok(false);
            }
        }
        Err(_) => return Ok(false),
    }

    if !acquire_cover_slot(cover_slots) {
        if let Ok(mut in_flight) = cover_in_flight.lock() {
            in_flight.remove(&key);
        }
        return Ok(false);
    }
    let result = fetch_and_cache_cover(
        store,
        runtime,
        secrets,
        saved,
        image_ref,
        EXTERNAL_PREFETCH_COVER_SIZE,
    );
    release_cover_slot(cover_slots);
    if let Ok(mut in_flight) = cover_in_flight.lock() {
        in_flight.remove(&key);
    }

    match result {
        Ok(path) => {
            let _sent = events.send(ControllerEvent::CoverReady { key, path });
        }
        Err(error) => {
            if external_metadata::is_expected_lookup_miss(&error) {
                debug!(%error, "synced external album cover was not available");
            } else {
                warn!(%error, "failed to prefetch synced external image");
            }
        }
    }
    Ok(true)
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

fn external_artist_image_refs_from_artists(
    mut artists: Vec<Artist>,
    settings: &AppSettings,
) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    let mut seen = HashSet::new();
    external_metadata::normalize_artists(&mut artists, settings);
    for image_ref in artists
        .iter()
        .filter_map(|artist| artist.image_ref.as_ref())
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
    } else if let Some(image) = external_metadata::artist_image_from_image_ref(&image_ref) {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings) {
            return Err("external metadata lookup is disabled".to_string());
        }
        external_metadata::fetch_artist_image(&image, settings.lastfm_api_key.trim())?
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

#[cfg(test)]
mod tests {
    use rufin_core::{Album, AlbumId, AppSettings, Artist, ArtistId, ImageRef};

    use super::{external_album_image_refs_from_albums, external_artist_image_refs_from_artists};
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
    fn synced_external_cover_candidates_include_artists_when_lastfm_is_configured() {
        let settings = AppSettings {
            external_metadata_enabled: true,
            lastfm_api_key: "key".to_string(),
            ..AppSettings::default()
        };
        let refs = external_artist_image_refs_from_artists(
            vec![
                artist_without_cover(1, "Slowdive"),
                artist_with_cover(2, "Ride"),
                artist_without_cover(3, "Slowdive"),
            ],
            &settings,
        );

        assert_eq!(refs.len(), 1);
        assert!(external_metadata::is_external_image_ref(&refs[0]));
        assert_eq!(
            external_metadata::artist_image_from_image_ref(&refs[0]).map(|image| image.artist),
            Some("Slowdive".to_string())
        );
    }

    #[test]
    fn synced_external_artist_candidates_require_lastfm_key() {
        assert!(
            external_artist_image_refs_from_artists(
                vec![artist_without_cover(1, "Slowdive")],
                &AppSettings {
                    external_metadata_enabled: true,
                    ..AppSettings::default()
                },
            )
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
