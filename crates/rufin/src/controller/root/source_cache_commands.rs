use super::*;

impl SourceCommands {
    #[cfg(test)]
    pub fn clear_active_source_cache(&self) {
        let controller = self.clone();
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        let source_notice = self.source_events.notice.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                warn!("cannot clear active source cache without an active source");
                return;
            };
            controller.forget_source_sync(&saved.source_id);
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source_id)?;
                Ok(())
            });
            if let Err(error) = result {
                warn!(%error, source_id = %saved.source_id, "failed to clear active source cache");
                return;
            }
            if let Err(error) =
                crate::controller::artwork::invalidate_source(&controller.artwork, &saved.source_id)
            {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate source artwork");
            }
            if let Err(error) =
                super::playback_waveforms::clear_store_disk_waveform_cache(&store, &saved.source_id)
            {
                warn!(%error, source_id = %saved.source_id, "failed to clear source waveform cache");
            }
            let _sent = source_notice.try_send(SourceNotice::CacheCleared);
            match load_source_presentation(&store) {
                Ok(presentation) => {
                    let _sent = source_presentation.try_send(presentation);
                }
                Err(error) => {
                    warn!(%error, "failed to reload source presentation after cache clear");
                }
            }
            controller.refresh_source_freshness();
        });
    }
    pub fn clear_source_cache(&self, source_id: SourceId) {
        let controller = self.clone();
        let store = self.store.clone();
        let source_presentation = self.source_events.presentation.clone();
        let source_notice = self.source_events.notice.clone();
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source_id == source_id))
            }) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    warn!(%source_id, "cannot clear cache for an unsaved source");
                    return;
                }
                Err(error) => {
                    warn!(%error, %source_id, "failed to load source before clearing cache");
                    return;
                }
            };
            controller.forget_source_sync(&saved.source_id);
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source_id)?;
                Ok(())
            });
            if let Err(error) = result {
                warn!(%error, source_id = %saved.source_id, "failed to clear source cache");
                return;
            }
            if let Err(error) =
                crate::controller::artwork::invalidate_source(&controller.artwork, &saved.source_id)
            {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate source artwork");
            }
            if let Err(error) =
                super::playback_waveforms::clear_store_disk_waveform_cache(&store, &saved.source_id)
            {
                warn!(%error, source_id = %saved.source_id, "failed to clear source waveform cache");
            }
            let _sent = source_notice.try_send(SourceNotice::CacheCleared);
            emit_source_presentation(&store, &source_presentation);
            if sync_target_is_current(&store, &saved.source_id) {
                controller.refresh_source_freshness();
            }
        });
    }
}
