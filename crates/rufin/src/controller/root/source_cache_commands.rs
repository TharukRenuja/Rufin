use super::*;

impl AppController {
    #[cfg(test)]
    pub fn clear_active_source_cache(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = self.sync_in_flight.clone();
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
            if sync_is_running(&sync_in_flight, &saved.source.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before clearing cache."
                        .to_string(),
                ));
                return;
            }
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to clear source cover cache");
            }
            if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to clear source waveform cache");
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Cached library cleared.".to_string(),
            ));
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn clear_source_cache(&self, source_id: SourceId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = self.sync_in_flight.clone();
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                Ok(store
                    .list_sources()?
                    .into_iter()
                    .find(|saved| saved.source.id == source_id))
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
            if sync_is_running(&sync_in_flight, &saved.source.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before clearing cache."
                        .to_string(),
                ));
                return;
            }
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.source.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_store_disk_cover_cache(&store, &saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to clear source cover cache");
            }
            if let Err(error) = clear_store_disk_waveform_cache(&store, &saved.source.id) {
                warn!(%error, source_id = %saved.source.id, "failed to clear source waveform cache");
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Cached library cleared.".to_string(),
            ));
            emit_snapshot(&store, &events);
        });
    }
}
