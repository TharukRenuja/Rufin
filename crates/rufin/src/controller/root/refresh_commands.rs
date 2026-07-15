use super::*;

impl SourceCommands {
    pub fn resync_server(&self, source_id: SourceId) {
        let saved = self
            .store
            .with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source_id == source_id))
            })
            .unwrap_or(None);
        if let Some(saved) = saved {
            self.request_manual_source_sync(saved.source_id);
        } else {
            warn!(%source_id, "cannot resync an unsaved source");
        }
    }
    pub fn resync_local_library(&self) {
        self.resync_server(SourceId::new(LOCAL_SOURCE_IDENTITY_ID));
    }
}

impl LibraryCommands {
    pub fn refresh_home_section_for_active(&self, source_id: SourceId, kind: HomeSectionKind) {
        self.start_home_refresh(source_id, HomeRefreshTarget::Section(kind));
    }
    pub fn prefetch_explore_for_active(&self, source_id: SourceId) {
        self.start_explore_prefetch(source_id);
    }
    pub fn save_explore_projection_for_active(&self, source_id: SourceId, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        if selected_active_source(&self.active_source, &source_id).is_err() {
            return;
        }
        start_home_promotion(
            self.store.clone(),
            self.library_events.clone(),
            source_id,
            section,
        );
    }
    pub(in crate::controller) fn start_explore_prefetch(&self, source_id: SourceId) {
        start_explore_prefetch_thread(
            ExplorePrefetchContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                active_source: Arc::clone(&self.active_source),
                secrets: Arc::clone(&self.secrets),
                library_events: self.library_events.clone(),
                explore_prefetch_in_flight: self.explore_prefetch_in_flight.clone(),
            },
            source_id,
        );
    }
    pub(in crate::controller) fn start_home_refresh(
        &self,
        source_id: SourceId,
        target: HomeRefreshTarget,
    ) {
        start_home_refresh_thread(
            HomeRefreshContext {
                store: self.store.clone(),
                runtime: Arc::clone(&self.runtime),
                active_source: Arc::clone(&self.active_source),
                secrets: Arc::clone(&self.secrets),
                library_events: self.library_events.clone(),
                home_refresh_in_flight: self.home_refresh_in_flight.clone(),
            },
            source_id,
            target,
        );
    }
}
