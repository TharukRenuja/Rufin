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
    pub fn refresh_home_sections_without_explore_for_active(&self) {
        let active = self
            .store
            .with_store(|store| store.active_server())
            .unwrap_or(None);
        if let Some(saved) = active {
            self.start_home_refresh_for_saved(saved, HomeRefreshTarget::WithoutExplore);
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
    fn start_explore_prefetch_for_saved(&self, saved: SavedServer) {
        start_explore_prefetch_thread(
            ExplorePrefetchContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                explore_prefetch_in_flight: Arc::clone(&self.explore_prefetch_in_flight),
            },
            saved,
        );
    }
    fn start_home_refresh_for_saved(&self, saved: SavedServer, target: HomeRefreshTarget) {
        start_home_refresh_thread(
            HomeRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                home_refresh_in_flight: Arc::clone(&self.home_refresh_in_flight),
            },
            saved,
            target,
        );
    }
    fn start_playlist_refresh_for_saved(&self, saved: SavedServer) {
        start_playlist_refresh_thread(
            PlaylistRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                secrets: Arc::clone(&self.secrets),
                events: self.events.clone(),
                sync_in_flight: Arc::clone(&self.sync_in_flight),
                playlist_refresh_in_flight: Arc::clone(&self.playlist_refresh_in_flight),
            },
            saved,
        );
    }
    fn sync_context(&self) -> SyncContext {
        SyncContext {
            store: self.store.clone(),
            runtime: Arc::clone(&self.runtime),
            secrets: Arc::clone(&self.secrets),
            events: self.events.clone(),
            sync_in_flight: Arc::clone(&self.sync_in_flight),
            cover_in_flight: Arc::clone(&self.cover_in_flight),
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
        let albums = self
            .store
            .with_store(|store| {
                store
                    .load_albums(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        let tracks = self
            .store
            .with_store(|store| {
                store
                    .load_tracks(&saved.server.id, 0, 1)
                    .map(|page| page.total)
            })
            .unwrap_or(0);
        if albums == 0 && tracks == 0 {
            return Some(500);
        }
        let sync_state = self
            .store
            .with_store(|store| store.sync_state(&saved.server.id))
            .ok();
        if sync_state
            .as_ref()
            .is_some_and(|state| state.status == "error")
        {
            return Some(8_000);
        }
        let age = self
            .store
            .with_store(|store| store.sync_completed_age_seconds(&saved.server.id))
            .ok()
            .flatten();
        match age {
            Some(seconds) if seconds < STARTUP_CACHE_STALE_SECONDS => None,
            _ => Some(8_000),
        }
    }
}
