impl Shell {
    fn drain_cover_decode_queue(self: &Rc<Self>) {
        loop {
            if self.state.cover_decodes.borrow().len() >= COVER_DECODE_MAX_IN_FLIGHT {
                break;
            }
            let Some(job) = self.state.cover_decode_queue.borrow_mut().pop_front() else {
                break;
            };
            if job.priority == CoverDecodePriority::Warm && self.cover_warm_is_paused() {
                self.state.cover_decode_queue.borrow_mut().push_front(job);
                break;
            }
            if self.apply_decoded_cover_if_available(&job.key, job.size) {
                continue;
            }
            if !self
                .state
                .cover_decodes
                .borrow_mut()
                .insert(job.key.clone())
            {
                continue;
            }
            self.spawn_cover_decode_job(job);
        }
    }
    fn spawn_cover_decode_job(self: &Rc<Self>, job: CoverDecodeJob) {
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let CoverDecodeJob {
                key,
                path,
                size,
                priority,
            } = job;
            shell.record_perf_cover_decode_start(&key);
            match load_cover_pixbuf(path.clone(), size, priority.glib_priority()).await {
                Ok(pixbuf) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_ok(&key);
                    let pixbuf = shell.remember_decoded_cover(key.clone(), pixbuf);
                    let bindings = shell.take_live_cover_bindings(&key);
                    apply_pixbuf_to_bindings(bindings, pixbuf);
                }
                Err(error) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_error(&key);
                    warn!(%error, path = %path.display(), "failed to load cached cover");
                    for binding in shell.take_live_cover_bindings(&key) {
                        if !binding.tile.clear_image_if_current(binding.generation) {
                            shell.record_perf_cover_stale_ignored();
                        }
                    }
                }
            }
            shell.drain_cover_decode_queue();
        });
    }
    fn finish_cover_decode(&self, key: &str) {
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
    fn cancel_queued_warm_cover_decodes(&self) {
        self.state
            .cover_decode_queue
            .borrow_mut()
            .retain(|job| job.priority != CoverDecodePriority::Warm);
    }
    fn pending_cover_size(&self, key: &str) -> Option<i32> {
        self.state
            .cover_bindings
            .borrow()
            .get(key)
            .and_then(|bindings| bindings.first())
            .map(|binding| binding.tile.size())
            .or_else(|| cover_size_from_cache_key(key))
    }
    fn take_live_cover_bindings(&self, key: &str) -> Vec<CoverBinding> {
        let Some(bindings) = self.state.cover_bindings.borrow_mut().remove(key) else {
            return Vec::new();
        };
        self.live_cover_bindings(key, bindings)
    }
    fn live_cover_bindings(&self, key: &str, bindings: Vec<CoverBinding>) -> Vec<CoverBinding> {
        let mut live = Vec::with_capacity(bindings.len());
        let mut stale = 0_usize;
        for binding in bindings {
            if binding.tile.is_live_generation(binding.generation) {
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
    fn remember_decoded_cover(&self, key: String, pixbuf: Pixbuf) -> Pixbuf {
        let size = pixbuf.width().min(pixbuf.height()).max(1);
        let mut covers = self.state.decoded_covers.borrow_mut();
        if let Some(existing) = covers.get(&key)
            && existing.size >= size
        {
            return existing.pixbuf.clone();
        }
        if !covers.contains_key(&key) {
            self.state
                .decoded_cover_order
                .borrow_mut()
                .push_back(key.clone());
        }
        covers.insert(
            key,
            DecodedCover {
                pixbuf: pixbuf.clone(),
                size,
            },
        );
        let mut order = self.state.decoded_cover_order.borrow_mut();
        while covers.len() > DECODED_COVER_CACHE_LIMIT {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            covers.remove(&oldest);
        }
        pixbuf
    }
    fn decoded_cover_has_min_size(&self, key: &str, min_size: i32) -> bool {
        self.state
            .decoded_covers
            .borrow()
            .get(key)
            .is_some_and(|cover| cover.size >= min_size)
    }
}
