impl UiPerfMonitor {
    fn new(options: UiPerfOptions) -> Self {
        Self {
            options,
            started_at: Instant::now(),
            inner: RefCell::new(UiPerfInner::default()),
        }
    }

    fn record_tick_gap(&self, gap: Duration) {
        let gap_ms = duration_ms(gap);
        let now = Instant::now();
        let elapsed_ms = duration_ms(self.started_at.elapsed());
        let mut inner = self.inner.borrow_mut();
        inner.ticks = inner.ticks.saturating_add(1);
        inner.max_gap_ms = inner.max_gap_ms.max(gap_ms);
        let mut gap_sample = None;
        if inner.active_scroll.is_some() {
            if gap_ms > self.options.max_gap_ms {
                inner.over_budget_ticks = inner.over_budget_ticks.saturating_add(1);
            }
            if let Some(active) = &mut inner.active_scroll {
                active.max_gap_ms = active.max_gap_ms.max(gap_ms);
                if gap_ms > self.options.max_gap_ms {
                    active.over_budget_ticks = active.over_budget_ticks.saturating_add(1);
                    gap_sample = Some(UiPerfGapSample {
                        phase: "scroll",
                        route: active.route.clone(),
                        scenario: active.scenario,
                        elapsed_ms,
                        gap_ms,
                    });
                    if self.options.terminal_events {
                        println!(
                            "RUFIN_PERF_TICK_GAP gap_ms={} phase=scroll route={} scenario={}",
                            gap_ms, active.route, active.scenario
                        );
                    }
                }
            }
        } else {
            inner.max_idle_gap_ms = inner.max_idle_gap_ms.max(gap_ms);
            if gap_ms > self.options.max_gap_ms {
                gap_sample = Some(UiPerfGapSample {
                    phase: "idle",
                    route: inner
                        .last_route_hint
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    scenario: "idle",
                    elapsed_ms,
                    gap_ms,
                });
            }
            if self.options.terminal_events && gap_ms > self.options.max_gap_ms {
                println!(
                    "RUFIN_PERF_IDLE_GAP gap_ms={} elapsed_ms={}",
                    gap_ms, elapsed_ms
                );
            }
            if gap_ms > self.options.asset_ms {
                inner.over_budget_ticks = inner.over_budget_ticks.saturating_add(1);
                inner.over_budget_idle_ticks = inner.over_budget_idle_ticks.saturating_add(1);
            }
        }
        if let Some(sample) = gap_sample {
            inner.gap_samples.push(sample);
        }
        let finish_manual_scroll = inner.active_scroll.as_ref().is_some_and(|active| {
            active.scenario == "manual"
                && now.saturating_duration_since(active.last_step_at)
                    >= Duration::from_millis(UI_PERF_MANUAL_SCROLL_IDLE_MS)
        });
        if finish_manual_scroll && let Some(active) = inner.active_scroll.take() {
            self.finish_scroll_sample(&mut inner, active);
        }
    }

    fn record_route_render(&self, route: String, elapsed: Duration) {
        let elapsed_ms = duration_ms(elapsed);
        if self.options.terminal_events {
            println!("RUFIN_PERF route_render route={route} elapsed_ms={elapsed_ms}");
        }
        self.inner
            .borrow_mut()
            .last_route_hint = Some(route.clone());
        self.inner
            .borrow_mut()
            .route_renders
            .push(UiPerfRouteRender { route, elapsed_ms });
    }

    fn begin_scroll(&self, route: String, scenario: UiPerfScenario) {
        let inner = self.inner.borrow();
        let now = Instant::now();
        let active = UiPerfActiveScroll {
            route,
            scenario: scenario.name(),
            started_at: now,
            last_step_at: now,
            steps: 0,
            max_gap_ms: 0,
            over_budget_ticks: 0,
            max_adjustment: 0.0,
            min_value: f64::MAX,
            max_value: 0.0,
            covers_ready_at_start: inner.cover_ready_events,
            decodes_at_start: inner.cover_decode_ok,
        };
        drop(inner);
        let mut inner = self.inner.borrow_mut();
        inner.last_route_hint = Some(active.route.clone());
        inner.active_scroll = Some(active);
    }

    fn record_scroll_step(&self, route: &str, value: f64, max_adjustment: f64) {
        let mut inner = self.inner.borrow_mut();
        let Some(active) = &mut inner.active_scroll else {
            return;
        };
        if active.route != route {
            return;
        }
        active.steps = active.steps.saturating_add(1);
        active.max_adjustment = active.max_adjustment.max(max_adjustment);
        active.min_value = active.min_value.min(value);
        active.max_value = active.max_value.max(value);
    }

    fn record_scroll_note(&self, route: &str, note: &str) {
        if self.options.terminal_events {
            println!("RUFIN_PERF scroll_note route={route} note={note}");
        }
    }

    fn finish_scroll(&self) {
        let mut inner = self.inner.borrow_mut();
        let Some(active) = inner.active_scroll.take() else {
            return;
        };
        self.finish_scroll_sample(&mut inner, active);
    }

    fn finish_scroll_sample(&self, inner: &mut UiPerfInner, active: UiPerfActiveScroll) {
        let elapsed_ms = duration_ms(active.started_at.elapsed());
        let covers_ready = inner
            .cover_ready_events
            .saturating_sub(active.covers_ready_at_start);
        let decoded_covers = inner
            .cover_decode_ok
            .saturating_sub(active.decodes_at_start);
        let min_value = if active.steps > 0 {
            active.min_value
        } else {
            0.0
        };
        if self.options.terminal_events {
            println!(
                "RUFIN_PERF route_scroll route={} scenario={} elapsed_ms={} steps={} max_gap_ms={} over_budget_ticks={} max_adjustment={:.0} min_value={:.0} max_value={:.0} covers_ready={} decoded_covers={}",
                active.route,
                active.scenario,
                elapsed_ms,
                active.steps,
                active.max_gap_ms,
                active.over_budget_ticks,
                active.max_adjustment,
                min_value,
                active.max_value,
                covers_ready,
                decoded_covers
            );
        }
        inner.route_scrolls.push(UiPerfRouteScroll {
            route: active.route,
            scenario: active.scenario,
            elapsed_ms,
            steps: active.steps,
            max_gap_ms: active.max_gap_ms,
            over_budget_ticks: active.over_budget_ticks,
            max_adjustment: active.max_adjustment,
            min_value,
            max_value: active.max_value,
            covers_ready,
            decoded_covers,
        });
    }

    fn record_manual_scroll_step(&self, route: &str, value: f64, max_adjustment: f64) {
        if !self.options.observe_scroll {
            return;
        }

        let mut inner = self.inner.borrow_mut();
        let route_changed = inner
            .active_scroll
            .as_ref()
            .is_some_and(|active| active.route != route || active.scenario != "manual");
        if route_changed && let Some(active) = inner.active_scroll.take() {
            self.finish_scroll_sample(&mut inner, active);
        }

        if inner.active_scroll.is_none() {
            let now = Instant::now();
            inner.last_route_hint = Some(route.to_string());
            inner.active_scroll = Some(UiPerfActiveScroll {
                route: route.to_string(),
                scenario: "manual",
                started_at: now,
                last_step_at: now,
                steps: 0,
                max_gap_ms: 0,
                over_budget_ticks: 0,
                max_adjustment: 0.0,
                min_value: f64::MAX,
                max_value: 0.0,
                covers_ready_at_start: inner.cover_ready_events,
                decodes_at_start: inner.cover_decode_ok,
            });
        }

        let Some(active) = &mut inner.active_scroll else {
            return;
        };
        active.steps = active.steps.saturating_add(1);
        active.last_step_at = Instant::now();
        active.max_adjustment = active.max_adjustment.max(max_adjustment);
        active.min_value = active.min_value.min(value);
        active.max_value = active.max_value.max(value);
    }

    fn record_cover_bind_request(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_bind_requests += 1;
        inner
            .cover_pending
            .entry(key.to_string())
            .or_insert_with(Instant::now);
    }

    fn record_coverless_tile(&self) {
        self.inner.borrow_mut().coverless_tiles += 1;
    }

    fn record_cover_cache_hit(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_cache_hits += 1;
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    fn record_cover_ready(&self, _key: &str) {
        self.inner.borrow_mut().cover_ready_events += 1;
    }

    fn record_cover_path_ready(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Some(started_at) = inner.cover_pending.get(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            inner.cover_path_ready.insert(key.to_string(), elapsed_ms);
        }
    }

    fn record_cover_decode_start(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Some(started_at) = inner.cover_pending.get(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            inner
                .cover_decode_started
                .insert(key.to_string(), elapsed_ms);
        }
    }

    fn record_cover_decode_ok(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_decode_ok += 1;
        if let Some(started_at) = inner.cover_pending.remove(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            let path_ready_ms = inner.cover_path_ready.remove(key);
            let decode_start_ms = inner.cover_decode_started.remove(key);
            let queue_wait_ms = match (path_ready_ms, decode_start_ms) {
                (Some(path_ready_ms), Some(decode_start_ms)) => {
                    Some(decode_start_ms.saturating_sub(path_ready_ms))
                }
                _ => None,
            };
            let decode_ms = decode_start_ms.map(|decode_start_ms| {
                elapsed_ms.saturating_sub(decode_start_ms)
            });
            inner.max_cover_latency_ms = inner.max_cover_latency_ms.max(elapsed_ms);
            if elapsed_ms > self.options.asset_ms {
                inner.over_budget_assets = inner.over_budget_assets.saturating_add(1);
            }
            inner.cover_latencies.push(UiPerfAssetLatency {
                key: key.to_string(),
                elapsed_ms,
                path_ready_ms,
                queue_wait_ms,
                decode_ms,
            });
        } else {
            inner.cover_path_ready.remove(key);
            inner.cover_decode_started.remove(key);
        }
    }

    fn record_cover_decode_error(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_decode_error += 1;
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    fn record_cover_stale_ignored(&self) {
        self.inner.borrow_mut().cover_stale_ignored += 1;
    }

    fn record_cover_stale_ignored_by(&self, count: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_stale_ignored = inner.cover_stale_ignored.saturating_add(count);
    }

    fn record_cover_stale_key(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    fn record_track_row_bind(&self, column: &'static str, elapsed: Duration) {
        let elapsed_us = duration_us(elapsed);
        let mut inner = self.inner.borrow_mut();
        let stats = inner.track_row_binds.entry(column).or_default();
        stats.samples = stats.samples.saturating_add(1);
        stats.total_us = stats.total_us.saturating_add(elapsed_us);
        stats.max_us = stats.max_us.max(elapsed_us);
        if elapsed_us > UI_PERF_TRACK_ROW_BIND_SLOW_US {
            stats.slow_samples = stats.slow_samples.saturating_add(1);
            if self.options.terminal_events {
                println!(
                    "RUFIN_PERF_TRACK_BIND_SLOW column={column} elapsed_us={elapsed_us}"
                );
            }
        }
    }

    fn record_tracks_row_contract(
        &self,
        scenario: &'static str,
        visible_start: usize,
        visible_end: usize,
        ready: usize,
        coverless: usize,
        pending: usize,
        missing: usize,
    ) {
        let failed = pending > 0 || missing > 0;
        {
            let mut inner = self.inner.borrow_mut();
            inner.tracks_row_contract_samples =
                inner.tracks_row_contract_samples.saturating_add(1);
            if failed {
                inner.tracks_row_contract_failures =
                    inner.tracks_row_contract_failures.saturating_add(1);
            }
            inner
                .track_row_contracts
                .push(UiPerfTrackRowContractSample {
                    scenario,
                    visible_start,
                    visible_end,
                    ready,
                    coverless,
                    pending,
                    missing,
                    failed,
                });
        }
        if self.options.terminal_events || failed {
            println!(
                "RUFIN_ACCEPT_TRACKS_ROW scenario={} visible_start={} visible_end={} ready={} coverless={} pending={} missing={} result={}",
                scenario,
                visible_start,
                visible_end,
                ready,
                coverless,
                pending,
                missing,
                if failed { "FAIL" } else { "PASS" }
            );
        }
    }

    fn pending_assets(&self) -> usize {
        self.inner.borrow().cover_pending.len()
    }

    fn failed(&self) -> bool {
        let inner = self.inner.borrow();
        let route_render_budget_ms = self
            .options
            .route_ms
            .max(self.options.max_gap_ms.saturating_mul(4));
        let idle_budget_ms = self.options.route_ms.max(self.options.asset_ms);
        inner.max_idle_gap_ms > idle_budget_ms
            || inner
                .route_renders
                .iter()
                .any(|sample| sample.elapsed_ms > route_render_budget_ms)
            || inner
                .route_scrolls
                .iter()
                .any(|sample| self.scroll_sample_failed(sample))
            || !inner.cover_pending.is_empty()
            || (self.options.require_assets
                && inner.cover_bind_requests == 0
                && inner.cover_cache_hits == 0
                && inner.cover_decode_ok == 0)
            || inner.cover_decode_error > 0
            || inner.tracks_row_contract_failures > 0
    }

    fn scroll_sample_failed(&self, sample: &UiPerfRouteScroll) -> bool {
        let meaningful_scroll = self.options.max_gap_ms.saturating_mul(2) as f64;
        if sample.max_adjustment < meaningful_scroll {
            return false;
        }
        let severe_gap_ms = self.options.max_gap_ms.saturating_mul(2);
        sample.max_gap_ms > severe_gap_ms || sample.over_budget_ticks > 1
    }

    fn report(&self) -> String {
        let status = if self.failed() { "FAIL" } else { "PASS" };
        let inner = self.inner.borrow();
        let mut report = String::new();
        let _ = writeln!(report, "RUFIN_PERF_RESULT {status}");
        let _ = writeln!(
            report,
            "RUFIN_PERF total_ms={} ticks={} max_gap_ms={} max_idle_gap_ms={} over_budget_ticks={} over_budget_idle_ticks={} budget_ms={} asset_budget_ms={} require_assets={}",
            duration_ms(self.started_at.elapsed()),
            inner.ticks,
            inner.max_gap_ms,
            inner.max_idle_gap_ms,
            inner.over_budget_ticks,
            inner.over_budget_idle_ticks,
            self.options.max_gap_ms,
            self.options.asset_ms,
            self.options.require_assets
        );
        for sample in &inner.route_renders {
            let _ = writeln!(
                report,
                "RUFIN_PERF_RENDER route={} elapsed_ms={}",
                sample.route, sample.elapsed_ms
            );
        }
        for sample in &inner.route_scrolls {
            let _ = writeln!(
                report,
                "RUFIN_PERF_SCROLL route={} scenario={} elapsed_ms={} steps={} max_gap_ms={} over_budget_ticks={} max_adjustment={:.0} min_value={:.0} max_value={:.0} covers_ready={} decoded_covers={}",
                sample.route,
                sample.scenario,
                sample.elapsed_ms,
                sample.steps,
                sample.max_gap_ms,
                sample.over_budget_ticks,
                sample.max_adjustment,
                sample.min_value,
                sample.max_value,
                sample.covers_ready,
                sample.decoded_covers
            );
        }
        let _ = writeln!(
            report,
            "RUFIN_PERF_ASSETS cover_bind_requests={} decoded_cache_hits={} cover_ready_events={} cover_decode_ok={} cover_decode_error={} stale_ignored={} coverless_tiles={} max_cover_latency_ms={} over_budget_assets={} pending_assets={}",
            inner.cover_bind_requests,
            inner.cover_cache_hits,
            inner.cover_ready_events,
            inner.cover_decode_ok,
            inner.cover_decode_error,
            inner.cover_stale_ignored,
            inner.coverless_tiles,
            inner.max_cover_latency_ms,
            inner.over_budget_assets,
            inner.cover_pending.len()
        );
        let _ = writeln!(
            report,
            "RUFIN_ACCEPT_TRACKS_ROW_SUMMARY samples={} failures={}",
            inner.tracks_row_contract_samples, inner.tracks_row_contract_failures
        );
        for sample in inner.track_row_contracts.iter().filter(|sample| sample.failed).take(30) {
            let _ = writeln!(
                report,
                "RUFIN_ACCEPT_TRACKS_ROW scenario={} visible_start={} visible_end={} ready={} coverless={} pending={} missing={} result=FAIL",
                sample.scenario,
                sample.visible_start,
                sample.visible_end,
                sample.ready,
                sample.coverless,
                sample.pending,
                sample.missing
            );
        }
        let mut gap_samples = inner.gap_samples.iter().collect::<Vec<_>>();
        gap_samples.sort_by_key(|sample| std::cmp::Reverse(sample.gap_ms));
        for sample in gap_samples.into_iter().take(20) {
            let _ = writeln!(
                report,
                "RUFIN_PERF_GAP phase={} route={} scenario={} elapsed_ms={} gap_ms={}",
                sample.phase,
                sample.route,
                sample.scenario,
                sample.elapsed_ms,
                sample.gap_ms
            );
        }
        let mut bind_stats = inner.track_row_binds.iter().collect::<Vec<_>>();
        bind_stats.sort_by_key(|(column, _)| **column);
        for (column, stats) in bind_stats {
            let avg_us = if stats.samples > 0 {
                stats.total_us / stats.samples as u64
            } else {
                0
            };
            let _ = writeln!(
                report,
                "RUFIN_PERF_TRACK_BIND column={} samples={} total_us={} avg_us={} max_us={} slow_samples={}",
                column,
                stats.samples,
                stats.total_us,
                avg_us,
                stats.max_us,
                stats.slow_samples
            );
        }
        let mut slow_assets = inner.cover_latencies.iter().collect::<Vec<_>>();
        slow_assets.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed_ms));
        for sample in slow_assets.into_iter().take(30) {
            let _ = writeln!(
                report,
                "RUFIN_PERF_ASSET key={} elapsed_ms={} path_ready_ms={} queue_wait_ms={} decode_ms={}",
                sample.key,
                sample.elapsed_ms,
                optional_ms(sample.path_ready_ms),
                optional_ms(sample.queue_wait_ms),
                optional_ms(sample.decode_ms)
            );
        }
        for key in inner.cover_pending.keys().take(30) {
            let _ = writeln!(report, "RUFIN_PERF_PENDING_ASSET key={key}");
        }
        report
    }
}
fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
fn optional_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "none".to_string(), |value| value.to_string())
}
fn default_ui_perf_output_path(prefix: &str) -> Option<PathBuf> {
    let directory = PathBuf::from(".local").join("perf");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "RUFIN_PERF failed_to_create_report_dir path={} error={error}",
            directory.display()
        );
        return None;
    }
    Some(directory.join(format!("{prefix}-{}.log", std::process::id())))
}
fn library_has_image_refs(library: &LibrarySnapshot) -> bool {
    library.albums.iter().any(|album| album.image_ref.is_some())
        || library
            .artists
            .iter()
            .any(|artist| artist.image_ref.is_some())
        || library
            .album_artists
            .iter()
            .any(|artist| artist.image_ref.is_some())
        || library.genres.iter().any(|genre| genre.image_ref.is_some())
        || library
            .playlists
            .iter()
            .any(|playlist| playlist.image_ref.is_some())
        || library.tracks.iter().any(|track| track.image_ref.is_some())
}
struct StartupCoverTarget {
    image_ref: ImageRef,
    fetch_size: u32,
    size: i32,
}
fn startup_cover_prime_jobs(shell: &Shell) -> Vec<StartupCoverWarmJob> {
    startup_cover_jobs_from_targets(
        shell,
        startup_cover_prime_targets(shell),
        Some(STARTUP_CACHED_COVER_PRIME_LIMIT),
    )
}
fn startup_cover_background_jobs(shell: &Shell) -> Vec<StartupCoverWarmJob> {
    startup_cover_jobs_from_targets(shell, startup_cover_background_targets(shell), None)
}
fn startup_cover_jobs_from_targets(
    shell: &Shell,
    targets: Vec<StartupCoverTarget>,
    limit: Option<usize>,
) -> Vec<StartupCoverWarmJob> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();

    for target in targets {
        let decode_size = cover_decode_size(target.size, target.fetch_size);
        let Some(key) = shell.cover_cache_key(&target.image_ref, target.fetch_size) else {
            continue;
        };
        if !seen.insert(key.clone())
            || shell
                .decoded_cover_for_ref(&target.image_ref, target.fetch_size, decode_size)
                .is_some()
        {
            continue;
        }
        jobs.push(StartupCoverWarmJob {
            key,
            image_ref: target.image_ref,
            fetch_size: target.fetch_size,
            size: decode_size,
        });
        if limit.is_some_and(|limit| jobs.len() >= limit) {
            break;
        }
    }

    jobs
}
fn startup_artist_cover_source(
    shell: &Shell,
    album_artist: bool,
    fallback: &[Artist],
    limit: usize,
) -> Vec<Artist> {
    match shell.controller.cached_artists_page(album_artist, 0, limit) {
        Ok(page) => page.items,
        Err(error) => {
            debug!(%error, album_artist, "failed to load startup artist cover refs");
            fallback.iter().take(limit).cloned().collect()
        }
    }
}
fn sidebar_route_visible(settings: &AppSettings, item: SidebarRouteItem) -> bool {
    settings
        .sidebar
        .route_items
        .iter()
        .any(|entry| entry.item == item && entry.visible)
}
fn startup_cover_prime_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    let (
        settings,
        home_sections,
        mut tracks,
        mut favorites,
        mut albums,
        artists,
        album_artists,
        mut genres,
        mut playlists,
    ) = {
        let library = shell.state.library.borrow();
        (
            shell.state.settings.borrow().clone(),
            library.home_sections.clone(),
            library.tracks.clone(),
            library.favorites.clone(),
            library.albums.clone(),
            library.artists.clone(),
            library.album_artists.clone(),
            library.genres.clone(),
            library.playlists.clone(),
        )
    };
    let mut targets = Vec::new();

    if let Some(album) = home::showcase_album(
        &shell.state.library.borrow(),
        shell.state.home_showcase_seed.get(),
    ) {
        push_startup_cover_target(
            &mut targets,
            album.image_ref.as_ref(),
            GRID_COVER_SIZE,
            GRID_COVER_SIZE as i32,
        );
    }

    for section in &home_sections {
        for album in section.albums.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                album.image_ref.as_ref(),
                GRID_COVER_SIZE,
                GRID_COVER_SIZE as i32,
            );
        }
        for track in section.tracks.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                GRID_COVER_SIZE,
                GRID_COVER_SIZE as i32,
            );
        }
    }

    let track_settings = settings.library_list(LibraryListKey::Tracks);
    library::sort_tracks(&mut tracks, &track_settings, false);
    if let Some((fetch_size, size)) = startup_cover_sizes(&track_settings) {
        for track in &tracks {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let favorite_settings = settings.library_list(LibraryListKey::FavoriteTracks);
    library::sort_tracks(&mut favorites, &favorite_settings, false);
    if let Some((fetch_size, size)) = startup_cover_sizes(&favorite_settings) {
        for track in favorites.iter().take(TRACK_ROUTE_PAGE_SIZE) {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let album_settings = settings.library_list(LibraryListKey::Albums);
    library::sort_albums(&mut albums, &album_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&album_settings) {
        for album in &albums {
            push_startup_cover_target(
                &mut targets,
                album.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let artist_settings = settings.library_list(LibraryListKey::Artists);
    let mut startup_artists =
        startup_artist_cover_source(shell, false, &artists, STARTUP_GRID_COVER_LIMIT);
    library::sort_artists(&mut startup_artists, &artist_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&artist_settings) {
        for artist in startup_artists.iter().take(STARTUP_GRID_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                artist.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let album_artist_settings = settings.library_list(LibraryListKey::AlbumArtists);
    if sidebar_route_visible(&settings, SidebarRouteItem::AlbumArtists) {
        let mut startup_album_artists =
            startup_artist_cover_source(shell, true, &album_artists, STARTUP_GRID_COVER_LIMIT);
        library::sort_artists(&mut startup_album_artists, &album_artist_settings);
        if let Some((fetch_size, size)) = startup_cover_sizes(&album_artist_settings) {
            for artist in startup_album_artists
                .iter()
                .take(STARTUP_GRID_COVER_LIMIT)
            {
                push_startup_cover_target(
                    &mut targets,
                    artist.image_ref.as_ref(),
                    fetch_size,
                    size,
                );
            }
        }
    }

    let genre_settings = settings.library_list(LibraryListKey::Genres);
    library::sort_genres(&mut genres, &genre_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&genre_settings) {
        for genre in genres.iter().take(STARTUP_GRID_COVER_LIMIT) {
            for image_ref in genre_grid_cover_refs_from_snapshot(&shell.state.library.borrow(), genre)
            {
                push_startup_cover_target(&mut targets, Some(&image_ref), fetch_size, size);
            }
            push_startup_cover_target(
                &mut targets,
                genre.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let playlist_settings = settings.library_list(LibraryListKey::Playlists);
    library::sort_playlists(&mut playlists, &playlist_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&playlist_settings) {
        for playlist in playlists.iter().take(STARTUP_GRID_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                playlist.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    if let Some((fetch_size, size)) = startup_cover_sizes(&track_settings) {
        for track in tracks.iter().skip(STARTUP_VISIBLE_TRACK_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    targets
}
fn startup_cover_background_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    let (
        settings,
        home_sections,
        mut tracks,
        mut favorites,
        mut albums,
        mut artists,
        mut album_artists,
        mut genres,
        mut playlists,
    ) = {
        let library = shell.state.library.borrow();
        (
            shell.state.settings.borrow().clone(),
            library.home_sections.clone(),
            library.tracks.clone(),
            library.favorites.clone(),
            library.albums.clone(),
            library.artists.clone(),
            library.album_artists.clone(),
            library.genres.clone(),
            library.playlists.clone(),
        )
    };
    let mut targets = Vec::new();

    for section in &home_sections {
        for album in &section.albums {
            push_startup_cover_target(
                &mut targets,
                album.image_ref.as_ref(),
                GRID_COVER_SIZE,
                GRID_COVER_SIZE as i32,
            );
        }
        for track in &section.tracks {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                GRID_COVER_SIZE,
                GRID_COVER_SIZE as i32,
            );
        }
    }

    let track_settings = settings.library_list(LibraryListKey::Tracks);
    library::sort_tracks(&mut tracks, &track_settings, false);
    if let Some((fetch_size, size)) = startup_cover_sizes(&track_settings) {
        for track in tracks.iter().take(STARTUP_VISIBLE_TRACK_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let favorite_settings = settings.library_list(LibraryListKey::FavoriteTracks);
    library::sort_tracks(&mut favorites, &favorite_settings, false);
    if let Some((fetch_size, size)) = startup_cover_sizes(&favorite_settings) {
        for track in &favorites {
            push_startup_cover_target(
                &mut targets,
                track.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let album_settings = settings.library_list(LibraryListKey::Albums);
    library::sort_albums(&mut albums, &album_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&album_settings) {
        for album in albums.iter().take(STARTUP_GRID_COVER_LIMIT) {
            push_startup_cover_target(
                &mut targets,
                album.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let artist_settings = settings.library_list(LibraryListKey::Artists);
    library::sort_artists(&mut artists, &artist_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&artist_settings) {
        for artist in &artists {
            push_startup_cover_target(
                &mut targets,
                artist.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let album_artist_settings = settings.library_list(LibraryListKey::AlbumArtists);
    if sidebar_route_visible(&settings, SidebarRouteItem::AlbumArtists) {
        library::sort_artists(&mut album_artists, &album_artist_settings);
        if let Some((fetch_size, size)) = startup_cover_sizes(&album_artist_settings) {
            for artist in &album_artists {
                push_startup_cover_target(
                    &mut targets,
                    artist.image_ref.as_ref(),
                    fetch_size,
                    size,
                );
            }
        }
    }

    let genre_settings = settings.library_list(LibraryListKey::Genres);
    library::sort_genres(&mut genres, &genre_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&genre_settings) {
        let library = shell.state.library.borrow();
        for genre in &genres {
            for image_ref in genre_grid_cover_refs_from_snapshot(&library, genre) {
                push_startup_cover_target(&mut targets, Some(&image_ref), fetch_size, size);
            }
            push_startup_cover_target(
                &mut targets,
                genre.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    let playlist_settings = settings.library_list(LibraryListKey::Playlists);
    library::sort_playlists(&mut playlists, &playlist_settings);
    if let Some((fetch_size, size)) = startup_cover_sizes(&playlist_settings) {
        for playlist in &playlists {
            push_startup_cover_target(
                &mut targets,
                playlist.image_ref.as_ref(),
                fetch_size,
                size,
            );
        }
    }

    targets
}
fn startup_cover_sizes(settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid | LibraryLayout::Detail => {
            Some((GRID_COVER_SIZE, GRID_COVER_SIZE as i32))
        }
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
fn row_layout_uses_cover(settings: &LibraryListSettings) -> bool {
    settings
        .row_fields
        .iter()
        .any(|field| matches!(field, LibraryField::Image | LibraryField::TitleMerged))
}
fn push_startup_cover_target(
    targets: &mut Vec<StartupCoverTarget>,
    image_ref: Option<&ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    let Some(image_ref) = image_ref else {
        return;
    };
    targets.push(StartupCoverTarget {
        image_ref: image_ref.clone(),
        fetch_size,
        size,
    });
}
fn unique_cover_refs(image_refs: Vec<ImageRef>) -> Vec<ImageRef> {
    let mut unique = Vec::new();
    for image_ref in image_refs {
        if unique.len() >= 4 {
            break;
        }
        if !unique.iter().any(|existing| existing == &image_ref) {
            unique.push(image_ref);
        }
    }
    unique
}
fn decoded_cover_candidate_sizes(preferred_size: u32) -> Vec<u32> {
    let mut sizes = Vec::from([preferred_size]);
    if preferred_size <= THUMB_COVER_SIZE {
        sizes.extend([THUMB_COVER_SIZE, GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else if preferred_size <= GRID_COVER_SIZE {
        sizes.extend([GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else {
        sizes.push(DETAIL_COVER_SIZE);
    }
    let mut seen = HashSet::new();
    sizes.retain(|size| seen.insert(*size));
    sizes
}
fn cover_decode_size(display_size: i32, fetch_size: u32) -> i32 {
    display_size.max(fetch_size as i32).max(1)
}
fn first_run_cover_prime_refs(library: &LibrarySnapshot) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    for section in &library.home_sections {
        for album in section
            .albums
            .iter()
            .take(FIRST_RUN_HOME_SECTION_COVER_LIMIT)
        {
            push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
        }
        for track in section
            .tracks
            .iter()
            .take(FIRST_RUN_HOME_SECTION_COVER_LIMIT)
        {
            push_first_run_cover_ref(&mut refs, &mut seen, track.image_ref.as_ref());
        }
    }

    for track in library.tracks.iter().take(TRACK_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, track.image_ref.as_ref());
    }

    for genre in library.genres.iter().take(GRID_ROUTE_PAGE_SIZE) {
        for image_ref in genre_grid_cover_refs_from_snapshot(library, genre) {
            push_first_run_cover_ref(&mut refs, &mut seen, Some(&image_ref));
        }
        push_first_run_cover_ref(&mut refs, &mut seen, genre.image_ref.as_ref());
    }

    for album in library.albums.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
    }
    for artist in library
        .artists
        .iter()
        .chain(library.album_artists.iter())
        .take(GRID_ROUTE_PAGE_SIZE * 2)
    {
        push_first_run_cover_ref(&mut refs, &mut seen, artist.image_ref.as_ref());
    }
    for playlist in library.playlists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, playlist.image_ref.as_ref());
    }

    refs
}
fn genre_grid_cover_refs_from_snapshot(library: &LibrarySnapshot, genre: &Genre) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    for album in &library.albums {
        if album.genres.iter().any(|name| name == &genre.name) {
            push_unique_image_ref(&mut refs, album.image_ref.as_ref());
            if refs.len() >= 4 {
                return refs;
            }
        }
    }
    if !refs.is_empty() {
        return refs;
    }

    let mut seen_albums = HashSet::new();
    for track in &library.tracks {
        if track.genres.iter().any(|name| name == &genre.name)
            && !seen_albums.contains(&track.album_id)
        {
            let before = refs.len();
            push_unique_image_ref(&mut refs, track.image_ref.as_ref());
            if refs.len() > before {
                seen_albums.insert(track.album_id.clone());
            }
            if refs.len() >= 4 {
                return refs;
            }
        }
    }
    refs
}
fn push_unique_image_ref(refs: &mut Vec<ImageRef>, image_ref: Option<&ImageRef>) {
    if refs.len() >= 4 {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    if !refs.iter().any(|existing| existing == image_ref) {
        refs.push(image_ref.clone());
    }
}
fn push_first_run_cover_ref(
    refs: &mut Vec<ImageRef>,
    seen: &mut HashSet<(String, String)>,
    image_ref: Option<&ImageRef>,
) {
    if refs.len() >= FIRST_RUN_GRID_COVER_PRIME_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    let key = (
        image_ref.item_id.clone(),
        image_ref.tag.clone().unwrap_or_default(),
    );
    if seen.insert(key) {
        refs.push(image_ref.clone());
    }
}
fn prefetched_explore_from_snapshot(snapshot: &LibrarySnapshot) -> Option<PrefetchedHomeSection> {
    Some(PrefetchedHomeSection {
        server_id: snapshot.server.as_ref()?.id.clone(),
        section: snapshot.prefetched_explore.clone()?,
    })
}
fn upsert_snapshot_home_section(sections: &mut Vec<HomeSection>, section: HomeSection) {
    if let Some(existing) = sections
        .iter_mut()
        .find(|existing| existing.kind == section.kind)
    {
        *existing = section;
    } else if section.kind == HomeSectionKind::Explore {
        sections.insert(0, section);
    } else {
        sections.push(section);
    }
}
fn reset_home_section_pages(states: &mut HashMap<HomeSectionKind, HomeSectionState>) {
    states.clear();
}
