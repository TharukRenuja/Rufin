use super::*;

impl AppController {
    pub(in crate::controller) fn auto_dj_top_up_or_emit_error(&self) -> bool {
        match self.auto_dj_top_up() {
            Ok(topped_up) => topped_up,
            Err(error) => {
                let _sent = self.events.send(ControllerEvent::Error(error));
                false
            }
        }
    }
    pub(in crate::controller) fn auto_dj_top_up(&self) -> Result<bool, String> {
        if !self
            .auto_dj_enabled
            .lock()
            .map(|enabled| *enabled)
            .unwrap_or_default()
        {
            return Ok(false);
        }
        let Some((server_id, current, queued_track_ids, remaining)) = self.auto_dj_queue_state()
        else {
            return Ok(false);
        };
        let settings = load_settings_for_active_server(&self.store);
        if remaining >= usize::from(settings.auto_dj_refill_threshold) {
            return Ok(false);
        }
        let mut tracks = self
            .store
            .with_store(|store| store.load_tracks(&server_id, 0, AUTO_DJ_LIBRARY_LIMIT))
            .map(|page| page.items)?;
        external_metadata::normalize_tracks(&mut tracks, &settings);
        let candidates = auto_dj_candidates(&tracks, &current, &queued_track_ids, shuffle_seed());
        if candidates.is_empty() {
            return Ok(false);
        }
        self.with_queue_mut(|queue| {
            for track in &candidates {
                queue.append(track);
            }
            Ok(())
        })?;
        Ok(true)
    }
    pub(in crate::controller) fn auto_dj_queue_state(
        &self,
    ) -> Option<(ServerId, QueueEntry, HashSet<TrackId>, usize)> {
        self.queue.lock().ok().and_then(|queue| {
            let queue = queue.as_ref()?;
            let snapshot = queue.snapshot();
            let current = queue.current()?.clone();
            let queued = queue
                .entries()
                .iter()
                .map(|entry| entry.track_id.clone())
                .collect::<HashSet<_>>();
            Some((
                snapshot.server_id,
                current,
                queued,
                queue.remaining_after_current(),
            ))
        })
    }
}
