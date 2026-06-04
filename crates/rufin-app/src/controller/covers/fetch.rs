use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use gdk_pixbuf::{InterpType, Pixbuf};
use rufin_core::ImageRef;
use rufin_provider::{ImageKind, ImageRequest, MusicProvider};
use rufin_provider_local::{LOCAL_PROVIDER_ID, LocalProvider};
use rufin_secrets::SecretStore;
use rufin_store::{CoverCacheEntry, SavedServer, image_cache_key};
use tokio::runtime::Runtime;

use crate::controller::{
    IMAGE_TAG_UNTAGGED, StoreHandle, cover_cache_path_for_key, load_settings_from_store,
    local_folder_paths, provider_for_saved,
};
use crate::external_metadata;

pub(super) fn fetch_and_cache_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    secrets: &Arc<dyn SecretStore>,
    saved: &SavedServer,
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
    if saved.server.provider == LOCAL_PROVIDER_ID && image_ref.item_id.starts_with("local:cover:") {
        let settings = load_settings_from_store(store);
        let image = LocalProvider::image_bytes_for_cover_item_id(
            &image_ref.item_id,
            local_folder_paths(&settings),
        )
        .map_err(|error| error.to_string())?;
        if image.bytes.is_empty() {
            return Err("cover response was empty".to_string());
        }
        return save_cover_bytes(store, saved, image_ref, size, image.bytes);
    }
    let provider = provider_for_saved(store, runtime, secrets, saved)?;
    fetch_and_cache_provider_cover(
        store,
        runtime,
        saved,
        provider.as_music_provider(),
        image_ref,
        size,
    )
}

pub(super) fn fetch_and_cache_provider_cover(
    store: &StoreHandle,
    runtime: &Runtime,
    saved: &SavedServer,
    provider: &dyn MusicProvider,
    image_ref: ImageRef,
    size: u32,
) -> Result<PathBuf, String> {
    let image = runtime
        .block_on(provider.image_bytes(ImageRequest {
            item_id: image_ref.item_id.clone(),
            kind: ImageKind::Primary,
            tag: image_ref.tag.clone(),
            size,
        }))
        .map_err(|error| error.to_string())?;
    if image.bytes.is_empty() {
        return Err("cover response was empty".to_string());
    }
    save_cover_bytes(store, saved, image_ref, size, image.bytes)
}

fn save_cover_bytes(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: ImageRef,
    size: u32,
    bytes: Vec<u8>,
) -> Result<PathBuf, String> {
    let tag = image_ref
        .tag
        .clone()
        .unwrap_or_else(|| IMAGE_TAG_UNTAGGED.to_string());
    let key = image_cache_key(&saved.server.id, &image_ref.item_id, &tag, size);
    let path = cover_cache_path_for_key(&key)
        .ok_or_else(|| "cache directory is unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, normalize_cover_bytes(bytes, size)).map_err(|error| error.to_string())?;
    fs::rename(&temp_path, &path).map_err(|error| error.to_string())?;

    store.with_store(|store| {
        store.save_cover_cache_entry(&CoverCacheEntry {
            server_id: saved.server.id.clone(),
            item_id: image_ref.item_id,
            image_tag: tag,
            size,
            path: path.to_string_lossy().to_string(),
        })
    })?;

    Ok(path)
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

pub(in crate::controller) fn is_provider_not_found_error(error: &str) -> bool {
    error == "provider item was not found"
}

#[cfg(test)]
mod tests {
    use gdk_pixbuf::{Colorspace, Pixbuf};

    use super::normalize_cover_bytes;

    #[test]
    fn normalize_cover_bytes_downscales_large_cache_images() {
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
}
