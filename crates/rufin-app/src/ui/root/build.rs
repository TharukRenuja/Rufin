use super::*;

impl UiPerfMonitor {
    pub(in crate::ui) fn new(options: UiPerfOptions) -> Self {
        let started_at = options.launch_started_at;
        Self {
            options,
            started_at,
            inner: RefCell::new(UiPerfInner::default()),
        }
    }

    pub(in crate::ui) fn started_at(&self) -> Instant {
        self.started_at
    }

    pub(in crate::ui) fn record_startup_reveal(&self) {
        let elapsed_ms = duration_ms(self.started_at.elapsed());
        self.inner.borrow_mut().startup_reveal_ms = Some(elapsed_ms);
        if self.options.terminal_events {
            println!(
                "RUFIN_ACCEPT_STARTUP launch_elapsed_ms={} budget_ms={} result={}",
                elapsed_ms,
                UI_PERF_STARTUP_REVEAL_BUDGET_MS,
                if elapsed_ms > UI_PERF_STARTUP_REVEAL_BUDGET_MS {
                    "FAIL"
                } else {
                    "PASS"
                }
            );
        }
    }

    pub(in crate::ui) fn record_tick_gap(&self, gap: Duration) {
        let gap_ms = duration_ms(gap);
        let now = Instant::now();
        let elapsed_ms = duration_ms(self.started_at.elapsed());
        let idle_gap_budget_ms = if self.options.strict_contracts {
            self.options.max_gap_ms
        } else {
            self.options.route_ms.max(self.options.asset_ms)
        };
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
            if gap_ms > idle_gap_budget_ms {
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
            if self.options.terminal_events && gap_ms > idle_gap_budget_ms {
                println!(
                    "RUFIN_PERF_IDLE_GAP gap_ms={} elapsed_ms={}",
                    gap_ms, elapsed_ms
                );
            }
            if gap_ms > idle_gap_budget_ms {
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

    pub(in crate::ui) fn record_route_render(&self, route: String, elapsed: Duration) {
        let elapsed_ms = duration_ms(elapsed);
        if self.options.terminal_events {
            println!("RUFIN_PERF route_render route={route} elapsed_ms={elapsed_ms}");
        }
        self.inner.borrow_mut().last_route_hint = Some(route.clone());
        self.inner
            .borrow_mut()
            .route_renders
            .push(UiPerfRouteRender { route, elapsed_ms });
    }

    pub(in crate::ui) fn record_route_ready(
        &self,
        route: String,
        elapsed: Duration,
        gate_wait: Duration,
    ) {
        let elapsed_ms = duration_ms(elapsed);
        let gate_wait_ms = duration_ms(gate_wait);
        let failed = self.options.strict_contracts && elapsed_ms > self.options.route_ready_ms;
        self.inner
            .borrow_mut()
            .route_ready_samples
            .push(UiPerfRouteReadySample {
                route: route.clone(),
                elapsed_ms,
                gate_wait_ms,
                failed,
            });
        if self.options.terminal_events || failed {
            println!(
                "RUFIN_ACCEPT_ROUTE_READY route={} elapsed_ms={} gate_wait_ms={} budget_ms={} result={}",
                route,
                elapsed_ms,
                gate_wait_ms,
                self.options.route_ready_ms,
                if failed { "FAIL" } else { "PASS" }
            );
        }
    }

    pub(in crate::ui) fn begin_scroll(&self, route: String, scenario: UiPerfScenario) {
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

    pub(in crate::ui) fn record_scroll_step(&self, route: &str, value: f64, max_adjustment: f64) {
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

    pub(in crate::ui) fn record_scroll_note(&self, route: &str, note: &str) {
        if self.options.terminal_events {
            println!("RUFIN_PERF scroll_note route={route} note={note}");
        }
    }

    pub(in crate::ui) fn finish_scroll(&self) {
        let mut inner = self.inner.borrow_mut();
        let Some(active) = inner.active_scroll.take() else {
            return;
        };
        self.finish_scroll_sample(&mut inner, active);
    }

    pub(in crate::ui) fn finish_scroll_sample(
        &self,
        inner: &mut UiPerfInner,
        active: UiPerfActiveScroll,
    ) {
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

    pub(in crate::ui) fn record_manual_scroll_step(
        &self,
        route: &str,
        value: f64,
        max_adjustment: f64,
    ) {
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

    pub(in crate::ui) fn record_cover_bind_request(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_bind_requests += 1;
        inner
            .cover_pending
            .entry(key.to_string())
            .or_insert_with(Instant::now);
    }

    pub(in crate::ui) fn record_coverless_tile(&self) {
        self.inner.borrow_mut().coverless_tiles += 1;
    }

    pub(in crate::ui) fn record_cover_cache_hit(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_cache_hits += 1;
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    pub(in crate::ui) fn record_cover_ready(&self, _key: &str) {
        self.inner.borrow_mut().cover_ready_events += 1;
    }

    pub(in crate::ui) fn record_cover_path_ready(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Some(started_at) = inner.cover_pending.get(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            inner.cover_path_ready.insert(key.to_string(), elapsed_ms);
        }
    }

    pub(in crate::ui) fn record_cover_decode_start(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Some(started_at) = inner.cover_pending.get(key) {
            let elapsed_ms = duration_ms(started_at.elapsed());
            inner
                .cover_decode_started
                .insert(key.to_string(), elapsed_ms);
        }
    }

    pub(in crate::ui) fn record_cover_decode_ok(&self, key: &str) {
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
            let decode_ms =
                decode_start_ms.map(|decode_start_ms| elapsed_ms.saturating_sub(decode_start_ms));
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

    pub(in crate::ui) fn record_cover_decode_error(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_decode_error += 1;
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    pub(in crate::ui) fn record_cover_stale_ignored(&self) {
        self.inner.borrow_mut().cover_stale_ignored += 1;
    }

    pub(in crate::ui) fn record_cover_stale_ignored_by(&self, count: usize) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_stale_ignored = inner.cover_stale_ignored.saturating_add(count);
    }

    pub(in crate::ui) fn record_cover_stale_key(&self, key: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.cover_pending.remove(key);
        inner.cover_path_ready.remove(key);
        inner.cover_decode_started.remove(key);
    }

    pub(in crate::ui) fn record_track_row_bind(&self, column: &'static str, elapsed: Duration) {
        let elapsed_us = duration_us(elapsed);
        let mut inner = self.inner.borrow_mut();
        let stats = inner.track_row_binds.entry(column).or_default();
        stats.samples = stats.samples.saturating_add(1);
        stats.total_us = stats.total_us.saturating_add(elapsed_us);
        stats.max_us = stats.max_us.max(elapsed_us);
        if elapsed_us > UI_PERF_TRACK_ROW_BIND_SLOW_US {
            stats.slow_samples = stats.slow_samples.saturating_add(1);
            if self.options.terminal_events {
                println!("RUFIN_PERF_TRACK_BIND_SLOW column={column} elapsed_us={elapsed_us}");
            }
        }
    }

    pub(in crate::ui) fn record_tracks_row_contract(&self, contract: UiPerfTrackRowContract) {
        let failed =
            contract.scenario != "initial" && (contract.pending > 0 || contract.missing > 0);
        {
            let mut inner = self.inner.borrow_mut();
            inner.tracks_row_contract_samples = inner.tracks_row_contract_samples.saturating_add(1);
            if failed {
                inner.tracks_row_contract_failures =
                    inner.tracks_row_contract_failures.saturating_add(1);
            }
            inner
                .track_row_contracts
                .push(UiPerfTrackRowContractSample {
                    scenario: contract.scenario,
                    visible_start: contract.visible_start,
                    visible_end: contract.visible_end,
                    ready: contract.ready,
                    coverless: contract.coverless,
                    pending: contract.pending,
                    missing: contract.missing,
                    failed,
                });
        }
        if self.options.terminal_events || failed {
            println!(
                "RUFIN_ACCEPT_TRACKS_ROW scenario={} visible_start={} visible_end={} ready={} coverless={} pending={} missing={} result={}",
                contract.scenario,
                contract.visible_start,
                contract.visible_end,
                contract.ready,
                contract.coverless,
                contract.pending,
                contract.missing,
                if failed { "FAIL" } else { "PASS" }
            );
        }
    }

    pub(in crate::ui) fn record_route_model_contract(&self, contract: UiPerfRouteModelContract) {
        let failed = !contract.complete || contract.paginated || contract.loaded < contract.total;
        {
            let mut inner = self.inner.borrow_mut();
            inner.route_model_contract_samples =
                inner.route_model_contract_samples.saturating_add(1);
            if failed {
                inner.route_model_contract_failures =
                    inner.route_model_contract_failures.saturating_add(1);
            }
            inner
                .route_model_contracts
                .push(UiPerfRouteModelContractSample {
                    route: contract.route,
                    layout: contract.layout,
                    loaded: contract.loaded,
                    total: contract.total,
                    complete: contract.complete,
                    paginated: contract.paginated,
                    failed,
                });
        }
        if self.options.terminal_events || failed {
            println!(
                "RUFIN_ACCEPT_ROUTE_MODEL route={} layout={} loaded={} total={} complete={} paginated={} result={}",
                contract.route,
                contract.layout,
                contract.loaded,
                contract.total,
                contract.complete,
                contract.paginated,
                if failed { "FAIL" } else { "PASS" }
            );
        }
    }

    pub(in crate::ui) fn record_route_visible_contract(
        &self,
        contract: UiPerfRouteVisibleContract,
    ) {
        let accepts_drag_visible_state = self.options.strict_contracts
            && contract.phase == "drag_mid"
            && ui_perf_image_route(&contract.route);
        let accepts_visible_final_state =
            contract.phase != "drag_mid" || accepts_drag_visible_state;
        let unaccounted_visible = route_visible_contract_has_unaccounted_visible_work(
            contract.expected_visible,
            contract.ready,
            contract.final_missing,
            contract.pending,
        );
        let failed = accepts_visible_final_state
            && (route_visible_contract_has_pending_work(
                contract.pending,
                contract.fallback_after_reveal,
                contract.pending_assets,
                contract.active_decodes,
                contract.queued_decodes,
                contract.path_lookups,
            ) || unaccounted_visible
                || route_visible_contract_has_rendered_work(
                    contract.expected_visible,
                    contract.rendered_expected,
                    contract.rendered_fallback,
                    contract.rendered_ready,
                    contract.rendered_final_missing,
                ));
        {
            let mut inner = self.inner.borrow_mut();
            inner.route_visible_contract_samples =
                inner.route_visible_contract_samples.saturating_add(1);
            if failed {
                inner.route_visible_contract_failures =
                    inner.route_visible_contract_failures.saturating_add(1);
            }
            inner
                .route_visible_contracts
                .push(UiPerfRouteVisibleContractSample {
                    phase: contract.phase,
                    route: contract.route.clone(),
                    layout: contract.layout,
                    visible_start: contract.visible_start,
                    visible_end: contract.visible_end,
                    expected_visible: contract.expected_visible,
                    ready: contract.ready,
                    final_missing: contract.final_missing,
                    pending: contract.pending,
                    rendered_expected: contract.rendered_expected,
                    rendered_ready: contract.rendered_ready,
                    rendered_final_missing: contract.rendered_final_missing,
                    rendered_fallback: contract.rendered_fallback,
                    fallback_after_reveal: contract.fallback_after_reveal,
                    pending_assets: contract.pending_assets,
                    active_decodes: contract.active_decodes,
                    queued_decodes: contract.queued_decodes,
                    path_lookups: contract.path_lookups,
                    pending_samples: contract.pending_samples.clone(),
                    failed,
                });
        }
        if self.options.terminal_events || failed {
            println!(
                "RUFIN_ACCEPT_ROUTE_VISIBLE phase={} route={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={} result={}",
                contract.phase,
                contract.route,
                contract.layout,
                contract.visible_start,
                contract.visible_end,
                contract.expected_visible,
                contract.ready,
                contract.final_missing,
                contract.pending,
                contract.rendered_expected,
                contract.rendered_ready,
                contract.rendered_final_missing,
                contract.rendered_fallback,
                contract.fallback_after_reveal,
                contract.pending_assets,
                contract.active_decodes,
                contract.queued_decodes,
                contract.path_lookups,
                if failed { "FAIL" } else { "PASS" }
            );
            for sample in contract.pending_samples.iter().take(12) {
                println!(
                    "RUFIN_ACCEPT_ROUTE_VISIBLE_PENDING route={} layout={} key_hash={:016x} kind={} state={} fetch_size={} decode_size={}",
                    contract.route,
                    contract.layout,
                    sample.key_hash,
                    sample.kind,
                    sample.state,
                    sample.fetch_size,
                    sample.decode_size
                );
            }
        }
    }

    pub(in crate::ui) fn record_playback_event(&self, event: &PlaybackPerfEvent) {
        if self.options.terminal_events {
            println!(
                "RUFIN_PERF_PLAYBACK phase={} server_id={} track_id={} elapsed_ms={}",
                event.phase,
                event.server_id.as_str(),
                event.track_id.as_str(),
                event.elapsed_ms
            );
        }
        self.inner
            .borrow_mut()
            .playback_events
            .push(UiPerfPlaybackEvent {
                phase: event.phase,
                server_id: event.server_id.as_str().to_string(),
                track_id: event.track_id.as_str().to_string(),
                elapsed_ms: event.elapsed_ms,
            });
    }

    pub(in crate::ui) fn pending_assets(&self) -> usize {
        self.inner.borrow().cover_pending.len()
    }

    pub(in crate::ui) fn pending_assets_for_keys(&self, keys: &HashSet<String>) -> usize {
        self.inner
            .borrow()
            .cover_pending
            .keys()
            .filter(|key| keys.contains(*key))
            .count()
    }

    pub(in crate::ui) fn failed(&self) -> bool {
        let inner = self.inner.borrow();
        let route_render_budget_ms = if self.options.strict_contracts {
            self.options.route_ready_ms
        } else {
            self.options
                .route_ms
                .max(self.options.max_gap_ms.saturating_mul(4))
        };
        let idle_budget_ms = if self.options.strict_contracts {
            self.options.max_gap_ms
        } else {
            self.options.route_ms.max(self.options.asset_ms)
        };
        inner.max_idle_gap_ms > idle_budget_ms
            || inner.startup_reveal_ms.is_some_and(|elapsed_ms| {
                self.options.strict_contracts && elapsed_ms > UI_PERF_STARTUP_REVEAL_BUDGET_MS
            })
            || inner.route_ready_samples.iter().any(|sample| sample.failed)
            || self.image_route_drag_contract_failed(&inner)
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
            || inner.route_model_contract_failures > 0
            || inner.route_visible_contract_failures > 0
            || inner.tracks_row_contract_failures > 0
    }
    pub(in crate::ui) fn scroll_sample_failed(&self, sample: &UiPerfRouteScroll) -> bool {
        let meaningful_scroll = self.options.max_gap_ms.saturating_mul(2) as f64;
        if sample.max_adjustment < meaningful_scroll {
            return false;
        }
        if self.options.strict_contracts {
            return sample.max_gap_ms > self.options.max_gap_ms || sample.over_budget_ticks > 0;
        }
        let severe_gap_ms = self.options.max_gap_ms.saturating_mul(2);
        sample.max_gap_ms > severe_gap_ms || sample.over_budget_ticks > 1
    }

    fn image_route_drag_contract_failed(&self, inner: &UiPerfInner) -> bool {
        if !self.options.strict_contracts {
            return false;
        }
        image_route_drag_failures(inner, self.options.max_gap_ms).1 > 0
    }

    pub(in crate::ui) fn report(&self) -> String {
        let status = if self.failed() { "FAIL" } else { "PASS" };
        let inner = self.inner.borrow();
        let mut report = String::new();
        let _ = writeln!(report, "RUFIN_PERF_RESULT {status}");
        let _ = writeln!(
            report,
            "RUFIN_PERF total_ms={} ticks={} max_gap_ms={} max_idle_gap_ms={} over_budget_ticks={} over_budget_idle_ticks={} budget_ms={} route_ready_budget_ms={} drag_ms={} asset_budget_ms={} require_assets={} strict_contracts={}",
            duration_ms(self.started_at.elapsed()),
            inner.ticks,
            inner.max_gap_ms,
            inner.max_idle_gap_ms,
            inner.over_budget_ticks,
            inner.over_budget_idle_ticks,
            self.options.max_gap_ms,
            self.options.route_ready_ms,
            self.options.drag_ms,
            self.options.asset_ms,
            self.options.require_assets,
            self.options.strict_contracts
        );
        let startup_result = inner.startup_reveal_ms.map_or("MISSING", |elapsed_ms| {
            if self.options.strict_contracts && elapsed_ms > UI_PERF_STARTUP_REVEAL_BUDGET_MS {
                "FAIL"
            } else {
                "PASS"
            }
        });
        let _ = writeln!(
            report,
            "RUFIN_ACCEPT_STARTUP launch_elapsed_ms={} budget_ms={} result={}",
            optional_ms(inner.startup_reveal_ms),
            UI_PERF_STARTUP_REVEAL_BUDGET_MS,
            startup_result
        );
        for sample in &inner.route_ready_samples {
            let _ = writeln!(
                report,
                "RUFIN_ACCEPT_ROUTE_READY route={} elapsed_ms={} gate_wait_ms={} budget_ms={} result={}",
                sample.route,
                sample.elapsed_ms,
                sample.gate_wait_ms,
                self.options.route_ready_ms,
                if sample.failed { "FAIL" } else { "PASS" }
            );
        }
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
            "RUFIN_ACCEPT_ROUTE_MODEL_SUMMARY samples={} failures={}",
            inner.route_model_contract_samples, inner.route_model_contract_failures
        );
        for sample in &inner.route_model_contracts {
            let _ = writeln!(
                report,
                "RUFIN_ACCEPT_ROUTE_MODEL route={} layout={} loaded={} total={} complete={} paginated={} result={}",
                sample.route,
                sample.layout,
                sample.loaded,
                sample.total,
                sample.complete,
                sample.paginated,
                if sample.failed { "FAIL" } else { "PASS" }
            );
        }
        let _ = writeln!(
            report,
            "RUFIN_ACCEPT_ROUTE_VISIBLE_SUMMARY samples={} failures={}",
            inner.route_visible_contract_samples, inner.route_visible_contract_failures
        );
        let (image_route_drag_routes, image_route_drag_failures) =
            image_route_drag_failures(&inner, self.options.max_gap_ms);
        let _ = writeln!(
            report,
            "RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes={} failures={}",
            image_route_drag_routes, image_route_drag_failures
        );
        for sample in &inner.route_visible_contracts {
            let _ = writeln!(
                report,
                "RUFIN_ACCEPT_ROUTE_VISIBLE phase={} route={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={} result={}",
                sample.phase,
                sample.route,
                sample.layout,
                sample.visible_start,
                sample.visible_end,
                sample.expected_visible,
                sample.ready,
                sample.final_missing,
                sample.pending,
                sample.rendered_expected,
                sample.rendered_ready,
                sample.rendered_final_missing,
                sample.rendered_fallback,
                sample.fallback_after_reveal,
                sample.pending_assets,
                sample.active_decodes,
                sample.queued_decodes,
                sample.path_lookups,
                if sample.failed { "FAIL" } else { "PASS" }
            );
            for pending in sample.pending_samples.iter().take(12) {
                let _ = writeln!(
                    report,
                    "RUFIN_ACCEPT_ROUTE_VISIBLE_PENDING route={} layout={} key_hash={:016x} kind={} state={} fetch_size={} decode_size={}",
                    sample.route,
                    sample.layout,
                    pending.key_hash,
                    pending.kind,
                    pending.state,
                    pending.fetch_size,
                    pending.decode_size
                );
            }
        }
        let _ = writeln!(
            report,
            "RUFIN_ACCEPT_TRACKS_ROW_SUMMARY samples={} failures={}",
            inner.tracks_row_contract_samples, inner.tracks_row_contract_failures
        );
        for event in &inner.playback_events {
            let _ = writeln!(
                report,
                "RUFIN_PERF_PLAYBACK phase={} server_id={} track_id={} elapsed_ms={}",
                event.phase, event.server_id, event.track_id, event.elapsed_ms
            );
        }
        for sample in inner
            .track_row_contracts
            .iter()
            .filter(|sample| sample.failed)
            .take(30)
        {
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
                sample.phase, sample.route, sample.scenario, sample.elapsed_ms, sample.gap_ms
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
                column, stats.samples, stats.total_us, avg_us, stats.max_us, stats.slow_samples
            );
        }
        let mut slow_assets = inner.cover_latencies.iter().collect::<Vec<_>>();
        slow_assets.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed_ms));
        for sample in slow_assets.into_iter().take(30) {
            let _ = writeln!(
                report,
                "RUFIN_PERF_ASSET key_hash={:016x} elapsed_ms={} path_ready_ms={} queue_wait_ms={} decode_ms={}",
                ui_perf_hash_label(&sample.key),
                sample.elapsed_ms,
                optional_ms(sample.path_ready_ms),
                optional_ms(sample.queue_wait_ms),
                optional_ms(sample.decode_ms)
            );
        }
        for key in inner.cover_pending.keys().take(30) {
            let _ = writeln!(
                report,
                "RUFIN_PERF_PENDING_ASSET key_hash={:016x}",
                ui_perf_hash_label(key)
            );
        }
        report
    }
}
fn image_route_drag_failures(inner: &UiPerfInner, max_gap_ms: u64) -> (usize, usize) {
    let meaningful_scroll = max_gap_ms.saturating_mul(2) as f64;
    let mut routes = HashSet::new();
    for sample in &inner.route_scrolls {
        if sample.scenario == "drag_sweep"
            && sample.max_adjustment >= meaningful_scroll
            && ui_perf_image_route(&sample.route)
        {
            routes.insert(sample.route.clone());
        }
    }

    let mut failures = 0_usize;
    for route in &routes {
        let has_clean_phase = |phase: &str| {
            inner.route_visible_contracts.iter().any(|sample| {
                &sample.route == route
                    && sample.phase == phase
                    && !route_visible_contract_has_pending_work(
                        sample.pending,
                        sample.fallback_after_reveal,
                        sample.pending_assets,
                        sample.active_decodes,
                        sample.queued_decodes,
                        sample.path_lookups,
                    )
                    && !route_visible_contract_has_unaccounted_visible_work(
                        sample.expected_visible,
                        sample.ready,
                        sample.final_missing,
                        sample.pending,
                    )
                    && !route_visible_contract_has_rendered_work(
                        sample.expected_visible,
                        sample.rendered_expected,
                        sample.rendered_fallback,
                        sample.rendered_ready,
                        sample.rendered_final_missing,
                    )
            })
        };
        if !UI_PERF_IMAGE_ROUTE_DRAG_PHASES
            .iter()
            .all(|phase| has_clean_phase(phase))
        {
            failures = failures.saturating_add(1);
        }
    }

    (routes.len(), failures)
}
pub(in crate::ui) const UI_PERF_IMAGE_ROUTE_DRAG_PHASES: [&str; 5] = [
    "ready_before_drag",
    "drag_25",
    "drag_50",
    "drag_75",
    "drag_done",
];
pub(in crate::ui) fn route_visible_contract_has_pending_work(
    pending: usize,
    fallback_after_reveal: usize,
    pending_assets: usize,
    active_decodes: usize,
    queued_decodes: usize,
    path_lookups: usize,
) -> bool {
    pending > 0
        || fallback_after_reveal > 0
        || pending_assets > 0
        || active_decodes > 0
        || queued_decodes > 0
        || path_lookups > 0
}
pub(in crate::ui) fn route_visible_contract_has_unaccounted_visible_work(
    expected_visible: usize,
    ready: usize,
    final_missing: usize,
    pending: usize,
) -> bool {
    ready.saturating_add(final_missing).saturating_add(pending) < expected_visible
}

pub(in crate::ui) fn route_visible_contract_has_rendered_work(
    expected_visible: usize,
    rendered_expected: usize,
    rendered_fallback: usize,
    rendered_ready: usize,
    rendered_final_missing: usize,
) -> bool {
    let rendered_final = rendered_ready
        .saturating_add(rendered_final_missing)
        .saturating_add(rendered_fallback);
    rendered_fallback > 0
        || rendered_final < rendered_expected
        || (expected_visible > 0 && rendered_expected == 0)
}

pub(in crate::ui) fn ui_perf_image_route(route: &str) -> bool {
    matches!(
        route,
        "Home"
            | "Favorites"
            | "Tracks"
            | "Albums"
            | "Artists"
            | "AlbumArtists"
            | "Genres"
            | "Playlists"
            | "SmartPlaylists"
    )
}
pub(in crate::ui) fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
pub(in crate::ui) fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
pub(in crate::ui) fn optional_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "none".to_string(), |value| value.to_string())
}
pub(in crate::ui) fn ui_perf_hash_label(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
pub(in crate::ui) fn default_ui_perf_output_path(prefix: &str) -> Option<PathBuf> {
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
pub(in crate::ui) fn library_has_image_refs(library: &LibrarySnapshot) -> bool {
    library.albums.iter().any(|album| album.image_ref.is_some())
        || library
            .artists
            .iter()
            .any(|artist| artist.image_ref.is_some())
        || library
            .album_artists
            .iter()
            .any(|artist| artist.image_ref.is_some())
        || library
            .genres
            .iter()
            .any(|genre| genre.image_ref.is_some() || !genre.image_refs.is_empty())
        || library
            .playlists
            .iter()
            .any(|playlist| playlist.image_ref.is_some() || !playlist.image_refs.is_empty())
        || library.tracks.iter().any(|track| track.image_ref.is_some())
}
pub(in crate::ui) struct StartupCoverTarget {
    pub(in crate::ui) image_ref: ImageRef,
    pub(in crate::ui) fetch_size: u32,
    pub(in crate::ui) size: i32,
}
pub(in crate::ui) fn startup_cover_prime_jobs(shell: &Shell) -> Vec<CoverWarmJob> {
    startup_cover_jobs_from_targets(
        shell,
        startup_cover_prime_targets(shell),
        Some(STARTUP_CACHED_COVER_PRIME_LIMIT),
    )
}
pub(in crate::ui) fn startup_cover_jobs_from_targets(
    shell: &Shell,
    targets: Vec<StartupCoverTarget>,
    limit: Option<usize>,
) -> Vec<CoverWarmJob> {
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
        jobs.push(CoverWarmJob {
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
pub(in crate::ui) fn sidebar_route_visible(settings: &AppSettings, item: SidebarRouteItem) -> bool {
    settings
        .sidebar
        .route_items
        .iter()
        .any(|entry| entry.item == item && entry.visible)
}
pub(in crate::ui) fn startup_cover_prime_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    startup_cover_prime_targets_from_snapshot(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
pub(in crate::ui) fn startup_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<StartupCoverTarget> {
    startup_home_cover_prime_targets_from_snapshot(library, settings, home_showcase_seed)
}
#[cfg(test)]
pub(in crate::ui) fn library_route_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
) -> Vec<StartupCoverTarget> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    push_startup_route_prime_targets(&mut targets, &mut seen, library, settings);
    targets
}
pub(in crate::ui) fn startup_home_cover_prime_targets(shell: &Shell) -> Vec<StartupCoverTarget> {
    startup_home_cover_prime_targets_from_snapshot(
        &shell.state.library.borrow(),
        &shell.state.settings.borrow(),
        shell.state.home_showcase_seed.get(),
    )
}
pub(in crate::ui) fn startup_home_cover_prime_targets_from_snapshot(
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) -> Vec<StartupCoverTarget> {
    let mut targets = Vec::new();
    push_startup_home_prime_targets(&mut targets, library, settings, home_showcase_seed);
    targets
}
fn push_startup_home_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    library: &LibrarySnapshot,
    settings: &AppSettings,
    home_showcase_seed: u64,
) {
    let mut section_blocks = 0_usize;
    for block in &settings.home_blocks {
        match block {
            HomeBlockKind::Showcase => {
                if let Some(album) = home::showcase_album(library, home_showcase_seed) {
                    push_startup_cover_target(
                        targets,
                        album.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
            }
            HomeBlockKind::Genres => {}
            _ => {
                if section_blocks >= STARTUP_HOME_SECTION_LIMIT {
                    continue;
                }
                let Some(kind) = block.section_kind() else {
                    continue;
                };
                let Some(section) = library
                    .home_sections
                    .iter()
                    .find(|section| section.kind == kind)
                else {
                    continue;
                };

                section_blocks = section_blocks.saturating_add(1);
                for album in section.albums.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
                    push_startup_cover_target(
                        targets,
                        album.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
                for track in section.tracks.iter().take(STARTUP_HOME_SECTION_COVER_LIMIT) {
                    push_startup_cover_target(
                        targets,
                        track.image_ref.as_ref(),
                        GRID_COVER_SIZE,
                        GRID_COVER_SIZE as i32,
                    );
                }
            }
        }
    }
}
pub(in crate::ui) fn row_layout_uses_cover(settings: &LibraryListSettings) -> bool {
    settings
        .row_fields
        .iter()
        .any(|field| matches!(field, LibraryField::Image | LibraryField::TitleMerged))
}
pub(in crate::ui) fn push_startup_cover_target(
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
#[cfg(test)]
fn push_startup_route_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    library: &LibrarySnapshot,
    settings: &AppSettings,
) {
    if sidebar_route_visible(settings, SidebarRouteItem::Tracks) {
        let list_settings = settings.library_list(LibraryListKey::Tracks);
        push_track_startup_prime_targets(
            targets,
            seen,
            library.tracks.clone(),
            &list_settings,
            false,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Albums) {
        let list_settings = settings.library_list(LibraryListKey::Albums);
        push_album_startup_prime_targets(targets, seen, library.albums.clone(), &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Artists) {
        let list_settings = settings.library_list(LibraryListKey::Artists);
        push_artist_startup_prime_targets(targets, seen, library.artists.clone(), &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::AlbumArtists) {
        let list_settings = settings.library_list(LibraryListKey::AlbumArtists);
        push_artist_startup_prime_targets(
            targets,
            seen,
            library.album_artists.clone(),
            &list_settings,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Genres) {
        let list_settings = settings.library_list(LibraryListKey::Genres);
        push_genre_startup_prime_targets(targets, seen, library, &list_settings);
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Favorites) {
        let list_settings = settings.library_list(LibraryListKey::FavoriteTracks);
        push_track_startup_prime_targets(
            targets,
            seen,
            library.favorites.clone(),
            &list_settings,
            true,
        );
    }
    if sidebar_route_visible(settings, SidebarRouteItem::Playlists) {
        let list_settings = settings.library_list(LibraryListKey::Playlists);
        push_playlist_startup_prime_targets(
            targets,
            seen,
            library.playlists.clone(),
            &list_settings,
        );
    }
}
#[cfg(test)]
fn push_track_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut tracks: Vec<Track>,
    settings: &LibraryListSettings,
    favorite_first: bool,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_tracks(&mut tracks, settings, favorite_first);
    for track in &tracks {
        push_unique_startup_cover_target(targets, seen, track.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_album_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut albums: Vec<Album>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_albums(&mut albums, settings);
    for album in &albums {
        push_unique_startup_cover_target(targets, seen, album.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_artist_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut artists: Vec<Artist>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_artists(&mut artists, settings);
    for artist in &artists {
        push_unique_startup_cover_target(
            targets,
            seen,
            artist.image_ref.as_ref(),
            fetch_size,
            size,
        );
    }
}
#[cfg(test)]
fn push_genre_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    library: &LibrarySnapshot,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    let mut genres = library.genres.clone();
    library::sort_genres(&mut genres, settings);
    for genre in &genres {
        for image_ref in &genre.image_refs {
            push_unique_startup_cover_target(targets, seen, Some(image_ref), fetch_size, size);
        }
        push_unique_startup_cover_target(targets, seen, genre.image_ref.as_ref(), fetch_size, size);
    }
}
#[cfg(test)]
fn push_playlist_startup_prime_targets(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    mut playlists: Vec<Playlist>,
    settings: &LibraryListSettings,
) {
    let Some((fetch_size, size)) = startup_route_cover_size(settings) else {
        return;
    };
    library::sort_playlists(&mut playlists, settings);
    for playlist in &playlists {
        for image_ref in &playlist.image_refs {
            push_unique_startup_cover_target(targets, seen, Some(image_ref), fetch_size, size);
        }
        push_unique_startup_cover_target(
            targets,
            seen,
            playlist.image_ref.as_ref(),
            fetch_size,
            size,
        );
    }
}
#[cfg(test)]
fn startup_route_cover_size(settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid | LibraryLayout::Detail => {
            Some((GRID_COVER_SIZE, GRID_COVER_SIZE as i32))
        }
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}
#[cfg(test)]
fn push_unique_startup_cover_target(
    targets: &mut Vec<StartupCoverTarget>,
    seen: &mut HashSet<String>,
    image_ref: Option<&ImageRef>,
    fetch_size: u32,
    size: i32,
) {
    if targets.len() >= STARTUP_CACHED_COVER_PRIME_LIMIT {
        return;
    }
    let Some(image_ref) = image_ref else {
        return;
    };
    let seen_key = startup_cover_target_dedupe_key(image_ref, fetch_size);
    if !seen.insert(seen_key) {
        return;
    }
    push_startup_cover_target(targets, Some(image_ref), fetch_size, size);
}
#[cfg(test)]
fn startup_cover_target_dedupe_key(image_ref: &ImageRef, fetch_size: u32) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        image_ref.item_id,
        image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
        fetch_size
    )
}
pub(in crate::ui) fn cover_group_slots(image_refs: &[ImageRef]) -> Vec<ImageRef> {
    let Some(first) = image_refs.first() else {
        return Vec::new();
    };
    if image_refs.len() == 1 {
        return vec![first.clone()];
    }
    (0..4)
        .filter_map(|index| image_refs.get(index % image_refs.len()).cloned())
        .collect()
}
pub(in crate::ui) fn decoded_cover_candidate_sizes(preferred_size: u32) -> Vec<u32> {
    let mut sizes = Vec::from([preferred_size]);
    if preferred_size <= THUMB_COVER_SIZE {
        sizes.extend([THUMB_COVER_SIZE, GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else if preferred_size <= GRID_COVER_SIZE {
        sizes.extend([GRID_COVER_SIZE, DETAIL_COVER_SIZE]);
    } else {
        sizes.extend([DETAIL_COVER_SIZE, GRID_COVER_SIZE]);
    }
    let mut seen = HashSet::new();
    sizes.retain(|size| seen.insert(*size));
    sizes
}
pub(in crate::ui) fn cover_decode_size(display_size: i32, fetch_size: u32) -> i32 {
    display_size.max(fetch_size as i32).max(1)
}
pub(in crate::ui) fn first_run_cover_prime_refs(library: &LibrarySnapshot) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    for section in library
        .home_sections
        .iter()
        .take(FIRST_RUN_HOME_SECTION_LIMIT)
    {
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
    for album in library.albums.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, album.image_ref.as_ref());
    }
    for artist in library.artists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, artist.image_ref.as_ref());
    }
    for artist in library.album_artists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        push_first_run_cover_ref(&mut refs, &mut seen, artist.image_ref.as_ref());
    }
    for genre in library.genres.iter().take(GRID_ROUTE_PAGE_SIZE) {
        for image_ref in &genre.image_refs {
            push_first_run_cover_ref(&mut refs, &mut seen, Some(image_ref));
        }
        push_first_run_cover_ref(&mut refs, &mut seen, genre.image_ref.as_ref());
    }
    for playlist in library.playlists.iter().take(GRID_ROUTE_PAGE_SIZE) {
        for image_ref in &playlist.image_refs {
            push_first_run_cover_ref(&mut refs, &mut seen, Some(image_ref));
        }
        push_first_run_cover_ref(&mut refs, &mut seen, playlist.image_ref.as_ref());
    }

    refs
}
pub(in crate::ui) fn push_first_run_cover_ref(
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
pub(in crate::ui) fn prefetched_explore_from_snapshot(
    snapshot: &LibrarySnapshot,
) -> Option<PrefetchedHomeSection> {
    Some(PrefetchedHomeSection {
        server_id: snapshot.server.as_ref()?.id.clone(),
        section: snapshot.prefetched_explore.clone()?,
    })
}
pub(in crate::ui) fn upsert_snapshot_home_section(
    sections: &mut Vec<HomeSection>,
    section: HomeSection,
) {
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
pub(in crate::ui) fn reset_home_section_pages(
    states: &mut HashMap<HomeSectionKind, HomeSectionState>,
) {
    states.clear();
}
