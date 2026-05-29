use super::*;

#[derive(Clone, Copy)]
enum CoverWarmIntent {
    Route,
    Startup,
}

#[derive(Clone, Copy)]
struct CoverWarmSchedule {
    intent: CoverWarmIntent,
    generation: u64,
    initial_delay_ms: u64,
}

impl CoverWarmIntent {
    fn current_generation(self, shell: &Shell) -> u64 {
        match self {
            Self::Route => shell.state.cover_warm_generation.get(),
            Self::Startup => shell.state.startup_cover_warm_generation.get(),
        }
    }

    fn batch_size(self) -> usize {
        match self {
            Self::Route => COVER_WARM_BATCH_SIZE,
            Self::Startup => STARTUP_COVER_WARM_BATCH_SIZE,
        }
    }

    fn interval_ms(self) -> u64 {
        match self {
            Self::Route => COVER_WARM_INTERVAL_MS,
            Self::Startup => STARTUP_COVER_WARM_INTERVAL_MS,
        }
    }

    fn job_is_decoded(self, shell: &Shell, job: &CoverWarmJob) -> bool {
        match self {
            Self::Route => shell.decoded_cover_has_min_size(&job.key, job.size),
            Self::Startup => shell
                .decoded_cover_for_ref(&job.image_ref, job.fetch_size, job.size)
                .is_some(),
        }
    }
}

impl CoverWarmSchedule {
    fn new(intent: CoverWarmIntent, generation: u64, initial_delay_ms: u64) -> Self {
        Self {
            intent,
            generation,
            initial_delay_ms,
        }
    }
}

impl Shell {
    pub(in crate::ui) fn warm_cover_refs(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
    ) {
        self.schedule_route_cover_warm_refs(
            image_refs,
            fetch_size,
            size,
            COVER_WARM_INITIAL_DELAY_MS,
        );
    }

    pub(in crate::ui) fn warm_cover_refs_now(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
    ) {
        self.schedule_route_cover_warm_refs(image_refs, fetch_size, size, 0);
    }

    fn schedule_route_cover_warm_refs(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
        initial_delay_ms: u64,
    ) {
        let jobs = self.cover_warm_jobs_from_refs(image_refs, fetch_size, size);
        if jobs.is_empty() {
            return;
        }

        let generation = self.next_cover_warm_generation();
        self.schedule_cover_warm_jobs(
            Rc::new(RefCell::new(jobs)),
            CoverWarmSchedule::new(CoverWarmIntent::Route, generation, initial_delay_ms),
        );
    }

    pub(in crate::ui) fn schedule_startup_cover_warm(self: &Rc<Self>) {
        let generation = self.next_startup_cover_warm_generation();
        let jobs = self.startup_cover_warm_jobs();
        if jobs.is_empty() {
            return;
        }

        info!(covers = jobs.len(), "scheduled startup cover warm");
        self.schedule_cover_warm_jobs(
            Rc::new(RefCell::new(jobs)),
            CoverWarmSchedule::new(
                CoverWarmIntent::Startup,
                generation,
                STARTUP_COVER_WARM_DELAY_MS,
            ),
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

    fn startup_cover_warm_jobs(&self) -> VecDeque<CoverWarmJob> {
        startup_cover_background_jobs(self).into_iter().collect()
    }

    fn next_cover_warm_generation(&self) -> u64 {
        let generation = self.state.cover_warm_generation.get().saturating_add(1);
        self.state.cover_warm_generation.set(generation);
        generation
    }

    fn next_startup_cover_warm_generation(&self) -> u64 {
        let generation = self
            .state
            .startup_cover_warm_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_warm_generation.set(generation);
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

    fn cover_warm_jobs_from_refs(
        &self,
        image_refs: Vec<ImageRef>,
        fetch_size: u32,
        size: i32,
    ) -> VecDeque<CoverWarmJob> {
        let decode_size = cover_decode_size(size, fetch_size);
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
            jobs.push_back(CoverWarmJob {
                key,
                image_ref,
                fetch_size,
                size: decode_size,
            });
        }

        jobs
    }

    fn schedule_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<CoverWarmJob>>>,
        schedule: CoverWarmSchedule,
    ) {
        let shell = Rc::clone(self);
        if schedule.initial_delay_ms == 0 {
            glib::idle_add_local_once(move || {
                if schedule.intent.current_generation(&shell) == schedule.generation {
                    shell.start_cover_warm_jobs(jobs, schedule);
                }
            });
            return;
        }

        glib::timeout_add_local_once(
            Duration::from_millis(schedule.initial_delay_ms),
            move || {
                if schedule.intent.current_generation(&shell) == schedule.generation {
                    shell.start_cover_warm_jobs(jobs, schedule);
                }
            },
        );
    }

    fn start_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<CoverWarmJob>>>,
        schedule: CoverWarmSchedule,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(schedule.intent.interval_ms()),
            move || {
                if schedule.intent.current_generation(&shell) != schedule.generation {
                    return glib::ControlFlow::Break;
                }
                if jobs.borrow().is_empty() {
                    return glib::ControlFlow::Break;
                }
                if shell.cover_warm_is_paused() {
                    return glib::ControlFlow::Continue;
                }

                let in_flight = shell.cover_pipeline_in_flight();
                if in_flight >= COVER_PATH_LOOKUP_MAX_IN_FLIGHT {
                    return glib::ControlFlow::Continue;
                }

                let capacity = COVER_PATH_LOOKUP_MAX_IN_FLIGHT.saturating_sub(in_flight);
                let mut processed = 0;
                while processed < schedule.intent.batch_size().min(capacity) {
                    let Some(job) = jobs.borrow_mut().pop_front() else {
                        break;
                    };
                    processed += 1;
                    if shell.cover_warm_job_is_ready_or_in_flight(&job, schedule.intent) {
                        continue;
                    }
                    shell.start_warm_cover_path_lookup(job);
                }

                if jobs.borrow().is_empty() {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }

    fn cover_pipeline_in_flight(&self) -> usize {
        self.state
            .cover_decodes
            .borrow()
            .len()
            .saturating_add(self.state.cover_path_lookups.borrow().len())
    }

    fn cover_warm_job_is_ready_or_in_flight(
        &self,
        job: &CoverWarmJob,
        intent: CoverWarmIntent,
    ) -> bool {
        intent.job_is_decoded(self, job)
            || self.state.cover_decodes.borrow().contains(&job.key)
            || self
                .state
                .cover_path_lookups
                .borrow()
                .contains_key(&job.key)
    }

    fn start_warm_cover_path_lookup(self: &Rc<Self>, job: CoverWarmJob) {
        self.start_cached_cover_path_lookup(CoverPathLookupRequest {
            key: job.key,
            image_ref: job.image_ref,
            fetch_size: job.fetch_size,
            size: job.size,
            intent: CoverPathLookupIntent::Warm,
        });
    }
}
