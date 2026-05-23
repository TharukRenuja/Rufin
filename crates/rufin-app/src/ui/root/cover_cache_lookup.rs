impl Shell {
    fn cover_cache_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
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
    fn cover_cache_candidate_keys(&self, image_ref: &ImageRef, preferred_size: u32) -> Vec<String> {
        decoded_cover_candidate_sizes(preferred_size)
            .into_iter()
            .filter_map(|size| self.cover_cache_key(image_ref, size))
            .collect()
    }
    fn decoded_cover_for_ref(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
        min_size: i32,
    ) -> Option<(String, Pixbuf)> {
        for size in decoded_cover_candidate_sizes(preferred_size) {
            let Some(key) = self.cover_cache_key(image_ref, size) else {
                continue;
            };
            if let Some(cover) = self.state.decoded_covers.borrow().get(&key).cloned()
                && cover.size >= min_size
            {
                return Some((key, cover.pixbuf));
            }
        }
        None
    }
    fn apply_cover_ready(self: &Rc<Self>, key: &str, path: &Path) {
        self.record_perf_cover_ready(key);
        let size = self
            .pending_cover_size(key)
            .unwrap_or(GRID_COVER_SIZE as i32);
        if let Some(cover) = self.state.decoded_covers.borrow().get(key).cloned()
            && cover.size >= size
        {
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
    fn start_cover_decode_from_path(
        self: &Rc<Self>,
        key: String,
        path: PathBuf,
        size: i32,
        priority: CoverDecodePriority,
    ) {
        if self.apply_decoded_cover_if_available(&key, size) {
            return;
        }

        if self.state.cover_decodes.borrow().contains(&key) {
            return;
        }

        {
            let mut queue = self.state.cover_decode_queue.borrow_mut();
            if let Some(position) = queue.iter().position(|job| job.key == key) {
                let Some(mut job) = queue.remove(position) else {
                    return;
                };
                job.size = job.size.max(size);
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
            };
            if priority == CoverDecodePriority::Visible {
                queue.push_front(job);
            } else {
                queue.push_back(job);
            }
        }

        self.drain_cover_decode_queue();
    }
    fn apply_decoded_cover_if_available(&self, key: &str, min_size: i32) -> bool {
        let Some(cover) = self.state.decoded_covers.borrow().get(key).cloned() else {
            return false;
        };
        if cover.size < min_size {
            return false;
        }
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
