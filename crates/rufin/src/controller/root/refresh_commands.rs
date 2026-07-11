use super::*;

impl AppController {
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
            self.request_manual_source_sync(saved.source.id);
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
                active_source: Arc::clone(&self.active_source),
                events: self.events.clone(),
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
                active_source: Arc::clone(&self.active_source),
                events: self.events.clone(),
                home_refresh_in_flight: self.home_refresh_in_flight.clone(),
            },
            saved,
            target,
        );
    }
}
