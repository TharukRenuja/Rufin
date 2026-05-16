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
    HomeSection, HomeSectionKind, ImageRef, Playlist, PlaylistId, QueueEntry, QueueEntryId,
    QueueSnapshot, RepeatMode, Route, RouteStack, SearchKind, ServerIdentity, Track, TrackSortKey,
    TrackTableColumn, TrackTableSettings, format_duration,
};
use rufin_playback::PlaybackState;
use rufin_provider::{LyricLine, Lyrics};
use rufin_store::{CachedArtistDetail, CachedGenreDetail, image_cache_key};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{AppController, ControllerEvent, LibrarySnapshot, PlaybackSnapshot};
use crate::i18n::tr;

const COMPACT_RAIL_WIDTH: i32 = 80;
const MAIN_PANEL_UNITS: i32 = 1;
const TOTAL_PANEL_UNITS: i32 = 2;
const RIGHT_PANEL_MIN_PERCENT: i32 = 10;
const RIGHT_PANEL_MAX_PERCENT: i32 = 50;
const NORMAL_SIDEBAR_WIDTH: i32 = 220;
const HOME_ALBUM_GAP: i32 = 14;
const HOME_ALBUM_MIN_SIZE: i32 = 150;
const HOME_ALBUM_TARGET_SIZE: i32 = 180;
const HOME_ALBUM_MAX_SIZE: i32 = 210;
const HOME_ALBUM_MIN_COLUMNS: usize = 2;
const HOME_ALBUM_MAX_COLUMNS: usize = 12;
const HOME_ALBUM_HORIZONTAL_MARGINS: i32 = 56;
const CARD_LABEL_LINE_HEIGHT: i32 = 18;
const HOME_ALBUM_CARD_LABEL_GAP: i32 = 4;
const HOME_ALBUM_TITLE_LINES: i32 = 2;
const HOME_ALBUM_ARTIST_LINES: i32 = 2;
const HOME_ALBUM_YEAR_LINES: i32 = 1;
const GRID_ROUTE_PAGE_SIZE: usize = 16;
const TRACK_ROUTE_PAGE_SIZE: usize = 64;
const GRID_COVER_SIZE: u32 = 256;
const DETAIL_COVER_SIZE: u32 = 512;
const THUMB_COVER_SIZE: u32 = 96;
const BOTTOM_PLAYER_HEIGHT: i32 = 82;
const BOTTOM_PLAYER_COVER_SIZE: i32 = 72;
const BOTTOM_PLAYER_IDENTITY_WIDTH: i32 = 190;
const BOTTOM_PLAYER_IDENTITY_MAX_CHARS: i32 = 24;
const BOTTOM_PLAYER_TRANSPORT_WIDTH: i32 = 400;
const BOTTOM_PLAYER_TRANSPORT_OFFSET: i32 = 80;
const BOTTOM_PLAYER_PROGRESS_WIDTH: i32 = 300;
const BOTTOM_PLAYER_BUTTON_ROW_HEIGHT: i32 = 40;
const BOTTOM_PLAYER_BUTTON_SIZE: i32 = 36;
const BOTTOM_PLAYER_BUTTON_STEP: f64 = 44.0;
const BOTTOM_PLAYER_TRANSPORT_MARGIN_TOP: i32 = 6;
const MIN_RESTORED_WINDOW_WIDTH: i32 = 480;
const MIN_RESTORED_WINDOW_HEIGHT: i32 = 360;
const MAX_RESTORED_WINDOW_WIDTH: i32 = 1400;
const MAX_RESTORED_WINDOW_HEIGHT: i32 = 900;
const QUEUE_LYRICS_MIN_PANE_HEIGHT: i32 = 120;
const QUEUE_LYRICS_READY_MIN_HEIGHT: i32 = QUEUE_LYRICS_MIN_PANE_HEIGHT * 3;
const QUEUE_LYRICS_DEFAULT_QUEUE_UNITS: i32 = 5;
const QUEUE_LYRICS_DEFAULT_LYRICS_UNITS: i32 = 2;
const IMAGE_TAG_UNTAGGED: &str = "untagged";
const DECODED_COVER_CACHE_LIMIT: usize = 800;
const INITIAL_COVER_PRIME_LIMIT: usize = 24;
const INITIAL_COVER_PRIME_BUDGET: Duration = Duration::from_millis(300);
const FAVORITE_EMPTY_GLYPH: &str = "♡";
const FAVORITE_FILLED_GLYPH: &str = "♥";
const DEFAULT_LYRICS_SCROLL_ANIMATION_MS: u64 = 300;
const MIN_LYRICS_SCROLL_ANIMATION_MS: u64 = 80;
const LYRICS_SCROLL_FINISH_BEFORE_NEXT_MS: u64 = 200;
const LYRICS_USER_SCROLL_PAUSE_MS: u64 = 3_000;
const RESPONSIVE_RENDER_DELAY_MS: u64 = 16;

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
    lyrics_scroll_generation: Cell<u64>,
    lyrics_timing_generation: Cell<u64>,
    lyrics_timing_source: RefCell<Option<glib::SourceId>>,
    lyrics_follow_pause_until: Cell<Option<Instant>>,
    mpris_player: RefCell<Option<Rc<MprisPlayer>>>,
    updating_player_controls: Cell<bool>,
    seeking_player_controls: Cell<bool>,
    seek_generation: Cell<u64>,
    right_panel_visible: Cell<bool>,
    split_width: Cell<i32>,
    split_position: Cell<i32>,
    responsive_render_queued: Cell<bool>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LyricsFollowScrollPause {
    Inactive,
    Active,
    Expired,
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

#[derive(Clone, Copy)]
enum AlbumCardLabelLayout {
    Natural,
    StableHome,
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
    server_selector: ServerSelector,
    content_split: gtk::Paned,
    route_title: gtk::Label,
    route_host: gtk::Box,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    right_panel: gtk::Box,
    player_controls: PlayerControls,
}

struct ServerSelector {
    normal_button: gtk::MenuButton,
    normal_name: gtk::Label,
    normal_subtitle: gtk::Label,
    compact_button: gtk::MenuButton,
    compact_label: gtk::Label,
}

struct ServerSelectorContent {
    name: String,
    subtitle: String,
    detail: String,
    has_server: bool,
}

struct PlayerControls {
    root: gtk::Overlay,
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
    queue_button: gtk::Button,
    queue_icon: gtk::DrawingArea,
    queue_icon_open: Rc<Cell<bool>>,
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
        lyrics_scroll_generation: Cell::new(0),
        lyrics_timing_generation: Cell::new(0),
        lyrics_timing_source: RefCell::new(None),
        lyrics_follow_pause_until: Cell::new(None),
        mpris_player: RefCell::new(None),
        updating_player_controls: Cell::new(false),
        seeking_player_controls: Cell::new(false),
        seek_generation: Cell::new(0),
        right_panel_visible: Cell::new(true),
        split_width: Cell::new(0),
        split_position: Cell::new(0),
        responsive_render_queued: Cell::new(false),
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
    if let Some((width, height)) =
        restored_window_size(settings.window_width, settings.window_height)
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
    let server_selector = build_server_selector();

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
    let main_menu = primary_menu_button();

    header.append(&back_button);
    header.append(&forward_button);
    header.append(&route_title);
    header.append(&main_menu);

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
        server_selector,
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
    shell.update_server_selector();
    connect_shell_actions(&shell, main_menu);
    connect_player_controls(&shell);
    install_mpris(&shell);
    shell.update_density();
    prime_first_cached_cover(&shell);
    shell.render_current_route();
    shell.render_queue_panel();
    shell.update_bottom_player();
    shell.request_initial_lyrics_if_needed();
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
    shell.queue_responsive_route_render();

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

    fn set_external_lyrics_enabled(self: &Rc<Self>, enabled: bool) {
        {
            let mut settings = self.state.settings.borrow_mut();
            if settings.external_lyrics_enabled == enabled {
                return;
            }
            settings.external_lyrics_enabled = enabled;
            if let Err(error) = self.controller.save_settings(&settings) {
                warn!(%error, "failed to save lyrics setting");
            }
        }
        self.render_queue_panel();
        if enabled && current_playback_track_id(&self.state.player.borrow()).is_some() {
            self.controller.request_lyrics_for_current();
        }
    }

    fn save_window_state(&self) {
        let mut settings = self.state.settings.borrow_mut();
        let mut changed = false;

        if !self.window.is_maximized()
            && !self.window.is_fullscreen()
            && let Some((width, height)) =
                restored_window_size(Some(self.window.width()), Some(self.window.height()))
            && (settings.window_width != Some(width) || settings.window_height != Some(height))
        {
            settings.window_width = Some(width);
            settings.window_height = Some(height);
            changed = true;
        }

        let split_position = if self.state.right_panel_visible.get() {
            self.content_split.position()
        } else {
            self.state.split_position.get()
        };
        if update_right_panel_split_settings(
            &mut settings,
            self.content_split.width(),
            split_position,
        ) {
            changed = true;
        }

        if !changed {
            return;
        }
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save window state");
        }
    }

    fn save_right_panel_split_position(&self, split_width: i32, position: i32) {
        if !self.state.right_panel_visible.get() {
            return;
        }
        let mut settings = self.state.settings.borrow_mut();
        if !update_right_panel_split_settings(&mut settings, split_width, position) {
            return;
        }
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save right panel split position");
        }
    }

    fn save_queue_lyrics_split_position(&self, available_height: i32, position: i32) {
        if available_height < QUEUE_LYRICS_READY_MIN_HEIGHT || position <= 0 {
            return;
        }
        let position = clamp_queue_lyrics_position(available_height, position);
        let ratio = queue_lyrics_position_ratio(available_height, position);
        let mut settings = self.state.settings.borrow_mut();
        if settings.queue_lyrics_position == Some(position)
            && settings.queue_lyrics_ratio == Some(ratio)
        {
            return;
        }
        settings.queue_lyrics_position = Some(position);
        settings.queue_lyrics_ratio = Some(ratio);
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save queue lyrics split position");
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
            self.queue_responsive_route_render();
        } else if route_uses_responsive_cards(self.state.routes.borrow().current()) {
            self.queue_responsive_route_render();
        }
    }

    fn update_server_selector(&self) {
        let content = {
            let library = self.state.library.borrow();
            server_selector_content(&library)
        };
        let tooltip = format!("{}: {}", tr("Server"), content.name);

        self.server_selector.normal_name.set_text(&content.name);
        self.server_selector
            .normal_subtitle
            .set_text(&content.subtitle);
        self.server_selector
            .normal_button
            .set_tooltip_text(Some(&tooltip));
        self.server_selector
            .normal_button
            .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
        self.server_selector
            .normal_button
            .set_popover(Some(&server_selection_popover(&content)));

        self.server_selector
            .compact_label
            .set_text(&compact_sidebar_label_text(&content.name));
        self.server_selector
            .compact_button
            .set_tooltip_text(Some(&tooltip));
        self.server_selector
            .compact_button
            .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
        self.server_selector
            .compact_button
            .set_popover(Some(&server_selection_popover(&content)));
    }

    fn update_content_split(&self) -> bool {
        let split_width = self.content_split.width();
        if split_width <= 1 {
            return false;
        }

        let previous_width = self.state.split_width.replace(split_width);
        let current_position = self.content_split.position().clamp(0, split_width);
        let stored_position = self.state.split_position.get();
        let saved_ratio = self.state.settings.borrow().right_panel_ratio;
        let width_changed = previous_width != split_width;

        if !self.state.right_panel_visible.get() {
            let position_changed = current_position != split_width;
            if position_changed {
                debug!(
                    split_width,
                    position = split_width,
                    "collapse content split"
                );
                self.content_split.set_position(split_width);
            }
            return width_changed || position_changed;
        }

        let position = content_split_target_position(
            split_width,
            previous_width,
            stored_position,
            current_position,
            saved_ratio,
        );
        let position_changed = self.state.split_position.replace(position) != position;
        if position_changed && !width_changed && current_position == position {
            self.save_right_panel_split_position(split_width, position);
        }

        if current_position != position {
            debug!(split_width, position, "update content split");
            self.content_split.set_position(position);
        }

        width_changed || position_changed
    }

    fn remember_right_panel_open_position(&self) {
        let split_width = self.content_split.width();
        if split_width <= 1 {
            return;
        }
        let current_position = self.content_split.position();
        if current_position <= 1 || current_position >= split_width {
            return;
        }
        let position = clamp_content_split_position(split_width, current_position);
        self.state.split_position.set(position);
        self.save_right_panel_split_position(split_width, position);
    }

    fn right_panel_open_position(&self, split_width: i32) -> i32 {
        let stored = self.state.split_position.get();
        let target = if stored > 1 && stored < split_width {
            stored
        } else {
            let saved_ratio = self.state.settings.borrow().right_panel_ratio;
            content_split_initial_position(split_width, saved_ratio)
        };
        clamp_content_split_position(split_width, target)
    }

    fn queue_responsive_route_render(self: &Rc<Self>) {
        if !route_uses_responsive_cards(self.state.routes.borrow().current()) {
            return;
        }
        if self.state.responsive_render_queued.replace(true) {
            return;
        }

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS),
            move || {
                if !shell.state.responsive_render_queued.replace(false) {
                    return;
                }
                shell.update_content_split();
                if route_uses_responsive_cards(shell.state.routes.borrow().current()) {
                    shell.render_current_route();
                }
            },
        );
    }

    fn render_responsive_route_now(self: &Rc<Self>) {
        self.state.responsive_render_queued.set(false);
        self.update_content_split();
        if route_uses_responsive_cards(self.state.routes.borrow().current()) {
            self.render_current_route();
        }
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
        let position = Time::from_millis(snapshot.position_millis.min(i64::MAX as u64) as i64);
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

    fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }

    fn request_initial_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        *self.state.lyrics_track_id.borrow_mut() = Some(track_id);
        self.controller.request_lyrics_for_current();
    }

    fn lyrics_empty_status(&self) -> String {
        let settings = self.state.settings.borrow();
        if settings.private_mode {
            tr("No server lyrics for the current track. Private mode is on.")
        } else if !settings.external_lyrics_enabled {
            tr("No server lyrics for the current track. External lyric lookup is off.")
        } else {
            tr("No lyrics for the current track.")
        }
    }

    fn update_lyrics_highlight_at(self: &Rc<Self>, position_millis: u64) {
        let lyrics = self.state.lyrics.borrow().clone();
        let active_index = lyrics
            .as_ref()
            .and_then(|lyrics| active_lyrics_line_index(lyrics.lines.as_slice(), position_millis));
        let previous_index = self.state.lyrics_active_index.replace(active_index);
        let follow_pause = self.lyrics_follow_scroll_pause();
        let should_follow_scroll = follow_pause != LyricsFollowScrollPause::Active;
        let force_follow_scroll = follow_pause == LyricsFollowScrollPause::Expired;
        let scroll_target = {
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

            if should_follow_scroll {
                active_index
                    .filter(|index| force_follow_scroll || Some(*index) != previous_index)
                    .and_then(|index| {
                        let scroller = self.state.lyrics_scroller.borrow().clone()?;
                        let row = rows.get(index)?.row.clone().upcast::<gtk::Widget>();
                        let duration = lyrics
                            .as_ref()
                            .map(|lyrics| {
                                lyrics_scroll_animation_millis(
                                    lyrics.lines.as_slice(),
                                    index,
                                    position_millis,
                                )
                            })
                            .unwrap_or(DEFAULT_LYRICS_SCROLL_ANIMATION_MS);
                        Some((scroller, row, duration))
                    })
            } else {
                None
            }
        };

        if let Some((scroller, row, duration)) = scroll_target {
            let generation = self.state.lyrics_scroll_generation.get().saturating_add(1);
            self.state.lyrics_scroll_generation.set(generation);
            scroll_lyrics_row_into_view(Rc::clone(self), scroller, row, duration, generation);
        }
        self.schedule_next_lyrics_highlight(position_millis);
    }

    fn current_position_millis(&self) -> u64 {
        self.state.player.borrow().position_millis
    }

    fn lyrics_follow_scroll_pause(&self) -> LyricsFollowScrollPause {
        let pause = lyrics_follow_scroll_pause_state(
            self.state.lyrics_follow_pause_until.get(),
            Instant::now(),
        );
        if pause == LyricsFollowScrollPause::Expired {
            self.state.lyrics_follow_pause_until.set(None);
        }
        pause
    }

    fn pause_lyrics_follow_scroll(&self) {
        self.state.lyrics_follow_pause_until.set(Some(
            Instant::now() + Duration::from_millis(LYRICS_USER_SCROLL_PAUSE_MS),
        ));
    }

    fn seek_to_lyrics_position(self: &Rc<Self>, position_millis: u64) {
        self.state.lyrics_follow_pause_until.set(None);
        self.controller.seek_millis(position_millis);
        self.update_lyrics_highlight_at(position_millis);
    }

    fn cancel_scheduled_lyrics_highlight(&self) {
        self.state
            .lyrics_timing_generation
            .set(self.state.lyrics_timing_generation.get().saturating_add(1));
        if let Some(source) = self.state.lyrics_timing_source.borrow_mut().take() {
            source.remove();
        }
    }

    fn schedule_next_lyrics_highlight(self: &Rc<Self>, position_millis: u64) {
        let playing = matches!(self.state.player.borrow().state, PlaybackState::Playing);
        if !playing {
            return;
        }

        let Some(next_position_millis) = self
            .state
            .lyrics
            .borrow()
            .as_ref()
            .and_then(|lyrics| next_lyrics_line_start_after(&lyrics.lines, position_millis))
        else {
            return;
        };
        let delay_millis = next_position_millis.saturating_sub(position_millis);
        let generation = self.state.lyrics_timing_generation.get().saturating_add(1);
        self.state.lyrics_timing_generation.set(generation);

        let shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(delay_millis), move || {
            if shell.state.lyrics_timing_generation.get() != generation {
                return;
            }
            let _source = shell.state.lyrics_timing_source.borrow_mut().take();
            shell.update_lyrics_highlight_at(next_position_millis);
        });
        if let Some(previous_source) = self.state.lyrics_timing_source.borrow_mut().replace(source)
        {
            previous_source.remove();
        }
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

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
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
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
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
        artist.set_halign(gtk::Align::Start);
        artist.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(artist.upcast_ref(), &artist);
        if let Some(artist_id) = album.artist_id.clone() {
            let shell = Rc::clone(self);
            add_label_click(&artist, move || {
                shell.navigate(Route::ArtistDetail(artist_id.clone()))
            });
        } else if !album.artist.trim().is_empty() {
            let shell = Rc::clone(self);
            let artist_name = album.artist.clone();
            add_label_click(&artist, move || {
                shell.navigate(Route::Search {
                    query: artist_name.clone(),
                    kind: SearchKind::Artists,
                });
            });
        }
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

        let favorite = favorite_icon_button("Favorite");
        set_favorite_button_active(&favorite, album.favorite);
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
        configure.set_icon_name("view-more-symbolic");
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
        let favorite = favorite_icon_button("Favorite");
        set_favorite_button_active(&favorite, artist.favorite);
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

        let lyrics_group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        lyrics_group.add_css_class("settings-group");
        let lyrics_heading = gtk::Label::new(Some(&tr("Lyrics")));
        lyrics_heading.add_css_class("section-heading");
        lyrics_heading.set_xalign(0.0);
        lyrics_group.append(&lyrics_heading);

        let external_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        external_row.set_valign(gtk::Align::Center);
        let external_text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        external_text.set_hexpand(true);
        let external_title = gtk::Label::new(Some(&tr("External lyric lookup")));
        external_title.set_xalign(0.0);
        let external_note = gtk::Label::new(Some(&tr(
            "Use Jellyfin remote lyric providers when server lyrics are unavailable.",
        )));
        external_note.add_css_class("muted");
        external_note.set_wrap(true);
        external_note.set_xalign(0.0);
        external_text.append(&external_title);
        external_text.append(&external_note);
        let external_switch = gtk::Switch::new();
        external_switch.set_active(self.state.settings.borrow().external_lyrics_enabled);
        let shell = Rc::clone(self);
        external_switch.connect_active_notify(move |switch| {
            shell.set_external_lyrics_enabled(switch.is_active());
        });
        external_row.append(&external_text);
        external_row.append(&external_switch);
        lyrics_group.append(&external_row);
        wrapper.append(&lyrics_group);

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
        let lyrics_scroll_controller =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        lyrics_scroll_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let shell_for_lyrics_scroll = Rc::clone(self);
        lyrics_scroll_controller.connect_scroll(move |_, _, _| {
            shell_for_lyrics_scroll.pause_lyrics_follow_scroll();
            glib::Propagation::Proceed
        });
        lyrics_scroller.add_controller(lyrics_scroll_controller);
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
                    let shell = Rc::clone(self);
                    row.connect_clicked(move |_| {
                        shell.seek_to_lyrics_position(start_millis);
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
            let lyrics_status = gtk::Label::new(Some(&self.lyrics_empty_status()));
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
        queue_lyrics_split.set_shrink_end_child(true);
        queue_lyrics_split.set_start_child(Some(&queue));
        queue_lyrics_split.set_end_child(Some(&lyrics));
        let saved_ratio = self.state.settings.borrow().queue_lyrics_ratio;
        queue_lyrics_split.set_position(queue_lyrics_initial_position(
            queue_lyrics_available_height(self),
            saved_ratio,
        ));

        let suppress_split_position_save = Rc::new(Cell::new(0_u32));
        let applied_split_height = Rc::new(Cell::new(0));
        let position_shell = Rc::clone(self);
        let suppress_for_tick = Rc::clone(&suppress_split_position_save);
        let applied_height_for_tick = Rc::clone(&applied_split_height);
        queue_lyrics_split.add_tick_callback(move |split, _| {
            let available_height = split.height();
            if available_height >= QUEUE_LYRICS_READY_MIN_HEIGHT
                && applied_height_for_tick.replace(available_height) != available_height
            {
                let saved_ratio = position_shell.state.settings.borrow().queue_lyrics_ratio;
                set_queue_lyrics_split_position_without_saving(
                    split,
                    &suppress_for_tick,
                    saved_ratio,
                );
            }
            glib::ControlFlow::Continue
        });

        let split_interaction = gtk::GestureClick::new();
        split_interaction.set_propagation_phase(gtk::PropagationPhase::Capture);
        let split_for_release = queue_lyrics_split.clone();
        let shell_for_release = Rc::clone(self);
        let suppress_for_release = Rc::clone(&suppress_split_position_save);
        split_interaction.connect_released(move |_, _, _, _| {
            let split = split_for_release.clone();
            let shell = Rc::clone(&shell_for_release);
            let suppress = Rc::clone(&suppress_for_release);
            glib::idle_add_local_once(move || {
                if suppress.get() > 0 {
                    return;
                }
                shell.save_queue_lyrics_split_position(split.height(), split.position());
            });
        });
        queue_lyrics_split.add_controller(split_interaction);

        let shell = Rc::clone(self);
        let suppress_for_position = Rc::clone(&suppress_split_position_save);
        queue_lyrics_split.connect_notify_local(Some("position"), move |split, _| {
            if suppress_for_position.get() > 0 {
                return;
            }
            shell.save_queue_lyrics_split_position(split.height(), split.position());
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
        row.set_focusable(true);
        let accessible_label = format!("{} {}", entry.title, entry.artist);
        row.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
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
        row.append(&number);
        row.append(&cover);
        row.append(&labels);
        row.append(&year);
        install_queue_row_context_menu(&row, &self.controller, entry.id.clone());
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

    fn toggle_right_panel(self: &Rc<Self>) {
        self.set_right_panel_visible(!self.state.right_panel_visible.get());
    }

    fn set_right_panel_visible(self: &Rc<Self>, visible: bool) {
        if !visible {
            self.remember_right_panel_open_position();
        }

        if self.state.right_panel_visible.replace(visible) == visible {
            self.update_right_panel_button();
            return;
        }

        self.update_right_panel_button();
        apply_right_panel_visibility(Rc::clone(self), visible);
    }

    fn update_right_panel_button(&self) {
        let visible = self.state.right_panel_visible.get();
        let label = tr(if visible {
            "Hide sidebar"
        } else {
            "Show sidebar"
        });
        self.player_controls.queue_icon_open.set(visible);
        self.player_controls.queue_icon.queue_draw();
        self.player_controls
            .queue_button
            .set_tooltip_text(Some(&label));
        self.player_controls
            .queue_button
            .update_property(&[gtk::accessible::Property::Label(&label)]);
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
                AlbumCardLabelLayout::Natural,
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
        album_card_widget_with_size(
            self,
            album,
            size,
            Some(&self.controller),
            AlbumCardLabelLayout::StableHome,
        )
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
        let popover_for_sort = popover.clone();
        sort_dropdown.connect_selected_notify(move |dropdown| {
            let sort_key = track_sort_from_index(dropdown.selected());
            let mut settings = shell.state.settings.borrow().track_table.clone();
            if settings.sort_key == sort_key {
                return;
            }
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
            let popover = popover_for_sort.clone();
            glib::idle_add_local_once(move || popover.popdown());
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
        let column_checks = Rc::new(RefCell::new(Vec::new()));
        let syncing_column_checks = Rc::new(Cell::new(false));
        for column in TrackTableColumn::all() {
            let check = gtk::CheckButton::with_label(&tr(track_table_column_config_title(column)));
            check.set_active(visible.contains(&column));
            column_checks.borrow_mut().push((column, check.clone()));
            let shell = Rc::clone(self);
            let table_for_column = table.clone();
            let column_checks_for_column = Rc::clone(&column_checks);
            let syncing_column_checks_for_column = Rc::clone(&syncing_column_checks);
            check.connect_toggled(move |check| {
                if syncing_column_checks_for_column.get() {
                    return;
                }
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
                sync_track_column_checks(
                    &column_checks_for_column,
                    &settings,
                    &syncing_column_checks_for_column,
                );
                set_track_table_columns(&shell, &table_for_column, &settings);
            });
            content.append(&check);
        }

        popover.set_child(Some(&content));
        popover
    }
}

fn connect_shell_actions(shell: &Rc<Shell>, main_menu: gtk::MenuButton) {
    let back_shell = Rc::clone(shell);
    shell
        .back_button
        .connect_clicked(move |_| back_shell.go_back());

    let forward_shell = Rc::clone(shell);
    shell
        .forward_button
        .connect_clicked(move |_| forward_shell.go_forward());

    install_window_actions(shell);
    install_main_menu_shortcut(shell, main_menu);

    let close_shell = Rc::clone(shell);
    shell.window.connect_close_request(move |_| {
        close_shell.save_window_state();
        glib::Propagation::Proceed
    });

    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            if resize_shell.state.density_mode.get() == DensityMode::Auto {
                resize_shell.update_density();
            } else {
                resize_shell.update_content_split();
                resize_shell.queue_responsive_route_render();
            }
        });

    let split_shell = Rc::clone(shell);
    shell
        .content_split
        .connect_notify_local(Some("width"), move |_, _| {
            split_shell.update_content_split();
            split_shell.queue_responsive_route_render();
        });

    let split_shell = Rc::clone(shell);
    shell
        .content_split
        .connect_notify_local(Some("position"), move |_, _| {
            split_shell.update_content_split();
            split_shell.queue_responsive_route_render();
        });

    let split_shell = Rc::clone(shell);
    shell.content_split.add_tick_callback(move |_, _| {
        if split_shell.update_content_split() {
            split_shell.queue_responsive_route_render();
        }
        glib::ControlFlow::Continue
    });
}

fn install_window_actions(shell: &Rc<Shell>) {
    let preferences = gio::SimpleAction::new("preferences", None);
    let preferences_shell = Rc::clone(shell);
    preferences.connect_activate(move |_, _| preferences_shell.navigate(Route::Settings));
    shell.window.add_action(&preferences);

    let shortcuts = gio::SimpleAction::new("show-shortcuts", None);
    let shortcuts_shell = Rc::clone(shell);
    shortcuts.connect_activate(move |_, _| show_shortcuts_dialog(&shortcuts_shell));
    shell.window.add_action(&shortcuts);

    let fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
    let fullscreen_shell = Rc::clone(shell);
    fullscreen.connect_activate(move |_, _| {
        if fullscreen_shell.window.is_fullscreen() {
            fullscreen_shell.window.unfullscreen();
        } else {
            fullscreen_shell.window.fullscreen();
        }
    });
    shell.window.add_action(&fullscreen);

    let about = gio::SimpleAction::new("about", None);
    let about_shell = Rc::clone(shell);
    about.connect_activate(move |_, _| show_about_dialog(&about_shell));
    shell.window.add_action(&about);

    shell
        .application
        .set_accels_for_action("win.preferences", &["<Control>comma"]);
    shell
        .application
        .set_accels_for_action("win.show-shortcuts", &["<Control>question"]);
    shell
        .application
        .set_accels_for_action("win.toggle-fullscreen", &["F11"]);
}

fn install_main_menu_shortcut(shell: &Rc<Shell>, main_menu: gtk::MenuButton) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, key, _, state| {
        if key == gtk::gdk::Key::F10 && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
            main_menu.popup();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shell.window.add_controller(key_controller);
}

fn show_shortcuts_dialog(shell: &Shell) {
    let dialog = adw::ShortcutsDialog::builder()
        .title(tr("Keyboard Shortcuts"))
        .build();
    let section = adw::ShortcutsSection::new(Some(&tr("General")));
    section.add(adw::ShortcutsItem::new(&tr("Main Menu"), "F10"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Preferences"),
        "win.preferences",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Keyboard Shortcuts"),
        "win.show-shortcuts",
    ));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Toggle Fullscreen"),
        "win.toggle-fullscreen",
    ));
    dialog.add(section);
    dialog.present(Some(&shell.window));
}

fn show_about_dialog(shell: &Shell) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Rufin")
        .application_icon("io.github.screwys.Rufin")
        .developer_name("screwys")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(tr("Native GTK music client shell with Jellyfin playback."))
        .website("https://github.com/screwys/Rufin")
        .issue_url("https://github.com/screwys/Rufin/issues")
        .license_type(gtk::License::Gpl30)
        .build();
    dialog.present(Some(&shell.window));
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

    header.upcast()
}

fn install_queue_row_context_menu(
    row: &gtk::Box,
    controller: &AppController,
    entry_id: QueueEntryId,
) {
    let actions = gio::SimpleActionGroup::new();

    let remove = gio::SimpleAction::new("remove", None);
    let remove_controller = controller.clone();
    let remove_id = entry_id.clone();
    remove.connect_activate(move |_, _| remove_controller.remove_from_queue(remove_id.clone()));
    actions.add_action(&remove);

    let play_now = gio::SimpleAction::new("play-now", None);
    let play_now_controller = controller.clone();
    let play_now_id = entry_id.clone();
    play_now.connect_activate(move |_, _| {
        play_now_controller.activate_queue_entry(play_now_id.clone())
    });
    actions.add_action(&play_now);

    let play_next = gio::SimpleAction::new("play-next", None);
    let play_next_controller = controller.clone();
    play_next.connect_activate(move |_, _| {
        play_next_controller.move_queue_entry_after_current(entry_id.clone())
    });
    actions.add_action(&play_next);

    row.insert_action_group("queue", Some(&actions));

    let menu = gio::Menu::new();
    menu.append(Some(&tr("Remove from Queue")), Some("queue.remove"));
    menu.append(Some(&tr("Play Now")), Some("queue.play-now"));
    menu.append(Some(&tr("Play Next")), Some("queue.play-next"));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("queue-context-menu");
    popover.set_parent(row);

    let click_popover = popover.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        click_popover.set_pointing_to(Some(&rect));
        click_popover.popup();
    });
    row.add_controller(click);

    let key_popover = popover.clone();
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if opens_menu {
            key_popover.set_pointing_to(None);
            key_popover.popup();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    row.add_controller(key_controller);
}

fn build_bottom_player() -> PlayerControls {
    let root = gtk::Overlay::new();
    root.add_css_class("bottom-player");
    root.set_hexpand(true);
    root.set_vexpand(false);
    root.set_height_request(BOTTOM_PLAYER_HEIGHT);
    root.set_valign(gtk::Align::Center);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.set_hexpand(true);
    bar.set_valign(gtk::Align::Center);

    let now_playing = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    now_playing.add_css_class("player-now-playing");
    now_playing.set_valign(gtk::Align::Center);

    let cover = ArtworkTile::new(BOTTOM_PLAYER_COVER_SIZE, 42);
    cover.area.set_valign(gtk::Align::Center);
    now_playing.append(&cover.area);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 1);
    identity.add_css_class("player-identity");
    identity.set_size_request(BOTTOM_PLAYER_IDENTITY_WIDTH, -1);
    identity.set_hexpand(false);
    identity.set_valign(gtk::Align::Center);
    let title = player_link("player-title");
    let artist = player_link("muted");
    let album = player_link("muted");
    identity.append(&title);
    identity.append(&artist);
    identity.append(&album);
    now_playing.append(&identity);
    bar.append(&now_playing);

    let transport = gtk::Box::new(gtk::Orientation::Vertical, 5);
    transport.add_css_class("player-transport");
    transport.set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH);
    transport.set_valign(gtk::Align::Center);

    let buttons = gtk::Fixed::new();
    buttons.add_css_class("player-button-row");
    buttons.set_halign(gtk::Align::Center);
    buttons.set_valign(gtk::Align::Center);
    buttons.set_size_request(
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        BOTTOM_PLAYER_BUTTON_ROW_HEIGHT,
    );

    let stop_button = icon_button("media-playback-stop-symbolic", "Stop");
    stop_button.add_css_class("player-transport-button");
    let previous_button = skip_icon_button(false, "Previous");
    previous_button.add_css_class("player-transport-button");
    let (play_button, play_icon) = icon_button_with_image("media-playback-start-symbolic", "Play");
    play_button.add_css_class("player-transport-button");
    play_button.add_css_class("player-play-button");
    play_icon.set_halign(gtk::Align::Center);
    play_icon.set_valign(gtk::Align::Center);
    play_icon.set_pixel_size(17);
    let next_button = skip_icon_button(true, "Next");
    next_button.add_css_class("player-transport-button");
    let shuffle_button = icon_button("media-playlist-shuffle-symbolic", "Shuffle");
    shuffle_button.add_css_class("player-transport-button");
    let repeat_button = icon_button("media-playlist-repeat-symbolic", "Repeat off");
    repeat_button.add_css_class("player-transport-button");
    let dj_button = icon_button("media-optical-cd-audio-symbolic", "Auto DJ");
    dj_button.add_css_class("player-transport-button");
    for button in [
        &stop_button,
        &previous_button,
        &shuffle_button,
        &play_button,
        &next_button,
        &repeat_button,
        &dj_button,
    ] {
        button.set_size_request(BOTTOM_PLAYER_BUTTON_SIZE, BOTTOM_PLAYER_BUTTON_SIZE);
    }

    let button_center = f64::from(BOTTOM_PLAYER_TRANSPORT_WIDTH) / 2.0;
    let button_radius = f64::from(BOTTOM_PLAYER_BUTTON_SIZE) / 2.0;
    let button_y = f64::from(BOTTOM_PLAYER_BUTTON_ROW_HEIGHT - BOTTOM_PLAYER_BUTTON_SIZE) / 2.0;
    buttons.put(
        &stop_button,
        button_center - BOTTOM_PLAYER_BUTTON_STEP * 3.0 - button_radius,
        button_y,
    );
    buttons.put(
        &previous_button,
        button_center - BOTTOM_PLAYER_BUTTON_STEP * 2.0 - button_radius,
        button_y,
    );
    buttons.put(
        &shuffle_button,
        button_center - BOTTOM_PLAYER_BUTTON_STEP - button_radius,
        button_y,
    );
    buttons.put(&play_button, button_center - button_radius, button_y);
    buttons.put(
        &next_button,
        button_center + BOTTOM_PLAYER_BUTTON_STEP - button_radius,
        button_y,
    );
    buttons.put(
        &repeat_button,
        button_center + BOTTOM_PLAYER_BUTTON_STEP * 2.0 - button_radius,
        button_y,
    );
    buttons.put(
        &dj_button,
        button_center + BOTTOM_PLAYER_BUTTON_STEP * 3.0 - button_radius,
        button_y,
    );

    let progress_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    progress_row.add_css_class("player-progress-row");
    progress_row.set_halign(gtk::Align::Center);
    progress_row.set_valign(gtk::Align::Center);
    let elapsed = gtk::Label::new(Some("0:00"));
    elapsed.add_css_class("muted");
    elapsed.set_width_chars(4);
    elapsed.set_xalign(1.0);
    let progress = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    progress.add_css_class("player-progress");
    progress.set_draw_value(false);
    progress.set_width_request(BOTTOM_PLAYER_PROGRESS_WIDTH);
    let duration = gtk::Label::new(Some("0:00"));
    duration.add_css_class("muted");
    duration.set_width_chars(4);
    progress_row.append(&elapsed);
    progress_row.append(&progress);
    progress_row.append(&duration);

    transport.append(&buttons);
    transport.append(&progress_row);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_valign(gtk::Align::Center);
    let (queue_button, queue_icon, queue_icon_open) = queue_sidebar_button("Hide sidebar");
    actions.append(&queue_button);
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
    bar.append(&actions);

    let transport_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    transport_slot
        .set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH + BOTTOM_PLAYER_TRANSPORT_OFFSET);
    transport_slot.set_halign(gtk::Align::Center);
    transport_slot.set_valign(gtk::Align::Center);
    transport_slot.set_margin_top(BOTTOM_PLAYER_TRANSPORT_MARGIN_TOP);
    transport_slot.append(&transport);
    let offset_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    offset_spacer.set_width_request(BOTTOM_PLAYER_TRANSPORT_OFFSET);
    transport_slot.append(&offset_spacer);

    root.set_child(Some(&bar));
    root.add_overlay(&transport_slot);

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
        queue_button,
        queue_icon,
        queue_icon_open,
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

    let queue_shell = Rc::clone(shell);
    shell
        .player_controls
        .queue_button
        .connect_clicked(move |_| queue_shell.toggle_right_panel());

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

fn apply_right_panel_visibility(shell: Rc<Shell>, visible: bool) {
    let panel = shell.right_panel.clone();
    if panel.parent().is_none() {
        shell.content_split.set_end_child(Some(&panel));
    }

    let split_width = shell.content_split.width();
    if visible {
        panel.set_visible(true);
    } else {
        panel.set_visible(false);
    }
    panel.set_opacity(if visible { 1.0 } else { 0.0 });

    if split_width > 1 {
        let position = if visible {
            shell.right_panel_open_position(split_width)
        } else {
            split_width
        };
        shell.content_split.set_position(position);
    }

    shell.update_content_split();
    shell.render_responsive_route_now();
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
            let current = seek_shell.state.player.borrow().position_millis;
            let offset_millis = offset.as_micros() / 1_000;
            let target = if offset_millis.is_negative() {
                current.saturating_sub(offset_millis.unsigned_abs())
            } else {
                current.saturating_add(offset_millis as u64)
            };
            controller.seek_millis(target);
        });
        let controller = shell.controller.clone();
        player.connect_set_position(move |_, _, position| {
            controller.seek_millis((position.as_micros() / 1_000).max(0) as u64);
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
                    shell.update_server_selector();
                    shell.render_current_route();
                }
                ControllerEvent::Queue(queue) => {
                    *shell.state.queue.borrow_mut() = *queue;
                    shell.render_queue_panel();
                    shell.update_bottom_player();
                }
                ControllerEvent::Playback(player) => {
                    let previous_player = shell.state.player.borrow().clone();
                    let previous_track = previous_player
                        .current
                        .as_ref()
                        .map(|entry| entry.track_id.clone());
                    let next_snapshot = *player;
                    let next_track = next_snapshot
                        .current
                        .as_ref()
                        .map(|entry| entry.track_id.clone());
                    let lyrics_timing_changed = previous_track != next_track
                        || previous_player.state != next_snapshot.state
                        || previous_player.position_millis != next_snapshot.position_millis;
                    *shell.state.player.borrow_mut() = next_snapshot.clone();
                    if previous_track != next_track {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                        shell.state.lyrics_follow_pause_until.set(None);
                        shell.cancel_scheduled_lyrics_highlight();
                        shell.render_queue_panel();
                        if next_track.is_some() {
                            shell.controller.request_lyrics_for_current();
                        }
                        shell.notify_now_playing(&next_snapshot);
                    }
                    shell.update_bottom_player();
                    if lyrics_timing_changed {
                        shell.update_lyrics_highlight();
                    }
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

fn build_server_selector() -> ServerSelector {
    let normal_button = gtk::MenuButton::new();
    normal_button.add_css_class("server-selector");
    normal_button.add_css_class("server-card");
    normal_button.set_margin_start(12);
    normal_button.set_margin_end(12);
    normal_button.set_margin_bottom(12);

    let normal_content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    normal_content.set_halign(gtk::Align::Fill);
    normal_content.append(&gtk::Image::from_icon_name("network-server-symbolic"));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let normal_name = gtk::Label::new(None);
    normal_name.set_xalign(0.0);
    normal_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let normal_subtitle = gtk::Label::new(None);
    normal_subtitle.add_css_class("muted");
    normal_subtitle.set_xalign(0.0);
    normal_subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&normal_name);
    labels.append(&normal_subtitle);
    normal_content.append(&labels);
    normal_content.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    normal_button.set_child(Some(&normal_content));

    let compact_button = gtk::MenuButton::new();
    compact_button.add_css_class("nav-button");
    compact_button.add_css_class("flat");
    compact_button.add_css_class("rail-button");
    compact_button.add_css_class("server-selector");
    let compact_content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    compact_content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("network-server-symbolic");
    icon.set_pixel_size(24);
    compact_content.append(&icon);
    let compact_label = gtk::Label::new(None);
    configure_rail_label(&compact_label);
    compact_content.append(&compact_label);
    compact_button.set_child(Some(&compact_content));

    ServerSelector {
        normal_button,
        normal_name,
        normal_subtitle,
        compact_button,
        compact_label,
    }
}

fn server_selector_content(library: &LibrarySnapshot) -> ServerSelectorContent {
    let Some(server) = library.server.as_ref() else {
        return ServerSelectorContent {
            name: tr("No server"),
            subtitle: tr("No server"),
            detail: tr("No server"),
            has_server: false,
        };
    };

    let name = server_display_name(server);
    let subtitle = tr("Current server");
    let detail = if server.base_url.trim().is_empty() {
        provider_display_name(&server.provider)
    } else {
        server.base_url.clone()
    };

    ServerSelectorContent {
        name,
        subtitle,
        detail,
        has_server: true,
    }
}

fn server_display_name(server: &ServerIdentity) -> String {
    let name = server.name.trim();
    if name.is_empty() {
        provider_display_name(&server.provider)
    } else {
        name.to_string()
    }
}

fn provider_display_name(provider: &str) -> String {
    match provider {
        "jellyfin" => "Jellyfin".to_string(),
        "fake" => tr("Local"),
        provider => provider.to_string(),
    }
}

fn server_selection_popover(content: &ServerSelectorContent) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.add_css_class("server-selector-popover");

    let row = gtk::Button::new();
    row.add_css_class("flat");
    row.add_css_class("server-option");
    row.set_sensitive(content.has_server);

    let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row_content.set_halign(gtk::Align::Fill);
    row_content.append(&gtk::Image::from_icon_name("object-select-symbolic"));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let name = gtk::Label::new(Some(&content.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let detail = gtk::Label::new(Some(&content.detail));
    detail.add_css_class("muted");
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&name);
    labels.append(&detail);
    row_content.append(&labels);
    row.set_child(Some(&row_content));

    let row_popover = popover.clone();
    row.connect_clicked(move |_| row_popover.popdown());

    wrapper.append(&row);
    popover.set_child(Some(&wrapper));
    popover
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

    shell
        .normal_nav
        .append(&shell.server_selector.normal_button);
}

fn build_compact_navigation(shell: &Rc<Shell>) {
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
    shell
        .compact_nav
        .append(&shell.server_selector.compact_button);
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
            icon_name: "starred-symbolic",
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
            icon_name: "media-playlist-consecutive-symbolic",
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
        let text = gtk::Label::new(Some(&compact_sidebar_label_text(label)));
        configure_rail_label(&text);
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

fn compact_sidebar_label_text(label: &str) -> String {
    let translated = tr(label);
    let compact = {
        let words = translated.split_whitespace().collect::<Vec<_>>();
        if words.len() == 2 {
            Some(format!("{}\n{}", words[0], words[1]))
        } else {
            None
        }
    };
    compact.unwrap_or(translated)
}

fn configure_rail_label(label: &gtk::Label) {
    label.add_css_class("rail-label");
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_lines(2);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
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
        TrackTableColumn::TrackNumber => track_row_index_column(),
        TrackTableColumn::Title => track_identity_column(shell),
        TrackTableColumn::Artist => track_link_column(shell, "Artist", 180, |track| {
            (track.artist.clone(), track_artist_route(track))
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

fn track_table_column_config_title(column: TrackTableColumn) -> &'static str {
    match column {
        TrackTableColumn::Title => "Title (merged)",
        _ => column.title(),
    }
}

fn sync_track_column_checks(
    checks: &Rc<RefCell<Vec<(TrackTableColumn, gtk::CheckButton)>>>,
    settings: &TrackTableSettings,
    syncing: &Cell<bool>,
) {
    syncing.set(true);
    for (column, check) in checks.borrow().iter() {
        check.set_active(settings.visible_columns.contains(column));
    }
    syncing.set(false);
}

fn route_uses_responsive_cards(route: &Route) -> bool {
    matches!(
        route,
        Route::Home
            | Route::Albums
            | Route::Artists
            | Route::AlbumArtists
            | Route::ArtistDetail(_)
            | Route::Genres
            | Route::GenreDetail(_)
            | Route::Playlists
            | Route::PlaylistDetail(_)
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

fn track_row_index_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        list_item.set_child(Some(&label));
    });

    factory.connect_bind(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(child) = list_item.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk::Label>() else {
            return;
        };
        label.set_text(&(list_item.position() + 1).to_string());
    });

    let column = gtk::ColumnViewColumn::new(Some("#"), Some(factory));
    column.set_fixed_width(54);
    column.set_resizable(false);
    column
}

fn track_identity_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
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
        let track = boxed.borrow::<Track>();
        let artist_text = track.artist.clone();
        let artist_route = track_artist_route(&track);
        let cover = shell.cover_tile_for(
            track.image_ref.as_ref(),
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("track-identity");
        row.set_valign(gtk::Align::Center);
        row.set_hexpand(true);
        row.append(&cover);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_valign(gtk::Align::Center);
        labels.set_hexpand(true);

        let title = gtk::Label::new(Some(&track.title));
        title.add_css_class("track-title");
        title.set_xalign(0.0);
        title.set_halign(gtk::Align::Fill);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);

        if !artist_text.trim().is_empty() {
            let artist = gtk::Label::new(Some(&artist_text));
            artist.add_css_class("muted");
            artist.add_css_class("table-link-label");
            artist.set_xalign(0.0);
            artist.set_halign(gtk::Align::Fill);
            artist.set_hexpand(true);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);

            if let Some(route) = artist_route {
                let button = gtk::Button::new();
                button.add_css_class("flat");
                button.add_css_class("table-link");
                button.add_css_class("track-artist-link");
                button.set_halign(gtk::Align::Fill);
                button.set_hexpand(true);
                button.set_cursor_from_name(Some("pointer"));
                add_link_hover(button.upcast_ref(), &artist, &artist_text);
                button.set_child(Some(&artist));

                let shell = Rc::clone(&shell);
                button.connect_clicked(move |_| shell.navigate(route.clone()));
                labels.append(&button);
            } else {
                labels.append(&artist);
            }
        }

        row.append(&labels);
        list_item.set_child(Some(&row));
    });

    factory.connect_unbind(|_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr("Title")), Some(factory));
    column.set_fixed_width(320);
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

fn track_artist_route(track: &Track) -> Option<Route> {
    if let Some(artist_id) = track.artist_id.clone() {
        Some(Route::ArtistDetail(artist_id))
    } else if !track.artist.trim().is_empty() {
        Some(Route::Search {
            query: track.artist.clone(),
            kind: SearchKind::Artists,
        })
    } else {
        None
    }
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
            ((width + HOME_ALBUM_GAP) / item_width)
                .clamp(HOME_ALBUM_MIN_COLUMNS as i32, HOME_ALBUM_MAX_COLUMNS as i32)
                as usize
        })
        .clamp(HOME_ALBUM_MIN_COLUMNS, HOME_ALBUM_MAX_COLUMNS);

    while page_size > HOME_ALBUM_MIN_COLUMNS
        && home_album_raw_card_size(width, page_size) < HOME_ALBUM_MIN_SIZE
    {
        page_size -= 1;
    }
    while page_size < HOME_ALBUM_MAX_COLUMNS
        && home_album_raw_card_size(width, page_size) > HOME_ALBUM_MAX_SIZE
    {
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

fn right_panel_position_ratio(split_width: i32, position: i32) -> f64 {
    if split_width <= 0 {
        return 0.0;
    }
    let right_width = split_width - position.clamp(0, split_width);
    f64::from(right_width) / f64::from(split_width)
}

fn content_split_position_from_right_panel_ratio(split_width: i32, ratio: f64) -> i32 {
    let right_width = (f64::from(split_width) * ratio.clamp(0.0, 1.0)).round() as i32;
    clamp_content_split_position(split_width, split_width - right_width)
}

fn content_split_initial_position(split_width: i32, saved_ratio: Option<f64>) -> i32 {
    saved_ratio
        .filter(|ratio| ratio.is_finite())
        .map(|ratio| content_split_position_from_right_panel_ratio(split_width, ratio))
        .unwrap_or_else(|| default_content_split_position(split_width))
}

fn content_split_target_position(
    split_width: i32,
    previous_width: i32,
    stored_position: i32,
    current_position: i32,
    saved_ratio: Option<f64>,
) -> i32 {
    let target_position = if previous_width <= 1 {
        content_split_initial_position(split_width, saved_ratio)
    } else if previous_width != split_width && stored_position > 1 {
        stored_position * split_width / previous_width
    } else if current_position > 1 {
        current_position
    } else {
        content_split_initial_position(split_width, saved_ratio)
    };
    clamp_content_split_position(split_width, target_position)
}

fn default_content_split_position(split_width: i32) -> i32 {
    split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS
}

fn update_right_panel_split_settings(
    settings: &mut AppSettings,
    split_width: i32,
    position: i32,
) -> bool {
    if split_width <= 1 || position <= 0 || position >= split_width {
        return false;
    }

    let position = clamp_content_split_position(split_width, position);
    let ratio = right_panel_position_ratio(split_width, position);
    if settings.right_panel_position == Some(position) && settings.right_panel_ratio == Some(ratio)
    {
        return false;
    }

    settings.right_panel_position = Some(position);
    settings.right_panel_ratio = Some(ratio);
    true
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
    home_album_content_width_for(
        shell.route_host.width(),
        shell.content_split.width(),
        shell.content_split.position(),
        shell.state.right_panel_visible.get(),
    )
}

fn home_album_content_width_for(
    route_width: i32,
    split_width: i32,
    split_position: i32,
    right_panel_visible: bool,
) -> i32 {
    let mut route_width = if !right_panel_visible && split_width > 1 {
        split_width
    } else {
        route_width
    };
    if right_panel_visible && split_position > 1 {
        route_width = if route_width > 1 {
            route_width.min(split_position)
        } else {
            split_position
        };
    }
    if route_width <= 1 && split_width > 1 {
        route_width = split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS;
    }
    (route_width - HOME_ALBUM_HORIZONTAL_MARGINS).max(HOME_ALBUM_MIN_SIZE)
}

fn home_album_card_size(width: i32, page_size: usize) -> i32 {
    home_album_raw_card_size(width, page_size).clamp(1, HOME_ALBUM_MAX_SIZE)
}

fn home_album_raw_card_size(width: i32, page_size: usize) -> i32 {
    let page_size = page_size.max(1) as i32;
    let gaps = HOME_ALBUM_GAP * (page_size - 1);
    ((width - gaps).max(page_size)) / page_size
}

fn restored_window_size(width: Option<i32>, height: Option<i32>) -> Option<(i32, i32)> {
    let (width, height) = (width?, height?);
    if width < MIN_RESTORED_WINDOW_WIDTH || height < MIN_RESTORED_WINDOW_HEIGHT {
        return None;
    }
    Some((
        width.clamp(MIN_RESTORED_WINDOW_WIDTH, MAX_RESTORED_WINDOW_WIDTH),
        height.clamp(MIN_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_HEIGHT),
    ))
}

fn queue_lyrics_available_height(shell: &Shell) -> i32 {
    let panel_height = shell.right_panel.height();
    if panel_height > QUEUE_LYRICS_MIN_PANE_HEIGHT * 2 {
        return panel_height;
    }
    let window_height = shell.window.height();
    if window_height > MIN_RESTORED_WINDOW_HEIGHT {
        return (window_height - BOTTOM_PLAYER_HEIGHT - 48).max(QUEUE_LYRICS_MIN_PANE_HEIGHT * 2);
    }
    let restored_height = shell
        .state
        .settings
        .borrow()
        .window_height
        .filter(|height| *height >= MIN_RESTORED_WINDOW_HEIGHT)
        .map(|height| height.clamp(MIN_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_HEIGHT))
        .unwrap_or(MAX_RESTORED_WINDOW_HEIGHT);
    (restored_height - BOTTOM_PLAYER_HEIGHT - 48).max(QUEUE_LYRICS_MIN_PANE_HEIGHT * 2)
}

fn clamp_queue_lyrics_position(available_height: i32, position: i32) -> i32 {
    let max_position =
        (available_height - QUEUE_LYRICS_MIN_PANE_HEIGHT).max(QUEUE_LYRICS_MIN_PANE_HEIGHT);
    position.clamp(QUEUE_LYRICS_MIN_PANE_HEIGHT, max_position)
}

fn queue_lyrics_default_position(available_height: i32) -> i32 {
    let total_units = QUEUE_LYRICS_DEFAULT_QUEUE_UNITS + QUEUE_LYRICS_DEFAULT_LYRICS_UNITS;
    let position = available_height * QUEUE_LYRICS_DEFAULT_QUEUE_UNITS / total_units;
    clamp_queue_lyrics_position(available_height, position)
}

fn queue_lyrics_position_ratio(available_height: i32, position: i32) -> f64 {
    if available_height <= 0 {
        return 0.0;
    }
    f64::from(position).clamp(0.0, f64::from(available_height)) / f64::from(available_height)
}

fn queue_lyrics_position_from_ratio(available_height: i32, ratio: f64) -> i32 {
    let position = (f64::from(available_height) * ratio.clamp(0.0, 1.0)).round() as i32;
    clamp_queue_lyrics_position(available_height, position)
}

fn queue_lyrics_initial_position(available_height: i32, saved_ratio: Option<f64>) -> i32 {
    saved_ratio
        .filter(|ratio| ratio.is_finite())
        .map(|ratio| queue_lyrics_position_from_ratio(available_height, ratio))
        .unwrap_or_else(|| queue_lyrics_default_position(available_height))
}

fn set_queue_lyrics_split_position_without_saving(
    split: &gtk::Paned,
    suppress_save: &Rc<Cell<u32>>,
    saved_ratio: Option<f64>,
) {
    let available_height = split.height();
    if available_height < QUEUE_LYRICS_READY_MIN_HEIGHT {
        return;
    }

    suppress_save.set(suppress_save.get().saturating_add(1));
    split.set_position(queue_lyrics_initial_position(available_height, saved_ratio));
    let suppress = Rc::clone(suppress_save);
    glib::idle_add_local_once(move || {
        suppress.set(suppress.get().saturating_sub(1));
    });
}

fn card_label_width_chars(size: i32) -> i32 {
    (size / 8).clamp(8, 28)
}

fn constrain_card_label(label: &gtk::Label, size: i32) {
    label.set_width_request(size);
    label.set_size_request(size, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(card_label_width_chars(size));
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(false);
}

fn clipped_card_label(label: &gtk::Label, size: i32) -> gtk::Widget {
    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("card-label-clip");
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_width_request(size);
    clip.set_size_request(size, -1);
    clip.set_min_content_width(size);
    clip.set_max_content_width(size);
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(true);
    clip.set_hexpand(false);
    clip.set_child(Some(label));
    clip.upcast()
}

fn clipped_card_label_with_lines(label: &gtk::Label, size: i32, lines: i32) -> gtk::Widget {
    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("card-label-clip");
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_width_request(size);
    clip.set_size_request(size, card_label_height(lines));
    clip.set_min_content_width(size);
    clip.set_max_content_width(size);
    clip.set_min_content_height(card_label_height(lines));
    clip.set_max_content_height(card_label_height(lines));
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(false);
    clip.set_hexpand(false);
    clip.set_child(Some(label));
    clip.upcast()
}

fn card_label_height(lines: i32) -> i32 {
    CARD_LABEL_LINE_HEIGHT * lines.max(1)
}

fn home_album_card_height(size: i32) -> i32 {
    size + HOME_ALBUM_CARD_LABEL_GAP * 3
        + card_label_height(HOME_ALBUM_TITLE_LINES)
        + card_label_height(HOME_ALBUM_ARTIST_LINES)
        + card_label_height(HOME_ALBUM_YEAR_LINES)
}

fn constrain_wrapped_card_label(label: &gtk::Label, size: i32, lines: i32) {
    constrain_card_label(label, size);
    label.set_lines(lines);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
}

fn constrain_single_line_card_label(label: &gtk::Label, size: i32) {
    constrain_card_label(label, size);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
}

fn album_card_widget_with_size(
    shell: &Rc<Shell>,
    album: &Album,
    size: i32,
    controller: Option<&AppController>,
    label_layout: AlbumCardLabelLayout,
) -> gtk::Widget {
    let card = gtk::Box::new(
        gtk::Orientation::Vertical,
        match label_layout {
            AlbumCardLabelLayout::Natural => 6,
            AlbumCardLabelLayout::StableHome => HOME_ALBUM_CARD_LABEL_GAP,
        },
    );
    card.add_css_class("album-card");
    card.set_width_request(size);
    match label_layout {
        AlbumCardLabelLayout::Natural => card.set_size_request(size, -1),
        AlbumCardLabelLayout::StableHome => {
            card.set_size_request(size, home_album_card_height(size))
        }
    };
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);
    let cover = album_cover_tile(shell, album, size, controller);
    card.append(&cover);

    let title = gtk::Label::new(Some(&album.title));
    title.add_css_class("album-title");
    title.set_xalign(0.0);
    constrain_wrapped_card_label(&title, size, 2);
    let title_clip = match label_layout {
        AlbumCardLabelLayout::Natural => clipped_card_label(&title, size),
        AlbumCardLabelLayout::StableHome => {
            clipped_card_label_with_lines(&title, size, HOME_ALBUM_TITLE_LINES)
        }
    };
    add_link_hover(&title_clip, &title, &album.title);
    let artist = gtk::Label::new(Some(&album.artist));
    artist.add_css_class("muted");
    artist.set_xalign(0.0);
    match label_layout {
        AlbumCardLabelLayout::Natural => constrain_single_line_card_label(&artist, size),
        AlbumCardLabelLayout::StableHome => {
            constrain_wrapped_card_label(&artist, size, HOME_ALBUM_ARTIST_LINES)
        }
    };
    let artist_clip = match label_layout {
        AlbumCardLabelLayout::Natural => clipped_card_label(&artist, size),
        AlbumCardLabelLayout::StableHome => {
            clipped_card_label_with_lines(&artist, size, HOME_ALBUM_ARTIST_LINES)
        }
    };
    add_link_hover(&artist_clip, &artist, &album.artist);
    let year = gtk::Label::new(Some(&album.year.to_string()));
    year.add_css_class("muted");
    year.set_xalign(0.0);
    constrain_single_line_card_label(&year, size);
    let year_clip = match label_layout {
        AlbumCardLabelLayout::Natural => clipped_card_label(&year, size),
        AlbumCardLabelLayout::StableHome => {
            clipped_card_label_with_lines(&year, size, HOME_ALBUM_YEAR_LINES)
        }
    };

    card.append(&title_clip);
    card.append(&artist_clip);
    card.append(&year_clip);
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
    constrain_wrapped_card_label(&name, size, 2);
    let name_clip = clipped_card_label(&name, size);

    let counts = gtk::Label::new(Some(&format!(
        "{} {} / {} {}",
        artist.album_count,
        tr("albums"),
        artist.track_count,
        tr("tracks")
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    constrain_single_line_card_label(&counts, size);
    let counts_clip = clipped_card_label(&counts, size);

    card.append(&name_clip);
    card.append(&counts_clip);
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
    constrain_wrapped_card_label(&name, size, 2);
    let name_clip = clipped_card_label(&name, size);
    let counts = gtk::Label::new(Some(&format!(
        "{} {} / {} {}",
        genre.album_count,
        tr("albums"),
        genre.track_count,
        tr("tracks")
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    constrain_single_line_card_label(&counts, size);
    let counts_clip = clipped_card_label(&counts, size);
    card.append(&name_clip);
    card.append(&counts_clip);
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
    constrain_wrapped_card_label(&name, size, 2);
    let name_clip = clipped_card_label(&name, size);
    let counts = gtk::Label::new(Some(&format!(
        "{} {} • {}",
        playlist.track_count,
        tr("tracks"),
        format_duration(playlist.duration_seconds)
    )));
    counts.add_css_class("muted");
    counts.set_xalign(0.0);
    constrain_single_line_card_label(&counts, size);
    let counts_clip = clipped_card_label(&counts, size);
    card.append(&name_clip);
    card.append(&counts_clip);
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
    label.set_width_chars(1);
    label.set_max_width_chars(BOTTOM_PLAYER_IDENTITY_MAX_CHARS);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(false);
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

fn current_playback_track_id(snapshot: &PlaybackSnapshot) -> Option<rufin_core::TrackId> {
    snapshot
        .current
        .as_ref()
        .map(|entry| entry.track_id.clone())
}

fn active_lyrics_line_index(lines: &[LyricLine], position_millis: u64) -> Option<usize> {
    let first_timed_index = lines.iter().position(|line| line.start_millis.is_some());
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let start = line.start_millis?;
            (start <= position_millis).then_some((index, start))
        })
        .max_by_key(|(_, start)| *start)
        .map(|(index, _)| index)
        .or(first_timed_index)
}

fn next_lyrics_line_start_after(lines: &[LyricLine], position_millis: u64) -> Option<u64> {
    lines
        .iter()
        .filter_map(|line| line.start_millis)
        .filter(|start| *start > position_millis)
        .min()
}

fn lyrics_follow_scroll_pause_state(
    paused_until: Option<Instant>,
    now: Instant,
) -> LyricsFollowScrollPause {
    match paused_until {
        Some(paused_until) if now < paused_until => LyricsFollowScrollPause::Active,
        Some(_) => LyricsFollowScrollPause::Expired,
        None => LyricsFollowScrollPause::Inactive,
    }
}

fn lyrics_scroll_animation_millis(
    lines: &[LyricLine],
    active_index: usize,
    position_millis: u64,
) -> u64 {
    let budget = lines
        .iter()
        .skip(active_index + 1)
        .filter_map(|line| line.start_millis)
        .find(|start| *start > position_millis)
        .and_then(|next_start| {
            next_start
                .saturating_sub(position_millis)
                .checked_sub(LYRICS_SCROLL_FINISH_BEFORE_NEXT_MS)
        });
    budget
        .map(|budget| {
            budget.clamp(
                MIN_LYRICS_SCROLL_ANIMATION_MS,
                DEFAULT_LYRICS_SCROLL_ANIMATION_MS,
            )
        })
        .unwrap_or(DEFAULT_LYRICS_SCROLL_ANIMATION_MS)
}

fn scroll_lyrics_row_into_view(
    shell: Rc<Shell>,
    scroller: gtk::ScrolledWindow,
    row: gtk::Widget,
    duration_millis: u64,
    generation: u64,
) {
    glib::idle_add_local_once(move || {
        let Some(bounds) = row.compute_bounds(&scroller) else {
            return;
        };
        let adjustment = scroller.vadjustment();
        let viewport_height = f64::from(scroller.height().max(1));
        let row_center = adjustment.value() + f64::from(bounds.y() + bounds.height() / 2.0);
        let target = row_center - viewport_height / 2.0;
        let upper = adjustment.upper() - adjustment.page_size();
        let target = target.clamp(adjustment.lower(), upper.max(adjustment.lower()));
        let start = adjustment.value();
        let delta = target - start;
        if duration_millis == 0 || delta.abs() < 1.0 {
            adjustment.set_value(target);
            return;
        }
        let started_at = Instant::now();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if shell.state.lyrics_scroll_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let elapsed = started_at.elapsed().as_millis() as f64;
            let progress = (elapsed / duration_millis as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            adjustment.set_value(start + delta * eased);
            if progress >= 1.0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
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

fn set_favorite_button_active(button: &gtk::Button, active: bool) {
    set_active_class(button, active);
    button.set_label(if active {
        FAVORITE_FILLED_GLYPH
    } else {
        FAVORITE_EMPTY_GLYPH
    });
}

fn primary_menu_button() -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_icon_name("open-menu-symbolic");
    button.set_primary(true);
    let label = tr("Main Menu");
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
    button.set_menu_model(Some(&primary_menu_model()));
    button
}

fn primary_menu_model() -> gio::Menu {
    let menu = gio::Menu::new();
    let view = gio::Menu::new();
    view.append(
        Some(&tr("Toggle Fullscreen")),
        Some("win.toggle-fullscreen"),
    );
    menu.append_section(None, &view);

    let preferences = gio::Menu::new();
    preferences.append(Some(&tr("Preferences")), Some("win.preferences"));
    preferences.append(Some(&tr("Keyboard Shortcuts")), Some("win.show-shortcuts"));
    menu.append_section(None, &preferences);

    let about = gio::Menu::new();
    about.append(Some(&tr("About Rufin")), Some("win.about"));
    menu.append_section(None, &about);
    menu
}

fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));
    button
}

fn skip_icon_button(forward: bool, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&tr(label)));

    let icon = gtk::DrawingArea::new();
    icon.set_content_width(16);
    icon.set_content_height(16);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let width = f64::from(width);
        let height = f64::from(height);
        let center_y = height / 2.0;
        let top = center_y - 4.5;
        let bottom = center_y + 4.5;
        if forward {
            context.move_to(width * 0.30, top);
            context.line_to(width * 0.30, bottom);
            context.line_to(width * 0.70, center_y);
            context.close_path();
            let _ = context.fill();
            context.rectangle(width * 0.76, top, 2.0, bottom - top);
            let _ = context.fill();
        } else {
            context.rectangle(width * 0.20, top, 2.0, bottom - top);
            let _ = context.fill();
            context.move_to(width * 0.70, top);
            context.line_to(width * 0.70, bottom);
            context.line_to(width * 0.30, center_y);
            context.close_path();
            let _ = context.fill();
        }
    });
    button.set_child(Some(&icon));
    button
}

fn queue_sidebar_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    let label = tr(label);
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);

    let open = Rc::new(Cell::new(true));
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(16);
    icon.set_content_height(16);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);

    let icon_open = Rc::clone(&open);
    icon.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        let set_source = |alpha: f64| {
            context.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()) * alpha,
            );
        };

        let width = f64::from(width);
        let height = f64::from(height);
        let x = (width - 14.0) / 2.0;
        let y = (height - 12.0) / 2.0;
        let icon_width = 14.0;
        let icon_height = 12.0;
        let separator_x = x + icon_width - 4.5;
        let center_y = y + icon_height / 2.0;

        if icon_open.get() {
            set_source(0.32);
            context.rectangle(separator_x, y, icon_width - (separator_x - x), icon_height);
            let _ = context.fill();
        }

        set_source(1.0);
        context.set_line_width(1.4);
        context.rectangle(x + 0.7, y + 0.7, icon_width - 1.4, icon_height - 1.4);
        let _ = context.stroke();

        context.move_to(separator_x, y + 1.2);
        context.line_to(separator_x, y + icon_height - 1.2);
        let _ = context.stroke();

        if !icon_open.get() {
            context.set_line_width(1.5);
            context.move_to(separator_x + 2.6, center_y - 3.0);
            context.line_to(separator_x + 1.0, center_y);
            context.line_to(separator_x + 2.6, center_y + 3.0);
            let _ = context.stroke();
        }
    });
    button.set_child(Some(&icon));
    (button, icon, open)
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
        HOME_ALBUM_GAP, HOME_ALBUM_MAX_COLUMNS, HOME_ALBUM_MAX_SIZE, LyricsFollowScrollPause,
        active_lyrics_line_index, card_label_height, clamp_content_split_position,
        clamp_home_album_page_start, clamp_queue_lyrics_position, content_split_initial_position,
        content_split_position_from_right_panel_ratio, content_split_target_position,
        current_playback_track_id, default_content_split_position, home_album_card_height,
        home_album_card_size, home_album_content_width_for, home_album_page_size,
        lyrics_follow_scroll_pause_state, lyrics_scroll_animation_millis,
        next_lyrics_line_start_after, queue_lyrics_default_position, queue_lyrics_initial_position,
        queue_lyrics_position_from_ratio, queue_lyrics_position_ratio, restored_window_size,
        right_panel_position_ratio, update_right_panel_split_settings,
    };
    use rufin_core::{AppSettings, QueueEntry, QueueEntryId, TrackId};
    use rufin_provider::LyricLine;
    use std::time::{Duration, Instant};

    #[test]
    fn home_album_page_size_uses_stable_content_width() {
        let three_cards_width = super::HOME_ALBUM_TARGET_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, None), 3);
        assert_eq!(home_album_page_size(three_cards_width + 1, None), 3);

        let four_cards_width = super::HOME_ALBUM_TARGET_SIZE * 4 + HOME_ALBUM_GAP * 3;
        assert_eq!(home_album_page_size(four_cards_width, None), 4);
        assert_eq!(home_album_page_size(1, None), 2);
        assert_eq!(home_album_page_size(10_000, None), HOME_ALBUM_MAX_COLUMNS);
    }

    #[test]
    fn home_album_page_size_changes_without_bouncing_near_size_bounds() {
        let three_cards_width = super::HOME_ALBUM_MIN_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_width - 1, Some(3)), 3);
        assert_eq!(
            home_album_page_size(
                (super::HOME_ALBUM_MIN_SIZE - 20) * 3 + HOME_ALBUM_GAP * 2,
                Some(3)
            ),
            2
        );

        let three_cards_max_width = HOME_ALBUM_MAX_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_max_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_max_width + 3, Some(3)), 4);
    }

    #[test]
    fn home_album_page_size_adds_columns_on_wide_layouts() {
        let ten_target_cards_width = super::HOME_ALBUM_TARGET_SIZE * 10 + HOME_ALBUM_GAP * 9;

        assert_eq!(home_album_page_size(ten_target_cards_width, None), 10);
        assert_eq!(home_album_page_size(ten_target_cards_width, Some(7)), 9);
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
    fn home_album_width_uses_full_split_width_when_right_panel_is_hidden() {
        let stale_route_width = 640;
        let split_width = 1_000;
        assert_eq!(
            home_album_content_width_for(stale_route_width, split_width, 650, false),
            split_width - super::HOME_ALBUM_HORIZONTAL_MARGINS
        );
        assert_eq!(
            home_album_content_width_for(900, split_width, 650, true),
            650 - super::HOME_ALBUM_HORIZONTAL_MARGINS
        );
    }

    #[test]
    fn home_album_card_height_reserves_five_text_rows() {
        assert_eq!(
            home_album_card_height(180),
            180 + super::HOME_ALBUM_CARD_LABEL_GAP * 3 + card_label_height(5)
        );
    }

    #[test]
    fn content_split_position_limits_right_panel() {
        assert_eq!(clamp_content_split_position(1_000, 100), 500);
        assert_eq!(clamp_content_split_position(1_000, 950), 900);
        assert_eq!(clamp_content_split_position(1_000, 625), 625);
        assert_eq!(default_content_split_position(1_000), 500);
        assert_eq!(content_split_initial_position(1_000, None), 500);
        assert_eq!(content_split_initial_position(1_000, Some(0.25)), 750);
        assert_eq!(
            content_split_position_from_right_panel_ratio(1_000, 0.25),
            750
        );
        assert_eq!(right_panel_position_ratio(1_000, 750), 0.25);
        assert_eq!(content_split_target_position(1_000, 0, 0, 600, None), 500);
        assert_eq!(
            content_split_target_position(1_400, 1_000, 500, 700, None),
            700
        );
        let mut settings = AppSettings::default();
        assert!(update_right_panel_split_settings(&mut settings, 1_000, 650));
        assert_eq!(settings.right_panel_position, Some(650));
        assert_eq!(settings.right_panel_ratio, Some(0.35));
    }

    #[test]
    fn restored_window_size_ignores_tiny_and_clamps_huge_geometry() {
        assert_eq!(restored_window_size(None, Some(700)), None);
        assert_eq!(restored_window_size(Some(400), Some(700)), None);
        assert_eq!(
            restored_window_size(Some(1061), Some(2251)),
            Some((1061, 900))
        );
        assert_eq!(
            restored_window_size(Some(1800), Some(1200)),
            Some((1400, 900))
        );
    }

    #[test]
    fn queue_lyrics_position_clamps_to_available_height() {
        assert_eq!(clamp_queue_lyrics_position(800, 1701), 680);
        assert_eq!(clamp_queue_lyrics_position(800, 10), 120);
        assert_eq!(clamp_queue_lyrics_position(200, 1701), 120);
        assert_eq!(queue_lyrics_default_position(700), 500);
        assert_eq!(queue_lyrics_default_position(1400), 1000);
        assert_eq!(queue_lyrics_initial_position(700, None), 500);
        assert_eq!(queue_lyrics_initial_position(700, Some(0.5)), 350);
        assert_eq!(queue_lyrics_initial_position(700, Some(2.0)), 580);
        assert_eq!(queue_lyrics_initial_position(700, Some(f64::NAN)), 500);
        assert_eq!(queue_lyrics_position_from_ratio(700, 0.5), 350);
        assert_eq!(queue_lyrics_position_ratio(700, 350), 0.5);
        let saved_default_ratio = queue_lyrics_position_ratio(700, 500);
        assert_eq!(
            queue_lyrics_initial_position(1400, Some(saved_default_ratio)),
            1000
        );
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
                start_millis: Some(5_500),
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

        assert_eq!(active_lyrics_line_index(&lines, 999), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 1_000), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 5_499), Some(0));
        assert_eq!(active_lyrics_line_index(&lines, 5_500), Some(1));
        assert_eq!(active_lyrics_line_index(&lines, 8_999), Some(1));
        assert_eq!(active_lyrics_line_index(&lines, 9_000), Some(3));
    }

    #[test]
    fn synced_lyrics_without_timed_lines_have_no_highlight() {
        let lines = vec![LyricLine {
            text: "plain".to_string(),
            start_millis: None,
        }];

        assert_eq!(active_lyrics_line_index(&lines, 0), None);
    }

    #[test]
    fn synced_lyrics_schedule_next_started_line() {
        let lines = vec![
            LyricLine {
                text: "intro".to_string(),
                start_millis: Some(1_000),
            },
            LyricLine {
                text: "verse".to_string(),
                start_millis: Some(5_500),
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

        assert_eq!(next_lyrics_line_start_after(&lines, 999), Some(1_000));
        assert_eq!(next_lyrics_line_start_after(&lines, 1_000), Some(5_500));
        assert_eq!(next_lyrics_line_start_after(&lines, 5_499), Some(5_500));
        assert_eq!(next_lyrics_line_start_after(&lines, 5_500), Some(9_000));
        assert_eq!(next_lyrics_line_start_after(&lines, 9_000), None);
    }

    #[test]
    fn lyrics_scroll_animation_finishes_before_next_line() {
        let lines = vec![
            LyricLine {
                text: "current".to_string(),
                start_millis: Some(5_500),
            },
            LyricLine {
                text: "next".to_string(),
                start_millis: Some(6_000),
            },
        ];

        let duration = lyrics_scroll_animation_millis(&lines, 0, 5_500);

        assert!(duration <= 300);
        assert!(duration >= 80);
        assert_eq!(
            lyrics_scroll_animation_millis(&lines, 0, 5_501),
            duration - 1
        );
    }

    #[test]
    fn lyrics_follow_scroll_pause_expires() {
        let now = Instant::now();

        assert_eq!(
            lyrics_follow_scroll_pause_state(None, now),
            LyricsFollowScrollPause::Inactive
        );
        assert_eq!(
            lyrics_follow_scroll_pause_state(Some(now + Duration::from_millis(1)), now),
            LyricsFollowScrollPause::Active
        );
        assert_eq!(
            lyrics_follow_scroll_pause_state(Some(now), now),
            LyricsFollowScrollPause::Expired
        );
    }

    #[test]
    fn current_playback_track_id_uses_restored_current_entry() {
        let track_id = TrackId::fake(7);
        let snapshot = super::PlaybackSnapshot {
            current: Some(QueueEntry {
                id: QueueEntryId::new("queue-7"),
                track_id: track_id.clone(),
                album_id: None,
                title: "Restored".to_string(),
                artist: "Artist".to_string(),
                artist_id: None,
                album: "Album".to_string(),
                year: 2026,
                duration_seconds: 180,
                favorite: false,
                image_ref: None,
            }),
            ..super::PlaybackSnapshot::default()
        };

        assert_eq!(current_playback_track_id(&snapshot), Some(track_id));
        assert_eq!(
            current_playback_track_id(&super::PlaybackSnapshot::default()),
            None
        );
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
