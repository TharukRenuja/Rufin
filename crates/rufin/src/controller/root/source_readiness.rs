use super::*;

pub(in crate::controller) fn active_source_needs_sync(
    store: &StoreHandle,
    active: &ActiveSource,
) -> bool {
    active_source_readiness_inner(store, active, false)
        .map(|readiness| readiness.sync_required_reason.is_some())
        .unwrap_or(true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::controller) enum SyncRequiredReason {
    EmptyCache,
    PreviousSyncError,
    CacheStale,
    FullRefresh,
    ArtworkMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceSyncReadiness {
    pub(in crate::controller) metadata_fresh: bool,
    pub(in crate::controller) artwork_fresh: bool,
    pub(in crate::controller) sync_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) prefetch_required_reason: Option<SyncRequiredReason>,
    pub(in crate::controller) startup_delay_ms: Option<u64>,
}

fn source_sync_readiness(
    full_ingest: bool,
    incremental: bool,
    configured: bool,
    cached_item_count: usize,
    sync_status: Option<&str>,
    sync_completed_age_seconds: Option<i64>,
    artwork_missing: bool,
) -> SourceSyncReadiness {
    let stale = sync_completed_age_seconds.is_none_or(|age| age >= STARTUP_CACHE_STALE_SECONDS);
    let sync_required_reason = if sync_status == Some("error") {
        Some(SyncRequiredReason::PreviousSyncError)
    } else if full_ingest && configured {
        if sync_status == Some("running") {
            Some(SyncRequiredReason::PreviousSyncError)
        } else if cached_item_count == 0 {
            Some(SyncRequiredReason::EmptyCache)
        } else if stale {
            Some(SyncRequiredReason::FullRefresh)
        } else {
            None
        }
    } else if configured && cached_item_count == 0 && sync_completed_age_seconds.is_none() {
        Some(SyncRequiredReason::EmptyCache)
    } else if stale && incremental {
        Some(SyncRequiredReason::CacheStale)
    } else {
        None
    };
    let prefetch_required_reason =
        (cached_item_count > 0 && artwork_missing).then_some(SyncRequiredReason::ArtworkMissing);
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
                    | SyncRequiredReason::CacheStale
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
    active: &ActiveSource,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, active, true)
}

pub(in crate::controller) fn active_source_startup_readiness(
    store: &StoreHandle,
    active: &ActiveSource,
) -> Result<SourceSyncReadiness, String> {
    active_source_readiness_inner(store, active, true)
}

fn active_source_readiness_inner(
    store: &StoreHandle,
    active: &ActiveSource,
    include_artwork: bool,
) -> Result<SourceSyncReadiness, String> {
    (active.readiness)(store, include_artwork)
}

pub(crate) fn local_readiness_evaluator(
    source_id: SourceId,
    roots: crate::sources::LocalRootsLoader,
) -> crate::sources::ReadinessEvaluator {
    Arc::new(move |store, include_artwork| {
        evaluate_source_readiness(
            store,
            &source_id,
            true,
            false,
            !(roots)().is_empty(),
            include_artwork && filesystem_source_cover_cache_missing(store, &source_id, false),
        )
    })
}

pub(crate) fn incremental_readiness_evaluator(
    source_id: SourceId,
) -> crate::sources::ReadinessEvaluator {
    Arc::new(move |store, _include_artwork| {
        evaluate_source_readiness(store, &source_id, false, true, true, false)
    })
}

pub(crate) fn filesystem_initial_cover_requirement(
    source_id: SourceId,
) -> crate::sources::InitialCoverCacheRequirement {
    Arc::new(move |store| filesystem_initial_cover_cache_required(store, &source_id))
}

pub(crate) fn source_initial_cover_requirement(
    source_id: SourceId,
) -> crate::sources::InitialCoverCacheRequirement {
    Arc::new(move |store| {
        store
            .with_store(|store| store.selected_source_cover_cache_missing(&source_id))
            .unwrap_or(true)
    })
}

fn evaluate_source_readiness(
    store: &StoreHandle,
    source_id: &SourceId,
    full_ingest: bool,
    incremental: bool,
    configured: bool,
    artwork_missing: bool,
) -> Result<SourceSyncReadiness, String> {
    let (cached_item_count, sync_status, sync_completed_age_seconds) =
        store.with_store(|store| {
            let albums = store.load_albums(source_id, 0, 1)?.total;
            let tracks = store.load_tracks(source_id, 0, 1)?.total;
            let sync_status = store.sync_state(source_id).ok().map(|state| state.status);
            let sync_completed_age_seconds = store.sync_completed_age_seconds(source_id)?;
            Ok((
                albums.saturating_add(tracks),
                sync_status,
                sync_completed_age_seconds,
            ))
        })?;
    Ok(source_sync_readiness(
        full_ingest,
        incremental,
        configured,
        cached_item_count,
        sync_status.as_deref(),
        sync_completed_age_seconds,
        artwork_missing,
    ))
}
