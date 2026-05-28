use super::*;

impl Shell {
    pub(in crate::ui) fn record_perf_route_render(&self, route: String, elapsed: Duration) {
        if let Some(perf) = &self.state.perf {
            perf.record_route_render(route, elapsed);
        }
    }
    pub(in crate::ui) fn record_perf_cover_bind_request(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_bind_request(key);
        }
    }
    pub(in crate::ui) fn record_perf_coverless_tile(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_coverless_tile();
        }
    }
    pub(in crate::ui) fn record_perf_cover_cache_hit(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_cache_hit(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_ready(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_path_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_path_ready(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_decode_start(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_start(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_decode_ok(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_ok(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_decode_error(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_error(key);
        }
    }
    pub(in crate::ui) fn record_perf_cover_stale_ignored(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored();
        }
    }
    pub(in crate::ui) fn record_perf_cover_stale_ignored_by(&self, count: usize) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored_by(count);
        }
    }
    pub(in crate::ui) fn record_perf_cover_stale_key(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_key(key);
        }
    }
    pub(in crate::ui) fn record_perf_track_row_bind(
        &self,
        column: &'static str,
        elapsed: Duration,
    ) {
        if let Some(perf) = &self.state.perf {
            perf.record_track_row_bind(column, elapsed);
        }
    }
}
