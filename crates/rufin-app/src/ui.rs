use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

mod favorites;
mod layout;
mod mpris;
mod navigation;
mod player;
mod preferences;
mod queue;
mod right_panel;

use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
use mpris_server::Player as MprisPlayer;
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, DensityMode, EffectiveDensity, Genre,
    HomeSection, HomeSectionKind, ImageRef, Playlist, PlaylistId, QueueSnapshot, Route, RouteStack,
    SearchKind, Track, TrackSortKey, TrackTableColumn, TrackTableSettings, format_duration,
};
use rufin_playback::PlaybackState;
use rufin_provider::{FavoriteItemId, Lyrics};
use rufin_store::{CachedArtistDetail, CachedGenreDetail, image_cache_key};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{AppController, ControllerEvent, LibrarySnapshot, PlaybackSnapshot};
use crate::i18n::tr;
use crate::lyrics::{LyricsPane, next_lyrics_line_start_after};
use favorites::{
    FavoriteControlKey, FavoriteControls, album_favorite_key, artist_favorite_key,
    clear_favorite_controls, favorite_change_needs_route_render, favorite_control_key,
    merge_favorite_snapshot, register_favorite_control, track_favorite_key,
    update_favorite_controls,
};
use layout::{
    COMPACT_RAIL_WIDTH, HOME_ALBUM_ARTIST_LINES, HOME_ALBUM_CARD_LABEL_GAP, HOME_ALBUM_GAP,
    HOME_ALBUM_TITLE_LINES, HOME_ALBUM_YEAR_LINES, NORMAL_SIDEBAR_WIDTH, PRIMARY_ROUTE_MARGIN_END,
    PRIMARY_ROUTE_MARGIN_START, clamp_home_album_page_start, clipped_card_label,
    clipped_card_label_with_lines, constrain_single_line_card_label, constrain_wrapped_card_label,
    content_split_target_position, home_album_card_height, home_album_card_size,
    home_album_content_width, home_album_page_size, restored_window_size,
    update_right_panel_split_settings,
};
use mpris::install_mpris;
use navigation::{
    ServerSelector, build_compact_navigation, build_normal_navigation, build_server_selector,
    sidebar_history_button,
};
use player::{PlayerControls, build_bottom_player, connect_player_controls};
use preferences::present_preferences_dialog;
use queue::connect_queue_panel_controls;
use right_panel::{
    apply_lyrics_panel_visibility, apply_right_panel_visibility, build_right_panel,
    connect_queue_lyrics_split,
};

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
    lyrics_timing_generation: Cell<u64>,
    lyrics_timing_source: RefCell<Option<glib::SourceId>>,
    mpris_player: RefCell<Option<Rc<MprisPlayer>>>,
    updating_player_controls: Cell<bool>,
    seeking_player_controls: Cell<bool>,
    seek_generation: Cell<u64>,
    queue_filter: RefCell<String>,
    right_panel_visible: Cell<bool>,
    lyrics_panel_visible: Cell<bool>,
    split_width: Cell<i32>,
    split_position: Cell<i32>,
    queue_lyrics_position_save_suppressed: Rc<Cell<u32>>,
    responsive_render_queued: Cell<bool>,
    card_grid_columns: Cell<usize>,
    home_section_state: RefCell<HashMap<HomeSectionKind, HomeSectionState>>,
    cover_bindings: RefCell<HashMap<String, Vec<CoverBinding>>>,
    cover_decodes: RefCell<HashSet<String>>,
    decoded_covers: RefCell<HashMap<String, Pixbuf>>,
    decoded_cover_order: RefCell<VecDeque<String>>,
    favorite_controls: FavoriteControls,
    perf: Option<Rc<UiPerfMonitor>>,
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
    normal_back_button: gtk::Button,
    normal_forward_button: gtk::Button,
    compact_back_button: gtk::Button,
    compact_forward_button: gtk::Button,
    right_panel: gtk::Box,
    queue_panel: gtk::Box,
    queue_search: gtk::SearchEntry,
    queue_clear_button: gtk::Button,
    queue_lyrics_split: gtk::Paned,
    lyrics_pane: LyricsPane,
    player_controls: PlayerControls,
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
        lyrics_timing_generation: Cell::new(0),
        lyrics_timing_source: RefCell::new(None),
        mpris_player: RefCell::new(None),
        updating_player_controls: Cell::new(false),
        seeking_player_controls: Cell::new(false),
        seek_generation: Cell::new(0),
        queue_filter: RefCell::new(String::new()),
        right_panel_visible: Cell::new(settings.right_panel_visible),
        lyrics_panel_visible: Cell::new(settings.lyrics_panel_visible),
        split_width: Cell::new(0),
        split_position: Cell::new(0),
        queue_lyrics_position_save_suppressed: Rc::new(Cell::new(0)),
        responsive_render_queued: Cell::new(false),
        card_grid_columns: Cell::new(0),
        home_section_state: RefCell::new(HashMap::new()),
        cover_bindings: RefCell::new(HashMap::new()),
        cover_decodes: RefCell::new(HashSet::new()),
        decoded_covers: RefCell::new(HashMap::new()),
        decoded_cover_order: RefCell::new(VecDeque::new()),
        favorite_controls: RefCell::new(HashMap::new()),
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

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 5);
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
    header.set_margin_end(52);

    let normal_back_button = sidebar_history_button("go-previous-symbolic", "Back");
    let normal_forward_button = sidebar_history_button("go-next-symbolic", "Forward");
    let compact_back_button = sidebar_history_button("go-previous-symbolic", "Back");
    let compact_forward_button = sidebar_history_button("go-next-symbolic", "Forward");
    let route_title = gtk::Label::new(None);
    route_title.add_css_class("route-title");
    route_title.set_xalign(0.0);
    route_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    route_title.set_halign(gtk::Align::Fill);
    route_title.set_valign(gtk::Align::Center);
    route_title.set_hexpand(true);
    let main_menu = primary_menu_button();

    header.append(&route_title);

    let route_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);

    main_area.append(&header);
    main_area.append(&route_host);

    let right_panel_parts = build_right_panel();
    let right_panel = right_panel_parts.root;
    let queue_panel = right_panel_parts.queue_panel;
    let queue_search = right_panel_parts.queue_search;
    let queue_clear_button = right_panel_parts.queue_clear_button;
    let queue_lyrics_split = right_panel_parts.queue_lyrics_split;
    let lyrics_pane = right_panel_parts.lyrics_pane;

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

    let upper_overlay = gtk::Overlay::new();
    upper_overlay.set_hexpand(true);
    upper_overlay.set_vexpand(true);
    upper_overlay.set_child(Some(&upper));
    main_menu.set_halign(gtk::Align::End);
    main_menu.set_valign(gtk::Align::Start);
    main_menu.set_margin_top(7);
    main_menu.set_margin_end(8);
    upper_overlay.add_overlay(&main_menu);

    root.append(&upper_overlay);
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
        normal_back_button,
        normal_forward_button,
        compact_back_button,
        compact_forward_button,
        right_panel,
        queue_panel,
        queue_search,
        queue_clear_button,
        queue_lyrics_split,
        lyrics_pane,
        player_controls,
    });

    build_normal_navigation(&shell);
    build_compact_navigation(&shell);
    shell.update_server_selector();
    connect_shell_actions(&shell, main_menu);
    connect_queue_panel_controls(&shell);
    connect_queue_lyrics_split(&shell);
    connect_player_controls(&shell);
    install_mpris(&shell);
    shell.update_density();
    prime_first_cached_cover(&shell);
    shell.render_current_route();
    shell.render_queue_panel();
    shell.render_lyrics_panel();
    shell.update_bottom_player();
    shell.update_right_panel_button();
    shell.update_lyrics_panel_button();
    if !shell.state.right_panel_visible.get() {
        apply_right_panel_visibility(Rc::clone(&shell), false);
    }
    if !shell.state.lyrics_panel_visible.get() {
        apply_lyrics_panel_visibility(Rc::clone(&shell), false);
    }
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
        self.refresh_search_results_for_route(&route);
        self.state.routes.borrow_mut().navigate(route);
        self.render_current_route();
    }

    fn go_back(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.refresh_search_results_for_route(&route);
            self.render_current_route();
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.refresh_search_results_for_route(&route);
            self.render_current_route();
        }
    }

    fn refresh_search_results_for_route(&self, route: &Route) {
        if let Route::Search { query, .. } = route {
            self.controller.search(query.clone());
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
        self.render_lyrics_panel();
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
        let right_panel_visible = self.state.right_panel_visible.get();
        if settings.right_panel_visible != right_panel_visible {
            settings.right_panel_visible = right_panel_visible;
            changed = true;
        }

        if !changed {
            return;
        }
        if let Err(error) = self.controller.save_settings(&settings) {
            warn!(%error, "failed to save window state");
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
        let library = self.state.library.borrow();
        navigation::update_server_selector(&self.server_selector, &library);
    }

    fn set_history_buttons_sensitive(&self, can_back: bool, can_forward: bool) {
        self.normal_back_button.set_sensitive(can_back);
        self.compact_back_button.set_sensitive(can_back);
        self.normal_forward_button.set_sensitive(can_forward);
        self.compact_forward_button.set_sensitive(can_forward);
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
        let lyrics = self.state.lyrics.borrow();
        self.lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.schedule_next_lyrics_highlight(position_millis);
    }

    fn current_position_millis(&self) -> u64 {
        self.state.player.borrow().position_millis
    }

    fn seek_to_lyrics_position(self: &Rc<Self>, position_millis: u64) {
        self.lyrics_pane.clear_follow_scroll_pause();
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
        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let first_run = self.state.library.borrow().first_run;
        if first_run {
            let route_name = "FirstRun".to_string();
            self.route_title.set_text(&tr("Add Jellyfin Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.route_host.append(&view);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.route_title.set_text(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );

        let view = match route {
            Route::Home => self.home_view(),
            Route::Albums => self.albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.tracks_route_view(),
            Route::Favorites => {
                let favorites = self.state.library.borrow().favorites.clone();
                self.tracks_view(favorites, "favorites")
            }
            Route::Artists => self.artist_list_view(false),
            Route::ArtistDetail(artist_id) => self.artist_detail_view(artist_id),
            Route::AlbumArtists => self.artist_list_view(true),
            Route::Genres => self.genre_list_view(),
            Route::GenreDetail(genre_id) => self.genre_detail_view(genre_id),
            Route::Playlists => self.playlist_list_view(),
            Route::PlaylistDetail(playlist_id) => self.playlist_detail_view(playlist_id),
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };

        self.route_host.append(&view);
        self.record_perf_route_render(route_name, render_started.elapsed());
    }

    fn register_favorite_button(&self, key: FavoriteControlKey, button: &gtk::Button) {
        register_favorite_control(&self.state.favorite_controls, key, button);
    }

    fn update_visible_favorite_buttons(&self, item_id: &FavoriteItemId, favorite: bool) {
        let key = favorite_control_key(item_id);
        update_favorite_controls(&self.state.favorite_controls, &key, favorite);
    }

    fn apply_favorite_changed(
        self: &Rc<Self>,
        item_id: FavoriteItemId,
        favorite: bool,
        snapshot: LibrarySnapshot,
    ) {
        let route = self.state.routes.borrow().current().clone();
        {
            let mut library = self.state.library.borrow_mut();
            merge_favorite_snapshot(
                &mut library,
                snapshot,
                &item_id,
                favorite,
                matches!(route, Route::Search { .. }),
            );
        }

        self.update_visible_favorite_buttons(&item_id, favorite);
        let track_sort_key = self.state.settings.borrow().track_table.sort_key;
        if favorite_change_needs_route_render(&route, &item_id, track_sort_key) {
            self.render_current_route();
        }
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
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(PRIMARY_ROUTE_MARGIN_END);

        for section in &self.state.library.borrow().home_sections {
            content.append(&self.home_section(section));
        }

        if self.state.library.borrow().home_sections.is_empty() {
            content
                .append(&self.route_empty_view(
                    "Cached library data will appear here as sync pages finish.",
                ));
        }

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn home_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        if !section_data.tracks.is_empty() {
            self.home_track_section(section_data)
        } else {
            self.home_album_section(section_data)
        }
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

    fn home_track_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);
        let section_kind = section_data.kind;
        let tracks = section_data.tracks.clone();

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
        let tracks_for_next = tracks.clone();
        next.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            let next_page = state.page_start.saturating_add(state.page_size);
            if next_page < tracks_for_next.len() {
                state.page_start = next_page;
            }
            drop(states);
            shell.render_current_route();
        });

        render_home_track_page(self, &row, &previous, &next, section_kind, &tracks);
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
        self.register_favorite_button(album_favorite_key(&album.id), &favorite);
        let controller = self.controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
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

    fn tracks_view(self: &Rc<Self>, tracks: Vec<Track>, context: &str) -> gtk::Widget {
        self.tracks_view_with_paging(tracks, context, None)
    }

    fn tracks_route_view(self: &Rc<Self>) -> gtk::Widget {
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
        self.tracks_view_with_paging(page.items, "tracks", Some((offset, total)))
    }

    fn tracks_view_with_paging(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
        paging: Option<(usize, usize)>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        wrapper.append(&self.tracks_table_with_paging(tracks, context, paging));
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

    fn artist_list_view(self: &Rc<Self>, album_artist: bool) -> gtk::Widget {
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
        self.register_favorite_button(artist_favorite_key(&artist.id), &favorite);
        let controller = self.controller.clone();
        let artist_id = artist.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_artist_favorite(artist_id.clone(), !favorite_button_is_active(button));
        });
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

    fn genre_list_view(self: &Rc<Self>) -> gtk::Widget {
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
            page.items.is_empty(),
            "Cached rows will appear here after the background sync finishes.",
            self.genre_cards_grid_for_model(model),
            Some(load_next),
        )
    }

    fn playlist_list_view(self: &Rc<Self>) -> gtk::Widget {
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
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_halign(gtk::Align::End);
        let create = text_button("list-add-symbolic", "New Playlist");
        let shell = Rc::clone(self);
        create.connect_clicked(move |_| shell.new_playlist_dialog());
        header.append(&create);
        wrapper.append(&header);

        if page.items.is_empty() {
            wrapper.append(&self.route_empty_view(
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
        let summary = format!("{} {}", detail.genre.track_count, tr("tracks"));
        self.grouped_detail_view(GroupedDetailData {
            title: detail.genre.name,
            image_ref: detail.genre.image_ref,
            seed,
            summary,
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
        empty: bool,
        empty_body: &str,
        grid: gtk::Widget,
        load_next: Option<Rc<dyn Fn()>>,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        if empty {
            wrapper.append(&self.route_empty_view(empty_body));
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

    fn search_view(self: &Rc<Self>, _query: &str, library: LibrarySnapshot) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        let has_albums = !library.search.albums.is_empty();
        let has_tracks = !library.search.tracks.is_empty();
        let has_artists = !library.search.artists.is_empty();
        let has_playlists = !library.search.playlists.is_empty();
        let albums = library.search.albums;
        if !albums.is_empty() {
            let section = HomeSection {
                kind: rufin_core::HomeSectionKind::Explore,
                albums,
                tracks: Vec::new(),
            };
            wrapper.append(&self.home_album_section(&section));
        }

        if has_tracks {
            wrapper.append(&self.tracks_table(library.search.tracks, "search"));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.route_empty_view("No cached results found."));
        }

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

    fn render_lyrics_panel(self: &Rc<Self>) {
        let empty_status = self.lyrics_empty_status();
        let seek_shell = Rc::clone(self);
        let seek: Rc<dyn Fn(u64)> = Rc::new(move |position_millis| {
            seek_shell.seek_to_lyrics_position(position_millis);
        });
        let lyrics = self.state.lyrics.borrow();
        self.lyrics_pane
            .set_content(lyrics.as_ref(), empty_status, seek);
        drop(lyrics);
        self.update_lyrics_highlight();
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

    fn route_empty_view(&self, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
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
    let normal_back_shell = Rc::clone(shell);
    shell
        .normal_back_button
        .connect_clicked(move |_| normal_back_shell.go_back());

    let compact_back_shell = Rc::clone(shell);
    shell
        .compact_back_button
        .connect_clicked(move |_| compact_back_shell.go_back());

    let normal_forward_shell = Rc::clone(shell);
    shell
        .normal_forward_button
        .connect_clicked(move |_| normal_forward_shell.go_forward());

    let compact_forward_shell = Rc::clone(shell);
    shell
        .compact_forward_button
        .connect_clicked(move |_| compact_forward_shell.go_forward());

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
            if split_shell.state.density_mode.get() == DensityMode::Auto {
                split_shell.update_density();
            } else {
                split_shell.update_content_split();
                split_shell.queue_responsive_route_render();
            }
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
    preferences.connect_activate(move |_, _| present_preferences_dialog(&preferences_shell));
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
                ControllerEvent::FavoriteChanged {
                    item_id,
                    favorite,
                    snapshot,
                } => {
                    shell.apply_favorite_changed(item_id, favorite, *snapshot);
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
                        shell.lyrics_pane.clear_follow_scroll_pause();
                        shell.cancel_scheduled_lyrics_highlight();
                        shell.render_queue_panel();
                        shell.render_lyrics_panel();
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
                    shell.render_lyrics_panel();
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
    routes.extend([Route::Tracks, Route::Albums, Route::Tracks, Route::Albums]);
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

fn route_displays_sync_status(_route: &Route, first_run: bool) -> bool {
    first_run
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
        shell.register_favorite_button(track_favorite_key(&track.id), &button);
        let controller = shell.controller.clone();
        let track_id = track.id.clone();
        button.connect_clicked(move |button| {
            controller.set_track_favorite(track_id.clone(), !favorite_button_is_active(button));
        });
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

fn render_home_track_page(
    shell: &Rc<Shell>,
    row: &gtk::Box,
    previous: &gtk::Button,
    next: &gtk::Button,
    section_kind: HomeSectionKind,
    tracks: &[Track],
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    if tracks.is_empty() {
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
        state.page_start = clamp_home_album_page_start(state.page_start, page_size, tracks.len());
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
    let page_end = page_start.saturating_add(page_size).min(tracks.len());

    previous.set_sensitive(page_start > 0);
    next.set_sensitive(page_end < tracks.len());

    for track in &tracks[page_start..page_end] {
        row.append(&track_card_widget_with_size(shell, track, card_size));
    }
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
    shell.register_favorite_button(album_favorite_key(&album.id), &favorite);
    if let Some(controller) = controller {
        let controller = controller.clone();
        let album_id = album.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_album_favorite(album_id.clone(), !favorite_button_is_active(button));
        });
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

fn track_card_widget_with_size(shell: &Rc<Shell>, track: &Track, size: i32) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, HOME_ALBUM_CARD_LABEL_GAP);
    card.add_css_class("album-card");
    card.set_width_request(size);
    card.set_size_request(size, home_album_card_height(size));
    card.set_hexpand(false);
    card.set_halign(gtk::Align::Start);

    let play = gtk::Button::new();
    play.add_css_class("album-cover-button");
    play.add_css_class("flat");
    play.set_width_request(size);
    play.set_height_request(size);
    play.set_size_request(size, size);
    play.set_hexpand(false);
    play.set_halign(gtk::Align::Start);
    play.set_child(Some(&shell.cover_tile_for(
        track.image_ref.as_ref(),
        stable_seed(track.id.as_str()),
        size,
        GRID_COVER_SIZE,
    )));
    let controller = shell.controller.clone();
    let track_for_play = track.clone();
    play.connect_clicked(move |_| controller.play_now(track_for_play.clone()));
    card.append(&play);

    let title = gtk::Label::new(Some(&track.title));
    title.add_css_class("album-title");
    title.set_xalign(0.0);
    constrain_wrapped_card_label(&title, size, HOME_ALBUM_TITLE_LINES);
    let title_clip = clipped_card_label_with_lines(&title, size, HOME_ALBUM_TITLE_LINES);
    add_link_hover(&title_clip, &title, &track.title);

    let artist = gtk::Label::new(Some(&track.artist));
    artist.add_css_class("muted");
    artist.set_xalign(0.0);
    constrain_single_line_card_label(&artist, size);
    let artist_clip = clipped_card_label_with_lines(&artist, size, 1);

    let album = gtk::Label::new(Some(&track.album));
    album.add_css_class("muted");
    album.set_xalign(0.0);
    constrain_single_line_card_label(&album, size);
    let album_clip = clipped_card_label_with_lines(&album, size, 1);

    card.append(&title_clip);
    card.append(&artist_clip);
    card.append(&album_clip);
    card.upcast()
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
    let counts = gtk::Label::new(Some(&format!("{} {}", genre.track_count, tr("tracks"))));
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

fn seekbar_target_seconds(value: f64, duration_seconds: u32) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, f64::from(duration_seconds)) as u32
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

fn favorite_button_is_active(button: &gtk::Button) -> bool {
    button.label().as_deref() == Some(FAVORITE_FILLED_GLYPH)
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
    use super::right_panel::{
        clamp_queue_lyrics_position, queue_lyrics_default_position, queue_lyrics_initial_position,
        queue_lyrics_position_from_ratio, queue_lyrics_position_ratio,
    };
    use super::{current_playback_track_id, seekbar_target_seconds};
    use rufin_core::{QueueEntry, QueueEntryId, TrackId};

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
    fn seekbar_target_seconds_uses_committed_clamped_value() {
        assert_eq!(seekbar_target_seconds(42.4, 180), 42);
        assert_eq!(seekbar_target_seconds(42.5, 180), 43);
        assert_eq!(seekbar_target_seconds(-10.0, 180), 0);
        assert_eq!(seekbar_target_seconds(220.0, 180), 180);
        assert_eq!(seekbar_target_seconds(f64::NAN, 180), 0);
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
