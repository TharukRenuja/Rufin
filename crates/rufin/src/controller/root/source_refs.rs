use super::*;

const GROUPED_COVER_REF_LIMIT: usize = 4;

pub(crate) fn grouped_cover_refs_for_items(albums: &[Album], tracks: &[Track]) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    for album in albums {
        push_unique_cover_ref(&mut image_refs, album.image_ref.as_ref());
    }
    for track in tracks {
        push_unique_cover_ref(&mut image_refs, track.image_ref.as_ref());
    }
    image_refs
}

pub(crate) fn track_cover_refs_for_items(tracks: &[Track]) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    for track in tracks {
        push_unique_cover_ref(&mut image_refs, track.image_ref.as_ref());
    }
    image_refs
}

pub(in crate::controller) fn track_album_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    tracks: &mut [Track],
    albums: &[Album],
) -> Result<(), String> {
    if tracks.is_empty() {
        return Ok(());
    }
    let settings = load_settings_for_saved(store, saved);
    track_album_refs_with_settings(store, saved, &settings, tracks, albums)
}

pub(in crate::controller) fn track_album_refs_with_settings(
    store: &StoreHandle,
    saved: &SavedServer,
    settings: &AppSettings,
    tracks: &mut [Track],
    albums: &[Album],
) -> Result<(), String> {
    if tracks.is_empty() {
        return Ok(());
    }
    let mut image_refs = albums
        .iter()
        .filter_map(|album| {
            let mut image_ref = album.image_ref.clone();
            scrub_source_image_ref(saved, &mut image_ref);
            image_ref.map(|image_ref| (album.id.clone(), image_ref))
        })
        .collect::<HashMap<_, _>>();
    let missing_album_ids = tracks
        .iter()
        .map(|track| track.album_id.clone())
        .filter(|album_id| !image_refs.contains_key(album_id))
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if !missing_album_ids.is_empty() {
        let mut loaded = store.with_store_fast(|store| {
            store.load_album_image_refs(&saved.server.id, &missing_album_ids)
        })?;
        loaded.retain(|_, image_ref| source_image_ref_allowed(saved, image_ref));
        image_refs.extend(loaded);
    }
    let missing_album_ids = tracks
        .iter()
        .map(|track| track.album_id.clone())
        .filter(|album_id| !image_refs.contains_key(album_id))
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if !missing_album_ids.is_empty() {
        let mut loaded = store.with_store_fast(|store| {
            store.load_albums_by_ids(&saved.server.id, &missing_album_ids)
        })?;
        for album in &mut loaded {
            scrub_source_image_ref(saved, &mut album.image_ref);
            cover_art_policy::bind_album(album, settings);
            if let Some(image_ref) = album.image_ref.clone() {
                image_refs.insert(album.id.clone(), image_ref);
            }
        }
    }
    for track in tracks {
        if let Some(image_ref) = image_refs.get(&track.album_id) {
            cover_art_policy::bind_track_with_album_ref(track, Some(image_ref), settings);
        }
    }
    Ok(())
}

pub(in crate::controller) fn album_track_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    albums: &mut [Album],
) -> Result<(), String> {
    if saved.server.provider != LOCAL_PROVIDER_ID || albums.is_empty() {
        return Ok(());
    }
    let album_ids = albums.iter().map(|album| album.id.clone()).fold(
        Vec::<AlbumId>::new(),
        |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        },
    );
    if album_ids.is_empty() {
        return Ok(());
    }
    let mut image_refs =
        store.with_store(|store| store.load_album_image_refs(&saved.server.id, &album_ids))?;
    image_refs.retain(|_, image_ref| source_image_ref_allowed(saved, image_ref));
    for album in albums {
        if let Some(image_ref) = image_refs.get(&album.id) {
            album.image_ref = Some(image_ref.clone());
        }
    }
    Ok(())
}

pub(in crate::controller) fn home_image_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    section: &mut HomeSection,
) -> Result<(), String> {
    let metadata_settings = load_settings_for_saved(store, saved);
    scrub_home_refs(saved, section);
    cover_art_policy::bind_home_section(section, &metadata_settings);
    scrub_home_refs(saved, section);
    home_local_refs(store, saved, section)
}

pub(in crate::controller) fn home_local_refs(
    store: &StoreHandle,
    saved: &SavedServer,
    section: &mut HomeSection,
) -> Result<(), String> {
    album_track_refs(store, saved, &mut section.albums)?;
    let albums = section.albums.clone();
    track_album_refs(store, saved, &mut section.tracks, &albums)
}

pub(in crate::controller) fn queue_album_refs(
    store: &StoreHandle,
    server: &ServerIdentity,
    settings: &AppSettings,
    entries: &mut [QueueEntry],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let missing_album_ids = entries
        .iter()
        .filter_map(|entry| entry.album_id.clone())
        .fold(Vec::<AlbumId>::new(), |mut ids, album_id| {
            if !ids.iter().any(|existing| existing == &album_id) {
                ids.push(album_id);
            }
            ids
        });
    if missing_album_ids.is_empty() {
        return Ok(());
    }
    let mut image_refs =
        store.with_store(|store| store.load_album_image_refs(&server.id, &missing_album_ids))?;
    image_refs.retain(|_, image_ref| image_ref_allowed(server, image_ref));
    for entry in entries {
        let Some(album_id) = &entry.album_id else {
            continue;
        };
        if let Some(image_ref) = image_refs.get(album_id) {
            cover_art_policy::bind_queue_entry_with_album_ref(entry, Some(image_ref), settings);
        }
    }
    Ok(())
}

pub(in crate::controller) fn push_unique_cover_ref(
    image_refs: &mut Vec<ImageRef>,
    image_ref: Option<&ImageRef>,
) {
    if image_refs.len() >= GROUPED_COVER_REF_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    if !image_refs.iter().any(|existing| existing == image_ref) {
        image_refs.push(image_ref.clone());
    }
}

pub(in crate::controller) fn sync_status_text(state: &SyncState) -> String {
    match state.status.as_str() {
        "running" => "Syncing library…".to_string(),
        "error" => "Sync needs attention".to_string(),
        _ => "Cached library ready".to_string(),
    }
}
