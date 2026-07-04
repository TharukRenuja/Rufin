use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use domain::ImageRef;
use gdk_pixbuf::{InterpType, Pixbuf};
use library::{CoverCacheEntry, SavedSource, Store, StoreResult, image_cache_key};
use secrets::SecretStore;
use source::{ImageKind, ImageRequest, MusicSource};
use source_local::{LOCAL_SOURCE_ID, LocalSource};
use tokio::runtime::Runtime;

use crate::controller::{
    IMAGE_TAG_UNTAGGED, StoreHandle, cover_cache_path_for_key, load_settings_from_store,
    local_folder_paths, source_for_saved,
};
use crate::external_metadata;

use super::cover_error_is_transient;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CoverFetchTiming {
    pub(super) bytes: usize,
    pub(super) fetch_ms: u64,
    pub(super) normalize_ms: u64,
    pub(super) write_ms: u64,
    pub(super) store_ms: u64,
    pub(super) total_ms: u64,
}

pub(super) struct CoverFetchResult {
    pub(super) path: PathBuf,
    pub(super) timing: CoverFetchTiming,
}

pub(super) fn fetch_and_cache_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedSource,
    image_ref: ImageRef,
    size: u32,
) -> Result<PathBuf, String> {
    if let Some(art) = external_metadata::album_art_from_image_ref(&image_ref) {
        let settings = load_settings_from_store(store);
        if !external_metadata::enabled(&settings) {
            return Err("external metadata lookup is disabled".to_string());
        }
        let bytes =
            external_metadata::fetch_album_cover(&art, size, settings.lastfm_api_key.trim())?;
        return save_cover_bytes(store, saved, image_ref, size, bytes);
    }
    if saved.source.kind == LOCAL_SOURCE_ID && image_ref.item_id.starts_with("local:cover:") {
        let settings = load_settings_from_store(store);
        let image =
            LocalSource::cover_item_bytes(&image_ref.item_id, local_folder_paths(&settings))
                .map_err(|error| error.to_string())?;
        if image.bytes.is_empty() {
            return Err("cover response was empty".to_string());
        }
        return save_cover_bytes(store, saved, image_ref, size, image.bytes);
    }
    let provider = source_for_saved(store, runtime, secrets, saved)?;
    fetch_and_cache_source_cover(
        store,
        runtime,
        saved,
        provider.as_music_source(),
        image_ref,
        size,
    )
}

pub(super) fn fetch_and_cache_source_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    saved: &SavedSource,
    source: &dyn MusicSource,
    image_ref: ImageRef,
    size: u32,
) -> Result<PathBuf, String> {
    fetch_and_cache_source_cover_timed(store, runtime, saved, source, image_ref, size)
        .map(|result| result.path)
}

pub(super) fn fetch_and_cache_source_cover_timed(
    store: &StoreHandle,
    runtime: &Runtime,
    saved: &SavedSource,
    source: &dyn MusicSource,
    image_ref: ImageRef,
    size: u32,
) -> Result<CoverFetchResult, String> {
    let total_started = Instant::now();
    let (image_bytes, mut timing) = fetch_provider_cover_image(runtime, source, &image_ref, size)?;
    let path = save_cover_bytes_timed(store, saved, image_ref, size, image_bytes, &mut timing)?;
    timing.total_ms = elapsed_ms(total_started);
    Ok(CoverFetchResult { path, timing })
}

pub(super) fn fetch_and_cache_source_cover_timed_with_store(
    cache_store: &Store,
    runtime: &Runtime,
    saved: &SavedSource,
    source: &dyn MusicSource,
    image_ref: ImageRef,
    size: u32,
) -> Result<CoverFetchResult, String> {
    let total_started = Instant::now();
    let (image_bytes, mut timing) = fetch_provider_cover_image(runtime, source, &image_ref, size)?;
    let path = save_cover_bytes_timed_with_store(
        cache_store,
        saved,
        image_ref,
        size,
        image_bytes,
        &mut timing,
    )?;
    timing.total_ms = elapsed_ms(total_started);
    Ok(CoverFetchResult { path, timing })
}

fn fetch_provider_cover_image(
    runtime: &Runtime,
    source: &dyn MusicSource,
    image_ref: &ImageRef,
    size: u32,
) -> Result<(Vec<u8>, CoverFetchTiming), String> {
    let fetch_started = Instant::now();
    let image = runtime
        .block_on(source.image_bytes(ImageRequest {
            item_id: image_ref.item_id.clone(),
            kind: provider_image_kind(image_ref),
            tag: image_ref.tag.clone(),
            size,
        }))
        .map_err(|error| error.to_string())?;
    let timing = CoverFetchTiming {
        bytes: image.bytes.len(),
        fetch_ms: elapsed_ms(fetch_started),
        ..CoverFetchTiming::default()
    };
    if image.bytes.is_empty() {
        return Err("cover response was empty".to_string());
    }
    Ok((image.bytes, timing))
}

fn save_cover_bytes(
    store: &StoreHandle,
    saved: &SavedSource,
    image_ref: ImageRef,
    size: u32,
    bytes: Vec<u8>,
) -> Result<PathBuf, String> {
    save_cover_bytes_timed(
        store,
        saved,
        image_ref,
        size,
        bytes,
        &mut CoverFetchTiming::default(),
    )
}

fn save_cover_bytes_timed(
    store: &StoreHandle,
    saved: &SavedSource,
    image_ref: ImageRef,
    size: u32,
    bytes: Vec<u8>,
    timing: &mut CoverFetchTiming,
) -> Result<PathBuf, String> {
    let (path, entry) = write_cover_cache_file(saved, image_ref.clone(), size, bytes, timing)?;
    let store_started = Instant::now();
    let saved_entry =
        store.with_store(|store| save_cover_cache_entry_to_store(store, &entry, &image_ref));
    timing.store_ms = elapsed_ms(store_started);
    if let Err(error) = saved_entry {
        if cover_error_is_transient(&error) {
            return Ok(path);
        }
        return Err(error);
    }

    Ok(path)
}

fn save_cover_bytes_timed_with_store(
    cache_store: &Store,
    saved: &SavedSource,
    image_ref: ImageRef,
    size: u32,
    bytes: Vec<u8>,
    timing: &mut CoverFetchTiming,
) -> Result<PathBuf, String> {
    let (path, entry) = write_cover_cache_file(saved, image_ref.clone(), size, bytes, timing)?;
    let store_started = Instant::now();
    let saved_entry = save_cover_cache_entry_to_store(cache_store, &entry, &image_ref);
    timing.store_ms = elapsed_ms(store_started);
    if let Err(error) = saved_entry {
        let error = error.to_string();
        if cover_error_is_transient(&error) {
            return Ok(path);
        }
        return Err(error);
    }

    Ok(path)
}

fn write_cover_cache_file(
    saved: &SavedSource,
    image_ref: ImageRef,
    size: u32,
    bytes: Vec<u8>,
    timing: &mut CoverFetchTiming,
) -> Result<(PathBuf, CoverCacheEntry), String> {
    let tag = image_ref
        .tag
        .clone()
        .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
    let key = image_cache_key(&saved.source.id, &image_ref.item_id, &tag, size);
    let path = cover_cache_path_for_key(&key)
        .ok_or_else(|| "cache directory is unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension("tmp");
    let normalize_started = Instant::now();
    let bytes = normalize_cover_bytes(bytes, size);
    timing.normalize_ms = elapsed_ms(normalize_started);
    let write_started = Instant::now();
    fs::write(&temp_path, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, &path).map_err(|error| error.to_string())?;
    timing.write_ms = elapsed_ms(write_started);
    let item_id = image_ref.item_id.clone();
    let path_string = path.to_string_lossy().to_string();

    Ok((
        path,
        CoverCacheEntry {
            source_id: saved.source.id.clone(),
            item_id,
            image_tag: tag,
            size,
            path: path_string,
        },
    ))
}

fn save_cover_cache_entry_to_store(
    store: &Store,
    entry: &CoverCacheEntry,
    image_ref: &ImageRef,
) -> StoreResult<()> {
    store.save_cover_cache_entry(entry)?;
    if external_metadata::is_external_image_ref(image_ref) {
        store.save_external_cover_content_path(
            &entry.item_id,
            &entry.image_tag,
            entry.size,
            &entry.path,
        )?;
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn normalize_cover_bytes(bytes: Vec<u8>, size: u32) -> Vec<u8> {
    let max_size = size.min(i32::MAX as u32) as i32;
    if max_size <= 0 {
        return bytes;
    }

    let Ok(pixbuf) = Pixbuf::from_read(Cursor::new(bytes.clone())) else {
        return bytes;
    };
    let width = pixbuf.width().max(1);
    let height = pixbuf.height().max(1);
    let max_dimension = width.max(height);
    if max_dimension <= max_size {
        return bytes;
    }

    let target_width = ((i64::from(width) * i64::from(max_size)) / i64::from(max_dimension))
        .max(1)
        .min(i64::from(i32::MAX)) as i32;
    let target_height = ((i64::from(height) * i64::from(max_size)) / i64::from(max_dimension))
        .max(1)
        .min(i64::from(i32::MAX)) as i32;
    let Some(scaled) = pixbuf.scale_simple(target_width, target_height, InterpType::Bilinear)
    else {
        return bytes;
    };

    scaled
        .save_to_bufferv("jpeg", &[("quality", "90")])
        .or_else(|_| scaled.save_to_bufferv("png", &[]))
        .unwrap_or(bytes)
}

fn provider_image_kind(image_ref: &ImageRef) -> ImageKind {
    if image_ref.item_id.starts_with("jellyfin:backdrop:") {
        ImageKind::Backdrop
    } else {
        ImageKind::Primary
    }
}

pub(in crate::controller) fn is_source_not_found_error(error: &str) -> bool {
    error == "source item was not found"
}

#[cfg(test)]
mod tests {
    use gdk_pixbuf::{Colorspace, Pixbuf};

    use super::{normalize_cover_bytes, provider_image_kind};
    use domain::ImageRef;
    use source::ImageKind;

    #[test]
    fn fetch_normalize_image() {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, 512, 384).expect("pixbuf");
        pixbuf.fill(0x336699ff);
        let bytes = pixbuf.save_to_bufferv("png", &[]).expect("png bytes");

        let normalized = normalize_cover_bytes(bytes, 96);
        let normalized_pixbuf =
            Pixbuf::from_read(std::io::Cursor::new(normalized)).expect("normalized pixbuf");

        assert_eq!(
            normalized_pixbuf.width().max(normalized_pixbuf.height()),
            96
        );
    }

    #[test]
    fn provider_image_kind_uses_jellyfin_backdrop_refs() {
        let image_ref = ImageRef::new("jellyfin:backdrop:item-one", Some("tag-one".to_string()));

        assert_eq!(provider_image_kind(&image_ref), ImageKind::Backdrop);
    }

    #[test]
    fn provider_image_kind_keeps_primary_default() {
        let image_ref = ImageRef::new("jellyfin:album:item-one", Some("tag-one".to_string()));

        assert_eq!(provider_image_kind(&image_ref), ImageKind::Primary);
    }
}
