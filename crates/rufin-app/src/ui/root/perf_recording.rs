impl Shell {
    fn record_perf_route_render(&self, route: String, elapsed: Duration) {
        if let Some(perf) = &self.state.perf {
            perf.record_route_render(route, elapsed);
        }
    }
    fn record_perf_cover_bind_request(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_bind_request(key);
        }
    }
    fn record_perf_coverless_tile(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_coverless_tile();
        }
    }
    fn record_perf_cover_cache_hit(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_cache_hit(key);
        }
    }
    fn record_perf_cover_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_ready(key);
        }
    }
    fn record_perf_cover_path_ready(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_path_ready(key);
        }
    }
    fn record_perf_cover_decode_start(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_start(key);
        }
    }
    fn record_perf_cover_decode_ok(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_ok(key);
        }
    }
    fn record_perf_cover_decode_error(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_decode_error(key);
        }
    }
    fn record_perf_cover_stale_ignored(&self) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored();
        }
    }
    fn record_perf_cover_stale_ignored_by(&self, count: usize) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_ignored_by(count);
        }
    }
    fn record_perf_cover_stale_key(&self, key: &str) {
        if let Some(perf) = &self.state.perf {
            perf.record_cover_stale_key(key);
        }
    }
    fn record_perf_track_row_bind(&self, column: &'static str, elapsed: Duration) {
        if let Some(perf) = &self.state.perf {
            perf.record_track_row_bind(column, elapsed);
        }
    }
}
