use super::*;

const ACTIVE_SOURCE_RECONCILIATION_DELAY_MS: u64 = 2_000;

impl AppController {
    pub fn start_background_sync_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_source())
            .unwrap_or(None);
        if let Some(saved) = active {
            if saved_server_needs_auth(&self.secrets, &saved) {
                debug!(
                    source_id = %saved.source.id,
                    source_kind = %saved.source.kind,
                    "skipping background sync until server sign-in completes"
                );
                return;
            }
            start_background_sync_thread(self.sync_context(), saved);
        }
    }
    #[cfg(test)]
    pub fn resync_active_source(&self) {
        let active = self
            .store
            .with_store(|store| store.active_source())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "No active music server is saved.".to_string(),
            ));
        }
    }
    pub fn resync_server(&self, source_id: SourceId) {
        let saved = self
            .store
            .with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source.id == source_id))
            })
            .unwrap_or(None);
        if let Some(saved) = saved {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "The selected server is no longer saved.".to_string(),
            ));
        }
    }
    pub fn resync_local_library(&self) {
        self.resync_server(SourceId::new(LOCAL_SOURCE_IDENTITY_ID));
    }
    pub fn refresh_home_section_for_active(&self, kind: HomeSectionKind) {
        let active = self
            .store
            .with_store(|store| store.active_source())
            .unwrap_or(None);
        if let Some(saved) = active {
            if saved_server_needs_auth(&self.secrets, &saved) {
                return;
            }
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::Section(kind));
        }
    }
    pub fn prefetch_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_source())
            .unwrap_or(None);
        if let Some(saved) = active {
            if saved_server_needs_auth(&self.secrets, &saved) {
                return;
            }
            self.start_explore_prefetch_for_saved(saved);
        }
    }
    pub fn promote_prefetched_explore_for_active(&self, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        let active = self
            .store
            .with_store(|store| store.active_source())
            .unwrap_or(None);
        let Some(saved) = active else {
            return;
        };
        start_home_promotion(
            self.store.clone(),
            self.events.clone(),
            saved.source.id,
            section,
        );
    }
    pub(in crate::controller) fn start_explore_prefetch_for_saved(&self, saved: SavedSource) {
        start_explore_prefetch_thread(
            ExplorePrefetchContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: self.sync_in_flight.clone(),
                explore_prefetch_in_flight: self.explore_prefetch_in_flight.clone(),
            },
            saved,
        );
    }
    pub(in crate::controller) fn start_home_refresh_for_saved(
        &self,
        saved: SavedSource,
        target: HomeRefreshTarget,
    ) {
        start_home_refresh_thread(
            HomeRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: self.sync_in_flight.clone(),
                home_refresh_in_flight: self.home_refresh_in_flight.clone(),
            },
            saved,
            target,
        );
    }
    pub(in crate::controller) fn sync_context(&self) -> SyncContext {
        SyncContext {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            queue: Arc::clone(&self.queue),
            queue_persist_generation: Arc::clone(&self.queue_persist_generation),
            playback_snapshot: Arc::clone(&self.playback_snapshot),
            auto_dj_enabled: Arc::clone(&self.auto_dj_enabled),
            sync_in_flight: self.sync_in_flight.clone(),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
            external_cover_retry_generation: Arc::clone(&self.external_cover_retry_generation),
            external_cover_prefetch_in_flight: Arc::clone(&self.external_cover_prefetch_in_flight),
            cover_slots: Arc::clone(&self.cover_slots),
        }
    }
    pub fn startup_sync_delay_ms(&self) -> Option<u64> {
        let saved = self
            .store
            .with_store(|store| store.active_source())
            .ok()
            .flatten()?;
        if saved_server_needs_auth(&self.secrets, &saved) {
            debug!(
                source_id = %saved.source.id,
                source_kind = %saved.source.kind,
                "skipping startup sync until server sign-in completes"
            );
            return None;
        }
        let readiness = active_source_startup_readiness(&self.store, &saved.source.id).ok()?;
        debug!(
            source_id = %saved.source.id,
            source_kind = %saved.source.kind,
            metadata_fresh = readiness.metadata_fresh,
            artwork_fresh = readiness.artwork_fresh,
            sync_required_reason = ?readiness.sync_required_reason,
            prefetch_required_reason = ?readiness.prefetch_required_reason,
            startup_delay_ms = ?readiness.startup_delay_ms,
            "evaluated active source readiness"
        );
        let local_source_configured = saved.source.kind != LOCAL_SOURCE_ID
            || !load_settings_from_store(&self.store)
                .sources
                .local_folders
                .is_empty();
        if local_source_configured
            && cached_library_exists(&self.store, &saved.source.id)
            && readiness.sync_required_reason.is_none()
            && (saved.source.kind == LOCAL_SOURCE_ID
                || active_source_reconciliation_supported(&saved))
        {
            Some(ACTIVE_SOURCE_RECONCILIATION_DELAY_MS)
        } else {
            readiness.startup_delay_ms
        }
    }
}
