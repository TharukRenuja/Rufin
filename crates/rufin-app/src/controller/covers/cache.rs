use std::path::PathBuf;

use rufin_core::ImageRef;
use rufin_store::{SavedServer, image_cache_key};

use crate::controller::{IMAGE_TAG_UNTAGGED, StoreHandle, cover_cache_path_for_key};
use crate::external_metadata;

use super::{EXTERNAL_DETAIL_COVER_SIZE, EXTERNAL_PREFETCH_COVER_SIZE, EXTERNAL_THUMB_COVER_SIZE};

pub(super) fn cached_cover_path_for_saved(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = saved_cover_path(store, saved, image_ref, size)? {
        return Ok(Some(path));
    }
    for candidate_size in cover_cache_size_candidates(size) {
        if candidate_size == size {
            continue;
        }
        if let Some(path) = saved_cover_path(store, saved, image_ref, candidate_size)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn saved_cover_path(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<Option<PathBuf>, String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    let key = image_cache_key(&saved.server.id, &image_ref.item_id, tag, size);
    if external_metadata::is_external_image_ref(image_ref)
        && let Some(path) = store.with_store(|store| {
            store.load_external_cover_content_path(&image_ref.item_id, tag, size)
        })?
    {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(Some(path));
        }
    }
    let mut entry = store.with_store(|store| {
        store.load_cover_cache_entry(&saved.server.id, &image_ref.item_id, tag, size)
    })?;
    if entry.is_none() && external_metadata::is_external_image_ref(image_ref) {
        entry = store.with_store(|store| {
            store.load_external_cover_cache_entry_by_content(&image_ref.item_id, tag, size)
        })?;
    }
    let Some(entry) = entry else {
        return Ok(cached_cover_path_for_key(&key));
    };
    let path = PathBuf::from(entry.path);
    if path.exists() {
        return Ok(Some(path));
    }
    if let Some(path) = cached_cover_path_for_key(&key) {
        return Ok(Some(path));
    }
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
        vec![EXTERNAL_DETAIL_COVER_SIZE]
    }
}

pub(super) fn external_lookup_miss_size_candidates(size: u32) -> Vec<u32> {
    let mut sizes = vec![size];
    for candidate_size in [
        EXTERNAL_THUMB_COVER_SIZE,
        EXTERNAL_PREFETCH_COVER_SIZE,
        EXTERNAL_DETAIL_COVER_SIZE,
    ] {
        if !sizes.contains(&candidate_size) {
            sizes.push(candidate_size);
        }
    }
    sizes
}

pub(super) fn external_lookup_miss_cached(
    store: &StoreHandle,
    saved: &SavedServer,
    image_ref: &ImageRef,
    size: u32,
) -> Result<bool, String> {
    let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
    if external_metadata::is_external_image_ref(image_ref)
        && store.with_store(|store| {
            store.load_external_cover_content_miss(&image_ref.item_id, tag, size)
        })?
    {
        return Ok(true);
    }
    if store.with_store(|store| {
        store.load_external_image_lookup_miss(&saved.server.id, &image_ref.item_id, tag, size)
    })? {
        return Ok(true);
    }
    if !external_metadata::is_external_image_ref(image_ref) {
        return Ok(false);
    }
    store.with_store(|store| {
        store.load_external_image_lookup_miss_by_content(&image_ref.item_id, tag, size)
    })
}

pub(super) fn save_external_lookup_miss(
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
    })?;
    if external_metadata::is_external_image_ref(image_ref) {
        store.with_store(|store| {
            store.save_external_cover_content_miss(&image_ref.item_id, tag, size, reason)
        })?;
    }
    Ok(())
}

pub(super) fn cached_cover_path_for_key(key: &str) -> Option<PathBuf> {
    let path = cover_cache_path_for_key(key)?;
    path.exists().then_some(path)
}
