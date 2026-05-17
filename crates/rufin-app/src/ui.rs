use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

mod artist;
mod cards;
mod chrome;
mod discord;
mod favorites;
mod home;
mod layout;
mod library;
mod login;
mod mpris;
mod navigation;
mod paging;
mod player;
mod player_icons;
mod preferences;
mod queue;
mod random_play;
mod right_panel;
mod settings_persistence;

use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
use mpris_server::Player as MprisPlayer;
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, DensityMode, EffectiveDensity, Genre, HomeSection,
    HomeSectionKind, ImageRef, LibraryListKey, Playlist, PlaylistId, QueueSnapshot, Route,
    RouteStack, SearchKind, Track, TrackSortKey, TrackTableColumn, TrackTableSettings,
    format_duration,
};
use rufin_playback::PlaybackState;
use rufin_provider::{FavoriteItemId, Lyrics, LyricsSource};
use rufin_store::{CachedGenreDetail, image_cache_key};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{
    AppController, ControllerEvent, DiscoveredServer, LibrarySnapshot, LyricsSearchResult,
    PlaybackSnapshot,
};
use crate::external_metadata;
use crate::i18n::tr;
use crate::lyrics::{LyricsPane, next_lyrics_line_start_after};
use chrome::{build_content_chrome, build_main_area};
use discord::DiscordPresence;
use favorites::{
    FavoriteControlKey, FavoriteControls, album_favorite_key, artist_favorite_key,
    clear_favorite_controls, favorite_change_needs_route_render, favorite_control_key,
    merge_favorite_snapshot, register_favorite_control, track_favorite_key,
    update_favorite_controls,
};
use layout::{
    COMPACT_RAIL_WIDTH, HOME_ALBUM_GAP, NORMAL_SIDEBAR_WIDTH, PRIMARY_ROUTE_MARGIN_END,
    PRIMARY_ROUTE_MARGIN_START, content_split_initial_position_for_density,
    content_split_target_position_for_density, restored_window_size, right_panel_saved_ratio,
};
use mpris::install_mpris;
use navigation::{
    ServerSelector, build_compact_navigation, build_normal_navigation, build_server_selector,
    sidebar_history_button,
};
use paging::{PagedGridConfig, PagedGridCursor, connect_paged_grid_loader, finish_grid_page};
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
const STARTUP_HOME_REFRESH_DELAY_MS: u64 = 750;

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
    lyrics_auto_search_attempted: RefCell<HashSet<rufin_core::TrackId>>,
    lyrics_search_dialog: RefCell<Option<LyricsSearchDialog>>,
    lyrics_timing_generation: Cell<u64>,
    lyrics_timing_source: RefCell<Option<glib::SourceId>>,
    mpris_player: RefCell<Option<Rc<MprisPlayer>>>,
    discord_presence: RefCell<DiscordPresence>,
    updating_player_controls: Cell<bool>,
    seeking_player_controls: Cell<bool>,
    seek_generation: Cell<u64>,
    queue_filter: RefCell<String>,
    right_panel_visible: Cell<bool>,
    lyrics_panel_visible: Cell<bool>,
    split_width: Cell<i32>,
    normal_split_position: Cell<i32>,
    compact_split_position: Cell<i32>,
    split_density: Cell<EffectiveDensity>,
    queue_lyrics_position_save_suppressed: Rc<Cell<u32>>,
    responsive_render_queued: Cell<bool>,
    card_grid_columns: Cell<usize>,
    home_section_state: RefCell<HashMap<HomeSectionKind, HomeSectionState>>,
    prefetched_explore: RefCell<Option<PrefetchedHomeSection>>,
    discovered_servers: RefCell<Vec<DiscoveredServer>>,
    server_discovery_status: RefCell<String>,
    server_discovery_running: Cell<bool>,
    server_discovery_started: Cell<bool>,
    cover_bindings: RefCell<HashMap<String, Vec<CoverBinding>>>,
    cover_decodes: RefCell<HashSet<String>>,
    decoded_covers: RefCell<HashMap<String, Pixbuf>>,
    decoded_cover_order: RefCell<VecDeque<String>>,
    favorite_controls: FavoriteControls,
    perf: Option<Rc<UiPerfMonitor>>,
}

#[derive(Clone)]
struct LyricsSearchDialog {
    dialog: adw::Dialog,
    track_id: rufin_core::TrackId,
    artist_entry: gtk::Entry,
    title_entry: gtk::Entry,
    search_button: gtk::Button,
    list: gtk::ListBox,
    status: gtk::Label,
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

#[derive(Clone)]
struct PrefetchedHomeSection {
    server_id: rufin_core::ServerId,
    section: HomeSection,
}

#[derive(Clone, Copy)]
struct TrackTableOptions {
    paging: Option<(usize, usize)>,
    expand: bool,
    max_visible_rows: Option<usize>,
    favorite_first: bool,
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
    route_title: adw::WindowTitle,
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
        cached_albums = library.cached_album_count,
        cached_tracks = library.cached_track_count,
        preloaded_albums = library.albums.len(),
        preloaded_tracks = library.tracks.len(),
        first_run = library.first_run,
        elapsed_ms = loaded_at.elapsed().as_millis(),
        "loaded cached music library snapshot"
    );
    let perf_requires_assets =
        options.ui_perf_run && options.fake_scale.is_none() && library_has_image_refs(&library);
    let prefetched_explore = prefetched_explore_from_snapshot(&library);

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
        lyrics_auto_search_attempted: RefCell::new(HashSet::new()),
        lyrics_search_dialog: RefCell::new(None),
        lyrics_timing_generation: Cell::new(0),
        lyrics_timing_source: RefCell::new(None),
        mpris_player: RefCell::new(None),
        discord_presence: RefCell::new(DiscordPresence::new()),
        updating_player_controls: Cell::new(false),
        seeking_player_controls: Cell::new(false),
        seek_generation: Cell::new(0),
        queue_filter: RefCell::new(String::new()),
        right_panel_visible: Cell::new(settings.right_panel_visible),
        lyrics_panel_visible: Cell::new(settings.lyrics_panel_visible),
        split_width: Cell::new(0),
        normal_split_position: Cell::new(0),
        compact_split_position: Cell::new(0),
        split_density: Cell::new(EffectiveDensity::Compact),
        queue_lyrics_position_save_suppressed: Rc::new(Cell::new(0)),
        responsive_render_queued: Cell::new(false),
        card_grid_columns: Cell::new(0),
        home_section_state: RefCell::new(HashMap::new()),
        prefetched_explore: RefCell::new(prefetched_explore),
        discovered_servers: RefCell::new(Vec::new()),
        server_discovery_status: RefCell::new("Searching will start automatically.".to_string()),
        server_discovery_running: Cell::new(false),
        server_discovery_started: Cell::new(false),
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

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 3);
    compact_nav.add_css_class("compact-rail");
    compact_nav.set_width_request(COMPACT_RAIL_WIDTH);
    let server_selector = build_server_selector();

    let normal_back_button = sidebar_history_button("go-previous-symbolic", "Back");
    let normal_forward_button = sidebar_history_button("go-next-symbolic", "Forward");
    let compact_back_button = sidebar_history_button("go-previous-symbolic", "Back");
    let compact_forward_button = sidebar_history_button("go-next-symbolic", "Forward");
    let main_area_parts = build_main_area();
    let main_area = main_area_parts.root;
    let route_title = main_area_parts.route_title;
    let route_host = main_area_parts.route_host;

    let right_panel_parts = build_right_panel();
    let right_panel = right_panel_parts.root;
    let queue_panel = right_panel_parts.queue_panel;
    let queue_search = right_panel_parts.queue_search;
    let queue_clear_button = right_panel_parts.queue_clear_button;
    let queue_lyrics_split = right_panel_parts.queue_lyrics_split;
    let lyrics_pane = right_panel_parts.lyrics_pane;

    let content_chrome = build_content_chrome(&main_area, &right_panel);
    let content_split = content_chrome.content_split;
    let main_menu = content_chrome.main_menu;
    let player_controls = build_bottom_player();

    upper.append(&normal_nav);
    upper.append(&compact_nav);
    upper.append(&content_chrome.root);

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
    connect_lyrics_search_controls(&shell);
    connect_player_controls(&shell);
    install_mpris(&shell);
    shell.update_density();
    prime_first_cached_cover(&shell);
    shell
        .controller
        .prefetch_external_metadata_covers(&shell.state.library.borrow());
    shell.render_current_route();
    shell.render_queue_panel();
    shell.render_lyrics_panel();
    shell.update_bottom_player();
    shell.update_discord_presence(&shell.state.player.borrow());
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
        schedule_startup_home_refresh(&shell);
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
    let density_shell = Rc::clone(&shell);
    glib::idle_add_local_once(move || {
        if density_shell.state.density_mode.get() == DensityMode::Auto {
            density_shell.update_density();
        }
    });
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

impl Shell {
    fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        let refresh_home = matches!(route, Route::Home);
        self.refresh_search_results_for_route(&route);
        self.state.routes.borrow_mut().navigate(route);
        self.render_current_route();
        if refresh_home {
            self.refresh_home_after_route_display();
        }
    }

    fn go_back(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.refresh_search_results_for_route(&route);
            self.render_current_route();
            self.refresh_home_sections_for_route(&route);
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.refresh_search_results_for_route(&route);
            self.render_current_route();
            self.refresh_home_sections_for_route(&route);
        }
    }

    fn refresh_search_results_for_route(&self, route: &Route) {
        if let Route::Search { query, .. } = route {
            self.controller.search(query.clone());
        }
    }

    fn refresh_home_sections_for_route(&self, route: &Route) {
        if matches!(route, Route::Home) {
            self.refresh_home_after_route_display();
        }
    }

    fn refresh_home_after_route_display(&self) {
        self.controller
            .refresh_home_sections_without_explore_for_active();
        self.controller.prefetch_explore_for_active();
    }

    fn refresh_home_section(self: &Rc<Self>, section_kind: HomeSectionKind) {
        if let Some(state) = self
            .state
            .home_section_state
            .borrow_mut()
            .get_mut(&section_kind)
        {
            state.page_start = 0;
        }

        if section_kind == HomeSectionKind::Explore && self.apply_prefetched_explore() {
            return;
        }

        self.controller
            .refresh_home_section_for_active(section_kind);
        if section_kind == HomeSectionKind::Explore {
            self.controller.prefetch_explore_for_active();
        }
    }

    fn apply_prefetched_explore(self: &Rc<Self>) -> bool {
        let Some(server_id) = self
            .state
            .library
            .borrow()
            .server
            .as_ref()
            .map(|server| server.id.clone())
        else {
            return false;
        };
        let Some(prefetched) = self.state.prefetched_explore.borrow_mut().take() else {
            return false;
        };
        if prefetched.server_id != server_id {
            return false;
        }

        {
            let mut library = self.state.library.borrow_mut();
            upsert_snapshot_home_section(&mut library.home_sections, prefetched.section.clone());
        }
        self.controller
            .promote_prefetched_explore_for_active(prefetched.section);
        self.controller.prefetch_explore_for_active();
        self.render_current_route_preserving_scroll();
        true
    }

    fn update_prefetched_explore_from_snapshot(
        &self,
        server_id: Option<rufin_core::ServerId>,
        prefetched: Option<PrefetchedHomeSection>,
    ) {
        if prefetched.is_some() {
            *self.state.prefetched_explore.borrow_mut() = prefetched;
            return;
        }

        let keep_current = {
            let current = self.state.prefetched_explore.borrow();
            current.as_ref().is_some_and(|current| {
                server_id
                    .as_ref()
                    .is_some_and(|server_id| &current.server_id == server_id)
            })
        };
        if !keep_current {
            *self.state.prefetched_explore.borrow_mut() = None;
        }
    }

    fn update_density(self: &Rc<Self>) {
        let width = self.density_width().max(1);
        self.update_density_for_width(width);
    }

    fn update_density_for_width(self: &Rc<Self>, width: i32) {
        let width = width.max(1);
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

    fn density_width(&self) -> i32 {
        self.window
            .surface()
            .map(|surface| surface.width())
            .filter(|width| *width > 1)
            .unwrap_or_else(|| self.window.width())
    }

    fn update_server_selector(self: &Rc<Self>) {
        navigation::update_server_selector(self);
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
        let density = self.right_panel_density();
        let previous_density = self.state.split_density.replace(density);
        let density_changed = previous_density != density;
        let current_position = self.content_split.position().clamp(0, split_width);
        if density_changed
            && self.state.right_panel_visible.get()
            && current_position > 1
            && current_position < split_width
        {
            self.set_right_panel_split_position_for(
                previous_density,
                layout::clamp_content_split_position_for_density(
                    split_width,
                    current_position,
                    previous_density,
                ),
            );
        }
        let stored_position = self.right_panel_split_position_for(density);
        let saved_ratio = right_panel_saved_ratio(&self.state.settings.borrow(), density);
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

        let position = if density_changed {
            if stored_position > 1 {
                layout::clamp_content_split_position_for_density(
                    split_width,
                    stored_position,
                    density,
                )
            } else {
                content_split_initial_position_for_density(split_width, saved_ratio, density)
            }
        } else {
            content_split_target_position_for_density(
                split_width,
                previous_width,
                stored_position,
                current_position,
                saved_ratio,
                density,
            )
        };
        let position_changed = self.right_panel_split_position_for(density) != position;
        self.set_right_panel_split_position_for(density, position);
        if position_changed && !width_changed && !density_changed && current_position == position {
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
        self.request_auto_lyrics_if_needed();
    }

    fn request_auto_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        if self.state.lyrics.borrow().is_some() {
            return;
        }
        let settings = self.state.settings.borrow();
        let request = auto_lyrics_request_for_settings(&settings, &track_id);
        drop(settings);
        let Some(request) = request else {
            return;
        };
        if !self
            .state
            .lyrics_auto_search_attempted
            .borrow_mut()
            .insert(track_id)
        {
            return;
        }
        match request {
            AutoLyricsRequest::Default => self.controller.request_lyrics_for_current(),
            AutoLyricsRequest::ServerOnly => self.controller.request_server_lyrics_for_current(),
        }
    }

    fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        {
            let mut attempted = self.state.lyrics_auto_search_attempted.borrow_mut();
            attempted.remove(&track_id);
        }
        {
            let mut settings = self.state.settings.borrow_mut();
            let id = track_id.as_str().to_string();
            if !settings.suppressed_auto_lyrics_track_ids.contains(&id) {
                settings.suppressed_auto_lyrics_track_ids.push(id);
                if let Err(error) = self.controller.save_settings(&settings) {
                    warn!(%error, "failed to save lyrics auto-search setting");
                }
            }
        }
        self.render_lyrics_panel();
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
            self.route_title.set_title(&tr("Connect to Music Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.route_host.append(&view);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );

        let view = match route {
            Route::Home => self.home_view(),
            Route::Albums => self.library_albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.library_tracks_route_view(),
            Route::Favorites => self.favorites_view(),
            Route::Artists => self.library_artist_list_view(false),
            Route::ArtistDetail(artist_id) => self.artist_detail_view(artist_id),
            Route::ArtistDiscography(artist_id) => self.artist_discography_view(artist_id),
            Route::ArtistTracks(artist_id) => self.artist_tracks_view(artist_id),
            Route::AlbumArtists => self.library_artist_list_view(true),
            Route::Genres => self.library_genre_list_view(),
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

    fn render_current_route_preserving_scroll(self: &Rc<Self>) {
        let scroll_value = self.current_route_scroll_value();
        self.render_current_route();
        if let Some(value) = scroll_value {
            self.restore_current_route_scroll(value);
        }
    }

    fn current_route_scroll_value(&self) -> Option<f64> {
        find_largest_scrolled_window(&self.route_host.clone().upcast())
            .map(|scroller| scroller.vadjustment().value())
    }

    fn restore_current_route_scroll(&self, value: f64) {
        let route_host = self.route_host.clone();
        glib::idle_add_local_once(move || {
            restore_scrolled_window_value(&route_host.clone().upcast(), value);
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                restore_scrolled_window_value(&route_host.clone().upcast(), value);
            });
        });
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

        let table =
            self.library_tracks_panel(tracks, LibraryListKey::AlbumDetailTracks, "album-detail");
        content.append(&table);

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn compact_artist_tracks_table(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
    ) -> gtk::Widget {
        self.tracks_table_with_options(
            tracks,
            context,
            TrackTableOptions {
                paging: None,
                expand: false,
                max_visible_rows: Some(5),
                favorite_first: true,
            },
        )
    }

    fn tracks_table_with_options(
        self: &Rc<Self>,
        tracks: Vec<Track>,
        context: &str,
        options: TrackTableOptions,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
        wrapper.set_vexpand(options.expand);
        let tracks = Rc::new(RefCell::new(tracks));
        let page_cursor = options.paging.map(|(offset, total)| {
            Rc::new(PagedGridCursor {
                offset: Cell::new(offset),
                total: Cell::new(total),
                loading: Cell::new(false),
            })
        });
        let server_search = page_cursor.is_some();
        let paged_query = Rc::new(RefCell::new(String::new()));

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
        populate_track_model_with_options(
            &model,
            &tracks.borrow(),
            &settings,
            "",
            options.favorite_first,
        );
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let table = gtk::ColumnView::new(Some(selection));
        table.add_css_class("track-table");
        table.set_vexpand(options.expand);
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
        let page_cursor_for_search = page_cursor.clone();
        let paged_query_for_search = Rc::clone(&paged_query);
        search.connect_search_changed(move |entry| {
            let settings = shell.state.settings.borrow().track_table.clone();
            if let Some(cursor) = page_cursor_for_search.as_ref() {
                let query = entry.text().trim().to_string();
                *paged_query_for_search.borrow_mut() = query.clone();
                cursor.offset.set(0);
                cursor.total.set(usize::MAX);
                cursor.loading.set(true);
                match shell
                    .controller
                    .cached_tracks_page_matching(&query, 0, TRACK_ROUTE_PAGE_SIZE)
                {
                    Ok(page) => {
                        let count = page.items.len();
                        *tracks_for_search.borrow_mut() = page.items;
                        let tracks = tracks_for_search.borrow();
                        populate_track_model_with_options(
                            &model_for_search,
                            &tracks,
                            &settings,
                            "",
                            options.favorite_first,
                        );
                        finish_grid_page(cursor, 0, count, page.total);
                    }
                    Err(error) => {
                        warn!(%error, "failed to search cached tracks page");
                        cursor.loading.set(false);
                    }
                }
            } else {
                let tracks = tracks_for_search.borrow();
                populate_track_model_with_options(
                    &model_for_search,
                    &tracks,
                    &settings,
                    entry.text().as_str(),
                    options.favorite_first,
                );
            }
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
            let search_text = search_for_sort.text();
            let query = if server_search {
                ""
            } else {
                search_text.as_str()
            };
            populate_track_model_with_options(
                &model_for_sort,
                &tracks,
                &settings,
                query,
                options.favorite_first,
            );
            set_track_sort_button_content(button, &settings);
        });

        configure.set_popover(Some(&self.track_table_popover(
            &table,
            &model,
            Rc::clone(&tracks),
            &search,
            &sort_button,
            options.favorite_first,
            server_search,
        )));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(options.expand);
        if let Some(max_visible_rows) = options.max_visible_rows {
            let visible_rows = tracks.borrow().len().min(max_visible_rows).max(1);
            let height = 92 + visible_rows as i32 * 58;
            scroller.set_min_content_height(height);
            scroller.set_max_content_height(height);
        }
        scroller.set_child(Some(&table));
        if let Some(cursor) = page_cursor {
            let shell = Rc::clone(self);
            let tracks_for_page = Rc::clone(&tracks);
            let model_for_page = model.clone();
            let paged_query_for_page = Rc::clone(&paged_query);
            let load_next = Rc::new(move || {
                if !shell.can_load_grid_page(&cursor, &Route::Tracks) {
                    return;
                }
                let offset = cursor.offset.get();
                let query = paged_query_for_page.borrow().clone();
                match shell.controller.cached_tracks_page_matching(
                    &query,
                    offset,
                    TRACK_ROUTE_PAGE_SIZE,
                ) {
                    Ok(page) => {
                        let count = page.items.len();
                        let mut items = page.items;
                        tracks_for_page.borrow_mut().extend(items.iter().cloned());
                        let settings = shell.state.settings.borrow().track_table.clone();
                        sort_tracks_with_options(&mut items, &settings, options.favorite_first);
                        append_tracks_to_model(&model_for_page, items);
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
        let playlists = Rc::new(RefCell::new(page.items));
        let model = playlist_model(&playlists.borrow());
        let (search, load_next) = self.searchable_grid_controls(
            model.clone(),
            Rc::clone(&playlists),
            PagedGridConfig {
                route: Route::Playlists,
                offset: playlists.borrow().len(),
                total: page.total,
                page_name: "playlists",
            },
            |controller, query, offset, limit| {
                controller.cached_playlists_page_matching(query, offset, limit)
            },
            replace_playlists_in_model,
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
        header.append(&search);
        let create = text_button("list-add-symbolic", "New Playlist");
        let shell = Rc::clone(self);
        create.connect_clicked(move |_| shell.new_playlist_dialog());
        header.append(&create);
        wrapper.append(&header);

        if playlists.borrow().is_empty() {
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
            let key = if data.table_context == "genre-detail" {
                LibraryListKey::GenreTracks
            } else {
                LibraryListKey::Tracks
            };
            wrapper.append(&self.library_tracks_panel(data.tracks, key, data.table_context));
        }
        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }

    fn search_view(self: &Rc<Self>, _query: &str, library: LibrarySnapshot) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

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
            wrapper.append(&self.library_tracks_panel(
                library.search.tracks,
                LibraryListKey::Tracks,
                "search",
            ));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.route_empty_view("No cached results found."));
        }

        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }

    fn confirm_clear_cache(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Clear Cached Library"))
            .body(tr(
                "This removes cached library metadata for the active server. Login stays saved.",
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
        let settings = self.state.settings.borrow();
        let current_track_id = current_playback_track_id(&self.state.player.borrow());
        let has_current_track = current_track_id.is_some();
        let (search_label, search_enabled) = if settings.private_mode {
            (tr("Private mode is on"), false)
        } else if has_current_track {
            (tr("Search lyrics"), true)
        } else {
            (tr("No track playing"), false)
        };
        let lyrics = self.state.lyrics.borrow();
        let clear_auto_search_enabled =
            auto_lyrics_skip_action_enabled(&settings, current_track_id.as_ref(), lyrics.as_ref());
        drop(settings);
        self.lyrics_pane
            .set_search_action(&search_label, search_enabled);
        self.lyrics_pane.set_clear_auto_search_action(
            &tr("Disable automatic lyric search for this track"),
            clear_auto_search_enabled,
        );
        let empty_status = self.lyrics_empty_status();
        let seek_shell = Rc::clone(self);
        let seek: Rc<dyn Fn(u64)> = Rc::new(move |position_millis| {
            seek_shell.seek_to_lyrics_position(position_millis);
        });
        self.lyrics_pane
            .set_content(lyrics.as_ref(), empty_status, seek);
        drop(lyrics);
        self.update_lyrics_highlight();
        self.request_auto_lyrics_if_needed();
    }

    fn present_lyrics_search_dialog(self: &Rc<Self>) {
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref() {
            dialog.dialog.present(Some(&self.window));
            dialog.title_entry.grab_focus();
            return;
        }

        let Some(current) = self.state.player.borrow().current.clone() else {
            return;
        };
        if self.state.settings.borrow().private_mode {
            return;
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_width_request(420);
        content.set_height_request(500);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&tr("Search Lyrics")));
        title.add_css_class("title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let close_button = icon_button("window-close-symbolic", "Close");
        header.append(&close_button);
        content.append(&header);

        let artist_entry = gtk::Entry::new();
        artist_entry.set_placeholder_text(Some(&tr("Artist")));
        artist_entry.set_text(&current.artist);
        artist_entry.set_hexpand(true);
        content.append(&artist_entry);

        let title_entry = gtk::Entry::new();
        title_entry.set_placeholder_text(Some(&tr("Song")));
        title_entry.set_text(&current.title);
        title_entry.set_hexpand(true);
        content.append(&title_entry);

        let search_button = text_button("system-search-symbolic", "Search");
        search_button.set_halign(gtk::Align::End);
        content.append(&search_button);

        let status = gtk::Label::new(Some(&tr("Ready")));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        status.set_wrap(true);
        content.append(&status);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        content.append(&scroller);

        let dialog = adw::Dialog::builder()
            .content_width(520)
            .content_height(560)
            .child(&content)
            .build();
        let search_dialog = LyricsSearchDialog {
            dialog: dialog.clone(),
            track_id: current.track_id,
            artist_entry: artist_entry.clone(),
            title_entry: title_entry.clone(),
            search_button: search_button.clone(),
            list,
            status,
        };
        *self.state.lyrics_search_dialog.borrow_mut() = Some(search_dialog.clone());

        let close_shell = Rc::clone(self);
        dialog.connect_closed(move |_| {
            close_shell.state.lyrics_search_dialog.borrow_mut().take();
        });

        let close_dialog = dialog.clone();
        close_button.connect_clicked(move |_| {
            close_dialog.close();
        });

        let search_shell = Rc::clone(self);
        search_button.connect_clicked(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        artist_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        title_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        dialog.present(Some(&self.window));
        search_dialog.title_entry.grab_focus();
        submit_lyrics_search(self);
    }

    fn apply_lyrics_search_results(
        self: &Rc<Self>,
        track_id: rufin_core::TrackId,
        _artist_name: String,
        _track_name: String,
        results: Vec<LyricsSearchResult>,
    ) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if dialog.track_id != track_id {
            return;
        }
        dialog.search_button.set_sensitive(true);
        clear_list_box(&dialog.list);
        if results.is_empty() {
            dialog.status.set_text(&tr("No lyrics found."));
            return;
        }

        dialog
            .status
            .set_text(&format!("{} {}", results.len(), tr("results")));
        for result in results {
            let title = format!("{} - {}", result.artist_name, result.track_name);
            let subtitle = lyrics_result_subtitle(&result);
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            let button = gtk::Button::with_label(&tr("Save"));
            button.set_valign(gtk::Align::Center);
            button.add_css_class("suggested-action");
            button.set_sensitive(lyrics_search_result_has_content(&result));
            row.add_suffix(&button);
            row.set_activatable_widget(Some(&button));

            let save_shell = Rc::clone(self);
            let save_track_id = track_id.clone();
            button.connect_clicked(move |_| {
                if save_shell.state.settings.borrow().ask_lyrics_save_path {
                    let shell = Rc::clone(&save_shell);
                    let track_id = save_track_id.clone();
                    let result = result.clone();
                    gtk::glib::spawn_future_local(async move {
                        let dialog = gtk::FileDialog::builder().title(tr("Save Lyrics")).build();
                        let Ok(file) = dialog.save_future(Some(&shell.window)).await else {
                            return;
                        };
                        let Some(path) = file.path() else {
                            return;
                        };
                        shell
                            .controller
                            .save_lyrics_search_result(track_id, result, Some(path));
                    });
                } else {
                    save_shell.controller.save_lyrics_search_result(
                        save_track_id.clone(),
                        result.clone(),
                        None,
                    );
                }
            });
            dialog.list.append(&row);
        }
    }

    fn apply_lyrics_saved(self: &Rc<Self>, path: PathBuf, lyrics: Lyrics) {
        let track_id = lyrics.track_id.clone();
        *self.state.lyrics.borrow_mut() = Some(lyrics);
        self.render_lyrics_panel();
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.track_id == track_id
        {
            dialog
                .status
                .set_text(&format!("{} {}", tr("Saved to"), path.display()));
        }
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
        if external_metadata::is_external_image_ref(image_ref)
            && !external_metadata::enabled(&self.state.settings.borrow())
        {
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

    #[allow(clippy::too_many_arguments)]
    fn track_table_popover(
        self: &Rc<Self>,
        table: &gtk::ColumnView,
        model: &gio::ListStore,
        tracks: Rc<RefCell<Vec<Track>>>,
        search: &gtk::SearchEntry,
        sort_button: &gtk::Button,
        favorite_first: bool,
        server_search: bool,
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
            let search_text = search_for_sort.text();
            let query = if server_search {
                ""
            } else {
                search_text.as_str()
            };
            populate_track_model_with_options(
                &model_for_sort,
                &tracks,
                &settings,
                query,
                favorite_first,
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
    connect_auto_density_resize(shell);

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

fn connect_lyrics_search_controls(shell: &Rc<Shell>) {
    let lyrics_shell = Rc::clone(shell);
    shell.lyrics_pane.connect_search_clicked(move || {
        if current_playback_track_id(&lyrics_shell.state.player.borrow()).is_none() {
            return;
        }
        lyrics_shell.present_lyrics_search_dialog();
    });
    let lyrics_shell = Rc::clone(shell);
    shell
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || lyrics_shell.suppress_auto_lyrics_for_current());
}

fn submit_lyrics_search(shell: &Rc<Shell>) {
    let Some(dialog) = shell.state.lyrics_search_dialog.borrow().clone() else {
        return;
    };
    let artist_name = dialog.artist_entry.text().trim().to_string();
    let track_name = dialog.title_entry.text().trim().to_string();
    if artist_name.is_empty() && track_name.is_empty() {
        dialog.status.set_text(&tr("Enter an artist or song."));
        return;
    }
    clear_list_box(&dialog.list);
    dialog.search_button.set_sensitive(false);
    dialog.status.set_text(&tr("Searching..."));
    shell
        .controller
        .search_lyrics_for_current(artist_name, track_name);
}

fn auto_lyrics_search_is_suppressed(
    settings: &AppSettings,
    track_id: &rufin_core::TrackId,
) -> bool {
    settings
        .suppressed_auto_lyrics_track_ids
        .iter()
        .any(|stored| stored == track_id.as_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoLyricsRequest {
    Default,
    ServerOnly,
}

fn auto_lyrics_request_for_settings(
    settings: &AppSettings,
    track_id: &rufin_core::TrackId,
) -> Option<AutoLyricsRequest> {
    if !settings.lyrics_panel_visible {
        return None;
    }
    if settings.private_mode
        || !settings.external_lyrics_enabled
        || auto_lyrics_search_is_suppressed(settings, track_id)
    {
        Some(AutoLyricsRequest::ServerOnly)
    } else {
        Some(AutoLyricsRequest::Default)
    }
}

fn auto_lyrics_skip_action_enabled(
    settings: &AppSettings,
    track_id: Option<&rufin_core::TrackId>,
    lyrics: Option<&Lyrics>,
) -> bool {
    let Some(track_id) = track_id else {
        return false;
    };
    if lyrics.is_some_and(|lyrics| lyrics.source == LyricsSource::Server) {
        return false;
    }
    !settings.private_mode
        && settings.external_lyrics_enabled
        && !auto_lyrics_search_is_suppressed(settings, track_id)
}

fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn lyrics_search_result_has_content(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
        || result
            .plain_lyrics
            .as_deref()
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
}

fn lyrics_result_subtitle(result: &LyricsSearchResult) -> String {
    let mut subtitle = String::new();
    if !result.album_name.trim().is_empty() {
        subtitle.push_str(&result.album_name);
    }
    if result.duration_seconds > 0 {
        if !subtitle.is_empty() {
            subtitle.push_str(" - ");
        }
        subtitle.push_str(&format_duration(result.duration_seconds));
    }
    if !subtitle.is_empty() {
        subtitle.push_str(" - ");
    }
    if result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
    {
        if result
            .plain_lyrics
            .as_deref()
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
        {
            subtitle.push_str(&tr("Synchronized + Unsynchronized"));
        } else {
            subtitle.push_str(&tr("Synchronized"));
        }
    } else if result
        .plain_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
    {
        subtitle.push_str(&tr("Unsynchronized"));
    } else {
        subtitle.push_str(&tr("No lyrics"));
    }
    subtitle
}

fn connect_auto_density_resize(shell: &Rc<Shell>) {
    let window = shell.window.clone();
    let shell = Rc::clone(shell);
    window.connect_realize(move |window| {
        let Some(surface) = window.surface() else {
            return;
        };
        let resize_shell = Rc::clone(&shell);
        surface.connect_width_notify(move |surface| {
            if resize_shell.state.density_mode.get() == DensityMode::Auto {
                resize_shell.update_density_for_width(surface.width());
            }
        });
        if shell.state.density_mode.get() == DensityMode::Auto {
            shell.update_density_for_width(surface.width());
        }
    });
}

fn install_window_actions(shell: &Rc<Shell>) {
    let go_back = gio::SimpleAction::new("go-back", None);
    let go_back_shell = Rc::clone(shell);
    go_back.connect_activate(move |_, _| go_back_shell.go_back());
    shell.window.add_action(&go_back);

    let go_forward = gio::SimpleAction::new("go-forward", None);
    let go_forward_shell = Rc::clone(shell);
    go_forward.connect_activate(move |_, _| go_forward_shell.go_forward());
    shell.window.add_action(&go_forward);

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
        .set_accels_for_action("win.go-back", &["<Alt>Left"]);
    shell
        .application
        .set_accels_for_action("win.go-forward", &["<Alt>Right"]);
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
    section.add(adw::ShortcutsItem::from_action(&tr("Back"), "win.go-back"));
    section.add(adw::ShortcutsItem::from_action(
        &tr("Forward"),
        "win.go-forward",
    ));
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
        .comments(tr("Thank you for trying out Rufin."))
        .website("https://github.com/screwys/Rufin")
        .build();
    dialog.add_link(&tr("Issues"), "https://github.com/screwys/Rufin/issues");
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

fn schedule_startup_home_refresh(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(
        Duration::from_millis(STARTUP_HOME_REFRESH_DELAY_MS),
        move || {
            debug!("refreshing home sections after startup");
            shell.refresh_home_after_route_display();
        },
    );
}

fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        shell.controller.poll_playback_events();
        while let Ok(event) = receiver.try_recv() {
            match event {
                ControllerEvent::Snapshot(snapshot) => {
                    let entering_first_run =
                        snapshot.first_run && !shell.state.library.borrow().first_run;
                    let server_id = snapshot.server.as_ref().map(|server| server.id.clone());
                    let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
                    *shell.state.library.borrow_mut() = *snapshot;
                    shell
                        .controller
                        .prefetch_external_metadata_covers(&shell.state.library.borrow());
                    if entering_first_run {
                        shell.state.server_discovery_started.set(false);
                        shell.state.server_discovery_running.set(false);
                        *shell.state.discovered_servers.borrow_mut() = Vec::new();
                        *shell.state.server_discovery_status.borrow_mut() =
                            "Searching will start automatically.".to_string();
                    }
                    shell.update_prefetched_explore_from_snapshot(server_id, prefetched_explore);
                    shell.update_server_selector();
                    shell.render_current_route_preserving_scroll();
                }
                ControllerEvent::HomeSectionPrefetched { server_id, section } => {
                    let active_server_id = shell
                        .state
                        .library
                        .borrow()
                        .server
                        .as_ref()
                        .map(|server| server.id.clone());
                    if active_server_id.as_ref() == Some(&server_id) {
                        *shell.state.prefetched_explore.borrow_mut() =
                            Some(PrefetchedHomeSection { server_id, section });
                    }
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
                    let auto_dj_enabled = next_snapshot.auto_dj_enabled;
                    *shell.state.player.borrow_mut() = next_snapshot.clone();
                    shell.sync_auto_dj_setting_from_playback(auto_dj_enabled);
                    if previous_track != next_track {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                        shell.lyrics_pane.clear_follow_scroll_pause();
                        shell.cancel_scheduled_lyrics_highlight();
                        shell.render_queue_panel();
                        shell.render_lyrics_panel();
                        shell.request_auto_lyrics_if_needed();
                        shell.notify_now_playing(&next_snapshot);
                    }
                    shell.update_bottom_player();
                    if lyrics_timing_changed {
                        shell.update_lyrics_highlight();
                    }
                    shell.update_mpris_player();
                    shell.update_discord_presence(&next_snapshot);
                }
                ControllerEvent::Lyrics(lyrics) => {
                    *shell.state.lyrics.borrow_mut() = *lyrics;
                    shell.render_lyrics_panel();
                }
                ControllerEvent::LyricsSearchResults {
                    track_id,
                    artist_name,
                    track_name,
                    results,
                } => {
                    shell.apply_lyrics_search_results(track_id, artist_name, track_name, results);
                }
                ControllerEvent::LyricsSaved { path, lyrics } => {
                    shell.apply_lyrics_saved(path, lyrics);
                }
                ControllerEvent::CoverReady { key, path } => {
                    shell.apply_cover_ready(&key, &path);
                }
                ControllerEvent::ServerDiscovery {
                    servers,
                    status,
                    running,
                } => {
                    *shell.state.discovered_servers.borrow_mut() = servers;
                    *shell.state.server_discovery_status.borrow_mut() = status;
                    shell.state.server_discovery_running.set(running);
                    if shell.state.library.borrow().first_run {
                        shell.render_current_route();
                    }
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

fn restore_scrolled_window_value(widget: &gtk::Widget, value: f64) {
    let Some(scroller) = find_largest_scrolled_window(widget) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let max_value = (adjustment.upper() - adjustment.page_size()).max(0.0);
    adjustment.set_value(value.clamp(0.0, max_value));
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

fn replace_albums_in_model(model: &gio::ListStore, albums: impl IntoIterator<Item = Album>) {
    let additions = albums
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_albums_to_model(model: &gio::ListStore, albums: impl IntoIterator<Item = Album>) {
    append_boxed_items_to_model(model, albums);
}

fn replace_artists_in_model(model: &gio::ListStore, artists: impl IntoIterator<Item = Artist>) {
    let additions = artists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_artists_to_model(model: &gio::ListStore, artists: impl IntoIterator<Item = Artist>) {
    append_boxed_items_to_model(model, artists);
}

fn replace_genres_in_model(model: &gio::ListStore, genres: impl IntoIterator<Item = Genre>) {
    let additions = genres
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_genres_to_model(model: &gio::ListStore, genres: impl IntoIterator<Item = Genre>) {
    append_boxed_items_to_model(model, genres);
}

fn playlist_model(playlists: &[Playlist]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    append_playlists_to_model(&model, playlists.iter().cloned());
    model
}

fn replace_playlists_in_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    let additions = playlists
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_playlists_to_model(
    model: &gio::ListStore,
    playlists: impl IntoIterator<Item = Playlist>,
) {
    append_boxed_items_to_model(model, playlists);
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

fn populate_track_model_with_options(
    model: &gio::ListStore,
    tracks: &[Track],
    settings: &TrackTableSettings,
    query: &str,
    favorite_first: bool,
) {
    let query = query.trim().to_lowercase();
    let mut filtered = tracks
        .iter()
        .filter(|track| query.is_empty() || track_matches_query(track, &query))
        .cloned()
        .collect::<Vec<_>>();
    sort_tracks_with_options(&mut filtered, settings, favorite_first);
    let additions = filtered
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn append_tracks_to_model(model: &gio::ListStore, tracks: impl IntoIterator<Item = Track>) {
    append_boxed_items_to_model(model, tracks);
}

fn append_boxed_items_to_model<T: 'static>(
    model: &gio::ListStore,
    items: impl IntoIterator<Item = T>,
) {
    let additions = items
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        model.splice(model.n_items(), 0, &additions);
    }
}

fn track_matches_query(track: &Track, query: &str) -> bool {
    track.title.to_lowercase().contains(query)
        || track.artist.to_lowercase().contains(query)
        || track.album.to_lowercase().contains(query)
        || track.year.to_string().contains(query)
}

fn sort_tracks_with_options(
    tracks: &mut [Track],
    settings: &TrackTableSettings,
    favorite_first: bool,
) {
    tracks.sort_by(|left, right| {
        let mut ordering = match settings.sort_key {
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
            ordering = ordering.reverse();
        }

        if favorite_first {
            right.favorite.cmp(&left.favorite).then(ordering)
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
            | Route::Favorites
            | Route::ArtistDetail(_)
            | Route::ArtistDiscography(_)
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
            artist.set_halign(gtk::Align::Start);
            artist.set_hexpand(false);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
            artist.set_width_chars(1);
            artist.set_max_width_chars(28);

            if let Some(route) = artist_route {
                let button = gtk::Button::new();
                button.add_css_class("flat");
                button.add_css_class("table-link");
                button.add_css_class("track-artist-link");
                button.set_halign(gtk::Align::Start);
                button.set_hexpand(false);
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
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));

        let Some(route) = route else {
            list_item.set_child(Some(&label));
            return;
        };

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("table-link");
        button.set_halign(gtk::Align::Start);
        button.set_hexpand(false);
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

fn album_artist_route(album: &Album) -> Option<Route> {
    if let Some(artist_id) = album.artist_id.clone() {
        Some(Route::ArtistDetail(artist_id))
    } else if !album.artist.trim().is_empty() {
        Some(Route::Search {
            query: album.artist.clone(),
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
    add_widget_click(label.upcast_ref(), callback);
}

fn add_widget_click(target: &gtk::Widget, callback: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, press_count, _, _| {
        if press_count == 1 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            callback();
        }
    });
    target.add_controller(click);
}

fn add_card_label_link(
    shell: &Rc<Shell>,
    target: &gtk::Widget,
    label: &gtk::Label,
    text: &str,
    route: Option<Route>,
) {
    let Some(route) = route else {
        return;
    };
    target.set_cursor_from_name(Some("pointer"));
    label.set_cursor_from_name(Some("pointer"));
    add_link_hover(target, label, text);
    let shell = Rc::clone(shell);
    add_widget_click(target, move || shell.navigate(route.clone()));
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
    use super::{
        AutoLyricsRequest, auto_lyrics_request_for_settings, auto_lyrics_skip_action_enabled,
        current_playback_track_id, seekbar_target_seconds,
    };
    use rufin_core::{
        Album, AlbumId, AppSettings, ArtistId, QueueEntry, QueueEntryId, Route, SearchKind, Track,
        TrackId, TrackSortKey, TrackTableSettings,
    };
    use rufin_provider::{LyricLine, Lyrics, LyricsSource};

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
    fn track_artist_route_prefers_detail_and_falls_back_to_artist_search() {
        let track = test_track("Track Artist", Some(ArtistId::fake(3)));
        assert_eq!(
            super::track_artist_route(&track),
            Some(Route::ArtistDetail(ArtistId::fake(3)))
        );

        let track = test_track("Loose Artist", None);
        assert_eq!(
            super::track_artist_route(&track),
            Some(Route::Search {
                query: "Loose Artist".to_string(),
                kind: SearchKind::Artists,
            })
        );

        assert_eq!(super::track_artist_route(&test_track("   ", None)), None);
    }

    #[test]
    fn album_artist_route_prefers_detail_and_falls_back_to_artist_search() {
        let album = test_album("Album Artist", Some(ArtistId::fake(5)));
        assert_eq!(
            super::album_artist_route(&album),
            Some(Route::ArtistDetail(ArtistId::fake(5)))
        );

        let album = test_album("Compilation Artist", None);
        assert_eq!(
            super::album_artist_route(&album),
            Some(Route::Search {
                query: "Compilation Artist".to_string(),
                kind: SearchKind::Artists,
            })
        );

        assert_eq!(super::album_artist_route(&test_album("", None)), None);
    }

    #[test]
    fn compact_artist_track_sort_keeps_favorites_first() {
        let mut favorite_late = test_track("Artist", Some(ArtistId::fake(1)));
        favorite_late.id = TrackId::fake(1);
        favorite_late.title = "Zulu".to_string();
        favorite_late.favorite = true;
        let mut ordinary_first = test_track("Artist", Some(ArtistId::fake(1)));
        ordinary_first.id = TrackId::fake(2);
        ordinary_first.title = "Alpha".to_string();
        let mut favorite_first = test_track("Artist", Some(ArtistId::fake(1)));
        favorite_first.id = TrackId::fake(3);
        favorite_first.title = "Bravo".to_string();
        favorite_first.favorite = true;

        let mut tracks = vec![
            ordinary_first.clone(),
            favorite_late.clone(),
            favorite_first.clone(),
        ];
        let settings = TrackTableSettings {
            sort_key: TrackSortKey::Title,
            ..TrackTableSettings::default()
        };

        super::sort_tracks_with_options(&mut tracks, &settings, true);

        assert_eq!(
            tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Bravo", "Zulu", "Alpha"]
        );
    }

    #[test]
    fn full_artist_track_sort_uses_selected_ranking() {
        let mut favorite_late = test_track("Artist", Some(ArtistId::fake(1)));
        favorite_late.id = TrackId::fake(1);
        favorite_late.title = "Zulu".to_string();
        favorite_late.favorite = true;
        let mut ordinary_first = test_track("Artist", Some(ArtistId::fake(1)));
        ordinary_first.id = TrackId::fake(2);
        ordinary_first.title = "Alpha".to_string();
        let mut favorite_first = test_track("Artist", Some(ArtistId::fake(1)));
        favorite_first.id = TrackId::fake(3);
        favorite_first.title = "Bravo".to_string();
        favorite_first.favorite = true;

        let mut tracks = vec![favorite_late, ordinary_first, favorite_first];
        let settings = TrackTableSettings {
            sort_key: TrackSortKey::Title,
            ..TrackTableSettings::default()
        };

        super::sort_tracks_with_options(&mut tracks, &settings, false);

        assert_eq!(
            tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Bravo", "Zulu"]
        );
    }

    #[test]
    fn artist_discography_uses_responsive_cards() {
        assert!(super::route_uses_responsive_cards(
            &Route::ArtistDiscography(ArtistId::fake(1))
        ));
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
    fn auto_lyrics_skip_action_only_enabled_for_unsuppressed_external_tracks() {
        let track_id = TrackId::fake(11);
        let mut settings = AppSettings {
            external_lyrics_enabled: true,
            ..AppSettings::default()
        };

        assert!(auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            None
        ));

        settings
            .suppressed_auto_lyrics_track_ids
            .push(track_id.as_str().to_string());
        assert!(!auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            None
        ));

        settings.suppressed_auto_lyrics_track_ids.clear();
        settings.external_lyrics_enabled = false;
        assert!(!auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            None
        ));

        settings.external_lyrics_enabled = true;
        settings.private_mode = true;
        assert!(!auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            None
        ));
        assert!(!auto_lyrics_skip_action_enabled(&settings, None, None));
    }

    #[test]
    fn auto_lyrics_skip_action_is_hidden_for_server_lyrics() {
        let track_id = TrackId::fake(13);
        let settings = AppSettings {
            external_lyrics_enabled: true,
            ..AppSettings::default()
        };
        let server_lyrics = Lyrics {
            track_id: track_id.clone(),
            source: LyricsSource::Server,
            lines: vec![LyricLine {
                text: "server line".to_string(),
                start_millis: None,
            }],
        };
        let remote_lyrics = Lyrics {
            track_id: track_id.clone(),
            source: LyricsSource::Remote,
            lines: vec![LyricLine {
                text: "remote line".to_string(),
                start_millis: None,
            }],
        };

        assert!(!auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            Some(&server_lyrics)
        ));
        assert!(auto_lyrics_skip_action_enabled(
            &settings,
            Some(&track_id),
            Some(&remote_lyrics)
        ));
    }

    #[test]
    fn auto_lyrics_request_keeps_server_lookup_when_external_search_is_suppressed() {
        let track_id = TrackId::fake(12);
        let mut settings = AppSettings {
            external_lyrics_enabled: true,
            ..AppSettings::default()
        };

        assert_eq!(
            auto_lyrics_request_for_settings(&settings, &track_id),
            Some(AutoLyricsRequest::Default)
        );

        settings
            .suppressed_auto_lyrics_track_ids
            .push(track_id.as_str().to_string());
        assert_eq!(
            auto_lyrics_request_for_settings(&settings, &track_id),
            Some(AutoLyricsRequest::ServerOnly)
        );

        settings.suppressed_auto_lyrics_track_ids.clear();
        settings.external_lyrics_enabled = false;
        assert_eq!(
            auto_lyrics_request_for_settings(&settings, &track_id),
            Some(AutoLyricsRequest::ServerOnly)
        );

        settings.external_lyrics_enabled = true;
        settings.private_mode = true;
        assert_eq!(
            auto_lyrics_request_for_settings(&settings, &track_id),
            Some(AutoLyricsRequest::ServerOnly)
        );

        settings.lyrics_panel_visible = false;
        assert_eq!(auto_lyrics_request_for_settings(&settings, &track_id), None);
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

    fn test_album(artist: &str, artist_id: Option<ArtistId>) -> Album {
        Album {
            id: AlbumId::fake(1),
            title: "Album".to_string(),
            artist: artist.to_string(),
            artist_id,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 180,
            favorite: false,
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
        }
    }

    fn test_track(artist: &str, artist_id: Option<ArtistId>) -> Track {
        Track {
            id: TrackId::fake(1),
            album_id: AlbumId::fake(1),
            title: "Track".to_string(),
            artist: artist.to_string(),
            artist_id,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            genres: Vec::new(),
            local_path: None,
        }
    }
}
