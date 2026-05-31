use super::*;

impl AppController {
    pub fn start_background_sync_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        }
    }
    #[cfg(test)]
    pub fn resync_active_server(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_sync(saved);
        } else {
            let _sent = self.events.send(ControllerEvent::Error(
                "No active music server is saved.".to_string(),
            ));
        }
    }
    pub fn resync_server(&self, server_id: ServerId) {
        let saved = self
            .store
            .with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id))
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
    pub fn refresh_home_section_for_active(&self, kind: HomeSectionKind) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::Section(kind));
        }
    }
    pub fn refresh_playlists_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_playlist_refresh_for_saved(saved);
        }
    }
    pub fn prefetch_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_explore_prefetch_for_saved(saved);
        }
    }
    pub fn promote_prefetched_explore_for_active(&self, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        let Some(saved) = active else {
            return;
        };
        start_prefetched_home_section_promotion_thread(
            self.store.clone(),
            self.events.clone(),
            saved.server.id,
            section,
        );
    }
    pub(in crate::controller) fn start_explore_prefetch_for_saved(&self, saved: SavedServer) {
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
        saved: SavedServer,
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
    pub(in crate::controller) fn start_playlist_refresh_for_saved(&self, saved: SavedServer) {
        start_playlist_refresh_thread(
            PlaylistRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: self.sync_in_flight.clone(),
                playlist_refresh_in_flight: self.playlist_refresh_in_flight.clone(),
            },
            saved,
        );
    }
    pub(in crate::controller) fn sync_context(&self) -> SyncContext {
        SyncContext {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
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
            .with_store(|store| store.active_server())
            .ok()
            .flatten()?;
        let readiness = active_source_startup_readiness(&self.store, &saved.server.id).ok()?;
        debug!(
            server_id = %saved.server.id,
            provider = %saved.server.provider,
            metadata_fresh = readiness.metadata_fresh,
            artwork_fresh = readiness.artwork_fresh,
            sync_required_reason = ?readiness.sync_required_reason,
            prefetch_required_reason = ?readiness.prefetch_required_reason,
            startup_delay_ms = ?readiness.startup_delay_ms,
            "evaluated active source readiness"
        );
        readiness.startup_delay_ms
    }
}
