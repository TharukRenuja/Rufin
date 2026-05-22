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
        let mut inner = self.inner.borrow_mut();
        inner.ticks = inner.ticks.saturating_add(1);
        inner.max_gap_ms = inner.max_gap_ms.max(gap_ms);
        if inner.active_scroll.is_some() {
            if gap_ms > self.options.max_gap_ms {
                inner.over_budget_ticks = inner.over_budget_ticks.saturating_add(1);
            }
            if let Some(active) = &mut inner.active_scroll {
                active.max_gap_ms = active.max_gap_ms.max(gap_ms);
                if gap_ms > self.options.max_gap_ms {
                    active.over_budget_ticks = active.over_budget_ticks.saturating_add(1);
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
            if self.options.terminal_events && gap_ms > self.options.max_gap_ms {
                println!(
                    "RUFIN_PERF_IDLE_GAP gap_ms={} elapsed_ms={}",
                    gap_ms,
                    duration_ms(self.started_at.elapsed())
                );
            }
            if gap_ms > self.options.asset_ms {
                inner.over_budget_ticks = inner.over_budget_ticks.saturating_add(1);
                inner.over_budget_idle_ticks = inner.over_budget_idle_ticks.saturating_add(1);
            }
        }
    }

    fn record_route_render(&self, route: String, elapsed: Duration) {
        let elapsed_ms = duration_ms(elapsed);
        if self.options.terminal_events {
            println!("RUFIN_PERF route_render route={route} elapsed_ms={elapsed_ms}");
        }
        self.inner
            .borrow_mut()
            .route_renders
            .push(UiPerfRouteRender { route, elapsed_ms });
    }

    fn begin_scroll(&self, route: String, scenario: UiPerfScenario) {
        let inner = self.inner.borrow();
        let active = UiPerfActiveScroll {
            route,
            scenario: scenario.name(),
            started_at: Instant::now(),
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
        self.inner.borrow_mut().active_scroll = Some(active);
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
            inner.active_scroll = Some(UiPerfActiveScroll {
                route: route.to_string(),
                scenario: "manual",
                started_at: Instant::now(),
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
    }

    fn record_cover_ready(&self, _key: &str) {
        self.inner.borrow_mut().cover_ready_events += 1;
    }

    fn record_cover_decode_ok(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_decode_ok += 1;
        if let Some(started_at) = inner.cover_pending.remove(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            inner.max_cover_latency_ms = inner.max_cover_latency_ms.max(elapsed_ms);
            if elapsed_ms > self.options.asset_ms {
                inner.over_budget_assets = inner.over_budget_assets.saturating_add(1);
            }
            inner.cover_latencies.push(UiPerfAssetLatency {
                key: key.to_string(),
                elapsed_ms,
            });
        }
    }

    fn record_cover_decode_error(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_decode_error += 1;
        inner.cover_pending.remove(key);
    }

    fn record_cover_stale_ignored(&self) {
        self.inner.borrow_mut().cover_stale_ignored += 1;
    }

    fn record_cover_stale_ignored_by(&self, count: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_stale_ignored = inner.cover_stale_ignored.saturating_add(count);
    }

    fn record_cover_stale_key(&self, key: &str) {
        self.inner.borrow_mut().cover_pending.remove(key);
    }

    fn pending_assets(&self) -> usize {
        self.inner.borrow().cover_pending.len()
    }

    fn failed(&self) -> bool {
        let inner = self.inner.borrow();
        inner.max_idle_gap_ms > self.options.asset_ms
            || inner
                .route_renders
                .iter()
                .any(|sample| sample.elapsed_ms > self.options.max_gap_ms)
            || inner
                .route_scrolls
                .iter()
                .any(|sample| sample.max_gap_ms > self.options.max_gap_ms)
            || inner.max_cover_latency_ms > self.options.asset_ms
            || !inner.cover_pending.is_empty()
            || (self.options.require_assets
                && inner.cover_bind_requests == 0
                && inner.cover_cache_hits == 0
                && inner.cover_decode_ok == 0)
            || inner.cover_decode_error > 0
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
        let mut slow_assets = inner.cover_latencies.iter().collect::<Vec<_>>();
        slow_assets.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed_ms));
        for sample in slow_assets.into_iter().take(30) {
            let _ = writeln!(
                report,
                "RUFIN_PERF_ASSET key={} elapsed_ms={}",
                sample.key, sample.elapsed_ms
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
fn startup_library_cover_refs(library: &LibrarySnapshot) -> Vec<ImageRef> {
    library
        .home_sections
        .iter()
        .flat_map(|section| {
            section
                .albums
                .iter()
                .filter_map(|album| album.image_ref.clone())
                .chain(
                    section
                        .tracks
                        .iter()
                        .filter_map(|track| track.image_ref.clone()),
                )
        })
        .chain(
            library
                .albums
                .iter()
                .filter_map(|album| album.image_ref.clone()),
        )
        .chain(
            library
                .artists
                .iter()
                .chain(library.album_artists.iter())
                .filter_map(|artist| artist.image_ref.clone()),
        )
        .chain(
            library
                .genres
                .iter()
                .filter_map(|genre| genre.image_ref.clone()),
        )
        .chain(
            library
                .playlists
                .iter()
                .filter_map(|playlist| playlist.image_ref.clone()),
        )
        .chain(
            library
                .tracks
                .iter()
                .filter_map(|track| track.image_ref.clone()),
        )
        .collect()
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
fn prime_first_cached_cover(shell: &Rc<Shell>) {
    let started_at = Instant::now();
    for (key, path) in initial_cached_grid_covers(shell) {
        if shell.state.decoded_covers.borrow().contains_key(&key) {
            continue;
        }
        match Pixbuf::from_file_at_scale(
            &path,
            GRID_COVER_SIZE as i32,
            GRID_COVER_SIZE as i32,
            true,
        ) {
            Ok(pixbuf) => shell.remember_decoded_cover(key, pixbuf),
            Err(error) => {
                debug!(%error, path = %path.display(), "failed to prime cached cover")
            }
        }
        if started_at.elapsed() >= INITIAL_COVER_PRIME_BUDGET {
            break;
        }
    }
}
fn prime_first_track_thumbnail_covers(shell: &Rc<Shell>) {
    let started_at = Instant::now();
    for (key, path) in initial_cached_track_thumbnail_covers(shell) {
        if shell.state.decoded_covers.borrow().contains_key(&key) {
            continue;
        }
        match Pixbuf::from_file_at_scale(&path, 48, 48, true) {
            Ok(pixbuf) => shell.remember_decoded_cover(key, pixbuf),
            Err(error) => {
                debug!(%error, path = %path.display(), "failed to prime cached track thumbnail")
            }
        }
        if started_at.elapsed() >= INITIAL_TRACK_THUMB_PRIME_BUDGET {
            break;
        }
    }
}
fn initial_cached_grid_covers(shell: &Rc<Shell>) -> Vec<(String, PathBuf)> {
    let (server, image_refs) = {
        let library = shell.state.library.borrow();
        let Some(server) = library.server.clone() else {
            return Vec::new();
        };
        if server.provider == "fake" {
            return Vec::new();
        }
        let image_refs = library
            .home_sections
            .iter()
            .flat_map(|section| section.albums.iter())
            .filter_map(|album| album.image_ref.clone())
            .chain(
                library
                    .albums
                    .iter()
                    .filter_map(|album| album.image_ref.clone()),
            )
            .chain(
                library
                    .artists
                    .iter()
                    .chain(library.album_artists.iter())
                    .filter_map(|artist| artist.image_ref.clone()),
            )
            .chain(
                library
                    .genres
                    .iter()
                    .filter_map(|genre| genre.image_ref.clone()),
            )
            .chain(
                library
                    .playlists
                    .iter()
                    .filter_map(|playlist| playlist.image_ref.clone()),
            )
            .collect::<Vec<_>>();
        (server, image_refs)
    };

    let mut seen = HashSet::new();
    image_refs
        .into_iter()
        .filter_map(|image_ref| {
            let tag = image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED);
            let key = image_cache_key(&server.id, &image_ref.item_id, tag, GRID_COVER_SIZE);
            if !seen.insert(key.clone()) {
                return None;
            }
            let path = shell.controller.cached_cover_path_for_key(&key)?;
            Some((key, path))
        })
        .take(INITIAL_COVER_PRIME_LIMIT)
        .collect()
}
fn initial_cached_track_thumbnail_covers(shell: &Rc<Shell>) -> Vec<(String, PathBuf)> {
    let image_refs = shell
        .state
        .library
        .borrow()
        .tracks
        .iter()
        .filter_map(|track| track.image_ref.clone())
        .take(INITIAL_TRACK_THUMB_PRIME_LIMIT)
        .collect::<Vec<_>>();

    let mut seen = HashSet::new();
    image_refs
        .into_iter()
        .filter_map(|image_ref| {
            let key = shell.cover_cache_key(&image_ref, THUMB_COVER_SIZE)?;
            if !seen.insert(key.clone()) {
                return None;
            }
            let path = shell.controller.cached_cover_path_for_key(&key)?;
            Some((key, path))
        })
        .collect()
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
