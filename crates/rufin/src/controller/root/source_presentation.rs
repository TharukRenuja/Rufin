use super::*;
use crate::source_setup::{
    configured_source_identity, configured_source_needs_auth, configured_source_selection,
    configured_source_supported, local_configured_source_for_store,
};

#[derive(Clone, Debug)]
struct SourcePresentationResolution {
    selected_source: LibrarySourceSelection,
    saved: StoredSource,
}

fn presentation_remote_sources(saved_sources: &[StoredSource]) -> Vec<StoredSource> {
    saved_sources
        .iter()
        .filter(|saved| {
            matches!(
                configured_source_selection(saved),
                LibrarySourceSelection::Source(_)
            )
        })
        .cloned()
        .collect()
}

fn source_identity_for_presentation(saved: &StoredSource) -> Result<SourceIdentity, String> {
    if configured_source_supported(&saved.kind) {
        return configured_source_identity(saved);
    }
    Ok(SourceIdentity {
        id: saved.source_id.clone(),
        kind: saved.kind.clone(),
        name: saved.name.clone(),
        base_url: String::new(),
    })
}

fn source_identities_for_presentation(
    remote_saved_sources: &[StoredSource],
) -> Result<Vec<SourceIdentity>, String> {
    remote_saved_sources
        .iter()
        .map(source_identity_for_presentation)
        .collect()
}

fn source_local_access_presentations(
    store: &StoreHandle,
    remote_saved_sources: &[StoredSource],
) -> Result<Vec<SourceLocalAccessPresentation>, String> {
    remote_saved_sources
        .iter()
        .map(|saved| source_local_access_presentation(store, saved))
        .collect()
}

fn source_local_access_presentation(
    store: &StoreHandle,
    saved: &StoredSource,
) -> Result<SourceLocalAccessPresentation, String> {
    let access = store.with_store(|store| store.source_local_access(&saved.source_id))?;
    let status = local_access_status_for_server(store, access.as_ref())?;
    let cached_album_count = store
        .with_store(|store| {
            store
                .load_albums(&saved.source_id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let cached_track_count = store
        .with_store(|store| {
            store
                .load_tracks(&saved.source_id, 0, 1)
                .map(|page| page.total)
        })
        .unwrap_or_default();
    let selected_music_folder_name = store
        .with_store(|store| {
            let selected = store.selected_music_folder_id(&saved.source_id)?;
            let folders = store.list_music_folders(&saved.source_id)?;
            Ok(selected.and_then(|selected| {
                folders
                    .into_iter()
                    .find(|folder| folder.id == selected)
                    .map(|folder| folder.name)
            }))
        })
        .unwrap_or_default();
    Ok(SourceLocalAccessPresentation {
        source_id: saved.source_id.clone(),
        access,
        status,
        selected_music_folder_name,
        cached_album_count,
        cached_track_count,
    })
}

pub(in crate::controller) fn load_source_local_access_presentation(
    store: &StoreHandle,
    source_id: &SourceId,
) -> Result<Option<SourceLocalAccessPresentation>, String> {
    store
        .with_store(|store| store.stored_source(source_id))?
        .as_ref()
        .map(|saved| source_local_access_presentation(store, saved))
        .transpose()
}

fn resolve_presentation_source(
    store: &StoreHandle,
    settings: &StoredSettings,
    saved_sources: &[StoredSource],
    remote_saved_sources: &[StoredSource],
) -> Result<Option<SourcePresentationResolution>, String> {
    let local_source_configured = local_source_configured(store, settings, saved_sources);
    let persisted_active = store.with_store(|store| store.active_source())?;
    let selected_source = resolve_selected_source(
        settings,
        remote_saved_sources,
        persisted_active.clone(),
        local_source_configured,
    );
    let Some(selected_source) = selected_source else {
        return Ok(None);
    };

    let saved = saved_source_for_presentation(
        store,
        remote_saved_sources,
        persisted_active.as_ref(),
        &selected_source,
    )?;

    Ok(Some(SourcePresentationResolution {
        selected_source,
        saved,
    }))
}

fn saved_source_for_presentation(
    store: &StoreHandle,
    remote_saved_sources: &[StoredSource],
    persisted_active: Option<&StoredSource>,
    selected_source: &LibrarySourceSelection,
) -> Result<StoredSource, String> {
    match selected_source {
        LibrarySourceSelection::Local => persisted_active
            .filter(|saved| {
                matches!(
                    configured_source_selection(saved),
                    LibrarySourceSelection::Local
                )
            })
            .cloned()
            .map_or_else(|| local_configured_source_for_store(store), Ok),
        LibrarySourceSelection::Source(source_id) => remote_saved_sources
            .iter()
            .find(|saved| &saved.source_id == source_id)
            .cloned()
            .ok_or_else(|| "The selected source is no longer saved.".to_string()),
    }
}

fn local_source_configured(
    store: &StoreHandle,
    settings: &StoredSettings,
    saved_sources: &[StoredSource],
) -> bool {
    if !settings.sources.local_folders.is_empty() {
        return true;
    }
    saved_sources.iter().any(|saved| {
        matches!(
            configured_source_selection(saved),
            LibrarySourceSelection::Local
        ) && store
            .with_store(|store| {
                let tracks = store.load_tracks(&saved.source_id, 0, 1)?.total;
                let albums = store.load_albums(&saved.source_id, 0, 1)?.total;
                Ok(tracks > 0 || albums > 0)
            })
            .unwrap_or(false)
    })
}

fn resolve_selected_source(
    settings: &StoredSettings,
    remote_saved_sources: &[StoredSource],
    active_source: Option<StoredSource>,
    local_source_configured: bool,
) -> Option<LibrarySourceSelection> {
    match &settings.sources.selected {
        Some(LibrarySourceSelection::Local) if local_source_configured => {
            return Some(LibrarySourceSelection::Local);
        }
        Some(LibrarySourceSelection::Source(source_id))
            if remote_saved_sources
                .iter()
                .any(|saved| saved.source_id == *source_id) =>
        {
            return Some(LibrarySourceSelection::Source(source_id.clone()));
        }
        _ => {}
    }

    if let Some(saved) = active_source {
        match configured_source_selection(&saved) {
            LibrarySourceSelection::Local if local_source_configured => {
                return Some(LibrarySourceSelection::Local);
            }
            LibrarySourceSelection::Source(source_id) => {
                return Some(LibrarySourceSelection::Source(source_id));
            }
            LibrarySourceSelection::Local => {}
        }
    }
    None
}

pub(in crate::controller) fn load_source_presentation(
    store: &StoreHandle,
) -> Result<SourcePresentationState, String> {
    let source_settings = load_settings_from_store(store);
    let saved_sources = store.with_store(|store| store.list_sources())?;
    let remote_saved_sources = presentation_remote_sources(&saved_sources);
    let sources = source_identities_for_presentation(&remote_saved_sources)?;
    let source_local_access = source_local_access_presentations(store, &remote_saved_sources)?;
    let Some(resolved_source) = resolve_presentation_source(
        store,
        &source_settings,
        &saved_sources,
        &remote_saved_sources,
    )?
    else {
        let mut presentation = SourcePresentationState::first_run();
        presentation.sources = sources;
        presentation.local_folders = source_settings.sources.local_folders.clone();
        presentation.source_local_access = source_local_access;
        return Ok(presentation);
    };
    let SourcePresentationResolution {
        selected_source,
        saved,
    } = resolved_source;
    load_active_source_presentation(
        store,
        source_settings,
        sources,
        source_local_access,
        selected_source,
        saved,
    )
}

pub(in crate::controller) fn load_runtime_source_presentation(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
) -> Result<SourcePresentationState, String> {
    let mut presentation = load_source_presentation(store)?;
    if active_source_needs_auth(store, &presentation, secrets)? {
        presentation.first_run = true;
    }
    Ok(presentation)
}

fn active_source_needs_auth(
    store: &StoreHandle,
    presentation: &SourcePresentationState,
    secrets: &Arc<dyn SecretStore>,
) -> Result<bool, String> {
    if matches!(
        presentation.selected_source,
        Some(LibrarySourceSelection::Local)
    ) {
        return Ok(false);
    }
    let Some(server) = presentation.source.as_ref() else {
        return Ok(false);
    };
    let saved = store
        .with_store(|store| store.stored_source(&server.id))?
        .ok_or_else(|| "The selected source is no longer saved.".to_string())?;
    if !configured_source_supported(&saved.kind) {
        return Ok(true);
    }
    configured_source_needs_auth(secrets, &saved)
}

fn load_active_source_presentation(
    store: &StoreHandle,
    source_settings: StoredSettings,
    sources: Vec<SourceIdentity>,
    mut source_local_access: Vec<SourceLocalAccessPresentation>,
    selected_source: LibrarySourceSelection,
    saved: StoredSource,
) -> Result<SourcePresentationState, String> {
    let source = source_identity_for_presentation(&saved)?;
    let (local_access, local_access_status, music_folders, selected_music_folder_id, sync_state) =
        store.with_store_session(|store| {
            store
                .read_snapshot(|store| {
                    let local_access = store.source_local_access(&saved.source_id)?;
                    let local_access_status =
                        local_access_status_from_store(store, local_access.as_ref())?;
                    Ok((
                        local_access,
                        local_access_status,
                        store.list_music_folders(&saved.source_id)?,
                        store.selected_music_folder_id(&saved.source_id)?,
                        store.sync_state(&saved.source_id).ok(),
                    ))
                })
                .map_err(|error| error.to_string())
        })?;
    if let Some(summary) = source_local_access
        .iter_mut()
        .find(|summary| summary.source_id == saved.source_id)
    {
        summary.access = local_access.clone();
        summary.status = local_access_status.clone();
        summary.selected_music_folder_name =
            selected_music_folder_id.as_ref().and_then(|selected| {
                music_folders
                    .iter()
                    .find(|folder| &folder.id == selected)
                    .map(|folder| folder.name.clone())
            });
    }
    let cache = sync_state.map_or(LibraryCacheState::NoCache { revision: 0 }, |state| {
        if state.last_all_completed_at.is_some() {
            LibraryCacheState::Committed {
                revision: state.cache_revision,
            }
        } else {
            LibraryCacheState::NoCache {
                revision: state.cache_revision,
            }
        }
    });

    Ok(SourcePresentationState {
        source: Some(source),
        sources,
        selected_source: Some(selected_source),
        local_folders: source_settings.sources.local_folders,
        source_local_access,
        local_access,
        local_access_status,
        music_folders,
        selected_music_folder_id,
        first_run: false,
        cache,
    })
}
