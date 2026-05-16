use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
use mpris_server::{
    LoopStatus, Metadata, PlaybackStatus, Player as MprisPlayer, Time, TrackId as MprisTrackId,
};
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, DensityMode, EffectiveDensity, Genre,
    HomeSection, HomeSectionKind, ImageRef, Playlist, PlaylistId, QueueEntry, QueueSnapshot,
    RepeatMode, Route, RouteStack, SearchKind, Track, TrackSortKey, TrackTableColumn,
    TrackTableSettings, format_duration,
};
use rufin_playback::PlaybackState;
use rufin_provider::{LyricLine, Lyrics};
use rufin_store::{CachedArtistDetail, CachedGenreDetail, image_cache_key};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{AppController, ControllerEvent, LibrarySnapshot, PlaybackSnapshot};
use crate::i18n::tr;

const COMPACT_RAIL_WIDTH: i32 = 92;
const MAIN_PANEL_UNITS: i32 = 5;
const TOTAL_PANEL_UNITS: i32 = 8;
const RIGHT_PANEL_MIN_PERCENT: i32 = 10;
const RIGHT_PANEL_MAX_PERCENT: i32 = 50;
const NORMAL_SIDEBAR_WIDTH: i32 = 220;
const HOME_ALBUM_GAP: i32 = 14;
const HOME_ALBUM_MIN_SIZE: i32 = 150;
const HOME_ALBUM_TARGET_SIZE: i32 = 220;
const HOME_ALBUM_MAX_SIZE: i32 = 260;
const GRID_ROUTE_PAGE_SIZE: usize = 16;
const TRACK_ROUTE_PAGE_SIZE: usize = 64;
const GRID_COVER_SIZE: u32 = 256;
const DETAIL_COVER_SIZE: u32 = 512;
const THUMB_COVER_SIZE: u32 = 96;
const IMAGE_TAG_UNTAGGED: &str = "untagged";
const DECODED_COVER_CACHE_LIMIT: usize = 800;
const INITIAL_COVER_PRIME_LIMIT: usize = 24;
const INITIAL_COVER_PRIME_BUDGET: Duration = Duration::from_millis(300);
const FAVORITE_EMPTY_GLYPH: &str = "♡";
const FAVORITE_FILLED_GLYPH: &str = "♥";

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub fake_scale: Option<FakeScale>,
    pub smoke_exit_ms: Option<u64>,
    pub ui_perf_run: bool,
    pub ui_perf_max_gap_ms: u64,
    pub ui_perf_route_ms: u64,
    pub ui_perf_duration_ms: u64,
    pub ui_perf_asset_ms: u64,
    pub ui_perf_output: Option<PathBuf>,
}

struct AppState {
    routes: RefCell<RouteStack>,
    settings: RefCell<AppSettings>,
    density_mode: Cell<DensityMode>,
    effective_density: Cell<EffectiveDensity>,
    library: RefCell<LibrarySnapshot>,
    queue: RefCell<Option<QueueSnapshot>>,
    player: RefCell<PlaybackSnapshot>,
    lyrics: RefCell<Option<Lyrics>>,
    lyrics_track_id: RefCell<Option<rufin_core::TrackId>>,
    lyrics_rows: RefCell<Vec<LyricsRow>>,
    lyrics_scroller: RefCell<Option<gtk::ScrolledWindow>>,
    lyrics_active_index: Cell<Option<usize>>,
    mpris_player: RefCell<Option<Rc<MprisPlayer>>>,
    updating_player_controls: Cell<bool>,
    seeking_player_controls: Cell<bool>,
    seek_generation: Cell<u64>,
    split_width: Cell<i32>,
    split_position: Cell<i32>,
    card_grid_columns: Cell<usize>,
    home_section_state: RefCell<HashMap<HomeSectionKind, HomeSectionState>>,
    cover_bindings: RefCell<HashMap<String, Vec<CoverBinding>>>,
    cover_decodes: RefCell<HashSet<String>>,
    decoded_covers: RefCell<HashMap<String, Pixbuf>>,
    decoded_cover_order: RefCell<VecDeque<String>>,
    perf: Option<Rc<UiPerfMonitor>>,
}

#[derive(Clone)]
struct LyricsRow {
    row: gtk::Button,
    label: gtk::Label,
}

#[derive(Clone)]
struct CoverBinding {
    tile: ArtworkTile,
    generation: u64,
}

#[derive(Clone)]
struct ArtworkTile {
    area: gtk::DrawingArea,
    size: i32,
    seed: Rc<Cell<u32>>,
    pixbuf: Rc<RefCell<Option<Pixbuf>>>,
    generation: Rc<Cell<u64>>,
}

struct HomeSectionState {
    page_start: usize,
    page_size: usize,
}

struct PagedGridCursor {
    offset: Cell<usize>,
    total: Cell<usize>,
    loading: Cell<bool>,
}

struct PagedGridConfig {
    route: Route,
    offset: usize,
    total: usize,
    page_name: &'static str,
}

struct UiPerfOptions {
    max_gap_ms: u64,
    route_ms: u64,
    duration_ms: u64,
    asset_ms: u64,
    require_assets: bool,
    output: Option<PathBuf>,
}

struct UiPerfMonitor {
    options: UiPerfOptions,
    started_at: Instant,
    inner: RefCell<UiPerfInner>,
}

#[derive(Default)]
struct UiPerfInner {
    ticks: usize,
    max_gap_ms: u64,
    max_idle_gap_ms: u64,
    over_budget_ticks: usize,
    over_budget_idle_ticks: usize,
    route_renders: Vec<UiPerfRouteRender>,
    route_scrolls: Vec<UiPerfRouteScroll>,
    active_scroll: Option<UiPerfActiveScroll>,
    cover_pending: HashMap<String, Instant>,
    cover_latencies: Vec<UiPerfAssetLatency>,
    max_cover_latency_ms: u64,
    over_budget_assets: usize,
    coverless_tiles: usize,
    cover_bind_requests: usize,
    cover_cache_hits: usize,
    cover_ready_events: usize,
    cover_decode_ok: usize,
    cover_decode_error: usize,
    cover_stale_ignored: usize,
}

struct UiPerfActiveScroll {
    route: String,
    scenario: &'static str,
    started_at: Instant,
    steps: usize,
    max_gap_ms: u64,
    over_budget_ticks: usize,
    max_adjustment: f64,
    min_value: f64,
    max_value: f64,
    covers_ready_at_start: usize,
    decodes_at_start: usize,
}

struct UiPerfRouteRender {
    route: String,
    elapsed_ms: u64,
}

struct UiPerfRouteScroll {
    route: String,
    scenario: &'static str,
    elapsed_ms: u64,
    steps: usize,
    max_gap_ms: u64,
    over_budget_ticks: usize,
    max_adjustment: f64,
    min_value: f64,
    max_value: f64,
    covers_ready: usize,
    decoded_covers: usize,
}

struct UiPerfAssetLatency {
    key: String,
    elapsed_ms: u64,
}

#[derive(Clone, Copy)]
enum UiPerfScenario {
    HumanScroll,
    FastScroll,
    Jump,
    DragSweep,
}

impl UiPerfScenario {
    const ALL: [Self; 4] = [
        Self::HumanScroll,
        Self::FastScroll,
        Self::Jump,
        Self::DragSweep,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::HumanScroll => "human_scroll",
            Self::FastScroll => "fast_scroll",
            Self::Jump => "jump",
            Self::DragSweep => "drag_sweep",
        }
    }
}

struct GroupedDetailData {
    title: String,
    image_ref: Option<ImageRef>,
    seed: u32,
    summary: String,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    table_context: &'static str,
}

struct Shell {
    state: AppState,
    controller: AppController,
    application: adw::Application,
    window: adw::ApplicationWindow,
    normal_nav: gtk::Box,
    compact_nav: gtk::Box,
    content_split: gtk::Paned,
    route_title: gtk::Label,
    route_host: gtk::Box,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    right_panel: gtk::Box,
    player_controls: PlayerControls,
}

struct PlayerControls {
    root: gtk::Box,
    cover: ArtworkTile,
    cover_key: RefCell<Option<String>>,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    stop_button: gtk::Button,
    previous_button: gtk::Button,
    play_button: gtk::Button,
    play_icon: gtk::Image,
    next_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    favorite_button: gtk::Button,
    elapsed: gtk::Label,
    progress: gtk::Scale,
    duration: gtk::Label,
    mute_button: gtk::Button,
    mute_icon: gtk::Image,
    volume: gtk::Scale,
}

pub fn build(app: &adw::Application, options: AppOptions) {
    install_css();

    let loaded_at = std::time::Instant::now();
    let (controller, events, library, queue, player) = AppController::bootstrap(options.fake_scale);
    let settings = controller.load_settings();
    info!(
        albums = library.albums.len(),
        tracks = library.tracks.len(),
        first_run = library.first_run,
        elapsed_ms = loaded_at.elapsed().as_millis(),
        "loaded cached music library snapshot"
    );
    let perf_requires_assets =
        options.ui_perf_run && options.fake_scale.is_none() && library_has_image_refs(&library);

    let state = AppState {
        routes: RefCell::new(RouteStack::new(Route::Home)),
        settings: RefCell::new(settings.clone()),
        density_mode: Cell::new(settings.density_mode),
        effective_density: Cell::new(EffectiveDensity::Compact),
        library: RefCell::new(library),
        queue: RefCell::new(queue),
        player: RefCell::new(player),
        lyrics: RefCell::new(None),
        lyrics_track_id: RefCell::new(None),
        lyrics_rows: RefCell::new(Vec::new()),
        lyrics_scroller: RefCell::new(None),
        lyrics_active_index: Cell::new(None),
        mpris_player: RefCell::new(None),
        updating_player_controls: Cell::new(false),
        seeking_player_controls: Cell::new(false),
        seek_generation: Cell::new(0),
        split_width: Cell::new(0),
        split_position: Cell::new(0),
        card_grid_columns: Cell::new(0),
        home_section_state: RefCell::new(HashMap::new()),
        cover_bindings: RefCell::new(HashMap::new()),
        cover_decodes: RefCell::new(HashSet::new()),
        decoded_covers: RefCell::new(HashMap::new()),
        decoded_cover_order: RefCell::new(VecDeque::new()),
        perf: options.ui_perf_run.then(|| {
            Rc::new(UiPerfMonitor::new(UiPerfOptions {
                max_gap_ms: options.ui_perf_max_gap_ms,
                route_ms: options.ui_perf_route_ms,
                duration_ms: options.ui_perf_duration_ms.max(15_000),
                asset_ms: options.ui_perf_asset_ms,
                require_assets: perf_requires_assets,
                output: options
                    .ui_perf_output
                    .clone()
                    .or_else(default_ui_perf_output_path),
            }))
        }),
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rufin")
        .build();
    if let (Some(width), Some(height)) = (settings.window_width, settings.window_height)
        && width >= 480
        && height >= 360
    {
        window.set_default_size(width, height);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("app-root");

    let upper = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    upper.set_vexpand(true);

    let normal_nav = gtk::Box::new(gtk::Orientation::Vertical, 10);
    normal_nav.add_css_class("wide-sidebar");
    normal_nav.set_width_request(NORMAL_SIDEBAR_WIDTH);

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 8);
    compact_nav.add_css_class("compact-rail");
    compact_nav.set_width_request(COMPACT_RAIL_WIDTH);

    let main_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_area.add_css_class("main-area");
    main_area.set_hexpand(true);
    main_area.set_vexpand(true);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("route-header");
    header.set_valign(gtk::Align::Center);

    let back_button = icon_button("go-previous-symbolic", "Back");
    let forward_button = icon_button("go-next-symbolic", "Forward");
    let route_title = gtk::Label::new(None);
    route_title.add_css_class("route-title");
    route_title.set_xalign(0.0);
    route_title.set_valign(gtk::Align::Center);
    route_title.set_hexpand(true);
    let settings_button = icon_button("emblem-system-symbolic", "Settings");

    header.append(&back_button);
    header.append(&forward_button);
    header.append(&route_title);
    header.append(&settings_button);

    let route_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);

    main_area.append(&header);
    main_area.append(&route_host);

    let right_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right_panel.add_css_class("right-panel");
    right_panel.set_vexpand(true);

    let content_split = gtk::Paned::new(gtk::Orientation::Horizontal);
    content_split.set_hexpand(true);
    content_split.set_vexpand(true);
    content_split.set_wide_handle(false);
    content_split.set_resize_start_child(true);
    content_split.set_resize_end_child(true);
    content_split.set_shrink_start_child(true);
    content_split.set_shrink_end_child(true);
    content_split.set_start_child(Some(&main_area));
    content_split.set_end_child(Some(&right_panel));
    let player_controls = build_bottom_player();

    upper.append(&normal_nav);
    upper.append(&compact_nav);
    upper.append(&content_split);
    root.append(&upper);
    root.append(&player_controls.root);

    window.set_content(Some(&root));

    let shell = Rc::new(Shell {
        state,
        controller,
        application: app.clone(),
        window,
        normal_nav,
        compact_nav,
        content_split,
        route_title,
        route_host,
        back_button,
        forward_button,
        right_panel,
        player_controls,
    });

    build_normal_navigation(&shell);
    build_compact_navigation(&shell);
    connect_shell_actions(&shell, settings_button);
    connect_player_controls(&shell);
    install_mpris(&shell);
    shell.update_density();
    prime_first_cached_cover(&shell);
    shell.render_current_route();
    shell.render_queue_panel();
    shell.update_bottom_player();
    install_event_pump(&shell, events);

    if options.fake_scale.is_none() && !options.ui_perf_run {
        schedule_startup_sync(&shell);
    }

    if let Some(delay_ms) = options.smoke_exit_ms {
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            info!(delay_ms, "smoke exit requested");
            app.quit();
        });
    }

    shell.window.present();

    if options.ui_perf_run {
        start_ui_perf_run(&shell, app);
    }
}

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
                    println!(
                        "RUFIN_PERF_TICK_GAP gap_ms={} phase=scroll route={} scenario={}",
                        gap_ms, active.route, active.scenario
                    );
                }
            }
        } else {
            inner.max_idle_gap_ms = inner.max_idle_gap_ms.max(gap_ms);
            if gap_ms > self.options.max_gap_ms {
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
        println!("RUFIN_PERF route_render route={route} elapsed_ms={elapsed_ms}");
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
        println!("RUFIN_PERF scroll_note route={route} note={note}");
    }

    fn finish_scroll(&self) {
        let mut inner = self.inner.borrow_mut();
        let Some(active) = inner.active_scroll.take() else {
            return;
        };
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

fn default_ui_perf_output_path() -> Option<PathBuf> {
    let directory = PathBuf::from(".local").join("perf");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "RUFIN_PERF failed_to_create_report_dir path={} error={error}",
            directory.display()
        );
        return None;
    }
    Some(directory.join(format!("rufin-ui-perf-{}.log", std::process::id())))
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

impl Shell {
    fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        self.state.routes.borrow_mut().navigate(route);
        self.render_current_route();
    }

    fn go_back(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.render_current_route();
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.render_current_route();
        }
    }

    fn set_density_mode(self: &Rc<Self>, density_mode: DensityMode) {
        self.state.density_mode.set(density_mode);
        {
            let mut settings = self.state.settings.borrow_mut();
            settings.density_mode = density_mode;
            if let Err(error) = self.controller.save_settings(&settings) {
                warn!(%error, "failed to save density setting");
            }
        }
        self.update_density();
    }

    fn save_window_geometry(&self) {
        let width = self.window.width();
        let height = self.window.height();
        if width < 480 || height < 360 {
            return;
        }
        let mut settings = self.state.settings.borrow_mut();
        if settings.window_width == Some(width) && settings.window_height == Some(height) {
            return;
        }
        settings.window_width = Some(width);
        settings.window_height = Some(height);
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save window geometry");
        }
    }

    fn update_track_table_settings(&self, update: impl FnOnce(&mut TrackTableSettings)) {
        let mut settings = self.state.settings.borrow_mut();
        update(&mut settings.track_table);
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save track table settings");
        }
    }

    fn update_density(self: &Rc<Self>) {
        let width = self.window.width().max(1);
        let next = self.state.density_mode.get().resolve(width);
        let previous = self.state.effective_density.replace(next);
        self.normal_nav
            .set_visible(next == EffectiveDensity::Normal);
        self.compact_nav
            .set_visible(next == EffectiveDensity::Compact);
        self.update_content_split();

        if next != previous {
            debug!(?next, width, "effective density changed");
            self.render_current_route();
        } else if route_uses_responsive_cards(self.state.routes.borrow().current()) {
            self.render_current_route();
        }
    }

    fn update_content_split(&self) -> bool {
        let split_width = self.content_split.width();
        if split_width <= 1 {
            return false;
        }

        let previous_width = self.state.split_width.replace(split_width);
        let current_position = self.content_split.position();
        let default_position = split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS;
        let target_position =
            if previous_width > 1 && previous_width != split_width && current_position > 1 {
                current_position * split_width / previous_width
            } else if current_position > 1 {
                current_position
            } else {
                default_position
            };
        let position = clamp_content_split_position(split_width, target_position);
        let position_changed = self.state.split_position.replace(position) != position;

        if current_position != position {
            debug!(split_width, position, "update content split");
            self.content_split.set_position(position);
        }

        previous_width != split_width || position_changed
    }

    fn update_mpris_player(&self) {
        let Some(player) = self.state.mpris_player.borrow().as_ref().cloned() else {
            return;
        };
        let snapshot = self.state.player.borrow().clone();
        let metadata = self.mpris_metadata(&snapshot);
        let playback_status = match snapshot.state {
            PlaybackState::Playing | PlaybackState::Buffering => PlaybackStatus::Playing,
            PlaybackState::Paused => PlaybackStatus::Paused,
            PlaybackState::Stopped => PlaybackStatus::Stopped,
        };
        let loop_status = match snapshot.repeat_mode {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::One => LoopStatus::Track,
            RepeatMode::All => LoopStatus::Playlist,
        };
        let has_current = snapshot.current.is_some();
        let position = Time::from_secs(i64::from(snapshot.position_seconds));
        let volume = snapshot.volume.clamp(0.0, 1.0);

        glib::spawn_future_local(async move {
            let _updated = player.set_playback_status(playback_status).await;
            let _updated = player.set_loop_status(loop_status).await;
            let _updated = player.set_shuffle(snapshot.shuffle_enabled).await;
            let _updated = player.set_metadata(metadata).await;
            let _updated = player.set_volume(volume).await;
            let _updated = player.set_can_play(has_current).await;
            let _updated = player.set_can_pause(has_current).await;
            let _updated = player.set_can_seek(has_current).await;
            let _updated = player.set_can_go_next(has_current).await;
            let _updated = player.set_can_go_previous(has_current).await;
            player.set_position(position);
        });
    }

    fn mpris_metadata(&self, snapshot: &PlaybackSnapshot) -> Metadata {
        let Some(entry) = snapshot.current.as_ref() else {
            return Metadata::builder().trackid(MprisTrackId::NO_TRACK).build();
        };
        let mut builder = Metadata::builder()
            .trackid(mpris_track_id(entry.track_id.as_str()))
            .title(entry.title.clone())
            .artist([entry.artist.clone()])
            .album(entry.album.clone())
            .length(Time::from_secs(i64::from(entry.duration_seconds)));
        if let Some(art_url) = self.current_art_url(entry) {
            builder = builder.art_url(art_url);
        }
        builder.build()
    }

    fn current_art_url(&self, entry: &QueueEntry) -> Option<String> {
        let server = self.state.library.borrow().server.as_ref()?.clone();
        let image_ref = entry.image_ref.as_ref()?;
        let key = image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            THUMB_COVER_SIZE,
        );
        let path = self.controller.cached_cover_path_for_key(&key)?;
        glib::filename_to_uri(path, None)
            .ok()
            .map(|uri| uri.to_string())
    }

    fn notify_now_playing(&self, snapshot: &PlaybackSnapshot) {
        let settings = self.state.settings.borrow().clone();
        if !settings.notifications_enabled || settings.private_mode {
            return;
        }
        if !matches!(
            snapshot.state,
            PlaybackState::Playing | PlaybackState::Buffering
        ) {
            return;
        }
        let Some(entry) = snapshot.current.as_ref() else {
            return;
        };
        let notification = gio::Notification::new(&entry.title);
        notification.set_body(Some(&format!("{} - {}", entry.artist, entry.album)));
        self.application
            .send_notification(Some("now-playing"), &notification);
    }

    fn update_lyrics_highlight(&self) {
        let active_index = self.state.lyrics.borrow().as_ref().and_then(|lyrics| {
            active_lyrics_line_index(lyrics.lines.as_slice(), self.current_position_millis())
        });
        let previous_index = self.state.lyrics_active_index.replace(active_index);
        let rows = self.state.lyrics_rows.borrow();
        for (index, row) in rows.iter().enumerate() {
            let active = Some(index) == active_index;
            if active {
                row.row.add_css_class("lyrics-row-active");
                row.label.add_css_class("lyrics-line-active");
            } else {
                row.row.remove_css_class("lyrics-row-active");
                row.label.remove_css_class("lyrics-line-active");
            }
        }

        if active_index != previous_index
            && let (Some(index), Some(scroller)) =
                (active_index, self.state.lyrics_scroller.borrow().clone())
            && let Some(row) = rows.get(index)
        {
            scroll_lyrics_row_into_view(scroller, row.row.clone().upcast());
        }
    }

    fn current_position_millis(&self) -> u64 {
        u64::from(self.state.player.borrow().position_seconds) * 1_000
    }

    fn render_current_route(self: &Rc<Self>) {
        let render_started = Instant::now();
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let first_run = self.state.library.borrow().first_run;
        if first_run {
            let route_name = "FirstRun".to_string();
            self.route_title.set_text(&tr("Add Jellyfin Server"));
            self.back_button.set_sensitive(false);
            self.forward_button.set_sensitive(false);
            let view = self.add_server_view();
            self.route_host.append(&view);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.route_title.set_text(&tr(route.title()));
        self.back_button
            .set_sensitive(self.state.routes.borrow().can_back());
        self.forward_button
            .set_sensitive(self.state.routes.borrow().can_forward());

        let view = match route {
            Route::Home => self.home_view(),
            Route::Albums => self.albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.tracks_route_view(&tr("Tracks")),
            Route::Settings => self.settings_view(),
            Route::Favorites => {
                let favorites = self.state.library.borrow().favorites.clone();
                self.tracks_view(favorites, &tr("Favorites"))
            }
            Route::Artists => self.artist_list_view(false, &tr("Artists")),
            Route::ArtistDetail(artist_id) => self.artist_detail_view(artist_id),
            Route::AlbumArtists => self.artist_list_view(true, &tr("Album Artists")),
            Route::Genres => self.genre_list_view(&tr("Genres")),
            Route::GenreDetail(genre_id) => self.genre_detail_view(genre_id),
            Route::Playlists => self.playlist_list_view(&tr("Playlists")),
            Route::PlaylistDetail(playlist_id) => self.playlist_detail_view(playlist_id),
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };

        self.route_host.append(&view);
        self.record_perf_route_render(route_name, render_started.elapsed());
    }

    fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(42);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(48);
        wrapper.set_margin_end(48);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);

        let heading = gtk::Label::new(Some(&tr("Add Jellyfin Server")));
        heading.add_css_class("detail-title");
        heading.set_xalign(0.0);
        let subtitle = gtk::Label::new(Some(&tr(
            "Tokens are saved in native Secret Service. Cached library metadata is saved in SQLite.",
        )));
        subtitle.add_css_class("muted");
        subtitle.set_wrap(true);
        subtitle.set_xalign(0.0);

        let url = gtk::Entry::new();
        url.set_placeholder_text(Some(&tr("Server URL")));
        url.set_text("http://");
        let username = gtk::Entry::new();
        username.set_placeholder_text(Some(&tr("Username")));
        let password = gtk::PasswordEntry::new();
        password.set_placeholder_text(Some(&tr("Password")));
        let trust = gtk::Switch::new();
        trust.set_active(false);
        let trust_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let trust_label = gtk::Label::new(Some(&tr("Trust invalid certificate for this server")));
        trust_label.set_xalign(0.0);
        trust_label.set_hexpand(true);
        trust_label.set_wrap(true);
        trust_row.append(&trust_label);
        trust_row.append(&trust);

        let status = gtk::Label::new(Some(&self.state.library.borrow().sync_status));
        status.add_css_class("muted");
        status.set_wrap(true);
        status.set_xalign(0.0);
        if let Some(error) = &self.state.library.borrow().last_error {
            status.set_text(error);
            status.add_css_class("error-text");
        }

        let login = text_button("network-server-symbolic", "Connect");
        let controller = self.controller.clone();
        let url_input = url.clone();
        let username_input = username.clone();
        let password_input = password.clone();
        let trust_input = trust.clone();
        login.connect_clicked(move |_| {
            controller.login(
                url_input.text().to_string(),
                username_input.text().to_string(),
                password_input.text().to_string(),
                trust_input.is_active(),
            );
        });

        wrapper.append(&heading);
        wrapper.append(&subtitle);
        wrapper.append(&url);
        wrapper.append(&username);
        wrapper.append(&password);
        wrapper.append(&trust_row);
        wrapper.append(&login);
        wrapper.append(&status);
        wrapper.upcast()
    }

    fn home_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 26);
        content.add_css_class("route-content");
        content.set_margin_top(24);
        content.set_margin_bottom(36);
        content.set_margin_start(28);
        content.set_margin_end(28);

        for section in &self.state.library.borrow().home_sections {
            content.append(&self.home_album_section(section));
        }

        if self.state.library.borrow().home_sections.is_empty() {
            content.append(&self.placeholder_view(
                "Home",
                "Cached library data will appear here as sync pages finish.",
            ));
        }

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn home_album_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 12);
        section.set_hexpand(true);
        let section_kind = section_data.kind;
        let albums = section_data.albums.clone();

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        header.append(&heading);

        let previous = icon_button("go-previous-symbolic", "Previous page");
        let next = icon_button("go-next-symbolic", "Next page");
        header.append(&previous);
        header.append(&next);
        section.append(&header);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, HOME_ALBUM_GAP);
        row.add_css_class("album-strip");
        row.set_hexpand(true);
        section.append(&row);

        let shell = Rc::clone(self);
        previous.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            state.page_start = state.page_start.saturating_sub(state.page_size);
            drop(states);
            shell.render_current_route();
        });

        let shell = Rc::clone(self);
        let albums_for_next = albums.clone();
        next.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            let next_page = state.page_start.saturating_add(state.page_size);
            if next_page < albums_for_next.len() {
                state.page_start = next_page;
            }
            drop(states);
            shell.render_current_route();
        });

        render_home_album_page(self, &row, &previous, &next, section_kind, &albums);
        section.upcast()
    }

    fn albums_view(self: &Rc<Self>) -> gtk::Widget {
        let page = self
            .controller
            .cached_albums_page(0, GRID_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached albums page");
                let albums = self
                    .state
                    .library
                    .borrow()
                    .albums
                    .iter()
                    .take(GRID_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(albums, self.state.library.borrow().albums.len())
            });
        let model = album_model(&page.items);
        let load_next = self.grid_loader(
            model.clone(),
            PagedGridConfig {
                route: Route::Albums,
                offset: page.items.len(),
                total: page.total,
                page_name: "albums",
            },
            |controller, offset, limit| controller.cached_albums_page(offset, limit),
            append_albums_to_model,
        );
        self.media_grid_view(
            &tr("Albums"),
            page.items.is_empty(),
            "Cached albums will appear here after the background sync finishes.",
            self.album_cards_grid_for_model(model),
            Some(load_next),
        )
    }

    fn album_detail_view(self: &Rc<Self>, album_id: AlbumId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_album_detail(&album_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let album = library
                    .albums
                    .iter()
                    .find(|album| album.id.as_str() == album_id.as_str())
                    .cloned()?;
                let tracks = library
                    .tracks
                    .iter()
                    .filter(|track| track.album_id.as_str() == album_id.as_str())
                    .cloned()
                    .collect::<Vec<_>>();
                Some((album, tracks))
            });
        let Some((album, tracks)) = detail else {
            return self.placeholder_view("Album", "The selected cached album was not found.");
        };

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.add_css_class("route-content");
        content.set_margin_top(28);
        content.set_margin_bottom(36);
        content.set_margin_start(32);
        content.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            188,
            DETAIL_COVER_SIZE,
        );
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let kind = gtk::Label::new(Some(&tr("Album")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.0);
        let facts = gtk::Label::new(Some(&format!(
            "{} • {} {} • {}",
            album.year,
            album.track_count,
            tr("tracks"),
            format_duration(album.duration_seconds)
        )));
        facts.add_css_class("muted");
        facts.set_xalign(0.0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let play_album = text_button("media-playback-start-symbolic", "Play");
        let controller = self.controller.clone();
        let album_tracks = tracks.clone();
        play_album.connect_clicked(move |_| controller.play_tracks_now(album_tracks.clone()));
        actions.append(&play_album);

        let play_next = text_button("media-skip-forward-symbolic", "Play next");
        let controller = self.controller.clone();
        let next_tracks = tracks.clone();
        play_next.connect_clicked(move |_| {
            for track in next_tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        });
        actions.append(&play_next);

        let (favorite, favorite_glyph) = favorite_text_button("Favorite");
        set_favorite_text_button_active(&favorite, &favorite_glyph, album.favorite);
        let controller = self.controller.clone();
        let favorite_album = album.clone();
        favorite.connect_clicked(move |_| controller.toggle_album_favorite(favorite_album.clone()));
        actions.append(&favorite);

        metadata.append(&kind);
        metadata.append(&title);
        metadata.append(&artist);
        metadata.append(&actions);
        metadata.append(&facts);
        header.append(&metadata);
        content.append(&header);

        let table = self.tracks_table(tracks, "album-detail");
        content.append(&table);

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn tracks_view(self: &Rc<Self>, tracks: Vec<Track>, title: &str) -> gtk::Widget {
        self.tracks_view_with_paging(tracks, title, None)
    }

    fn tracks_route_view(self: &Rc<Self>, title: &str) -> gtk::Widget {
        let page = self
            .controller
            .cached_tracks_page(0, TRACK_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached tracks page");
                let tracks = self
                    .state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .take(TRACK_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(tracks, self.state.library.borrow().tracks.len())
            });
        let offset = page.items.len();
        let total = page.total;
        self.tracks_view_with_paging(page.items, title, Some((offset, total)))
    }

    fn tracks_view_with_paging(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        title: &str,
        paging: Option<(usize, usize)>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);
        wrapper.append(&self.tracks_table_with_paging(tracks, title, paging));
        wrapper.upcast()
    }

    fn tracks_table(self: &Rc<Self>, tracks: Vec<Track>, context: &str) -> gtk::Widget {
        self.tracks_table_with_paging(tracks, context, None)
    }

    fn tracks_table_with_paging(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
        paging: Option<(usize, usize)>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_vexpand(true);
        let tracks = Rc::new(RefCell::new(tracks));

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        toolbar.append(&search);

        let settings = self.state.settings.borrow().track_table.clone();
        let sort_button = gtk::Button::new();
        sort_button.add_css_class("flat");
        set_track_sort_button_content(&sort_button, &settings);
        toolbar.append(&sort_button);

        let configure = gtk::MenuButton::new();
        configure.add_css_class("flat");
        configure.set_icon_name("emblem-system-symbolic");
        configure.set_tooltip_text(Some(&tr("Configure columns")));
        toolbar.append(&configure);
        wrapper.append(&toolbar);

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        populate_track_model(&model, &tracks.borrow(), &settings, "");
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let table = gtk::ColumnView::new(Some(selection));
        table.add_css_class("track-table");
        table.set_vexpand(true);
        table.set_hexpand(true);
        table.set_single_click_activate(false);
        set_track_table_columns(self, &table, &settings);

        let controller = self.controller.clone();
        let model_for_activate = model.clone();
        table.connect_activate(move |_, position| {
            let Some(item) = model_for_activate.item(position) else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            controller.play_now(boxed.borrow::<Track>().clone());
        });

        let model_for_search = model.clone();
        let tracks_for_search = Rc::clone(&tracks);
        let shell = Rc::clone(self);
        search.connect_search_changed(move |entry| {
            let settings = shell.state.settings.borrow().track_table.clone();
            let tracks = tracks_for_search.borrow();
            populate_track_model(&model_for_search, &tracks, &settings, entry.text().as_str());
        });

        let model_for_sort = model.clone();
        let tracks_for_sort = Rc::clone(&tracks);
        let shell = Rc::clone(self);
        let search_for_sort = search.clone();
        sort_button.connect_clicked(move |button| {
            let mut settings = shell.state.settings.borrow().track_table.clone();
            settings.descending = !settings.descending;
            shell.update_track_table_settings(|stored| *stored = settings.clone());
            let tracks = tracks_for_sort.borrow();
            populate_track_model(
                &model_for_sort,
                &tracks,
                &settings,
                search_for_sort.text().as_str(),
            );
            set_track_sort_button_content(button, &settings);
        });

        configure.set_popover(Some(&self.track_table_popover(
            &table,
            &model,
            Rc::clone(&tracks),
            &search,
            &sort_button,
        )));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&table));
        if let Some((offset, total)) = paging {
            let cursor = Rc::new(PagedGridCursor {
                offset: Cell::new(offset),
                total: Cell::new(total),
                loading: Cell::new(false),
            });
            let shell = Rc::clone(self);
            let tracks_for_page = Rc::clone(&tracks);
            let model_for_page = model.clone();
            let search_for_page = search.clone();
            let load_next = Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Tracks) {
                    return;
                }
                let offset = cursor.offset.get();
                match shell
                    .controller
                    .cached_tracks_page(offset, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        let items = page.items;
                        tracks_for_page.borrow_mut().extend(items.iter().cloned());
                        if search_for_page.text().trim().is_empty() {
                            append_tracks_to_model(&model_for_page, items);
                        } else {
                            let settings = shell.state.settings.borrow().track_table.clone();
                            let tracks = tracks_for_page.borrow();
                            populate_track_model(
                                &model_for_page,
                                &tracks,
                                &settings,
                                search_for_page.text().as_str(),
                            );
                        }
                        finish_grid_page(&cursor, offset, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to append cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            });
            connect_paged_grid_loader(&scroller, load_next);
        }
        wrapper.append(&scroller);
        wrapper.set_widget_name(context);
        wrapper.upcast()
    }

    fn artist_list_view(self: &Rc<Self>, album_artist: bool, title: &str) -> gtk::Widget {
        let page = self
            .controller
            .cached_artists_page(album_artist, 0, GRID_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, album_artist, "failed to load cached artists page");
                let library = self.state.library.borrow();
                let fallback = if album_artist {
                    &library.album_artists
                } else {
                    &library.artists
                };
                let artists = fallback
                    .iter()
                    .take(GRID_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(artists, fallback.len())
            });
        let model = artist_model(&page.items);
        let route = if album_artist {
            Route::AlbumArtists
        } else {
            Route::Artists
        };
        let load_next = self.grid_loader(
            model.clone(),
            PagedGridConfig {
                route,
                offset: page.items.len(),
                total: page.total,
                page_name: "artists",
            },
            move |controller, offset, limit| {
                controller.cached_artists_page(album_artist, offset, limit)
            },
            append_artists_to_model,
        );
        self.media_grid_view(
            title,
            page.items.is_empty(),
            "Cached rows will appear here after the background sync finishes.",
            self.artist_cards_grid_for_model(model),
            Some(load_next),
        )
    }

    fn artist_detail_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_artist_detail(&artist_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let artist = library
                    .artists
                    .iter()
                    .chain(library.album_artists.iter())
                    .find(|artist| artist.id.as_str() == artist_id.as_str())
                    .cloned()?;
                let albums = library
                    .albums
                    .iter()
                    .filter(|album| {
                        album.artist_id.as_ref().map(ArtistId::as_str) == Some(artist_id.as_str())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let tracks = library
                    .tracks
                    .iter()
                    .filter(|track| {
                        track.artist_id.as_ref().map(ArtistId::as_str) == Some(artist_id.as_str())
                            || albums.iter().any(|album| album.id == track.album_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Some(CachedArtistDetail {
                    artist,
                    albums,
                    tracks,
                })
            });
        let Some(detail) = detail else {
            return self.placeholder_view("Artist", "The selected cached artist was not found.");
        };
        let artist = detail.artist;
        let albums = detail.albums;
        let tracks = detail.tracks;

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);
        wrapper.set_vexpand(true);

        let title = gtk::Label::new(Some(&artist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        wrapper.append(&title);

        let summary = gtk::Label::new(Some(&format!(
            "{} {} / {} {}",
            artist.album_count,
            tr("albums"),
            artist.track_count,
            tr("tracks")
        )));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        wrapper.append(&summary);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let (favorite, favorite_glyph) = favorite_text_button("Favorite");
        set_favorite_text_button_active(&favorite, &favorite_glyph, artist.favorite);
        let controller = self.controller.clone();
        let favorite_artist = artist.clone();
        favorite
            .connect_clicked(move |_| controller.toggle_artist_favorite(favorite_artist.clone()));
        actions.append(&favorite);
        wrapper.append(&actions);

        if !albums.is_empty() {
            let album_heading = gtk::Label::new(Some(&tr("Albums")));
            album_heading.add_css_class("section-heading");
            album_heading.set_xalign(0.0);
            wrapper.append(&album_heading);
            wrapper.append(&self.album_cards_grid(&albums));
        }

        if tracks.is_empty() {
            wrapper.append(
                &self.placeholder_view("Tracks", "No cached tracks are linked to this artist yet."),
            );
        } else {
            wrapper.append(&self.tracks_table(tracks, "artist-detail"));
        }

        wrapper.upcast()
    }

    fn genre_list_view(self: &Rc<Self>, title: &str) -> gtk::Widget {
        let page = self
            .controller
            .cached_genres_page(0, GRID_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached genres page");
                let genres = self
                    .state
                    .library
                    .borrow()
                    .genres
                    .iter()
                    .take(GRID_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(genres, self.state.library.borrow().genres.len())
            });
        let model = genre_model(&page.items);
        let load_next = self.grid_loader(
            model.clone(),
            PagedGridConfig {
                route: Route::Genres,
                offset: page.items.len(),
                total: page.total,
                page_name: "genres",
            },
            |controller, offset, limit| controller.cached_genres_page(offset, limit),
            append_genres_to_model,
        );
        self.media_grid_view(
            title,
            page.items.is_empty(),
            "Cached rows will appear here after the background sync finishes.",
            self.genre_cards_grid_for_model(model),
            Some(load_next),
        )
    }

    fn playlist_list_view(self: &Rc<Self>, title: &str) -> gtk::Widget {
        let page = self
            .controller
            .cached_playlists_page(0, GRID_ROUTE_PAGE_SIZE)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to load cached playlists page");
                let playlists = self
                    .state
                    .library
                    .borrow()
                    .playlists
                    .iter()
                    .take(GRID_ROUTE_PAGE_SIZE)
                    .cloned()
                    .collect::<Vec<_>>();
                rufin_provider::PagedResponse::new(
                    playlists,
                    self.state.library.borrow().playlists.len(),
                )
            });
        let model = playlist_model(&page.items);
        let load_next = self.grid_loader(
            model.clone(),
            PagedGridConfig {
                route: Route::Playlists,
                offset: page.items.len(),
                total: page.total,
                page_name: "playlists",
            },
            |controller, offset, limit| controller.cached_playlists_page(offset, limit),
            append_playlists_to_model,
        );
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        let create = text_button("list-add-symbolic", "New Playlist");
        let shell = Rc::clone(self);
        create.connect_clicked(move |_| shell.new_playlist_dialog());
        header.append(&heading);
        header.append(&create);
        wrapper.append(&header);

        if page.items.is_empty() {
            wrapper.append(&self.placeholder_view(
                title,
                "Cached rows will appear here after the background sync finishes.",
            ));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroller.set_min_content_width(0);
            scroller.set_vexpand(true);
            scroller.set_child(Some(&self.playlist_cards_grid_for_model(model)));
            connect_paged_grid_loader(&scroller, load_next);
            wrapper.append(&scroller);
        }
        wrapper.upcast()
    }

    fn new_playlist_dialog(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("New Playlist"))
            .body(tr(
                "Create a playlist. If a track is playing, it will be added.",
            ))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("create", &tr("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(&tr("Playlist name")));
        dialog.set_extra_child(Some(&entry));
        let controller = self.controller.clone();
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        dialog.connect_response(None, move |_, response| {
            if response == "create" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    controller.create_playlist(name, current_track.clone().into_iter().collect());
                }
            }
        });
        dialog.present(Some(&self.window));
    }

    fn genre_detail_view(self: &Rc<Self>, genre_id: rufin_core::GenreId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_genre_detail(&genre_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let genre = library
                    .genres
                    .iter()
                    .find(|genre| genre.id.as_str() == genre_id.as_str())
                    .cloned()?;
                Some(CachedGenreDetail {
                    genre,
                    albums: Vec::new(),
                    tracks: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self.placeholder_view("Genre", "The selected cached genre was not found.");
        };
        let seed = stable_seed(detail.genre.id.as_str());
        let summary = format!(
            "{} {} / {} {}",
            detail.genre.album_count,
            tr("albums"),
            detail.genre.track_count,
            tr("tracks")
        );
        self.grouped_detail_view(GroupedDetailData {
            title: detail.genre.name,
            image_ref: detail.genre.image_ref,
            seed,
            summary,
            albums: detail.albums,
            tracks: detail.tracks,
            table_context: "genre-detail",
        })
    }

    fn playlist_detail_view(self: &Rc<Self>, playlist_id: PlaylistId) -> gtk::Widget {
        let detail = self
            .controller
            .cached_playlist_detail(&playlist_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let playlist = library
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id.as_str() == playlist_id.as_str())
                    .cloned()?;
                Some(rufin_provider::PlaylistDetail {
                    playlist,
                    tracks: Vec::new(),
                    entries: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self
                .placeholder_view("Playlist", "The selected cached playlist was not found.");
        };
        let seed = stable_seed(detail.playlist.id.as_str());
        let summary = format!(
            "{} {} • {}",
            detail.playlist.track_count,
            tr("tracks"),
            format_duration(detail.playlist.duration_seconds)
        );
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_tile_for(
            detail.playlist.image_ref.as_ref(),
            seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&detail.playlist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let summary = gtk::Label::new(Some(&summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let play = text_button("media-playback-start-symbolic", "Play");
        let controller = self.controller.clone();
        let tracks = detail.tracks.clone();
        play.connect_clicked(move |_| controller.play_tracks_now(tracks.clone()));
        actions.append(&play);
        let rename = text_button("document-edit-symbolic", "Rename");
        let shell = Rc::clone(self);
        let playlist_id_for_rename = detail.playlist.id.clone();
        let current_name = detail.playlist.name.clone();
        rename.connect_clicked(move |_| {
            shell.rename_playlist_dialog(playlist_id_for_rename.clone(), current_name.clone())
        });
        actions.append(&rename);
        let add_current = text_button("list-add-symbolic", "Add current");
        let current_track = self
            .state
            .player
            .borrow()
            .current
            .as_ref()
            .and_then(|entry| {
                self.state
                    .library
                    .borrow()
                    .tracks
                    .iter()
                    .find(|track| track.id == entry.track_id)
                    .cloned()
            });
        add_current.set_sensitive(current_track.is_some());
        let controller = self.controller.clone();
        let playlist_id_for_add = detail.playlist.id.clone();
        add_current.connect_clicked(move |_| {
            if let Some(track) = current_track.clone() {
                controller.add_tracks_to_playlist(playlist_id_for_add.clone(), vec![track]);
            }
        });
        actions.append(&add_current);
        metadata.append(&title);
        metadata.append(&summary);
        metadata.append(&actions);
        header.append(&metadata);
        wrapper.append(&header);

        if detail.entries.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            wrapper.append(&self.playlist_entries_view(&detail));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }

    fn playlist_entries_view(
        self: &Rc<Self>,
        detail: &rufin_provider::PlaylistDetail,
    ) -> gtk::Widget {
        let list = gtk::ListBox::new();
        list.add_css_class("track-table");
        list.set_selection_mode(gtk::SelectionMode::None);
        for (index, entry) in detail.entries.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("queue-row");
            row.set_valign(gtk::Align::Center);
            let number = gtk::Label::new(Some(&(index + 1).to_string()));
            number.add_css_class("muted");
            number.set_width_chars(3);
            row.append(&number);
            row.append(&self.cover_tile_for(
                entry.track.image_ref.as_ref(),
                stable_seed(entry.track.id.as_str()),
                36,
                THUMB_COVER_SIZE,
            ));
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_hexpand(true);
            let title = gtk::Label::new(Some(&entry.track.title));
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let artist = gtk::Label::new(Some(&entry.track.artist));
            artist.add_css_class("muted");
            artist.set_xalign(0.0);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
            labels.append(&title);
            labels.append(&artist);
            row.append(&labels);

            let play = icon_button("media-playback-start-symbolic", "Play track");
            let controller = self.controller.clone();
            let track = entry.track.clone();
            play.connect_clicked(move |_| controller.play_now(track.clone()));
            row.append(&play);

            let up = icon_button("go-up-symbolic", "Move up");
            up.set_sensitive(index > 0);
            let controller = self.controller.clone();
            let playlist_id = detail.playlist.id.clone();
            let entry_id = entry.entry_id.clone();
            up.connect_clicked(move |_| {
                controller.move_playlist_entry(playlist_id.clone(), entry_id.clone(), index - 1)
            });
            row.append(&up);

            let down = icon_button("go-down-symbolic", "Move down");
            down.set_sensitive(index + 1 < detail.entries.len());
            let controller = self.controller.clone();
            let playlist_id = detail.playlist.id.clone();
            let entry_id = entry.entry_id.clone();
            down.connect_clicked(move |_| {
                controller.move_playlist_entry(playlist_id.clone(), entry_id.clone(), index + 1)
            });
            row.append(&down);

            let remove = icon_button("user-trash-symbolic", "Remove from playlist");
            let controller = self.controller.clone();
            let playlist_id = detail.playlist.id.clone();
            let entry_id = entry.entry_id.clone();
            remove.connect_clicked(move |_| {
                controller.remove_playlist_entry(playlist_id.clone(), entry_id.clone())
            });
            row.append(&remove);

            let drag_source = gtk::DragSource::builder()
                .actions(gtk::gdk::DragAction::MOVE)
                .build();
            let entry_id = entry.entry_id.clone();
            drag_source.connect_prepare(move |_, _, _| {
                Some(gtk::gdk::ContentProvider::for_value(&entry_id.to_value()))
            });
            row.add_controller(drag_source);

            let drop_target =
                gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
            let controller = self.controller.clone();
            let playlist_id = detail.playlist.id.clone();
            let target_entry_id = entry.entry_id.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                let Ok(entry_id) = value.get::<String>() else {
                    return false;
                };
                if entry_id == target_entry_id {
                    return false;
                }
                controller.move_playlist_entry(playlist_id.clone(), entry_id, index);
                true
            });
            row.add_controller(drop_target);
            list.append(&row);
        }
        list.upcast()
    }

    fn rename_playlist_dialog(self: &Rc<Self>, playlist_id: PlaylistId, current_name: String) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Rename Playlist"))
            .body(tr("Enter a new playlist name."))
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("rename", &tr("Rename"));
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        let entry = gtk::Entry::new();
        entry.set_text(&current_name);
        dialog.set_extra_child(Some(&entry));
        let controller = self.controller.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "rename" {
                let name = entry.text().trim().to_string();
                if !name.is_empty() {
                    controller.rename_playlist(playlist_id.clone(), name);
                }
            }
        });
        dialog.present(Some(&self.window));
    }

    fn grouped_detail_view(self: &Rc<Self>, data: GroupedDetailData) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_tile_for(
            data.image_ref.as_ref(),
            data.seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&data.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let summary = gtk::Label::new(Some(&data.summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        metadata.append(&title);
        metadata.append(&summary);
        header.append(&metadata);
        wrapper.append(&header);

        if !data.albums.is_empty() {
            let album_heading = gtk::Label::new(Some(&tr("Albums")));
            album_heading.add_css_class("section-heading");
            album_heading.set_xalign(0.0);
            wrapper.append(&album_heading);
            wrapper.append(&self.album_cards_grid(&data.albums));
        }
        if data.tracks.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            wrapper.append(&self.tracks_table(data.tracks, data.table_context));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }

    fn media_grid_view(
        self: &Rc<Self>,
        title: &str,
        empty: bool,
        empty_body: &str,
        grid: gtk::Widget,
        load_next: Option<Rc<dyn Fn()>>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);

        if empty {
            wrapper.append(&self.placeholder_view(title, empty_body));
        } else {
            let scroller = gtk::ScrolledWindow::new();
            scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroller.set_min_content_width(0);
            scroller.set_vexpand(true);
            scroller.set_child(Some(&grid));
            if let Some(load_next) = load_next {
                connect_paged_grid_loader(&scroller, load_next);
            }
            wrapper.append(&scroller);
        }
        wrapper.upcast()
    }

    fn search_view(self: &Rc<Self>, query: &str, library: LibrarySnapshot) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(&format!("{}: {query}", tr("Search"))));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);

        let has_albums = !library.search.albums.is_empty();
        let has_tracks = !library.search.tracks.is_empty();
        let has_artists = !library.search.artists.is_empty();
        let has_playlists = !library.search.playlists.is_empty();
        let albums = library.search.albums;
        if !albums.is_empty() {
            let section = HomeSection {
                kind: rufin_core::HomeSectionKind::Explore,
                albums,
            };
            wrapper.append(&self.home_album_section(&section));
        }

        if has_tracks {
            wrapper.append(&self.tracks_table(library.search.tracks, "search"));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.placeholder_view(
                "Search",
                "Type a query in the sidebar search field to search the local cache.",
            ));
        }

        wrapper.upcast()
    }

    fn settings_view(self: &Rc<Self>) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        group.add_css_class("settings-group");

        let heading = gtk::Label::new(Some(&tr("Layout density")));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);

        let options = gtk::StringList::new(&[&tr("Auto"), &tr("Normal"), &tr("Compact")]);
        let dropdown = gtk::DropDown::new(Some(options), None::<gtk::Expression>);
        dropdown.set_selected(match self.state.density_mode.get() {
            DensityMode::Auto => 0,
            DensityMode::Normal => 1,
            DensityMode::Compact => 2,
        });

        let shell = Rc::clone(self);
        dropdown.connect_selected_notify(move |dropdown| {
            let density = match dropdown.selected() {
                1 => DensityMode::Normal,
                2 => DensityMode::Compact,
                _ => DensityMode::Auto,
            };
            shell.set_density_mode(density);
        });

        let note = gtk::Label::new(Some(&tr("Saved locally for the next launch.")));
        note.add_css_class("muted");
        note.set_wrap(true);
        note.set_xalign(0.0);

        group.append(&heading);
        group.append(&dropdown);
        group.append(&note);
        wrapper.append(&group);

        let library = self.state.library.borrow();
        let server_name = library
            .server
            .as_ref()
            .map(|server| server.name.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("No server"));
        let username = library
            .username
            .as_deref()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("no account"));
        let status = gtk::Label::new(Some(&format!(
            "{} ({username}): {} {} / {} {} • {}",
            server_name,
            library.albums.len(),
            tr("albums"),
            library.tracks.len(),
            tr("tracks"),
            library.sync_status
        )));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        wrapper.append(&status);

        let server_group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        server_group.add_css_class("settings-group");
        let server_heading = gtk::Label::new(Some(&tr("Jellyfin Server")));
        server_heading.add_css_class("section-heading");
        server_heading.set_xalign(0.0);
        server_group.append(&server_heading);

        let server_url = library
            .server
            .as_ref()
            .map(|server| server.base_url.clone())
            .unwrap_or_else(|| tr("No active server"));
        let details = gtk::Label::new(Some(&format!(
            "{}\n{}: {}\n{}: {} {} / {} {}",
            server_url,
            tr("User"),
            username,
            tr("Cached"),
            library.albums.len(),
            tr("albums"),
            library.tracks.len(),
            tr("tracks")
        )));
        details.add_css_class("muted");
        details.set_wrap(true);
        details.set_xalign(0.0);
        server_group.append(&details);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let resync = text_button("view-refresh-symbolic", "Resync Library");
        let clear_cache = text_button("edit-clear-symbolic", "Clear Cached Library");
        let forget = text_button("user-trash-symbolic", "Forget Server");
        forget.add_css_class("destructive-action");

        let controller = self.controller.clone();
        resync.connect_clicked(move |_| controller.resync_active_server());

        let clear_shell = Rc::clone(self);
        clear_cache.connect_clicked(move |_| clear_shell.confirm_clear_cache());

        let forget_shell = Rc::clone(self);
        forget.connect_clicked(move |_| forget_shell.confirm_forget_server());

        actions.append(&resync);
        actions.append(&clear_cache);
        actions.append(&forget);
        server_group.append(&actions);
        wrapper.append(&server_group);

        wrapper.upcast()
    }

    fn confirm_clear_cache(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Clear Cached Library"))
            .body(tr(
                "This removes cached Jellyfin library metadata for the active server. Login stays saved.",
            ))
            .build();
        let cancel = tr("Cancel");
        let clear = tr("Clear Cache");
        dialog.add_responses(&[("cancel", cancel.as_str()), ("clear", clear.as_str())]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        let controller = self.controller.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() == "clear" {
                    controller.clear_active_server_cache();
                }
            },
        );
    }

    fn confirm_forget_server(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Forget Server"))
            .body(tr(
                "This removes the active server, cached library metadata, queue snapshot, and saved token.",
            ))
            .build();
        let cancel = tr("Cancel");
        let forget = tr("Forget Server");
        dialog.add_responses(&[("cancel", cancel.as_str()), ("forget", forget.as_str())]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
        let controller = self.controller.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() == "forget" {
                    controller.forget_active_server();
                }
            },
        );
    }

    fn confirm_clear_queue(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Clear queue"))
            .body(tr("This removes all queued tracks and stops playback."))
            .build();
        let cancel = tr("Cancel");
        let clear = tr("Clear queue");
        dialog.add_responses(&[("cancel", cancel.as_str()), ("clear", clear.as_str())]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        let controller = self.controller.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() == "clear" {
                    controller.clear_queue();
                }
            },
        );
    }

    fn render_queue_panel(self: &Rc<Self>) {
        while let Some(child) = self.right_panel.first_child() {
            self.right_panel.remove(&child);
        }

        let queue_snapshot = self.state.queue.borrow().clone();
        let player = self.state.player.borrow().clone();
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.add_css_class("sidebar-header");
        sidebar_header.set_show_start_title_buttons(false);
        sidebar_header.set_show_end_title_buttons(false);
        let queue_title = gtk::Label::new(Some(&tr("Queue")));
        queue_title.add_css_class("panel-title");
        sidebar_header.set_title_widget(Some(&queue_title));

        let shuffle = icon_button(
            "media-playlist-shuffle-symbolic",
            if player.shuffle_enabled {
                "Shuffle on"
            } else {
                "Shuffle"
            },
        );
        if player.shuffle_enabled {
            shuffle.add_css_class("active-toggle");
        }
        let controller = self.controller.clone();
        shuffle.connect_clicked(move |_| controller.toggle_shuffle());
        sidebar_header.pack_start(&shuffle);

        let repeat = icon_button(
            "media-playlist-repeat-symbolic",
            repeat_label(player.repeat_mode),
        );
        if player.repeat_mode != RepeatMode::Off {
            repeat.add_css_class("active-toggle");
        }
        let controller = self.controller.clone();
        repeat.connect_clicked(move |_| controller.cycle_repeat());
        sidebar_header.pack_start(&repeat);

        let clear_shell = Rc::clone(self);
        let clear = icon_button("edit-clear-symbolic", "Clear queue");
        clear.connect_clicked(move |_| clear_shell.confirm_clear_queue());
        sidebar_header.pack_end(&clear);
        self.right_panel.append(&sidebar_header);

        let queue = gtk::Box::new(gtk::Orientation::Vertical, 8);
        queue.add_css_class("queue-panel");
        queue.set_vexpand(true);
        queue.set_margin_top(10);
        queue.set_margin_start(10);
        queue.set_margin_end(10);
        queue.set_margin_bottom(12);

        let queue_list = gtk::ListBox::new();
        queue_list.add_css_class("queue-list");
        queue_list.set_selection_mode(gtk::SelectionMode::None);
        if let Some(snapshot) = &queue_snapshot {
            if !snapshot.entries.is_empty() {
                queue.append(&queue_header_row());
            }
            for (index, entry) in snapshot.entries.iter().enumerate() {
                queue_list.append(&self.queue_row(index, entry, snapshot.current_index));
            }
        }
        if queue_list.first_child().is_none() {
            let empty = gtk::Label::new(Some(&tr("Add music to start a queue.")));
            empty.add_css_class("muted");
            empty.set_wrap(true);
            empty.set_margin_top(24);
            queue_list.append(&empty);
        }
        queue.append(&queue_list);

        let lyrics = gtk::Box::new(gtk::Orientation::Vertical, 10);
        lyrics.add_css_class("lyrics-panel");
        lyrics.set_vexpand(true);
        lyrics.set_margin_top(12);
        lyrics.set_margin_start(10);
        lyrics.set_margin_end(10);
        lyrics.set_margin_bottom(18);

        let lyrics_title = gtk::Label::new(Some(&tr("Lyrics")));
        lyrics_title.add_css_class("panel-title");
        lyrics_title.set_xalign(0.0);
        lyrics.append(&lyrics_title);

        let lyrics_scroller = gtk::ScrolledWindow::new();
        lyrics_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        lyrics_scroller.set_vexpand(true);
        let lyrics_body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        lyrics_body.set_vexpand(true);
        lyrics_body.add_css_class("lyrics-lines");
        self.state.lyrics_rows.borrow_mut().clear();
        *self.state.lyrics_scroller.borrow_mut() = Some(lyrics_scroller.clone());
        self.state.lyrics_active_index.set(None);
        if let Some(current_lyrics) = self.state.lyrics.borrow().clone() {
            for line in &current_lyrics.lines {
                let label = gtk::Label::new(Some(&line.text));
                label.set_wrap(true);
                label.set_xalign(0.5);
                label.set_justify(gtk::Justification::Center);
                label.set_hexpand(true);
                label.add_css_class("lyrics-line");
                let row = gtk::Button::new();
                row.add_css_class("lyrics-row");
                row.add_css_class("flat");
                row.set_hexpand(true);
                row.set_child(Some(&label));
                if let Some(start_millis) = line.start_millis {
                    let controller = self.controller.clone();
                    row.connect_clicked(move |_| {
                        controller.seek((start_millis / 1_000) as u32);
                    });
                } else {
                    row.set_sensitive(false);
                }
                lyrics_body.append(&row);
                self.state
                    .lyrics_rows
                    .borrow_mut()
                    .push(LyricsRow { row, label });
            }
        } else {
            let lyrics_status = gtk::Label::new(Some(&tr("No lyrics for the current track.")));
            lyrics_status.add_css_class("muted");
            lyrics_status.set_wrap(true);
            lyrics_status.set_justify(gtk::Justification::Center);
            lyrics_status.set_valign(gtk::Align::Center);
            lyrics_status.set_vexpand(true);
            lyrics_body.append(&lyrics_status);
        }
        lyrics_scroller.set_child(Some(&lyrics_body));
        lyrics.append(&lyrics_scroller);

        let queue_lyrics_split = gtk::Paned::new(gtk::Orientation::Vertical);
        queue_lyrics_split.add_css_class("queue-lyrics-split");
        queue_lyrics_split.set_vexpand(true);
        queue_lyrics_split.set_wide_handle(true);
        queue_lyrics_split.set_resize_start_child(true);
        queue_lyrics_split.set_resize_end_child(true);
        queue_lyrics_split.set_shrink_start_child(true);
        queue_lyrics_split.set_shrink_end_child(false);
        queue_lyrics_split.set_start_child(Some(&queue));
        queue_lyrics_split.set_end_child(Some(&lyrics));
        if let Some(position) = self.state.settings.borrow().queue_lyrics_position {
            queue_lyrics_split.set_position(position.max(120));
        }

        let shell = Rc::clone(self);
        queue_lyrics_split.connect_notify_local(Some("position"), move |split, _| {
            let position = split.position();
            if position <= 0 {
                return;
            }
            let mut settings = shell.state.settings.borrow_mut();
            if settings.queue_lyrics_position == Some(position) {
                return;
            }
            settings.queue_lyrics_position = Some(position);
            if let Err(error) = shell.controller.save_settings(&settings) {
                warn!(%error, "failed to save queue lyrics split position");
            }
        });

        self.right_panel.append(&queue_lyrics_split);
        self.update_lyrics_highlight();
    }

    fn queue_row(
        self: &Rc<Self>,
        index: usize,
        entry: &QueueEntry,
        current_index: Option<usize>,
    ) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("queue-row");
        row.set_valign(gtk::Align::Center);
        if current_index == Some(index) {
            row.add_css_class("queue-row-current");
        }
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("muted");
        number.set_width_chars(2);
        let cover = self.cover_tile_for(
            entry.image_ref.as_ref(),
            index as u32 * 7 + entry.duration_seconds,
            44,
            THUMB_COVER_SIZE,
        );
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&entry.title));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let artist = queue_link_label(&entry.artist);
        labels.append(&title);
        labels.append(&artist);
        if let Some(artist_id) = entry.artist_id.clone() {
            let shell = Rc::clone(self);
            add_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !entry.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = entry.artist.clone();
            add_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }
        let year_text = (entry.year != 0).then(|| entry.year.to_string());
        let year = gtk::Label::new(year_text.as_deref());
        year.add_css_class("muted");
        year.set_xalign(1.0);
        year.set_width_chars(4);
        year.set_halign(gtk::Align::End);
        let remove = icon_button("user-trash-symbolic", "Remove from queue");
        remove.add_css_class("queue-remove-button");
        let controller = self.controller.clone();
        let entry_id = entry.id.clone();
        remove.connect_clicked(move |_| controller.remove_from_queue(entry_id.clone()));
        row.append(&number);
        row.append(&cover);
        row.append(&labels);
        row.append(&year);
        row.append(&remove);
        row.upcast()
    }

    fn update_bottom_player(self: &Rc<Self>) {
        let player = self.state.player.borrow().clone();
        let controls = &self.player_controls;
        self.state.updating_player_controls.set(true);

        let cover_seed = player
            .current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or(42);
        controls.cover.set_seed(cover_seed);
        if let Some(image_ref) = player
            .current
            .as_ref()
            .and_then(|entry| entry.image_ref.as_ref())
        {
            if let Some(key) = self.cover_cache_key(image_ref, THUMB_COVER_SIZE) {
                if controls.cover_key.borrow().as_deref() != Some(key.as_str()) {
                    controls.cover.clear_image();
                    let generation = controls.cover.generation();
                    self.state
                        .cover_bindings
                        .borrow_mut()
                        .entry(key.clone())
                        .or_default()
                        .push(CoverBinding {
                            tile: controls.cover.clone(),
                            generation,
                        });
                    self.controller.request_cover_for_key(
                        key.clone(),
                        image_ref.clone(),
                        THUMB_COVER_SIZE,
                    );
                    *controls.cover_key.borrow_mut() = Some(key);
                }
            } else {
                let mut current_key = controls.cover_key.borrow_mut();
                if current_key.is_some() {
                    controls.cover.clear_image();
                    *current_key = None;
                }
            }
        } else {
            controls.cover.clear_image();
            *controls.cover_key.borrow_mut() = None;
        }

        let play_icon = match player.state {
            PlaybackState::Playing | PlaybackState::Buffering => "media-playback-pause-symbolic",
            PlaybackState::Paused | PlaybackState::Stopped => "media-playback-start-symbolic",
        };
        controls.play_icon.set_icon_name(Some(play_icon));
        controls
            .play_button
            .set_tooltip_text(Some(&tr(playback_state_label(player.state))));

        let title = player
            .current
            .as_ref()
            .map(|entry| entry.title.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Nothing playing"));
        let artist = player
            .current
            .as_ref()
            .map(|entry| entry.artist.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Queue a track to begin"));
        let album = player
            .current
            .as_ref()
            .map(|entry| entry.album.as_str())
            .unwrap_or("");
        controls.title.set_text(&title);
        controls.artist.set_text(&artist);
        controls.album.set_text(album);
        controls.title.set_sensitive(player.current.is_some());
        controls.artist.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.artist.is_empty()),
        );
        controls.album.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.album.is_empty()),
        );

        set_active_class(&controls.shuffle_button, player.shuffle_enabled);
        set_active_class(
            &controls.repeat_button,
            player.repeat_mode != RepeatMode::Off,
        );
        set_favorite_button_active(
            &controls.favorite_button,
            player.current.as_ref().is_some_and(|entry| entry.favorite),
        );
        controls
            .favorite_button
            .set_sensitive(player.current.is_some());
        controls
            .repeat_button
            .set_tooltip_text(Some(&tr(repeat_label(player.repeat_mode))));

        controls
            .elapsed
            .set_text(&format_duration(player.position_seconds));
        if !self.state.seeking_player_controls.get() {
            let max = f64::from(player.duration_seconds.max(1));
            controls.progress.set_range(0.0, max);
            controls.progress.set_value(f64::from(
                player.position_seconds.min(player.duration_seconds),
            ));
        }
        controls
            .duration
            .set_text(&format_duration(player.duration_seconds));

        controls.mute_icon.set_icon_name(Some(if player.muted {
            "audio-volume-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }));
        controls.volume.set_value(player.volume);
        self.state.updating_player_controls.set(false);
    }

    fn placeholder_view(&self, title: &str, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let heading = gtk::Label::new(Some(&tr(title)));
        heading.add_css_class("section-heading");
        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&heading);
        wrapper.append(&label);
        wrapper.upcast()
    }

    fn cover_tile_for(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new(size, seed);
        let widget = tile.widget();

        if let Some(image_ref) = image_ref
            && let Some(key) = self.cover_cache_key(image_ref, fetch_size)
        {
            if let Some(pixbuf) = self.state.decoded_covers.borrow().get(&key).cloned() {
                self.record_perf_cover_cache_hit(&key);
                tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            } else {
                let shell = Rc::clone(self);
                let tile_for_map = tile.clone();
                let image_ref = image_ref.clone();
                let started = Rc::new(Cell::new(false));
                widget.connect_map(move |_| {
                    if started.replace(true) {
                        return;
                    }
                    shell.request_cover_for_tile(
                        &tile_for_map,
                        key.clone(),
                        image_ref.clone(),
                        size,
                        fetch_size,
                    );
                });
            }
        } else if image_ref.is_none() {
            self.record_perf_coverless_tile();
        }

        widget
    }

    fn request_cover_for_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        key: String,
        image_ref: ImageRef,
        size: i32,
        fetch_size: u32,
    ) {
        if let Some(pixbuf) = self.state.decoded_covers.borrow().get(&key).cloned() {
            self.record_perf_cover_cache_hit(&key);
            tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            return;
        }

        self.record_perf_cover_bind_request(&key);
        let generation = tile.generation();
        {
            self.state
                .cover_bindings
                .borrow_mut()
                .entry(key.clone())
                .or_default()
                .push(CoverBinding {
                    tile: tile.clone(),
                    generation,
                });
        }
        if let Some(path) = self.controller.cached_cover_path_for_key(&key) {
            let shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                shell.record_perf_cover_ready(&key);
                shell.start_cover_decode_from_path(key, path, size);
            });
        } else {
            self.controller
                .request_cover_for_key(key, image_ref, fetch_size);
        }
    }

    fn cover_cache_key(&self, image_ref: &ImageRef, size: u32) -> Option<String> {
        let server = self.state.library.borrow().server.clone()?;
        if server.provider == "fake" {
            return None;
        }
        Some(image_cache_key(
            &server.id,
            &image_ref.item_id,
            image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED),
            size,
        ))
    }

    fn apply_cover_ready(self: &Rc<Self>, key: &str, path: &Path) {
        self.record_perf_cover_ready(key);
        let size = self
            .pending_cover_size(key)
            .unwrap_or(GRID_COVER_SIZE as i32);
        if let Some(pixbuf) = self.state.decoded_covers.borrow().get(key).cloned() {
            let bindings = self.take_live_cover_bindings(key);
            apply_pixbuf_to_bindings(bindings, pixbuf);
            return;
        }
        self.start_cover_decode_from_path(key.to_string(), path.to_path_buf(), size);
    }

    fn start_cover_decode_from_path(self: &Rc<Self>, key: String, path: PathBuf, size: i32) {
        if self.state.decoded_covers.borrow().contains_key(&key) {
            if let Some(pixbuf) = self.state.decoded_covers.borrow().get(&key).cloned() {
                let bindings = self.take_live_cover_bindings(&key);
                apply_pixbuf_to_bindings(bindings, pixbuf);
            }
            return;
        }
        if !self.state.cover_decodes.borrow_mut().insert(key.clone()) {
            return;
        }
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            match load_cover_pixbuf(path.clone(), size).await {
                Ok(pixbuf) => {
                    shell.finish_cover_decode(&key);
                    shell.record_perf_cover_decode_ok(&key);
                    shell.remember_decoded_cover(key.clone(), pixbuf.clone());
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
        });
    }

    fn finish_cover_decode(&self, key: &str) {
        self.state.cover_decodes.borrow_mut().remove(key);
    }

    fn pending_cover_size(&self, key: &str) -> Option<i32> {
        self.state
            .cover_bindings
            .borrow()
            .get(key)
            .and_then(|bindings| bindings.first())
            .map(|binding| binding.tile.size())
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

    fn remember_decoded_cover(&self, key: String, pixbuf: Pixbuf) {
        let mut covers = self.state.decoded_covers.borrow_mut();
        if !covers.contains_key(&key) {
            self.state
                .decoded_cover_order
                .borrow_mut()
                .push_back(key.clone());
        }
        covers.insert(key, pixbuf);
        let mut order = self.state.decoded_cover_order.borrow_mut();
        while covers.len() > DECODED_COVER_CACHE_LIMIT {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            covers.remove(&oldest);
        }
    }

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

    fn grid_loader<T>(
        self: &Rc<Self>,
        model: gio::ListStore,
        config: PagedGridConfig,
        load_page: impl Fn(
            &AppController,
            usize,
            usize,
        ) -> Result<rufin_provider::PagedResponse<T>, String>
        + 'static,
        append_items: impl Fn(&gio::ListStore, Vec<T>) + 'static,
    ) -> Rc<dyn Fn()> {
        let shell = Rc::clone(self);
        let cursor = Rc::new(PagedGridCursor {
            offset: Cell::new(config.offset),
            total: Cell::new(config.total),
            loading: Cell::new(false),
        });
        let route = config.route;
        let page_name = config.page_name;
        Rc::new(move || {
            if !shell.can_load_grid_page(&cursor, &route) {
                return;
            }
            let offset = cursor.offset.get();
            match load_page(&shell.controller, offset, GRID_ROUTE_PAGE_SIZE) {
                Ok(page) => {
                    let count = page.items.len();
                    append_items(&model, page.items);
                    finish_grid_page(&cursor, offset, count, page.total);
                }
                Err(error) => {
                    warn!(%error, page = page_name, "failed to append cached grid page");
                    cursor.loading.set(false);
                }
            }
        })
    }

    fn can_load_grid_page(&self, cursor: &PagedGridCursor, route: &Route) -> bool {
        if cursor.loading.get() || cursor.offset.get() >= cursor.total.get() {
            return false;
        }
        if self.state.routes.borrow().current() != route {
            return false;
        }
        cursor.loading.set(true);
        true
    }

    fn album_cards_grid(self: &Rc<Self>, albums: &[Album]) -> gtk::Widget {
        let model = album_model(albums);
        self.album_cards_grid_for_model(model)
    }

    fn album_cards_grid_for_model(self: &Rc<Self>, model: gio::ListStore) -> gtk::Widget {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.card_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.card_grid_columns.set(columns);
        let card_size = home_album_card_size(width, columns);

        let shell_for_factory = Rc::clone(self);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(move |_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(item) = list_item.item() else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let album = boxed.borrow::<Album>();
            list_item.set_child(Some(&album_card_widget_with_size(
                &shell_for_factory,
                &album,
                card_size,
                Some(&shell_for_factory.controller),
            )));
        });
        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("album-grid");
        grid.set_min_columns(columns as u32);
        grid.set_max_columns(columns as u32);
        grid.set_single_click_activate(false);
        grid.set_hexpand(true);
        grid.set_vexpand(true);

        grid.upcast()
    }

    fn artist_cards_grid_for_model(self: &Rc<Self>, model: gio::ListStore) -> gtk::Widget {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.card_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.card_grid_columns.set(columns);
        let card_size = home_album_card_size(width, columns);

        let shell_for_factory = Rc::clone(self);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(move |_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(item) = list_item.item() else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let artist = boxed.borrow::<Artist>();
            list_item.set_child(Some(&artist_card_widget_with_size(
                &shell_for_factory,
                &artist,
                card_size,
            )));
        });
        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("album-grid");
        grid.set_min_columns(columns as u32);
        grid.set_max_columns(columns as u32);
        grid.set_single_click_activate(true);
        grid.set_hexpand(true);
        grid.set_vexpand(true);

        let shell = Rc::clone(self);
        let model_for_activate = model.clone();
        grid.connect_activate(move |_, position| {
            let Some(item) = model_for_activate.item(position) else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            shell.navigate(Route::ArtistDetail(boxed.borrow::<Artist>().id.clone()));
        });

        grid.upcast()
    }

    fn genre_cards_grid_for_model(self: &Rc<Self>, model: gio::ListStore) -> gtk::Widget {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.card_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.card_grid_columns.set(columns);
        let card_size = home_album_card_size(width, columns);

        let shell_for_factory = Rc::clone(self);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(move |_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(item) = list_item.item() else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let genre = boxed.borrow::<Genre>();
            list_item.set_child(Some(&genre_card_widget_with_size(
                &shell_for_factory,
                &genre,
                card_size,
            )));
        });
        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("album-grid");
        grid.set_min_columns(columns as u32);
        grid.set_max_columns(columns as u32);
        grid.set_single_click_activate(true);
        grid.set_hexpand(true);
        grid.set_vexpand(true);

        let shell = Rc::clone(self);
        let model_for_activate = model.clone();
        grid.connect_activate(move |_, position| {
            let Some(item) = model_for_activate.item(position) else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            shell.navigate(Route::GenreDetail(boxed.borrow::<Genre>().id.clone()));
        });
        grid.upcast()
    }

    fn playlist_cards_grid_for_model(self: &Rc<Self>, model: gio::ListStore) -> gtk::Widget {
        let width = home_album_content_width(self);
        let current = nonzero_usize(self.state.card_grid_columns.get());
        let columns = home_album_page_size(width, current);
        self.state.card_grid_columns.set(columns);
        let card_size = home_album_card_size(width, columns);

        let shell_for_factory = Rc::clone(self);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(move |_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(item) = list_item.item() else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let playlist = boxed.borrow::<Playlist>();
            list_item.set_child(Some(&playlist_card_widget_with_size(
                &shell_for_factory,
                &playlist,
                card_size,
            )));
        });
        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("album-grid");
        grid.set_min_columns(columns as u32);
        grid.set_max_columns(columns as u32);
        grid.set_single_click_activate(true);
        grid.set_hexpand(true);
        grid.set_vexpand(true);

        let shell = Rc::clone(self);
        let model_for_activate = model.clone();
        grid.connect_activate(move |_, position| {
            let Some(item) = model_for_activate.item(position) else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            shell.navigate(Route::PlaylistDetail(boxed.borrow::<Playlist>().id.clone()));
        });
        grid.upcast()
    }

    fn album_card_with_size(self: &Rc<Self>, album: &Album, size: i32) -> gtk::Widget {
        album_card_widget_with_size(self, album, size, Some(&self.controller))
    }

    fn track_table_popover(
        self: &Rc<Self>,
        table: &gtk::ColumnView,
        model: &gio::ListStore,
        tracks: Rc<RefCell<Vec<Track>>>,
        search: &gtk::SearchEntry,
        sort_button: &gtk::Button,
    ) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);

        let sort_label = gtk::Label::new(Some(&tr("Sort by")));
        sort_label.add_css_class("muted");
        sort_label.set_xalign(0.0);
        content.append(&sort_label);

        let sort_titles = TrackSortKey::all()
            .iter()
            .map(|key| tr(key.title()))
            .collect::<Vec<_>>();
        let sort_title_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_title_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        let current_sort = self.state.settings.borrow().track_table.sort_key;
        sort_dropdown.set_selected(track_sort_index(current_sort));
        let shell = Rc::clone(self);
        let model_for_sort = model.clone();
        let tracks_for_sort = Rc::clone(&tracks);
        let search_for_sort = search.clone();
        let sort_button_for_sort = sort_button.clone();
        sort_dropdown.connect_selected_notify(move |dropdown| {
            let sort_key = track_sort_from_index(dropdown.selected());
            let mut settings = shell.state.settings.borrow().track_table.clone();
            settings.sort_key = sort_key;
            shell.update_track_table_settings(|stored| *stored = settings.clone());
            let tracks = tracks_for_sort.borrow();
            populate_track_model(
                &model_for_sort,
                &tracks,
                &settings,
                search_for_sort.text().as_str(),
            );
            set_track_sort_button_content(&sort_button_for_sort, &settings);
        });
        content.append(&sort_dropdown);

        let columns_label = gtk::Label::new(Some(&tr("Columns")));
        columns_label.add_css_class("muted");
        columns_label.set_xalign(0.0);
        content.append(&columns_label);

        let visible = self
            .state
            .settings
            .borrow()
            .track_table
            .visible_columns
            .clone();
        for column in TrackTableColumn::all() {
            let check = gtk::CheckButton::with_label(&tr(column.title()));
            check.set_active(visible.contains(&column));
            let shell = Rc::clone(self);
            let table_for_column = table.clone();
            check.connect_toggled(move |check| {
                shell.update_track_table_settings(|settings| {
                    if check.is_active() {
                        if !settings.visible_columns.contains(&column) {
                            settings.visible_columns.push(column);
                        }
                    } else {
                        settings.visible_columns.retain(|stored| *stored != column);
                        if settings.visible_columns.is_empty() {
                            settings.visible_columns.push(TrackTableColumn::Title);
                        }
                    }
                });
                let settings = shell.state.settings.borrow().track_table.clone();
                set_track_table_columns(&shell, &table_for_column, &settings);
            });
            content.append(&check);
        }

        popover.set_child(Some(&content));
        popover
    }
}

fn connect_shell_actions(shell: &Rc<Shell>, settings_button: gtk::Button) {
    let back_shell = Rc::clone(shell);
    shell
        .back_button
        .connect_clicked(move |_| back_shell.go_back());

    let forward_shell = Rc::clone(shell);
    shell
        .forward_button
        .connect_clicked(move |_| forward_shell.go_forward());

    let settings_shell = Rc::clone(shell);
    settings_button.connect_clicked(move |_| settings_shell.navigate(Route::Settings));

    let close_shell = Rc::clone(shell);
    shell.window.connect_close_request(move |_| {
        close_shell.save_window_geometry();
        glib::Propagation::Proceed
    });

    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            if resize_shell.state.density_mode.get() == DensityMode::Auto {
                resize_shell.update_density();
            } else {
                let split_changed = resize_shell.update_content_split();
                if split_changed
                    || route_uses_responsive_cards(resize_shell.state.routes.borrow().current())
                {
                    resize_shell.render_current_route();
                }
            }
        });

    let split_shell = Rc::clone(shell);
    shell
        .content_split
        .connect_notify_local(Some("width"), move |_, _| {
            let split_changed = split_shell.update_content_split();
            if split_changed
                || route_uses_responsive_cards(split_shell.state.routes.borrow().current())
            {
                split_shell.render_current_route();
            }
        });

    let split_shell = Rc::clone(shell);
    shell
        .content_split
        .connect_notify_local(Some("position"), move |_, _| {
            let split_changed = split_shell.update_content_split();
            if split_changed
                || route_uses_responsive_cards(split_shell.state.routes.borrow().current())
            {
                split_shell.render_current_route();
            }
        });

    let split_shell = Rc::clone(shell);
    shell.content_split.add_tick_callback(move |_, _| {
        if split_shell.update_content_split()
            && route_uses_responsive_cards(split_shell.state.routes.borrow().current())
        {
            split_shell.render_current_route();
        }
        glib::ControlFlow::Continue
    });
}

fn schedule_startup_sync(shell: &Rc<Shell>) {
    let Some(delay_ms) = shell.controller.startup_sync_delay_ms() else {
        return;
    };

    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
        debug!(delay_ms, "starting deferred background sync");
        shell.controller.start_background_sync_for_active();
    });
}

fn queue_header_row() -> gtk::Widget {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("queue-header");
    header.set_valign(gtk::Align::Center);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(70);
    header.append(&spacer);

    let title = gtk::Label::new(Some(&tr("Title").to_uppercase()));
    title.add_css_class("muted");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    header.append(&title);

    let year = gtk::Label::new(Some(&tr("Year").to_uppercase()));
    year.add_css_class("muted");
    year.set_xalign(1.0);
    year.set_width_chars(4);
    header.append(&year);

    let end_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    end_spacer.set_width_request(28);
    header.append(&end_spacer);
    header.upcast()
}

fn build_bottom_player() -> PlayerControls {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    root.add_css_class("bottom-player");
    root.set_height_request(44);
    root.set_valign(gtk::Align::Center);

    let cover = ArtworkTile::new(34, 42);
    cover.area.set_valign(gtk::Align::Center);
    root.append(&cover.area);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 2);
    identity.set_width_request(160);
    identity.set_valign(gtk::Align::Center);
    let title = player_link("player-title");
    let artist = player_link("muted");
    let album = player_link("muted");
    identity.append(&title);
    identity.append(&artist);
    identity.append(&album);
    root.append(&identity);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    transport.set_hexpand(true);
    transport.set_valign(gtk::Align::Center);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    buttons.set_valign(gtk::Align::Center);

    let stop_button = icon_button("media-playback-stop-symbolic", "Stop");
    stop_button.add_css_class("player-transport-button");
    let previous_button = icon_button("media-skip-backward-symbolic", "Previous");
    previous_button.add_css_class("player-transport-button");
    let (play_button, play_icon) = icon_button_with_image("media-playback-start-symbolic", "Play");
    play_button.add_css_class("player-transport-button");
    play_button.add_css_class("player-play-button");
    let next_button = icon_button("media-skip-forward-symbolic", "Next");
    next_button.add_css_class("player-transport-button");
    let shuffle_button = icon_button("media-playlist-shuffle-symbolic", "Shuffle");
    let repeat_button = icon_button("media-playlist-repeat-symbolic", "Repeat off");

    buttons.append(&stop_button);
    buttons.append(&previous_button);
    buttons.append(&play_button);
    buttons.append(&next_button);
    buttons.append(&shuffle_button);
    buttons.append(&repeat_button);

    let progress_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    progress_row.set_hexpand(true);
    progress_row.set_valign(gtk::Align::Center);
    let elapsed = gtk::Label::new(Some("0:00"));
    elapsed.add_css_class("muted");
    let progress = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    progress.set_draw_value(false);
    progress.set_hexpand(true);
    let duration = gtk::Label::new(Some("0:00"));
    duration.add_css_class("muted");
    progress_row.append(&elapsed);
    progress_row.append(&progress);
    progress_row.append(&duration);

    transport.append(&buttons);
    transport.append(&progress_row);
    root.append(&transport);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_valign(gtk::Align::Center);
    actions.append(&icon_button("view-list-symbolic", "Queue"));
    actions.append(&icon_button("insert-text-symbolic", "Lyrics"));
    let favorite_button = favorite_icon_button("Favorite");
    actions.append(&favorite_button);
    let (mute_button, mute_icon) = icon_button_with_image("audio-volume-high-symbolic", "Mute");
    actions.append(&mute_button);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    volume.add_css_class("volume-slider");
    volume.set_width_request(88);
    volume.set_value(1.0);
    volume.set_draw_value(false);
    actions.append(&volume);
    root.append(&actions);

    PlayerControls {
        root,
        cover,
        cover_key: RefCell::new(None),
        title,
        artist,
        album,
        stop_button,
        previous_button,
        play_button,
        play_icon,
        next_button,
        shuffle_button,
        repeat_button,
        favorite_button,
        elapsed,
        progress,
        duration,
        mute_button,
        mute_icon,
        volume,
    }
}

fn connect_player_controls(shell: &Rc<Shell>) {
    let controller = shell.controller.clone();
    shell
        .player_controls
        .stop_button
        .connect_clicked(move |_| controller.stop());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .previous_button
        .connect_clicked(move |_| controller.previous_track());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .play_button
        .connect_clicked(move |_| controller.play_pause());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .next_button
        .connect_clicked(move |_| controller.next_track());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .shuffle_button
        .connect_clicked(move |_| controller.toggle_shuffle());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .repeat_button
        .connect_clicked(move |_| controller.cycle_repeat());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .favorite_button
        .connect_clicked(move |_| controller.toggle_current_favorite());

    let title_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.title, move || {
        let Some(entry) = title_shell.state.player.borrow().current.clone() else {
            return;
        };
        title_shell.navigate(Route::Search {
            query: entry.title,
            kind: SearchKind::Tracks,
        });
    });

    let artist_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.artist, move || {
        let Some(entry) = artist_shell.state.player.borrow().current.clone() else {
            return;
        };
        if let Some(artist_id) = entry.artist_id {
            artist_shell.navigate(Route::ArtistDetail(artist_id));
        } else if !entry.artist.trim().is_empty() {
            artist_shell.navigate(Route::Search {
                query: entry.artist,
                kind: SearchKind::Artists,
            });
        }
    });

    let album_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.album, move || {
        let Some(entry) = album_shell.state.player.borrow().current.clone() else {
            return;
        };
        if let Some(album_id) = entry.album_id {
            album_shell.navigate(Route::AlbumDetail(album_id));
        } else if !entry.album.trim().is_empty() {
            album_shell.navigate(Route::Search {
                query: entry.album,
                kind: SearchKind::Albums,
            });
        }
    });

    let controller = shell.controller.clone();
    shell
        .player_controls
        .mute_button
        .connect_clicked(move |_| controller.toggle_mute());

    let seek_shell = Rc::clone(shell);
    shell
        .player_controls
        .progress
        .connect_value_changed(move |scale| {
            if seek_shell.state.updating_player_controls.get() {
                return;
            }
            seek_shell.state.seeking_player_controls.set(true);
            let generation = seek_shell.state.seek_generation.get().saturating_add(1);
            seek_shell.state.seek_generation.set(generation);
            let seconds = scale.value() as u32;
            seek_shell
                .player_controls
                .elapsed
                .set_text(&format_duration(seconds));
            let seek_shell = Rc::clone(&seek_shell);
            glib::timeout_add_local_once(Duration::from_millis(350), move || {
                if seek_shell.state.seek_generation.get() == generation {
                    seek_shell.controller.seek(seconds);
                    seek_shell.state.seeking_player_controls.set(false);
                }
            });
        });

    let volume_shell = Rc::clone(shell);
    shell
        .player_controls
        .volume
        .connect_value_changed(move |scale| {
            if volume_shell.state.updating_player_controls.get() {
                return;
            }
            volume_shell.controller.set_volume(scale.value());
        });
}

fn install_mpris(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        let player = match MprisPlayer::builder("io.github.screwys.Rufin")
            .identity("Rufin")
            .desktop_entry("io.github.screwys.Rufin")
            .supported_uri_schemes(["http", "https"])
            .supported_mime_types(["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"])
            .can_play(true)
            .can_pause(true)
            .can_go_next(true)
            .can_go_previous(true)
            .can_seek(true)
            .can_control(true)
            .build()
            .await
        {
            Ok(player) => Rc::new(player),
            Err(error) => {
                warn!(%error, "failed to start MPRIS server");
                return;
            }
        };

        let controller = shell.controller.clone();
        player.connect_play_pause(move |_| controller.play_pause());
        let play_shell = Rc::clone(&shell);
        player.connect_play(move |_| {
            let state = play_shell.state.player.borrow().state;
            if !matches!(state, PlaybackState::Playing | PlaybackState::Buffering) {
                play_shell.controller.play_pause();
            }
        });
        let pause_shell = Rc::clone(&shell);
        player.connect_pause(move |_| {
            let state = pause_shell.state.player.borrow().state;
            if matches!(state, PlaybackState::Playing | PlaybackState::Buffering) {
                pause_shell.controller.play_pause();
            }
        });
        let controller = shell.controller.clone();
        player.connect_stop(move |_| controller.stop());
        let controller = shell.controller.clone();
        player.connect_next(move |_| controller.next_track());
        let controller = shell.controller.clone();
        player.connect_previous(move |_| controller.previous_track());
        let controller = shell.controller.clone();
        let seek_shell = Rc::clone(&shell);
        player.connect_seek(move |_, offset| {
            let current = seek_shell.state.player.borrow().position_seconds;
            let offset_seconds = offset.as_micros() / 1_000_000;
            let target = if offset_seconds.is_negative() {
                current.saturating_sub(offset_seconds.unsigned_abs() as u32)
            } else {
                current.saturating_add(offset_seconds as u32)
            };
            controller.seek(target);
        });
        let controller = shell.controller.clone();
        player.connect_set_position(move |_, _, position| {
            controller.seek((position.as_micros() / 1_000_000) as u32);
        });

        let run_player = Rc::clone(&player);
        glib::spawn_future_local(async move {
            run_player.run().await;
        });
        *shell.state.mpris_player.borrow_mut() = Some(player);
        shell.update_mpris_player();
    });
}

fn mpris_track_id(track_id: &str) -> MprisTrackId {
    let mut encoded = String::with_capacity(track_id.len() * 2);
    for byte in track_id.as_bytes() {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    MprisTrackId::try_from(format!("/io/github/screwys/Rufin/track/{encoded}"))
        .unwrap_or(MprisTrackId::NO_TRACK)
}

fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        shell.controller.poll_playback_events();
        while let Ok(event) = receiver.try_recv() {
            match event {
                ControllerEvent::Snapshot(snapshot) => {
                    *shell.state.library.borrow_mut() = *snapshot;
                    shell.render_current_route();
                }
                ControllerEvent::Queue(queue) => {
                    *shell.state.queue.borrow_mut() = *queue;
                    shell.render_queue_panel();
                    shell.update_bottom_player();
                }
                ControllerEvent::Playback(player) => {
                    let previous_track = shell
                        .state
                        .player
                        .borrow()
                        .current
                        .as_ref()
                        .map(|entry| entry.track_id.clone());
                    let next_snapshot = *player;
                    let next_track = next_snapshot
                        .current
                        .as_ref()
                        .map(|entry| entry.track_id.clone());
                    *shell.state.player.borrow_mut() = next_snapshot.clone();
                    if previous_track != next_track {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                        shell.render_queue_panel();
                        if next_track.is_some() {
                            shell.controller.request_lyrics_for_current();
                        }
                        shell.notify_now_playing(&next_snapshot);
                    }
                    shell.update_bottom_player();
                    shell.update_lyrics_highlight();
                    shell.update_mpris_player();
                }
                ControllerEvent::Lyrics(lyrics) => {
                    *shell.state.lyrics.borrow_mut() = *lyrics;
                    shell.render_queue_panel();
                }
                ControllerEvent::CoverReady { key, path } => {
                    shell.apply_cover_ready(&key, &path);
                }
                ControllerEvent::LoginStatus(status) => {
                    let should_render = {
                        let mut library = shell.state.library.borrow_mut();
                        library.sync_status = status;
                        route_displays_sync_status(
                            shell.state.routes.borrow().current(),
                            library.first_run,
                        )
                    };
                    if should_render {
                        shell.render_current_route();
                    }
                }
                ControllerEvent::Error(error) => {
                    warn!(%error, "controller error");
                    let mut library = shell.state.library.borrow_mut();
                    library.sync_status = "Action failed.".to_string();
                    library.last_error = Some(error);
                    drop(library);
                    shell.render_current_route();
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn start_ui_perf_run(shell: &Rc<Shell>, app: &adw::Application) {
    let Some(perf) = shell.state.perf.clone() else {
        return;
    };
    println!(
        "RUFIN_PERF start max_gap_ms={} route_ms={} duration_ms={} asset_ms={} output={} terminal_only=true",
        perf.options.max_gap_ms,
        perf.options.route_ms,
        perf.options.duration_ms,
        perf.options.asset_ms,
        perf.options
            .output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "stdout_only".to_string())
    );
    let heartbeat = Rc::new(RefCell::new(Some(start_ui_perf_heartbeat(Rc::clone(
        &perf,
    )))));
    let routes = ui_perf_routes(shell);
    let runs = Rc::new(RefCell::new(ui_perf_plan(
        routes,
        perf.options.duration_ms,
        perf.options.route_ms,
    )));
    let shell = Rc::clone(shell);
    let app = app.clone();
    glib::timeout_add_local_once(Duration::from_millis(250), move || {
        run_next_ui_perf_route(shell, app, perf, runs, heartbeat);
    });
}

fn start_ui_perf_heartbeat(perf: Rc<UiPerfMonitor>) -> glib::SourceId {
    let last_tick = Rc::new(RefCell::new(Instant::now()));
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let now = Instant::now();
        let gap = now.saturating_duration_since(*last_tick.borrow());
        *last_tick.borrow_mut() = now;
        perf.record_tick_gap(gap);
        glib::ControlFlow::Continue
    })
}

fn run_next_ui_perf_route(
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    runs: Rc<RefCell<VecDeque<(Route, UiPerfScenario)>>>,
    heartbeat: Rc<RefCell<Option<glib::SourceId>>>,
) {
    let Some((route, scenario)) = runs.borrow_mut().pop_front() else {
        if let Some(source) = heartbeat.borrow_mut().take() {
            source.remove();
        }
        if perf.pending_assets() > 0 {
            glib::timeout_add_local_once(
                Duration::from_millis(perf.options.asset_ms.saturating_mul(2)),
                move || {
                    finish_ui_perf_run(perf, app);
                },
            );
            return;
        }
        finish_ui_perf_run(perf, app);
        return;
    };

    let route_name = format!("{route:?}");
    println!(
        "RUFIN_PERF route_begin route={route_name} scenario={}",
        scenario.name()
    );
    shell.navigate(route);

    let shell_for_scroll = Rc::clone(&shell);
    let app_for_next = app.clone();
    let perf_for_scroll = Rc::clone(&perf);
    let runs_for_next = Rc::clone(&runs);
    let heartbeat_for_next = Rc::clone(&heartbeat);
    glib::timeout_add_local_once(Duration::from_millis(120), move || {
        perf_for_scroll.begin_scroll(route_name.clone(), scenario);
        let scroll_source = Rc::new(RefCell::new(None::<glib::SourceId>));
        if let Some(scroller) =
            find_largest_scrolled_window(&shell_for_scroll.route_host.clone().upcast())
        {
            let direction = Rc::new(Cell::new(1.0_f64));
            let jump_index = Rc::new(Cell::new(0_usize));
            let perf_for_tick = Rc::clone(&perf_for_scroll);
            let route_for_tick = route_name.clone();
            let direction_for_tick = Rc::clone(&direction);
            let jump_index_for_tick = Rc::clone(&jump_index);
            let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                let adjustment = scroller.vadjustment();
                let page_size = adjustment.page_size().max(1.0);
                let max_value = (adjustment.upper() - page_size).max(0.0);
                if max_value > 1.0 {
                    let next = ui_perf_next_scroll_value(
                        scenario,
                        &adjustment,
                        max_value,
                        &direction_for_tick,
                        &jump_index_for_tick,
                    );
                    adjustment.set_value(next);
                    perf_for_tick.record_scroll_step(&route_for_tick, next, max_value);
                }
                glib::ControlFlow::Continue
            });
            *scroll_source.borrow_mut() = Some(id);
        } else {
            perf_for_scroll.record_scroll_note(&route_name, "no_scrolled_window");
        }

        glib::timeout_add_local_once(
            Duration::from_millis(perf_for_scroll.options.route_ms),
            move || {
                if let Some(source) = scroll_source.borrow_mut().take() {
                    source.remove();
                }
                perf_for_scroll.finish_scroll();
                run_next_ui_perf_route(
                    shell_for_scroll,
                    app_for_next,
                    perf_for_scroll,
                    runs_for_next,
                    heartbeat_for_next,
                );
            },
        );
    });
}

fn finish_ui_perf_run(perf: Rc<UiPerfMonitor>, app: adw::Application) {
    let report = perf.report();
    print!("{report}");
    if let Some(path) = &perf.options.output
        && let Err(error) = std::fs::write(path, &report)
    {
        eprintln!(
            "RUFIN_PERF failed_to_write_report path={} error={error}",
            path.display()
        );
    }
    let failed = perf.failed();
    app.quit();
    if failed {
        std::process::exit(1);
    }
}

fn ui_perf_routes(shell: &Shell) -> Vec<Route> {
    let library = shell.state.library.borrow();
    let mut routes = vec![Route::Home, Route::Albums];
    let image_album = library
        .albums
        .iter()
        .find(|album| album.image_ref.is_some())
        .or_else(|| library.albums.first());
    if let Some(album) = image_album {
        routes.push(Route::AlbumDetail(album.id.clone()));
    }
    if let Some(album) = library
        .albums
        .iter()
        .find(|album| album.image_ref.is_none())
        .filter(|album| image_album.is_none_or(|image_album| image_album.id != album.id))
    {
        routes.push(Route::AlbumDetail(album.id.clone()));
    }
    routes.push(Route::Favorites);
    routes.push(Route::Artists);
    let image_artist = library
        .artists
        .iter()
        .find(|artist| artist.image_ref.is_some())
        .or_else(|| library.artists.first());
    if let Some(artist) = image_artist {
        routes.push(Route::ArtistDetail(artist.id.clone()));
    }
    if let Some(artist) = library
        .artists
        .iter()
        .find(|artist| artist.image_ref.is_none())
        .filter(|artist| image_artist.is_none_or(|image_artist| image_artist.id != artist.id))
    {
        routes.push(Route::ArtistDetail(artist.id.clone()));
    }
    routes.push(Route::AlbumArtists);
    if let Some(artist) = library
        .album_artists
        .iter()
        .find(|artist| artist.image_ref.is_some())
        .or_else(|| library.album_artists.first())
    {
        routes.push(Route::ArtistDetail(artist.id.clone()));
    }
    routes.push(Route::Genres);
    if let Some(genre) = library
        .genres
        .iter()
        .find(|genre| genre.image_ref.is_some())
        .or_else(|| library.genres.first())
    {
        routes.push(Route::GenreDetail(genre.id.clone()));
    }
    routes.push(Route::Playlists);
    if let Some(playlist) = library
        .playlists
        .iter()
        .find(|playlist| playlist.image_ref.is_some())
        .or_else(|| library.playlists.first())
    {
        routes.push(Route::PlaylistDetail(playlist.id.clone()));
    }
    let search_query = library
        .albums
        .first()
        .map(|album| album.title.clone())
        .or_else(|| library.tracks.first().map(|track| track.title.clone()))
        .unwrap_or_else(|| "music".to_string());
    routes.push(Route::Search {
        query: search_query,
        kind: SearchKind::All,
    });
    routes.extend([
        Route::Tracks,
        Route::Settings,
        Route::Albums,
        Route::Tracks,
        Route::Albums,
    ]);
    routes
}

fn ui_perf_plan(
    routes: Vec<Route>,
    duration_ms: u64,
    route_ms: u64,
) -> VecDeque<(Route, UiPerfScenario)> {
    let base = routes
        .into_iter()
        .flat_map(|route| {
            UiPerfScenario::ALL
                .into_iter()
                .map(move |scenario| (route.clone(), scenario))
        })
        .collect::<Vec<_>>();
    let run_ms = route_ms.saturating_add(140).max(1);
    let needed = ((duration_ms.saturating_add(run_ms - 1)) / run_ms).max(base.len() as u64);
    base.iter().cloned().cycle().take(needed as usize).collect()
}

fn ui_perf_next_scroll_value(
    scenario: UiPerfScenario,
    adjustment: &gtk::Adjustment,
    max_value: f64,
    direction: &Cell<f64>,
    jump_index: &Cell<usize>,
) -> f64 {
    match scenario {
        UiPerfScenario::HumanScroll => {
            let step = (adjustment.page_size() * 0.20).clamp(80.0, 180.0);
            bounce_scroll_value(adjustment.value(), step, max_value, direction)
        }
        UiPerfScenario::FastScroll => {
            let step = (adjustment.page_size() * 0.95).max(260.0);
            bounce_scroll_value(adjustment.value(), step, max_value, direction)
        }
        UiPerfScenario::Jump => {
            let points = [0.0, 0.25, 0.85, 0.45, 1.0, 0.10, 0.65, 0.0];
            let index = jump_index.get();
            jump_index.set(index.saturating_add(1));
            max_value * points[index % points.len()]
        }
        UiPerfScenario::DragSweep => {
            let index = jump_index.get();
            jump_index.set(index.saturating_add(1));
            let phase = (index % 64) as f64 / 63.0;
            let fraction = if (index / 64).is_multiple_of(2) {
                phase
            } else {
                1.0 - phase
            };
            max_value * fraction
        }
    }
}

fn bounce_scroll_value(current: f64, step: f64, max_value: f64, direction: &Cell<f64>) -> f64 {
    let mut next = current + direction.get() * step;
    if next >= max_value {
        next = max_value;
        direction.set(-1.0);
    } else if next <= 0.0 {
        next = 0.0;
        direction.set(1.0);
    }
    next
}

fn find_largest_scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut best = None;
    collect_largest_scrolled_window(widget, &mut best);
    best.map(|(scroller, _)| scroller)
}

fn collect_largest_scrolled_window(
    widget: &gtk::Widget,
    best: &mut Option<(gtk::ScrolledWindow, f64)>,
) {
    if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
        let adjustment = scroller.vadjustment();
        let score = (adjustment.upper() - adjustment.page_size()).max(0.0);
        if best
            .as_ref()
            .is_none_or(|(_, best_score)| score > *best_score)
        {
            *best = Some((scroller, score));
        }
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect_largest_scrolled_window(&widget, best);
        child = widget.next_sibling();
    }
}

fn build_normal_navigation(shell: &Rc<Shell>) {
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search")));
    search.set_margin_top(18);
    search.set_margin_start(16);
    search.set_margin_end(16);
    let search_shell = Rc::clone(shell);
    search.connect_activate(move |entry| {
        let query = entry.text().trim().to_string();
        if query.is_empty() {
            return;
        }
        search_shell.controller.search(query.clone());
        search_shell.navigate(Route::Search {
            query,
            kind: SearchKind::All,
        });
    });
    shell.normal_nav.append(&search);

    let heading = gtk::Label::new(Some(&tr("My Library")));
    heading.add_css_class("nav-heading");
    heading.set_xalign(0.0);
    heading.set_margin_start(18);
    heading.set_margin_top(18);
    shell.normal_nav.append(&heading);

    for item in nav_items() {
        shell.normal_nav.append(&nav_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
            false,
        ));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.normal_nav.append(&spacer);

    let server = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    server.add_css_class("server-card");
    server.set_margin_start(14);
    server.set_margin_end(14);
    server.set_margin_bottom(14);
    server.append(&gtk::Image::from_icon_name("audio-x-generic-symbolic"));
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name = gtk::Label::new(Some("Rufin"));
    name.set_xalign(0.0);
    let subtitle = gtk::Label::new(Some(&tr("Cached library")));
    subtitle.add_css_class("muted");
    subtitle.set_xalign(0.0);
    labels.append(&name);
    labels.append(&subtitle);
    server.append(&labels);
    shell.normal_nav.append(&server);
}

fn build_compact_navigation(shell: &Rc<Shell>) {
    shell.compact_nav.append(&rail_button(
        shell,
        "open-menu-symbolic",
        "Menu",
        Route::Home,
    ));
    for item in nav_items() {
        shell.compact_nav.append(&rail_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
        ));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.compact_nav.append(&spacer);
    shell.compact_nav.append(&rail_button(
        shell,
        "audio-x-generic-symbolic",
        "Rufin",
        Route::Settings,
    ));
}

#[derive(Clone)]
struct NavItem {
    icon_name: &'static str,
    label: &'static str,
    route: Route,
}

fn nav_items() -> Vec<NavItem> {
    vec![
        NavItem {
            icon_name: "go-home-symbolic",
            label: "Home",
            route: Route::Home,
        },
        NavItem {
            icon_name: "emblem-favorite-symbolic",
            label: "Favorites",
            route: Route::Favorites,
        },
        NavItem {
            icon_name: "media-optical-symbolic",
            label: "Albums",
            route: Route::Albums,
        },
        NavItem {
            icon_name: "audio-x-generic-symbolic",
            label: "Tracks",
            route: Route::Tracks,
        },
        NavItem {
            icon_name: "avatar-default-symbolic",
            label: "Album Artists",
            route: Route::AlbumArtists,
        },
        NavItem {
            icon_name: "system-users-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        NavItem {
            icon_name: "flag-symbolic",
            label: "Genres",
            route: Route::Genres,
        },
        NavItem {
            icon_name: "folder-music-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
    ]
}

fn nav_button(
    shell: &Rc<Shell>,
    icon_name: &str,
    label: &str,
    route: Route,
    compact: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("nav-button");
    button.add_css_class("flat");
    if compact {
        button.add_css_class("rail-button");
    }
    button.set_tooltip_text(Some(&tr(label)));

    let content = gtk::Box::new(
        if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        },
        8,
    );
    content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(icon_name);
    content.append(&icon);
    if compact {
        icon.set_pixel_size(24);
        let text = gtk::Label::new(Some(&tr(label)));
        text.add_css_class("rail-label");
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&text);
    } else {
        let text = gtk::Label::new(Some(&tr(label)));
        text.set_xalign(0.0);
        content.append(&text);
    }
    button.set_child(Some(&content));

    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| shell.navigate(route.clone()));
    button
}

fn rail_button(shell: &Rc<Shell>, icon_name: &str, label: &str, route: Route) -> gtk::Button {
    nav_button(shell, icon_name, label, route, true)
}

fn repeat_label(repeat_mode: RepeatMode) -> &'static str {
    match repeat_mode {
        RepeatMode::Off => "Repeat off",
        RepeatMode::One => "Repeat one",
        RepeatMode::All => "Repeat all",
    }
}

fn playback_state_label(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Stopped => "Play",
        PlaybackState::Paused => "Resume",
        PlaybackState::Buffering => "Pause",
        PlaybackState::Playing => "Pause",
    }
}

fn album_model(albums: &[Album]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    append_albums_to_model(&model, albums.iter().cloned());
    model
}

fn append_albums_to_model(model: &gio::ListStore, albums: impl IntoIterator<Item = Album>) {
    let additions = albums
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}

fn artist_model(artists: &[Artist]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    append_artists_to_model(&model, artists.iter().cloned());
    model
}

fn append_artists_to_model(model: &gio::ListStore, artists: impl IntoIterator<Item = Artist>) {
    let additions = artists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}

fn genre_model(genres: &[Genre]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    append_genres_to_model(&model, genres.iter().cloned());
    model
}

fn append_genres_to_model(model: &gio::ListStore, genres: impl IntoIterator<Item = Genre>) {
    let additions = genres
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}

fn playlist_model(playlists: &[Playlist]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    append_playlists_to_model(&model, playlists.iter().cloned());
    model
}

fn append_playlists_to_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    let additions = playlists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}

fn finish_grid_page(cursor: &PagedGridCursor, previous_offset: usize, count: usize, total: usize) {
    let next_offset = previous_offset.saturating_add(count);
    cursor.offset.set(next_offset);
    cursor.total.set(if count == 0 {
        next_offset
    } else {
        total.max(next_offset)
    });
    cursor.loading.set(false);
}

fn connect_paged_grid_loader(scroller: &gtk::ScrolledWindow, load_next: Rc<dyn Fn()>) {
    let load_for_edge = Rc::clone(&load_next);
    scroller.connect_edge_reached(move |_, position| {
        if position == gtk::PositionType::Bottom {
            load_for_edge();
        }
    });

    let scroller_for_idle = scroller.clone();
    glib::idle_add_local_once(move || {
        if scroller_needs_more_items(&scroller_for_idle) {
            load_next();
        }
    });
}

fn scroller_needs_more_items(scroller: &gtk::ScrolledWindow) -> bool {
    let adjustment = scroller.vadjustment();
    adjustment.upper() <= adjustment.page_size() + 1.0
}

fn set_track_sort_button_content(button: &gtk::Button, settings: &TrackTableSettings) {
    let sort_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    sort_content.append(&gtk::Label::new(Some(&tr(settings.sort_key.title()))));
    sort_content.append(&gtk::Image::from_icon_name(if settings.descending {
        "view-sort-descending-symbolic"
    } else {
        "view-sort-ascending-symbolic"
    }));
    button.set_child(Some(&sort_content));
}

fn set_track_table_columns(
    shell: &Rc<Shell>,
    table: &gtk::ColumnView,
    settings: &TrackTableSettings,
) {
    let columns = table.columns();
    while columns.n_items() > 0 {
        let Some(column) = columns
            .item(0)
            .and_then(|item| item.downcast::<gtk::ColumnViewColumn>().ok())
        else {
            break;
        };
        table.remove_column(&column);
    }

    for column in &settings.visible_columns {
        table.append_column(&track_table_column(shell, *column));
    }
}

fn track_table_column(shell: &Rc<Shell>, column: TrackTableColumn) -> gtk::ColumnViewColumn {
    match column {
        TrackTableColumn::TrackNumber => {
            track_column("#", 54, |track| track.track_number.to_string())
        }
        TrackTableColumn::Title => track_column("Title", 240, |track| track.title.clone()),
        TrackTableColumn::Artist => track_link_column(shell, "Artist", 180, |track| {
            (
                track.artist.clone(),
                track
                    .artist_id
                    .as_ref()
                    .map(|artist_id| Route::ArtistDetail(artist_id.clone())),
            )
        }),
        TrackTableColumn::Album => track_link_column(shell, "Album", 220, |track| {
            (
                track.album.clone(),
                Some(Route::AlbumDetail(track.album_id.clone())),
            )
        }),
        TrackTableColumn::Year => track_column("Year", 70, |track| track.year.to_string()),
        TrackTableColumn::Duration => track_column("Duration", 90, |track| {
            format_duration(track.duration_seconds)
        }),
        TrackTableColumn::Favorite => track_favorite_column(shell),
    }
}

fn populate_track_model(
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &TrackTableSettings,
    query: &str,
) {
    let query = query.trim().to_lowercase();
    let mut filtered = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks(&mut filtered, settings);
    let additions = filtered
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_tracks_to_model(model: &gio::ListStore, tracks: Vec<Track>) {
    let additions = tracks
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(model.n_items(), 0, &additions);
}

fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}

fn sort_tracks(tracks: &mut [Track], settings: &TrackTableSettings) {
    tracks.sort_by(|left, right| {
        let ordering = match settings.sort_key {
            TrackSortKey::TrackNumber => left
                .disc_number
                .cmp(&right.disc_number)
                .then(left.track_number.cmp(&right.track_number))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase())),
            TrackSortKey::Title => left.title.to_lowercase().cmp(&right.title.to_lowercase()),
            TrackSortKey::Artist => left
                .artist
                .to_lowercase()
                .cmp(&right.artist.to_lowercase())
                .then_with(|| left.album.to_lowercase().cmp(&right.album.to_lowercase()))
                .then(left.track_number.cmp(&right.track_number)),
            TrackSortKey::Album => left
                .album
                .to_lowercase()
                .cmp(&right.album.to_lowercase())
                .then(left.disc_number.cmp(&right.disc_number))
                .then(left.track_number.cmp(&right.track_number)),
            TrackSortKey::Year => left
                .year
                .cmp(&right.year)
                .then_with(|| left.album.to_lowercase().cmp(&right.album.to_lowercase())),
            TrackSortKey::Duration => left.duration_seconds.cmp(&right.duration_seconds),
            TrackSortKey::Favorite => left.favorite.cmp(&right.favorite),
        };
        if settings.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn track_sort_index(sort_key: TrackSortKey) -> u32 {
    TrackSortKey::all()
        .iter()
        .position(|candidate| *candidate == sort_key)
        .unwrap_or(0) as u32
}

fn track_sort_from_index(index: u32) -> TrackSortKey {
    TrackSortKey::all()
        .get(index as usize)
        .copied()
        .unwrap_or(TrackSortKey::TrackNumber)
}

fn route_uses_responsive_cards(route: &Route) -> bool {
    matches!(
        route,
        Route::Home
            | Route::Albums
            | Route::Artists
            | Route::AlbumArtists
            | Route::ArtistDetail(_)
            | Route::Search { .. }
    )
}

fn route_displays_sync_status(route: &Route, first_run: bool) -> bool {
    first_run || matches!(route, Route::Settings)
}

fn nonzero_usize(value: usize) -> Option<usize> {
    if value == 0 { None } else { Some(value) }
}

fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}

fn track_column<F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        list_item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(child) = list_item.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk::Label>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>();
        label.set_text(&value(&track));
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}

fn track_link_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> (String, Option<Route>) + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    let shell = Rc::clone(shell);

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let (text, route) = value(&boxed.borrow::<Track>());
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);

        let Some(route) = route else {
            list_item.set_child(Some(&label));
            return;
        };

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Fill);
        button.set_hexpand(true);
        button.set_cursor_from_name(Some("pointer"));

        add_link_hover(button.upcast_ref(), &label, &text);

        button.set_child(Some(&label));

        let shell = Rc::clone(&shell);
        button.connect_clicked(move |_| shell.navigate(route.clone()));
        list_item.set_child(Some(&button));
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}

fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let button = favorite_icon_button("Favorite");
        set_favorite_button_active(&button, track.favorite);
        let controller = shell.controller.clone();
        button.connect_clicked(move |_| controller.toggle_track_favorite(track.clone()));
        list_item.set_child(Some(&button));
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr("Favorite")), Some(factory));
    column.set_fixed_width(76);
    column.set_resizable(false);
    column
}

fn add_link_hover(target: &gtk::Widget, label: &gtk::Label, text: &str) {
    let escaped_text = glib::markup_escape_text(text);
    let enter_label = label.clone();
    let enter_markup = format!("<u>{escaped_text}</u>");
    let leave_label = label.clone();
    let leave_text = text.to_string();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&enter_markup);
    });
    motion.connect_leave(move |_| {
        leave_label.remove_css_class("hovered-link");
        leave_label.set_text(&leave_text);
    });
    target.add_controller(motion);
}

fn add_dynamic_link_hover(target: &gtk::Widget, label: &gtk::Label) {
    let enter_label = label.clone();
    let leave_label = label.clone();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        let text = enter_label.text();
        let escaped_text = glib::markup_escape_text(text.as_str());
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&format!("<u>{escaped_text}</u>"));
    });
    motion.connect_leave(move |_| {
        let text = leave_label.text().to_string();
        leave_label.remove_css_class("hovered-link");
        leave_label.set_text(&text);
    });
    target.add_controller(motion);
}

fn render_home_album_page(
    shell: &Rc<Shell>,
    row: &gtk::Box,
    previous: &gtk::Button,
    next: &gtk::Button,
    section_kind: HomeSectionKind,
    albums: &[Album],
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    if albums.is_empty() {
        previous.set_sensitive(false);
        next.set_sensitive(false);
        return;
    }

    let width = home_album_content_width(shell);
    let page_start = {
        let mut states = shell.state.home_section_state.borrow_mut();
        let existing_page_size = states.get(&section_kind).map(|state| state.page_size);
        let page_size = home_album_page_size(width, existing_page_size);
        let state = states.entry(section_kind).or_insert(HomeSectionState {
            page_start: 0,
            page_size,
        });
        if state.page_size != page_size {
            state.page_start -= state.page_start % page_size.max(1);
            state.page_size = page_size;
        }
        state.page_start = clamp_home_album_page_start(state.page_start, page_size, albums.len());
        state.page_start
    };
    let page_size = {
        shell
            .state
            .home_section_state
            .borrow()
            .get(&section_kind)
            .map(|state| state.page_size)
            .unwrap_or_else(|| home_album_page_size(width, None))
    };
    let card_size = home_album_card_size(width, page_size);
    let page_end = page_start.saturating_add(page_size).min(albums.len());

    previous.set_sensitive(page_start > 0);
    next.set_sensitive(page_end < albums.len());

    for album in &albums[page_start..page_end] {
        let card = shell.album_card_with_size(album, card_size);
        row.append(&card);
    }
}

fn home_album_page_size(width: i32, current_page_size: Option<usize>) -> usize {
    let width = width.max(1);
    let mut page_size = current_page_size
        .unwrap_or_else(|| {
            let item_width = HOME_ALBUM_TARGET_SIZE + HOME_ALBUM_GAP;
            ((width + HOME_ALBUM_GAP) / item_width).clamp(2, 8) as usize
        })
        .clamp(2, 8);

    while page_size > 2 && home_album_raw_card_size(width, page_size) < HOME_ALBUM_MIN_SIZE {
        page_size -= 1;
    }
    while page_size < 8 && home_album_raw_card_size(width, page_size) > HOME_ALBUM_MAX_SIZE {
        page_size += 1;
    }

    page_size
}

fn clamp_content_split_position(split_width: i32, position: i32) -> i32 {
    if split_width <= 1 {
        return position;
    }
    let min_right_width = split_width * RIGHT_PANEL_MIN_PERCENT / 100;
    let max_right_width = split_width * RIGHT_PANEL_MAX_PERCENT / 100;
    let min_position = split_width - max_right_width;
    let max_position = split_width - min_right_width;
    position.clamp(min_position, max_position)
}

fn clamp_home_album_page_start(page_start: usize, page_size: usize, album_count: usize) -> usize {
    if album_count == 0 {
        return 0;
    }
    let page_size = page_size.max(1);
    let last_page_start = ((album_count - 1) / page_size) * page_size;
    page_start.min(last_page_start)
}

fn home_album_content_width(shell: &Shell) -> i32 {
    let mut route_width = shell.route_host.width();
    let split_position = shell.content_split.position();
    if split_position > 1 {
        route_width = if route_width > 1 {
            route_width.min(split_position)
        } else {
            split_position
        };
    }
    if route_width <= 1 {
        let split_width = shell.content_split.width();
        if split_width > 1 {
            route_width = split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS;
        }
    }
    let horizontal_margins = 56;
    (route_width - horizontal_margins).max(HOME_ALBUM_MIN_SIZE)
}

fn home_album_card_size(width: i32, page_size: usize) -> i32 {
    home_album_raw_card_size(width, page_size).clamp(1, HOME_ALBUM_MAX_SIZE)
}

fn home_album_raw_card_size(width: i32, page_size: usize) -> i32 {
    let page_size = page_size.max(1) as i32;
    let gaps = HOME_ALBUM_GAP * (page_size - 1);
    ((width - gaps).max(page_size)) / page_size
}

fn album_card_widget_with_size(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: Option<&AppController>,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, -1);
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    let cover = album_cover_tile(shell, album, size, controller);
    card.append(&cover);

    let title = gtk::Label::new(Some(&album.title));
    title.add_css_class("album-title");
    title.set_xalign(0.0);
    title.set_width_request(size);
    title.set_size_request(size, -1);
    title.set_max_width_chars((size / 8).max(8));
    title.set_lines(2);
    title.set_wrap(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    add_link_hover(title.upcast_ref(), &title, &album.title);
    let artist = gtk::Label::new(Some(&album.artist));
    artist.add_css_class("muted");
    artist.set_xalign(0.0);
    artist.set_width_request(size);
    artist.set_size_request(size, -1);
    artist.set_max_width_chars((size / 8).max(8));
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    add_link_hover(artist.upcast_ref(), &artist, &album.artist);
    let year = gtk::Label::new(Some(&album.year.to_string()));
    year.add_css_class("muted");
    year.set_xalign(0.0);
    year.set_width_request(size);

    card.append(&title);
    card.append(&artist);
    card.append(&year);
    card.upcast()
}

fn album_cover_tile(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: Option<&AppController>,
) -> gtk::Widget {
    let overlay = gtk::Overlay::new();
    overlay.set_width_request(size);
    overlay.set_height_request(size);
    overlay.set_size_request(size, size);
    overlay.set_hexpand(false);
    overlay.set_halign(gtk::Align::Start);

    let album_button = gtk::Button::new();
    album_button.add_css_class("album-cover-button");
    album_button.add_css_class("flat");
    album_button.set_width_request(size);
    album_button.set_height_request(size);
    album_button.set_size_request(size, size);
    album_button.set_hexpand(false);
    album_button.set_halign(gtk::Align::Start);
    album_button.set_child(Some(&shell.cover_tile_for(
        album.image_ref.as_ref(),
        album.color_seed,
        size,
        GRID_COVER_SIZE,
    )));
    let open_shell = Rc::clone(shell);
    let open_album_id = album.id.clone();
    album_button
        .connect_clicked(move |_| open_shell.navigate(Route::AlbumDetail(open_album_id.clone())));
    overlay.set_child(Some(&album_button));

    let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shade.add_css_class("cover-hover-layer");
    shade.set_width_request(size);
    shade.set_height_request(size);
    shade.set_size_request(size, size);
    shade.set_can_target(false);
    shade.set_visible(false);
    overlay.add_overlay(&shade);

    let play = icon_button("media-playback-start-symbolic", "Play album");
    play.add_css_class("cover-hover-button");
    play.add_css_class("cover-play-button");
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    play.set_visible(false);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        play.connect_clicked(move |_| controller.play_album_now(album_id.clone()));
    }
    overlay.add_overlay(&play);

    let favorite = favorite_icon_button("Favorite");
    favorite.add_css_class("cover-hover-button");
    favorite.add_css_class("cover-favorite-button");
    favorite.set_halign(gtk::Align::End);
    favorite.set_valign(gtk::Align::Start);
    favorite.set_margin_top(8);
    favorite.set_margin_end(8);
    favorite.set_visible(false);
    set_favorite_button_active(&favorite, album.favorite);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album = album.clone();
        favorite.connect_clicked(move |_| controller.toggle_album_favorite(album.clone()));
    }
    overlay.add_overlay(&favorite);

    let motion = gtk::EventControllerMotion::new();
    let shade_for_enter = shade.clone();
    let play_for_enter = play.clone();
    let favorite_for_enter = favorite.clone();
    motion.connect_enter(move |_, _, _| {
        shade_for_enter.set_visible(true);
        play_for_enter.set_visible(true);
        favorite_for_enter.set_visible(true);
    });
    motion.connect_leave(move |_| {
        shade.set_visible(false);
        play.set_visible(false);
        favorite.set_visible(false);
    });
    overlay.add_controller(motion);

    overlay.upcast()
}

fn artist_card_widget_with_size(shell: &Rc<Shell>, artist: &Artist, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, -1);
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    card.append(&shell.cover_tile_for(
        artist.image_ref.as_ref(),
        stable_seed(artist.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));

    let name = gtk::Label::new(Some(&artist.name));
    name.add_css_class("album-title");
    name.set_xalign(0.0);
    name.set_width_request(size);
    name.set_size_request(size, -1);
    name.set_max_width_chars((size / 8).max(8));
    name.set_lines(2);
    name.set_wrap(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let counts = gtk::Label::new(Some(&format!(
        "{} {} / {} {}",
        artist.album_count,
        tr("albums"),
        artist.track_count,
        tr("tracks")
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    counts.set_width_request(size);
    counts.set_size_request(size, -1);
    counts.set_ellipsize(gtk::pango::EllipsizeMode::End);

    card.append(&name);
    card.append(&counts);
    card.upcast()
}

fn genre_card_widget_with_size(shell: &Rc<Shell>, genre: &Genre, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, -1);
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    card.append(&shell.cover_tile_for(
        genre.image_ref.as_ref(),
        stable_seed(genre.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));

    let name = gtk::Label::new(Some(&genre.name));
    name.add_css_class("album-title");
    name.set_xalign(0.0);
    name.set_width_request(size);
    name.set_lines(2);
    name.set_wrap(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let counts = gtk::Label::new(Some(&format!(
        "{} {} / {} {}",
        genre.album_count,
        tr("albums"),
        genre.track_count,
        tr("tracks")
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    counts.set_width_request(size);
    counts.set_ellipsize(gtk::pango::EllipsizeMode::End);
    card.append(&name);
    card.append(&counts);
    card.upcast()
}

fn playlist_card_widget_with_size(
    shell: &Rc<Shell>,
    playlist: &Playlist,
    size: i32,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, -1);
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    card.append(&shell.cover_tile_for(
        playlist.image_ref.as_ref(),
        stable_seed(playlist.id.as_str()),
        size,
        GRID_COVER_SIZE,
    ));

    let name = gtk::Label::new(Some(&playlist.name));
    name.add_css_class("album-title");
    name.set_xalign(0.0);
    name.set_width_request(size);
    name.set_lines(2);
    name.set_wrap(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let counts = gtk::Label::new(Some(&format!(
        "{} {} • {}",
        playlist.track_count,
        tr("tracks"),
        format_duration(playlist.duration_seconds)
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    counts.set_width_request(size);
    counts.set_ellipsize(gtk::pango::EllipsizeMode::End);
    card.append(&name);
    card.append(&counts);
    card.upcast()
}

impl ArtworkTile {
    fn new(size: i32, seed: u32) -> Self {
        let area = gtk::DrawingArea::new();
        area.add_css_class("cover-tile");
        area.add_css_class("card");
        area.set_content_width(size);
        area.set_content_height(size);
        area.set_width_request(size);
        area.set_height_request(size);
        area.set_size_request(size, size);
        area.set_hexpand(false);
        area.set_vexpand(false);
        area.set_halign(gtk::Align::Start);
        area.set_valign(gtk::Align::Start);

        let seed = Rc::new(Cell::new(seed));
        let pixbuf = Rc::new(RefCell::new(None::<Pixbuf>));
        let generation = Rc::new(Cell::new(0));
        let draw_seed = Rc::clone(&seed);
        let draw_pixbuf = Rc::clone(&pixbuf);
        area.set_draw_func(move |_, context, width, height| {
            clip_rounded_rect(context, width, height, 12.0);
            if let Some(pixbuf) = draw_pixbuf.borrow().as_ref() {
                draw_pixbuf_cover(context, pixbuf, width, height);
            } else {
                draw_fallback_cover(context, draw_seed.get(), width, height);
            }
        });

        Self {
            area,
            size,
            seed,
            pixbuf,
            generation,
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.area.clone().upcast()
    }

    fn size(&self) -> i32 {
        self.size
    }

    fn generation(&self) -> u64 {
        self.generation.get()
    }

    fn is_live_generation(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }

    fn set_seed(&self, seed: u32) {
        self.seed.set(seed);
        self.area.queue_draw();
    }

    fn set_pixbuf_if_current(&self, generation: u64, pixbuf: Pixbuf) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        *self.pixbuf.borrow_mut() = Some(pixbuf);
        self.area.queue_draw();
        true
    }

    fn clear_image(&self) {
        self.generation.set(self.generation.get().saturating_add(1));
        *self.pixbuf.borrow_mut() = None;
        self.area.queue_draw();
    }

    fn clear_image_if_current(&self, generation: u64) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        self.generation.set(self.generation.get().saturating_add(1));
        *self.pixbuf.borrow_mut() = None;
        self.area.queue_draw();
        true
    }
}

async fn load_cover_pixbuf(path: PathBuf, size: i32) -> Result<Pixbuf, glib::Error> {
    let file = gio::File::for_path(path);
    let stream = file.read_future(glib::Priority::LOW).await?;
    Pixbuf::from_stream_at_scale_future(&stream, size, size, true).await
}

fn apply_pixbuf_to_bindings(bindings: Vec<CoverBinding>, pixbuf: Pixbuf) {
    for binding in bindings {
        binding
            .tile
            .set_pixbuf_if_current(binding.generation, pixbuf.clone());
    }
}

fn draw_fallback_cover(context: &gtk::cairo::Context, seed: u32, width: i32, height: i32) {
    let red = f64::from((seed & 0xff) as u8) / 255.0;
    let green = f64::from(((seed >> 8) & 0xff) as u8) / 255.0;
    let blue = f64::from(((seed >> 16) & 0xff) as u8) / 255.0;
    context.set_source_rgb(red * 0.7 + 0.18, green * 0.7 + 0.18, blue * 0.7 + 0.18);
    context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
    let _paint = context.fill();

    context.set_source_rgba(1.0, 1.0, 1.0, 0.18);
    context.move_to(0.0, f64::from(height) * 0.2);
    context.line_to(f64::from(width) * 0.8, 0.0);
    context.line_to(f64::from(width), f64::from(height) * 0.8);
    context.line_to(f64::from(width) * 0.2, f64::from(height));
    context.close_path();
    let _fill = context.fill();
}

fn draw_pixbuf_cover(context: &gtk::cairo::Context, pixbuf: &Pixbuf, width: i32, height: i32) {
    let rect = cover_draw_rect(pixbuf.width(), pixbuf.height(), width, height);
    let _save = context.save();
    context.translate(rect.x, rect.y);
    context.scale(rect.scale, rect.scale);
    context.set_source_pixbuf(pixbuf, 0.0, 0.0);
    let _paint = context.paint();
    let _restore = context.restore();
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CoverDrawRect {
    x: f64,
    y: f64,
    scale: f64,
}

fn cover_draw_rect(
    image_width: i32,
    image_height: i32,
    target_width: i32,
    target_height: i32,
) -> CoverDrawRect {
    let image_width = image_width.max(1);
    let image_height = image_height.max(1);
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let scale = (f64::from(target_width) / f64::from(image_width))
        .max(f64::from(target_height) / f64::from(image_height));
    let drawn_width = f64::from(image_width) * scale;
    let drawn_height = f64::from(image_height) * scale;
    CoverDrawRect {
        x: (f64::from(target_width) - drawn_width) / 2.0,
        y: (f64::from(target_height) - drawn_height) / 2.0,
        scale,
    }
}

fn clip_rounded_rect(context: &gtk::cairo::Context, width: i32, height: i32, radius: f64) {
    let width = f64::from(width);
    let height = f64::from(height);
    let radius = radius.min(width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        width - radius,
        radius,
        radius,
        (-90.0_f64).to_radians(),
        0.0,
    );
    context.arc(
        width - radius,
        height - radius,
        radius,
        0.0,
        90.0_f64.to_radians(),
    );
    context.arc(
        radius,
        height - radius,
        radius,
        90.0_f64.to_radians(),
        180.0_f64.to_radians(),
    );
    context.arc(
        radius,
        radius,
        radius,
        180.0_f64.to_radians(),
        270.0_f64.to_radians(),
    );
    context.close_path();
    context.clip();
}

fn player_link(css_class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("player-link");
    label.add_css_class(css_class);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), &label);
    label
}

fn queue_link_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("queue-link");
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), &label);
    label
}

fn add_label_click(label: &gtk::Label, callback: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| callback());
    label.add_controller(click);
}

fn active_lyrics_line_index(lines: &[LyricLine], position_millis: u64) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let start = line.start_millis?;
            (start <= position_millis).then_some((index, start))
        })
        .max_by_key(|(_, start)| *start)
        .map(|(index, _)| index)
}

fn scroll_lyrics_row_into_view(scroller: gtk::ScrolledWindow, row: gtk::Widget) {
    glib::idle_add_local_once(move || {
        let Some(bounds) = row.compute_bounds(&scroller) else {
            return;
        };
        let adjustment = scroller.vadjustment();
        let viewport_height = f64::from(scroller.height().max(1));
        let row_center = adjustment.value() + f64::from(bounds.y() + bounds.height() / 2.0);
        let target = row_center - viewport_height / 2.0;
        let upper = adjustment.upper() - adjustment.page_size();
        adjustment.set_value(target.clamp(adjustment.lower(), upper.max(adjustment.lower())));
    });
}

fn set_active_class(widget: &impl IsA<gtk::Widget>, active: bool) {
    if active {
        widget.add_css_class("active-toggle");
    } else {
        widget.remove_css_class("active-toggle");
    }
}

fn favorite_icon_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(FAVORITE_EMPTY_GLYPH);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("favorite-toggle");
    button.set_tooltip_text(Some(&tr(label)));
    button
}

fn favorite_text_button(label: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    button.add_css_class("favorite-toggle");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let glyph = gtk::Label::new(Some(FAVORITE_EMPTY_GLYPH));
    glyph.add_css_class("favorite-glyph");
    content.append(&glyph);
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    (button, glyph)
}

fn set_favorite_button_active(button: &gtk::Button, active: bool) {
    set_active_class(button, active);
    button.set_label(if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    });
}

fn set_favorite_text_button_active(button: &gtk::Button, glyph: &gtk::Label, active: bool) {
    set_active_class(button, active);
    glyph.set_text(if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    });
}

fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    button
}

fn icon_button_with_image(icon_name: &str, label: &str) -> (gtk::Button, gtk::Image) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    let image = gtk::Image::from_icon_name(icon_name);
    button.set_child(Some(&image));
    (button, image)
}

fn text_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        HOME_ALBUM_GAP, HOME_ALBUM_MAX_SIZE, active_lyrics_line_index,
        clamp_content_split_position, clamp_home_album_page_start, home_album_card_size,
        home_album_page_size,
    };
    use rufin_provider::LyricLine;

    #[test]
    fn home_album_page_size_uses_stable_content_width() {
        let three_cards_width = super::HOME_ALBUM_TARGET_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, None), 3);
        assert_eq!(home_album_page_size(three_cards_width + 1, None), 3);

        let four_cards_width = super::HOME_ALBUM_TARGET_SIZE * 4 + HOME_ALBUM_GAP * 3;
        assert_eq!(home_album_page_size(four_cards_width, None), 4);
        assert_eq!(home_album_page_size(1, None), 2);
        assert_eq!(home_album_page_size(10_000, None), 8);
    }

    #[test]
    fn home_album_page_size_changes_only_at_size_bounds() {
        let three_cards_width = super::HOME_ALBUM_MIN_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_width - 1, Some(3)), 2);

        let three_cards_max_width = HOME_ALBUM_MAX_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_max_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_max_width + 3, Some(3)), 4);
    }

    #[test]
    fn home_album_page_start_stays_on_full_pages() {
        assert_eq!(clamp_home_album_page_start(0, 3, 0), 0);
        assert_eq!(clamp_home_album_page_start(3, 3, 10), 3);
        assert_eq!(clamp_home_album_page_start(9, 3, 10), 9);
        assert_eq!(clamp_home_album_page_start(12, 3, 10), 9);
    }

    #[test]
    fn home_album_card_size_remains_bounded() {
        assert_eq!(home_album_card_size(10_000, 2), HOME_ALBUM_MAX_SIZE);
        assert_eq!(home_album_card_size(1, 8), 1);
    }

    #[test]
    fn content_split_position_limits_right_panel() {
        assert_eq!(clamp_content_split_position(1_000, 100), 500);
        assert_eq!(clamp_content_split_position(1_000, 950), 900);
        assert_eq!(clamp_content_split_position(1_000, 625), 625);
    }

    #[test]
    fn synced_lyrics_highlight_last_started_line_only() {
        let lines = vec![
            LyricLine {
                text: "intro".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "verse".to_string(),
                start_millis: Some(5_000),
            },
            LyricLine {
                text: "unsynced".to_string(),
                start_millis: None,
            },
            LyricLine {
                text: "chorus".to_string(),
                start_millis: Some(9_000),
            },
        ];

        assert_eq!(active_lyrics_line_index(&lines, 999), None);
        assert_eq!(active_lyrics_line_index(&lines, 1_000), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 8_999), Some(1));
        assert_eq!(active_lyrics_line_index(&lines, 9_000), Some(3));
    }

    #[test]
    fn cover_draw_rect_crops_portrait_images_to_square_targets() {
        let rect = super::cover_draw_rect(100, 200, 34, 34);
        assert!((rect.scale - 0.34).abs() < f64::EPSILON);
        assert!((rect.x - 0.0).abs() < f64::EPSILON);
        assert!((rect.y + 17.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cover_draw_rect_crops_landscape_images_to_square_targets() {
        let rect = super::cover_draw_rect(200, 100, 44, 44);
        assert!((rect.scale - 0.44).abs() < f64::EPSILON);
        assert!((rect.x + 22.0).abs() < f64::EPSILON);
        assert!((rect.y - 0.0).abs() < f64::EPSILON);
    }
}
