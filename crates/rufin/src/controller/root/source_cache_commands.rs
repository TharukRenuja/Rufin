use super::*;

impl AppController {
    #[cfg(test)]
    pub fn clear_active_source_cache(&self) {
        let controller = self.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_source())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            controller.forget_source_sync(&saved.source_id);
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source_id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = controller.invalidate_artwork_source(&saved.source_id) {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate source artwork");
            }
            if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source_id) {
                warn!(%error, source_id = %saved.source_id, "failed to clear source waveform cache");
            }
            let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::CacheCleared));
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
            controller.refresh_source_freshness();
        });
    }
    pub fn clear_source_cache(&self, source_id: SourceId) {
        let controller = self.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source_id == source_id))
            }) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            controller.forget_source_sync(&saved.source_id);
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source_id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = controller.invalidate_artwork_source(&saved.source_id) {
                warn!(%error, source_id = %saved.source_id, "failed to invalidate source artwork");
            }
            if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source_id) {
                warn!(%error, source_id = %saved.source_id, "failed to clear source waveform cache");
            }
            let _sent = events.send(ControllerEvent::SourceNotice(SourceNotice::CacheCleared));
            emit_snapshot(&store, &events);
            if sync_target_is_current(&store, &saved.source_id) {
                controller.refresh_source_freshness();
            }
        });
    }
}
