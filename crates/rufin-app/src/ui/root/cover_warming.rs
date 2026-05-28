use super::*;

impl Shell {
    pub(in crate::ui) fn start_warm_cover_path_lookup(
        self: &Rc<Self>,
        key: String,
        image_ref: ImageRef,
        fetch_size: u32,
        size: i32,
    ) {
        if self
            .state
            .cover_path_lookups
            .borrow_mut()
            .insert(key.clone())
        {
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
                shell.state.cover_path_lookups.borrow_mut().remove(&key);
                if let Some(path) = path {
                    shell.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Warm);
                }
            });
        }
    }
    pub(in crate::ui) fn warm_cover_refs(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
    ) {
        let decode_size = cover_decode_size(size, fetch_size);
        let generation = self.next_cover_warm_generation();
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self
                    .decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
                    .is_some()
            {
                continue;
            }
            jobs.push_back((key, image_ref));
        }

        if jobs.is_empty() {
            return;
        }

        self.schedule_cover_warm_jobs(
            Rc::new(RefCell::new(jobs)),
            fetch_size,
            decode_size,
            generation,
            COVER_WARM_INITIAL_DELAY_MS,
        );
    }
    pub(in crate::ui) fn warm_cover_refs_now(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
    ) {
        let decode_size = cover_decode_size(size, fetch_size);
        let generation = self.next_cover_warm_generation();
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self
                    .decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
                    .is_some()
            {
                continue;
            }
            jobs.push_back((key, image_ref));
        }

        if jobs.is_empty() {
            return;
        }

        self.schedule_cover_warm_jobs(
            Rc::new(RefCell::new(jobs)),
            fetch_size,
            decode_size,
            generation,
            0,
        );
    }
    pub(in crate::ui) fn schedule_startup_cover_warm(self: &Rc<Self>) {
        let generation = self
            .state
            .startup_cover_warm_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_warm_generation.set(generation);

        let jobs = self.startup_cover_warm_jobs();
        if jobs.is_empty() {
            return;
        }

        info!(covers = jobs.len(), "scheduled startup cover warm");
        let jobs = Rc::new(RefCell::new(jobs));
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(STARTUP_COVER_WARM_DELAY_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() == generation {
                    shell.start_startup_cover_warm_jobs(jobs, generation);
                }
            },
        );
    }
    pub(in crate::ui) fn cancel_startup_cover_warm(&self) {
        self.state.startup_cover_warm_generation.set(
            self.state
                .startup_cover_warm_generation
                .get()
                .saturating_add(1),
        );
        self.cancel_queued_warm_cover_decodes();
    }
    pub(in crate::ui) fn startup_cover_warm_jobs(&self) -> VecDeque<StartupCoverWarmJob> {
        startup_cover_background_jobs(self).into_iter().collect()
    }
    pub(in crate::ui) fn start_startup_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<StartupCoverWarmJob>>>,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_COVER_WARM_INTERVAL_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }
                if jobs.borrow().is_empty() {
                    return glib::ControlFlow::Break;
                }
                if shell.cover_warm_is_paused() {
                    return glib::ControlFlow::Continue;
                }

                let in_flight = shell
                    .state
                    .cover_decodes
                    .borrow()
                    .len()
                    .saturating_add(shell.state.cover_path_lookups.borrow().len());
                if in_flight >= COVER_PATH_LOOKUP_MAX_IN_FLIGHT {
                    return glib::ControlFlow::Continue;
                }

                let capacity = COVER_PATH_LOOKUP_MAX_IN_FLIGHT.saturating_sub(in_flight);
                let mut processed = 0;
                while processed < STARTUP_COVER_WARM_BATCH_SIZE.min(capacity) {
                    let Some(job) = jobs.borrow_mut().pop_front() else {
                        break;
                    };
                    processed += 1;
                    if shell
                        .decoded_cover_for_ref(&job.image_ref, job.fetch_size, job.size)
                        .is_some()
                        || shell.state.cover_decodes.borrow().contains(&job.key)
                        || shell.state.cover_path_lookups.borrow().contains(&job.key)
                    {
                        continue;
                    }
                    shell.start_warm_cover_path_lookup(
                        job.key,
                        job.image_ref,
                        job.fetch_size,
                        job.size,
                    );
                }

                if jobs.borrow().is_empty() {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }
    pub(in crate::ui) fn next_cover_warm_generation(&self) -> u64 {
        let generation = self.state.cover_warm_generation.get().saturating_add(1);
        self.state.cover_warm_generation.set(generation);
        generation
    }
    pub(in crate::ui) fn cancel_cover_warm(&self) {
        self.state
            .cover_warm_generation
            .set(self.state.cover_warm_generation.get().saturating_add(1));
        self.cancel_queued_warm_cover_decodes();
    }
    pub(in crate::ui) fn pause_cover_warm_for_interaction(&self) {
        self.state.cover_warm_paused_until.set(Some(
            Instant::now() + Duration::from_millis(COVER_WARM_SCROLL_PAUSE_MS),
        ));
    }
    pub(in crate::ui) fn cover_warm_is_paused(&self) -> bool {
        let Some(until) = self.state.cover_warm_paused_until.get() else {
            return false;
        };
        if Instant::now() < until {
            return true;
        }
        self.state.cover_warm_paused_until.set(None);
        false
    }
    pub(in crate::ui) fn schedule_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
        initial_delay_ms: u64,
    ) {
        let shell = Rc::clone(self);
        if initial_delay_ms == 0 {
            glib::idle_add_local_once(move || {
                if shell.state.cover_warm_generation.get() == generation {
                    shell.start_cover_warm_jobs(jobs, fetch_size, size, generation);
                }
            });
            return;
        }

        glib::timeout_add_local_once(Duration::from_millis(initial_delay_ms), move || {
            if shell.state.cover_warm_generation.get() == generation {
                shell.start_cover_warm_jobs(jobs, fetch_size, size, generation);
            }
        });
    }
    pub(in crate::ui) fn start_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(COVER_WARM_INTERVAL_MS), move || {
            if shell.state.cover_warm_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            if jobs.borrow().is_empty() {
                return glib::ControlFlow::Break;
            }
            if shell.cover_warm_is_paused() {
                return glib::ControlFlow::Continue;
            }

            let in_flight = shell
                .state
                .cover_decodes
                .borrow()
                .len()
                .saturating_add(shell.state.cover_path_lookups.borrow().len());
            if in_flight >= COVER_PATH_LOOKUP_MAX_IN_FLIGHT {
                return glib::ControlFlow::Continue;
            }

            let capacity = COVER_PATH_LOOKUP_MAX_IN_FLIGHT.saturating_sub(in_flight);
            let mut processed = 0;
            while processed < COVER_WARM_BATCH_SIZE.min(capacity) {
                let Some((key, image_ref)) = jobs.borrow_mut().pop_front() else {
                    break;
                };
                processed += 1;
                if shell.decoded_cover_has_min_size(&key, size)
                    || shell.state.cover_decodes.borrow().contains(&key)
                    || shell.state.cover_path_lookups.borrow().contains(&key)
                {
                    continue;
                }
                shell.start_warm_cover_path_lookup(key, image_ref, fetch_size, size);
            }

            if jobs.borrow().is_empty() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}
