use super::*;

impl Shell {
    pub(in crate::ui) fn reset_queued_cover_work_for_route_render(&self) {
        self.cancel_route_cover_warm();
        self.state.cover_bindings.borrow_mut().clear();
        clear_queued_route_cover_work(
            &mut self.state.cover_path_lookups.borrow_mut(),
            &mut self.state.cover_decode_queue.borrow_mut(),
        );
    }

    pub(in crate::ui) fn drain_cover_decode_queue(self: &Rc<Self>) {
        while let Some(job) = self.next_cover_decode_job() {
            if job.requires_live_binding && !self.cover_binding_has_live(&job.key) {
                self.record_perf_cover_stale_key(&job.key);
                continue;
            }
            if self.apply_decoded_cover_if_available(&job.key, job.size, job.priority) {
                continue;
            }
            if job.priority == CoverDecodePriority::Warm
                && !self.decoded_cover_has_warm_capacity(job.size)
            {
                continue;
            }
            if self
                .state
                .cover_decodes
                .borrow_mut()
                .insert(job.key.clone(), job.priority)
                .is_some()
            {
                continue;
            }
            self.spawn_cover_decode_job(job);
        }
    }
    fn next_cover_decode_job(&self) -> Option<CoverDecodeJob> {
        let mut queue = self.state.cover_decode_queue.borrow_mut();
        let active = self.state.cover_decodes.borrow();
        if cover_decode_has_capacity(&active, CoverDecodePriority::Visible)
            && let Some(visible) = queue
                .iter()
                .position(|job| job.priority == CoverDecodePriority::Visible)
        {
            return queue.remove(visible);
        }
        if self.cover_warm_is_paused()
            || !cover_decode_has_capacity(&active, CoverDecodePriority::Warm)
        {
            return None;
        }
        let warm = queue
            .iter()
            .position(|job| job.priority == CoverDecodePriority::Warm)?;
        queue.remove(warm)
    }
    pub(in crate::ui) fn spawn_cover_decode_job(self: &Rc<Self>, job: CoverDecodeJob) {
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let CoverDecodeJob {
                key,
                path,
                size,
                priority,
                requires_live_binding: _,
            } = job;
            shell.record_perf_cover_decode_start(&key);
            match load_cover_pixbuf(path.clone(), size, priority.glib_priority()).await {
                Ok(pixbuf) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_ok(&key);
                    let pixbuf = shell.remember_decoded_cover(key.clone(), pixbuf, priority);
                    let bindings = shell.take_live_cover_bindings(&key);
                    apply_pixbuf_to_bindings(bindings, pixbuf);
                }
                Err(error) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_error(&key);
                    warn!(%error, path = %path.display(), "failed to load cached cover");
                    for binding in shell.take_live_cover_bindings(&key) {
                        if !binding.clear_on_failure {
                            continue;
                        }
                        let cleared = binding
                            .tile
                            .upgrade()
                            .is_some_and(|tile| tile.clear_image_if_current(binding.generation));
                        if !cleared {
                            shell.record_perf_cover_stale_ignored();
                        }
                    }
                }
            }
            shell.drain_cover_decode_queue();
        });
    }
    pub(in crate::ui) fn finish_cover_decode(&self, key: &str) {
        self.state.cover_decodes.borrow_mut().remove(key);
        self.state
            .startup_cover_prime_pending
            .borrow_mut()
            .remove(key);
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .remove(key);
    }
    pub(in crate::ui) fn cancel_queued_warm_cover_decodes(&self) {
        self.state
            .cover_decode_queue
            .borrow_mut()
            .retain(|job| job.priority != CoverDecodePriority::Warm);
    }
    pub(in crate::ui) fn pending_cover_size(&self, key: &str) -> Option<i32> {
        self.state
            .cover_bindings
            .borrow()
            .get(key)
            .and_then(|bindings| bindings.first())
            .map(|binding| binding.tile.size())
            .or_else(|| cover_size_from_cache_key(key))
    }
    pub(in crate::ui) fn take_live_cover_bindings(&self, key: &str) -> Vec<CoverBinding> {
        let Some(bindings) = self.state.cover_bindings.borrow_mut().remove(key) else {
            return Vec::new();
        };
        self.live_cover_bindings(key, bindings)
    }
    pub(in crate::ui) fn live_cover_bindings(
        &self,
        key: &str,
        bindings: Vec<CoverBinding>,
    ) -> Vec<CoverBinding> {
        let mut live = Vec::with_capacity(bindings.len());
        let mut stale = 0_usize;
        for binding in bindings {
            if binding.tile.is_current_generation(binding.generation) {
                live.push(binding);
            } else {
                stale = stale.saturating_add(1);
            }
        }
        if stale > 0 {
            self.record_perf_cover_stale_ignored_by(stale);
        }
        if live.is_empty() {
            self.record_perf_cover_stale_key(key);
        }
        live
    }
    pub(in crate::ui) fn cover_binding_has_live(&self, key: &str) -> bool {
        let mut all_bindings = self.state.cover_bindings.borrow_mut();
        let live = if let Some(bindings) = all_bindings.get_mut(key) {
            bindings.retain(|binding| binding.tile.is_current_generation(binding.generation));
            !bindings.is_empty()
        } else {
            false
        };
        if !live {
            all_bindings.remove(key);
        }
        live
    }
    pub(in crate::ui) fn remember_decoded_cover(
        &self,
        key: String,
        pixbuf: Pixbuf,
        priority: CoverDecodePriority,
    ) -> Pixbuf {
        let size = pixbuf.width().min(pixbuf.height()).max(1);
        let bytes = pixbuf_bytes(&pixbuf);
        let last_used = self.next_decoded_cover_touch();
        let mut priority = priority;
        let mut covers = self.state.decoded_covers.borrow_mut();
        if let Some(existing) = covers.get_mut(&key)
            && existing.size >= size
        {
            existing.last_used = last_used;
            if priority == CoverDecodePriority::Visible {
                existing.priority = priority;
            }
            self.state
                .decoded_cover_order
                .borrow_mut()
                .push_back(DecodedCoverOrderEntry { key, last_used });
            return existing.pixbuf.clone();
        }

        if let Some(existing) = covers.remove(&key) {
            if existing.priority == CoverDecodePriority::Visible {
                priority = CoverDecodePriority::Visible;
            }
            self.state.decoded_cover_bytes.set(
                self.state
                    .decoded_cover_bytes
                    .get()
                    .saturating_sub(existing.bytes),
            );
        }
        covers.insert(
            key.clone(),
            DecodedCover {
                pixbuf: pixbuf.clone(),
                size,
                bytes,
                last_used,
                priority,
            },
        );
        self.state
            .decoded_cover_bytes
            .set(self.state.decoded_cover_bytes.get().saturating_add(bytes));
        self.state
            .decoded_cover_order
            .borrow_mut()
            .push_back(DecodedCoverOrderEntry { key, last_used });
        drop(covers);
        self.evict_decoded_covers();
        pixbuf
    }
    pub(in crate::ui) fn next_decoded_cover_touch(&self) -> u64 {
        let next = self.state.decoded_cover_touch.get().saturating_add(1);
        self.state.decoded_cover_touch.set(next);
        next
    }
    pub(in crate::ui) fn touch_decoded_cover(&self, key: &str, priority: CoverDecodePriority) {
        let last_used = self.next_decoded_cover_touch();
        let mut covers = self.state.decoded_covers.borrow_mut();
        let Some(cover) = covers.get_mut(key) else {
            return;
        };
        cover.last_used = last_used;
        if priority == CoverDecodePriority::Visible {
            cover.priority = priority;
        }
        self.state
            .decoded_cover_order
            .borrow_mut()
            .push_back(DecodedCoverOrderEntry {
                key: key.to_string(),
                last_used,
            });
    }
    pub(in crate::ui) fn decoded_cover_has_warm_capacity(&self, size: i32) -> bool {
        self.state
            .decoded_cover_bytes
            .get()
            .saturating_add(estimated_decoded_cover_bytes(size))
            <= DECODED_COVER_CACHE_SOFT_BYTES
    }
    pub(in crate::ui) fn evict_decoded_covers(&self) {
        let mut covers = self.state.decoded_covers.borrow_mut();
        let mut order = self.state.decoded_cover_order.borrow_mut();
        let mut bytes = self.state.decoded_cover_bytes.get();
        while (bytes > DECODED_COVER_CACHE_SOFT_BYTES || covers.len() > DECODED_COVER_CACHE_LIMIT)
            && !covers.is_empty()
        {
            let candidate = decoded_cover_eviction_candidate(&mut order, &covers, true)
                .or_else(|| decoded_cover_eviction_candidate(&mut order, &covers, false));
            let Some(key) = candidate else {
                break;
            };
            if let Some(cover) = covers.remove(&key) {
                bytes = bytes.saturating_sub(cover.bytes);
            }
        }
        self.state.decoded_cover_bytes.set(bytes);
    }
    pub(in crate::ui) fn decoded_cover_has_min_size(&self, key: &str, min_size: i32) -> bool {
        self.cloned_decoded_cover(key, min_size).is_some()
    }
    pub(in crate::ui) fn cloned_decoded_cover(
        &self,
        key: &str,
        min_size: i32,
    ) -> Option<DecodedCover> {
        self.state
            .decoded_covers
            .borrow()
            .get(key)
            .filter(|cover| decoded_cover_satisfies_request(key, cover.size, min_size))
            .cloned()
    }
}
pub(in crate::ui) fn decoded_cover_satisfies_request(
    key: &str,
    cover_size: i32,
    min_size: i32,
) -> bool {
    cover_size >= min_size
        || cover_size_from_cache_key(key).is_some_and(|requested_size| requested_size >= min_size)
}
pub(in crate::ui) fn decoded_cover_eviction_candidate(
    order: &mut VecDeque<DecodedCoverOrderEntry>,
    covers: &HashMap<String, DecodedCover>,
    prefer_warm: bool,
) -> Option<String> {
    let count = order.len();
    for _ in 0..count {
        let entry = order.pop_front()?;
        let Some(cover) = covers.get(&entry.key) else {
            continue;
        };
        if cover.last_used != entry.last_used {
            continue;
        }
        if prefer_warm && cover.priority == CoverDecodePriority::Visible {
            order.push_back(entry);
            continue;
        }
        return Some(entry.key);
    }
    None
}

pub(in crate::ui) fn pixbuf_bytes(pixbuf: &Pixbuf) -> usize {
    (pixbuf.rowstride().max(0) as usize).saturating_mul(pixbuf.height().max(0) as usize)
}
pub(in crate::ui) fn estimated_decoded_cover_bytes(size: i32) -> usize {
    let size = cover_pixbuf_decode_size(size).max(1) as usize;
    size.saturating_mul(size).saturating_mul(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_cover_accepts_source_limited_image_for_requested_size_key() {
        assert!(decoded_cover_satisfies_request(
            "library/local_cover_example/untagged/256",
            180,
            256
        ));
    }

    #[test]
    fn decoded_cover_rejects_smaller_thumbnail_key_for_larger_request() {
        assert!(!decoded_cover_satisfies_request(
            "library/local_cover_example/untagged/96",
            96,
            256
        ));
    }
}
