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
    pub fn save_explore_projection_for_active(&self, source_id: SourceId, section: HomeSection) {
        if section.kind != HomeSectionKind::Explore {
            return;
        }
        let Ok(active) = selected_active_source(&self.active_source, &source_id) else {
            return;
        };
        start_home_promotion(
            self.store.clone(),
            self.library_events.clone(),
            Arc::clone(&self.active_source),
            active,
            source_id,
            section,
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
