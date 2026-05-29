use super::*;

impl Shell {
    pub(in crate::ui) fn cover_cache_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
        let server = self.state.library.borrow().server.clone()?;
        if server.provider == "fake" {
            return None;
        }
        if external_metadata::is_external_image_ref(image_ref)
            && !external_metadata::enabled(&self.state.settings.borrow())
        {
            return None;
        }
        Some(image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            size,
        ))
    }
    pub(in crate::ui) fn cover_cache_candidate_keys(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
    ) -> Vec<String> {
        decoded_cover_candidate_sizes(preferred_size)
            .into_iter()
            .filter_map(|size| self.cover_cache_key(image_ref, size))
            .collect()
    }
    pub(in crate::ui) fn decoded_cover_for_ref(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
        min_size: i32,
    ) -> Option<(String, Pixbuf)> {
        for size in decoded_cover_candidate_sizes(preferred_size) {
            let Some(key) = self.cover_cache_key(image_ref, size) else {
                continue;
            };
            if let Some(cover) = self.cloned_decoded_cover(&key, min_size) {
                return Some((key, cover.pixbuf));
            }
        }
        None
    }
    pub(in crate::ui) fn start_cached_cover_path_lookup(
        self: &Rc<Self>,
        request: CoverPathLookupRequest,
    ) {
        let CoverPathLookupRequest {
            key,
            image_ref,
            fetch_size,
            size,
            intent,
        } = request;
        let should_start = record_cover_path_lookup_request(
            &mut self.state.cover_path_lookups.borrow_mut(),
            key.clone(),
            intent,
        );
        if !should_start {
            return;
        }

        let shell = Rc::clone(self);
        let controller = self.controller.clone();
        let candidate_keys = self.cover_cache_candidate_keys(&image_ref, fetch_size);
        glib::spawn_future_local(async move {
            let path = gtk::gio::spawn_blocking(move || {
                candidate_keys
                    .iter()
                    .find_map(|key| controller.cached_cover_path_for_key(key))
            })
            .await
            .ok()
            .flatten();
            let intent = shell
                .state
                .cover_path_lookups
                .borrow_mut()
                .remove(&key)
                .unwrap_or(intent);
            shell.finish_cached_cover_path_lookup(key, size, intent, path);
        });
    }
    pub(in crate::ui) fn finish_cached_cover_path_lookup(
        self: &Rc<Self>,
        key: String,
        size: i32,
        intent: CoverPathLookupIntent,
        path: Option<PathBuf>,
    ) {
        match intent {
            CoverPathLookupIntent::Warm => {
                if let Some(path) = path {
                    self.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Warm);
                }
            }
            CoverPathLookupIntent::Visible => {
                self.finish_visible_cover_path_lookup(key, size, path);
            }
        }
    }
    fn finish_visible_cover_path_lookup(
        self: &Rc<Self>,
        key: String,
        size: i32,
        path: Option<PathBuf>,
    ) {
        let Some(path) = path else {
            self.state.cover_bindings.borrow_mut().remove(&key);
            self.record_perf_cover_stale_key(&key);
            self.record_perf_coverless_tile();
            return;
        };

        if !self.cover_binding_has_live(&key) {
            self.record_perf_cover_stale_key(&key);
            return;
        }

        let size = self
            .pending_cover_size(&key)
            .map(|pending_size| {
                let fetch_size = cover_size_from_cache_key(&key).unwrap_or(size).max(1) as u32;
                cover_decode_size(pending_size, fetch_size).max(size)
            })
            .unwrap_or(size);
        self.record_perf_cover_path_ready(&key);
        self.record_perf_cover_ready(&key);
        self.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Visible);
    }
    pub(in crate::ui) fn apply_cover_ready(self: &Rc<Self>, key: &str, path: &Path) {
        self.record_perf_cover_ready(key);
        let size = self
            .pending_cover_size(key)
            .unwrap_or(GRID_COVER_SIZE as i32);
        if let Some(cover) = self.cloned_decoded_cover(key, size) {
            self.touch_decoded_cover(key, CoverDecodePriority::Visible);
            let bindings = self.take_live_cover_bindings(key);
            apply_pixbuf_to_bindings(bindings, cover.pixbuf);
            return;
        }
        self.start_cover_decode_from_path(
            key.to_string(),
            path.to_path_buf(),
            size,
            CoverDecodePriority::Visible,
        );
    }
    pub(in crate::ui) fn start_cover_decode_from_path(
        self: &Rc<Self>,
        key: String,
        path: PathBuf,
        size: i32,
        priority: CoverDecodePriority,
    ) {
        if self.apply_decoded_cover_if_available(&key, size, priority) {
            return;
        }
        if priority == CoverDecodePriority::Warm && !self.decoded_cover_has_warm_capacity(size) {
            return;
        }

        if self.state.cover_decodes.borrow().contains(&key) {
            return;
        }

        {
            let mut queue = self.state.cover_decode_queue.borrow_mut();
            let requires_live_binding = priority == CoverDecodePriority::Visible
                && self.state.cover_bindings.borrow().contains_key(&key);
            if let Some(position) = queue.iter().position(|job| job.key == key) {
                let Some(mut job) = queue.remove(position) else {
                    return;
                };
                job.size = job.size.max(size);
                job.requires_live_binding |= requires_live_binding;
                job.priority = if job.priority == CoverDecodePriority::Visible
                    || priority == CoverDecodePriority::Visible
                {
                    CoverDecodePriority::Visible
                } else {
                    CoverDecodePriority::Warm
                };
                if job.priority == CoverDecodePriority::Visible {
                    queue.push_front(job);
                } else {
                    queue.push_back(job);
                }
                drop(queue);
                self.drain_cover_decode_queue();
                return;
            }

            let job = CoverDecodeJob {
                key,
                path,
                size,
                priority,
                requires_live_binding,
            };
            if priority == CoverDecodePriority::Visible {
                queue.push_front(job);
            } else {
                queue.push_back(job);
            }
        }

        self.drain_cover_decode_queue();
    }
    pub(in crate::ui) fn apply_decoded_cover_if_available(
        &self,
        key: &str,
        min_size: i32,
        priority: CoverDecodePriority,
    ) -> bool {
        let Some(cover) = self.cloned_decoded_cover(key, min_size) else {
            return false;
        };
        self.touch_decoded_cover(key, priority);
        self.state
            .startup_cover_prime_pending
            .borrow_mut()
            .remove(key);
        self.state
            .first_run_cover_prime_pending
            .borrow_mut()
            .remove(key);
        let bindings = self.take_live_cover_bindings(key);
        apply_pixbuf_to_bindings(bindings, cover.pixbuf);
        true
    }
}
