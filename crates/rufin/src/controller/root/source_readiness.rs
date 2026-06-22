use super::*;

pub(in crate::controller) fn active_server_needs_sync(
    store: &StoreHandle,
    server_id: &ServerId,
) -> bool {
    active_source_readiness_inner(store, server_id, false)
        .map(|readiness| readiness.sync_required_reason.is_some())
        .unwrap_or(true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) enum SyncRequiredReason {
    EmptyCache,
    PreviousSyncError,
    RemoteCacheStale,
    LocalManifestRefresh,
    LocalArtworkMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) struct SourceSyncReadiness {
    pub(in crate::controller) metadata_fresh: bool,
    pub(in crate::controller) artwork_fresh: bool,
    pub(in crate::controller) sync_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) prefetch_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) startup_delay_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) struct SourceSyncReadinessInput<'a> {
    pub(in crate::controller) provider: &'a str,
    pub(in crate::controller) cached_item_count: usize,
    pub(in crate::controller) sync_status: Option<&'a str>,
    pub(in crate::controller) sync_completed_age_seconds: Option<i64>,
    pub(in crate::controller) local_library_configured: bool,
    pub(in crate::controller) local_artwork_missing: bool,
}

pub(in crate::controller) fn source_sync_readiness(
    input: SourceSyncReadinessInput<'_>,
) -> SourceSyncReadiness {
    let stale = input
        .sync_completed_age_seconds
        .is_none_or(|age| age >= STARTUP_CACHE_STALE_SECONDS);
    let sync_required_reason = if input.sync_status == Some("error") {
        Some(SyncRequiredReason::PreviousSyncError)
    } else if input.cached_item_count == 0 && input.sync_completed_age_seconds.is_none() {
        Some(SyncRequiredReason::EmptyCache)
    } else if input.provider == LOCAL_PROVIDER_ID
        && input.local_library_configured
        && (input.sync_status == Some("running") || input.cached_item_count == 0 || stale)
    {
        Some(SyncRequiredReason::LocalManifestRefresh)
    } else if stale && input.provider != LOCAL_PROVIDER_ID {
        Some(SyncRequiredReason::RemoteCacheStale)
    } else {
        None
    };
    let prefetch_required_reason = (input.provider == LOCAL_PROVIDER_ID
        && input.cached_item_count > 0
        && input.local_artwork_missing)
        .then_some(SyncRequiredReason::LocalArtworkMissing);
    let startup_delay_ms = match sync_required_reason {
        Some(SyncRequiredReason::EmptyCache) => Some(500),
        Some(_) => Some(8_000),
        None => None,
    };
    SourceSyncReadiness {
        metadata_fresh: !matches!(
            sync_required_reason,
            Some(
                SyncRequiredReason::EmptyCache
                    | SyncRequiredReason::PreviousSyncError
                    | SyncRequiredReason::RemoteCacheStale
            )
        ),
        artwork_fresh: prefetch_required_reason.is_none(),
        sync_required_reason,
        prefetch_required_reason,
        startup_delay_ms,
    }
}

#[cfg(test)]
pub(in crate::controller) fn active_source_readiness(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, server_id, true)
}

pub(in crate::controller) fn active_source_startup_readiness(
    store: &StoreHandle,
    server_id: &ServerId,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, server_id, true)
}

fn active_source_readiness_inner(
    store: &StoreHandle,
    server_id: &ServerId,
    include_local_artwork: bool,
) -> Result<SourceSyncReadiness, String> {
    let local_library_configured = server_id.as_str() == LOCAL_SOURCE_SERVER_ID
        && !load_settings_from_store(store)
            .sources
            .local_folders
            .is_empty();
    let (provider, cached_item_count, sync_status, sync_completed_age_seconds) =
        store.with_store(|store| {
            let provider = store
                .list_servers()?
                .into_iter()
                .find(|saved| saved.server.id == *server_id)
                .map(|saved| saved.server.provider)
                .unwrap_or_else(|| {
                    if server_id.as_str() == LOCAL_SOURCE_SERVER_ID {
                        LOCAL_PROVIDER_ID.to_string()
                    } else {
                        String::new()
                    }
                });
            let albums = store.load_albums(server_id, 0, 1)?.total;
            let tracks = store.load_tracks(server_id, 0, 1)?.total;
            let sync_status = store.sync_state(server_id).ok().map(|state| state.status);
            let sync_completed_age_seconds = store.sync_completed_age_seconds(server_id)?;
            Ok((
                provider,
                albums.saturating_add(tracks),
                sync_status,
                sync_completed_age_seconds,
            ))
        })?;
    let local_artwork_missing = include_local_artwork
        && provider == LOCAL_PROVIDER_ID
        && local_cover_cache_missing(store, server_id, false);
    Ok(source_sync_readiness(SourceSyncReadinessInput {
        provider: &provider,
        cached_item_count,
        sync_status: sync_status.as_deref(),
        sync_completed_age_seconds,
        local_library_configured,
        local_artwork_missing,
    }))
}
