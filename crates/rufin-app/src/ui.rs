use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod artist;
mod cards;
mod chrome;
mod discord;
mod favorites;
mod folders;
mod home;
mod layout;
mod library;
mod local_access_mapping;
mod login;
#[cfg(unix)]
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
mod source_selector;

use adw::prelude::*;
use gdk_pixbuf::Pixbuf;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gio;
use gtk::glib;
#[cfg(unix)]
use mpris_server::Player as MprisPlayer;
use rufin_core::{
    Album, AlbumId, AppSettings, Artist, ArtistId, FolderPathItem, Genre, HomeSection,
    HomeSectionKind, ImageRef, LeftSidebarMode, LibraryListKey, Playlist, PlaylistId, QueueEntry,
    QueueSnapshot, RightSidebarMode, Route, RouteStack, SearchKind, Track, TrackId, TrackSortKey,
    TrackTableColumn, TrackTableSettings, format_duration,
};
use rufin_playback::PlaybackState;
use rufin_provider::{FavoriteItemId, FolderDetail, Lyrics, LyricsSource, PlaylistEntry};
use rufin_store::{CachedGenreDetail, image_cache_key};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{
    AppController, ControllerEvent, DiscoveredServer, LibrarySnapshot, LyricsSearchResult,
    PlaybackSnapshot, grouped_cover_refs_for_items, track_cover_refs_for_items,
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
    PRIMARY_ROUTE_MARGIN_START, ResolvedLayout, resolve_layout, route_content_width,
};
#[cfg(unix)]
use mpris::install_mpris;
use navigation::{
    build_compact_navigation, build_normal_navigation, rebuild_navigation, sidebar_history_button,
    update_navigation_selection,
};
use paging::{PagedGridCursor, connect_paged_grid_loader, finish_grid_page};
use player::{PlayerControls, build_bottom_player, connect_player_controls};
use preferences::{present_library_preferences_dialog, present_preferences_dialog};
use queue::connect_queue_panel_controls;
use right_panel::{apply_lyrics_panel_visibility, build_right_panel, connect_queue_lyrics_split};
use source_selector::{ServerSelector, build_server_selector};

const GRID_ROUTE_PAGE_SIZE: usize = 16;
const TRACK_ROUTE_PAGE_SIZE: usize = 64;
const CONTEXT_MENU_PLAYLIST_LIMIT: usize = 100;
const GRID_COVER_SIZE: u32 = 256;
const DETAIL_COVER_SIZE: u32 = 512;
const THUMB_COVER_SIZE: u32 = 96;
const IMAGE_TAG_UNTAGGED: &str = "untagged";
const DECODED_COVER_CACHE_LIMIT: usize = 3_072;
const COVER_WARM_BATCH_SIZE: usize = 3;
const COVER_WARM_MAX_IN_FLIGHT: usize = 6;
const COVER_WARM_INITIAL_DELAY_MS: u64 = 250;
const COVER_WARM_INTERVAL_MS: u64 = 32;
const COVER_DECODE_MAX_IN_FLIGHT: usize = 8;
const STARTUP_COVER_WARM_DELAY_MS: u64 = 450;
const STARTUP_COVER_WARM_BATCH_SIZE: usize = 2;
const STARTUP_COVER_WARM_INTERVAL_MS: u64 = 40;
const STARTUP_ROUTE_REVEAL_MIN_MS: u64 = 320;
const STARTUP_ROUTE_REVEAL_MAX_MS: u64 = 900;
const STARTUP_ROUTE_REVEAL_POLL_MS: u64 = 32;
const STARTUP_TRACK_THUMB_PRIME_DELAY_MS: u64 = 80;
const INITIAL_COVER_PRIME_LIMIT: usize = 24;
const INITIAL_COVER_PRIME_BUDGET: Duration = Duration::from_millis(300);
const INITIAL_TRACK_THUMB_PRIME_LIMIT: usize = 18;
const INITIAL_TRACK_THUMB_PRIME_BUDGET: Duration = Duration::from_millis(120);
const FAVORITE_EMPTY_GLYPH: &str = "♡";
const FAVORITE_FILLED_GLYPH: &str = "♥";
const PLAYLIST_ENTRY_DRAG_WIDTH: i32 = 18;
const PLAYLIST_ENTRY_NUMBER_WIDTH: i32 = 24;
const PLAYLIST_ENTRY_COVER_WIDTH: i32 = 36;
const PLAYLIST_ENTRY_DURATION_WIDTH: i32 = 64;
const PLAYLIST_ENTRY_REMOVE_WIDTH: i32 = 34;
const PLAYLIST_ENTRY_COLUMN_GAP: i32 = 8;
const PLAYLIST_ENTRY_TEXT_COLUMN_GAP: i32 = 16;
const PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH: i32 = 120;
const PLAYLIST_ENTRY_NUMBER_XALIGN: f32 = 0.35;
const PLAYLIST_ENTRY_TITLE_MAX_CHARS: i32 = 44;
const PLAYLIST_ENTRY_ALBUM_MAX_CHARS: i32 = 18;
pub(super) const PLAY_NEXT_ICON: &str = "view-sort-ascending-symbolic";
pub(super) const PLAY_LATER_ICON: &str = "view-sort-descending-symbolic";
const RESPONSIVE_RENDER_DELAY_MS: u64 = 16;
static HOME_SHOWCASE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub fake_scale: Option<FakeScale>,
    pub smoke_exit_ms: Option<u64>,
    pub ui_perf_run: bool,
    pub ui_perf_observe: bool,
    pub ui_perf_max_gap_ms: u64,
    pub ui_perf_route_ms: u64,
    pub ui_perf_duration_ms: u64,
    pub ui_perf_asset_ms: u64,
    pub ui_perf_output: Option<PathBuf>,
}

struct AppState {
    routes: RefCell<RouteStack>,
    settings: RefCell<AppSettings>,
    resolved_left_sidebar: Cell<LeftSidebarMode>,
    resolved_right_sidebar: Cell<RightSidebarMode>,
    resolved_right_sidebar_width: Cell<i32>,
    main_content_width: Cell<i32>,
    library: RefCell<LibrarySnapshot>,
    queue: RefCell<Option<QueueSnapshot>>,
    player: RefCell<PlaybackSnapshot>,
    lyrics: RefCell<Option<Lyrics>>,
    lyrics_track_id: RefCell<Option<rufin_core::TrackId>>,
    lyrics_auto_search_attempted: RefCell<HashSet<rufin_core::TrackId>>,
    lyrics_search_dialog: RefCell<Option<LyricsSearchDialog>>,
    lyrics_timing_generation: Cell<u64>,
    lyrics_timing_source: RefCell<Option<glib::SourceId>>,
    #[cfg(unix)]
    mpris_player: RefCell<Option<Rc<MprisPlayer>>>,
    discord_presence: RefCell<DiscordPresence>,
    updating_player_controls: Cell<bool>,
    seek_preview_seconds: Cell<Option<u32>>,
    seek_generation: Cell<u64>,
    queue_filter: RefCell<String>,
    lyrics_panel_visible: Cell<bool>,
    queue_lyrics_position_save_suppressed: Rc<Cell<u32>>,
    responsive_render_queued: Cell<bool>,
    card_grid_columns: Cell<usize>,
    home_section_state: RefCell<HashMap<HomeSectionKind, HomeSectionState>>,
    home_section_views: RefCell<HashMap<HomeSectionKind, HomeSectionView>>,
    prefetched_explore: RefCell<Option<PrefetchedHomeSection>>,
    home_refresh_started_for_visit: Cell<bool>,
    playlist_refresh_started_for_visit: Cell<bool>,
    home_showcase_seed: Cell<u64>,
    startup_route_revealed: Cell<bool>,
    first_run_connection_pending: Cell<bool>,
    first_run_connection_ready: Cell<bool>,
    discovered_servers: RefCell<Vec<DiscoveredServer>>,
    server_discovery_status: RefCell<String>,
    server_discovery_running: Cell<bool>,
    server_discovery_started: Cell<bool>,
    cover_bindings: RefCell<HashMap<String, Vec<CoverBinding>>>,
    cover_decodes: RefCell<HashSet<String>>,
    cover_decode_queue: RefCell<VecDeque<CoverDecodeJob>>,
    cover_warm_generation: Cell<u64>,
    startup_cover_warm_generation: Cell<u64>,
    decoded_covers: RefCell<HashMap<String, Pixbuf>>,
    decoded_cover_order: RefCell<VecDeque<String>>,
    favorite_controls: FavoriteControls,
    folder_request_generation: Cell<u64>,
    folder_state: RefCell<FolderRouteState>,
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

struct CoverDecodeJob {
    key: String,
    path: PathBuf,
    size: i32,
    priority: CoverDecodePriority,
}

struct StartupCoverWarmJob {
    key: String,
    image_ref: ImageRef,
    fetch_size: u32,
    size: i32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CoverDecodePriority {
    Visible,
    Warm,
}

impl CoverDecodePriority {
    fn glib_priority(self) -> glib::Priority {
        match self {
            Self::Visible => glib::Priority::DEFAULT,
            Self::Warm => glib::Priority::LOW,
        }
    }
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
struct HomeSectionView {
    root: gtk::Widget,
    row: gtk::Box,
    previous: gtk::Button,
    next: gtk::Button,
}

#[derive(Clone)]
struct PrefetchedHomeSection {
    server_id: rufin_core::ServerId,
    section: HomeSection,
}

#[derive(Clone, Default)]
struct FolderRouteState {
    request_id: u64,
    path: Vec<FolderPathItem>,
    loading: bool,
    detail: Option<FolderDetail>,
    error: Option<String>,
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
    terminal_events: bool,
    observe_scroll: bool,
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
    cover_refs: Vec<ImageRef>,
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
    root_stack: gtk::Stack,
    app_root: gtk::Box,
    login_host: gtk::Box,
    normal_nav: gtk::Box,
    compact_nav: gtk::Box,
    server_selector: ServerSelector,
    route_title: adw::WindowTitle,
    route_host: gtk::Box,
    normal_back_button: gtk::Button,
    normal_forward_button: gtk::Button,
    compact_back_button: gtk::Button,
    compact_forward_button: gtk::Button,
    right_panel_slot: gtk::ScrolledWindow,
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
    let first_run = library.first_run;
    let perf_observe = options.ui_perf_observe && !options.ui_perf_run;
    let perf_enabled = options.ui_perf_run || perf_observe;
    let defer_initial_route = !options.ui_perf_run && !first_run;
    let perf_requires_assets =
        perf_enabled && options.fake_scale.is_none() && library_has_image_refs(&library);
    let prefetched_explore = prefetched_explore_from_snapshot(&library);

    let state = AppState {
        routes: RefCell::new(RouteStack::new(Route::Home)),
        settings: RefCell::new(settings.clone()),
        resolved_left_sidebar: Cell::new(LeftSidebarMode::Full),
        resolved_right_sidebar: Cell::new(RightSidebarMode::Hidden),
        resolved_right_sidebar_width: Cell::new(0),
        main_content_width: Cell::new(1),
        library: RefCell::new(library),
        queue: RefCell::new(queue),
        player: RefCell::new(player),
        lyrics: RefCell::new(None),
        lyrics_track_id: RefCell::new(None),
        lyrics_auto_search_attempted: RefCell::new(HashSet::new()),
        lyrics_search_dialog: RefCell::new(None),
        lyrics_timing_generation: Cell::new(0),
        lyrics_timing_source: RefCell::new(None),
        #[cfg(unix)]
        mpris_player: RefCell::new(None),
        discord_presence: RefCell::new(DiscordPresence::new()),
        updating_player_controls: Cell::new(false),
        seek_preview_seconds: Cell::new(None),
        seek_generation: Cell::new(0),
        queue_filter: RefCell::new(String::new()),
        lyrics_panel_visible: Cell::new(settings.lyrics_panel_visible),
        queue_lyrics_position_save_suppressed: Rc::new(Cell::new(0)),
        responsive_render_queued: Cell::new(false),
        card_grid_columns: Cell::new(0),
        home_section_state: RefCell::new(HashMap::new()),
        home_section_views: RefCell::new(HashMap::new()),
        prefetched_explore: RefCell::new(prefetched_explore),
        home_refresh_started_for_visit: Cell::new(false),
        playlist_refresh_started_for_visit: Cell::new(false),
        home_showcase_seed: Cell::new(next_home_showcase_seed()),
        startup_route_revealed: Cell::new(!defer_initial_route),
        first_run_connection_pending: Cell::new(false),
        first_run_connection_ready: Cell::new(false),
        discovered_servers: RefCell::new(Vec::new()),
        server_discovery_status: RefCell::new("Searching will start automatically".to_string()),
        server_discovery_running: Cell::new(false),
        server_discovery_started: Cell::new(false),
        cover_bindings: RefCell::new(HashMap::new()),
        cover_decodes: RefCell::new(HashSet::new()),
        cover_decode_queue: RefCell::new(VecDeque::new()),
        cover_warm_generation: Cell::new(0),
        startup_cover_warm_generation: Cell::new(0),
        decoded_covers: RefCell::new(HashMap::new()),
        decoded_cover_order: RefCell::new(VecDeque::new()),
        favorite_controls: RefCell::new(HashMap::new()),
        folder_request_generation: Cell::new(0),
        folder_state: RefCell::new(FolderRouteState::default()),
        perf: perf_enabled.then(|| {
            Rc::new(UiPerfMonitor::new(UiPerfOptions {
                max_gap_ms: options.ui_perf_max_gap_ms,
                route_ms: options.ui_perf_route_ms,
                duration_ms: options.ui_perf_duration_ms.max(15_000),
                asset_ms: options.ui_perf_asset_ms,
                require_assets: perf_requires_assets,
                terminal_events: options.ui_perf_run,
                observe_scroll: perf_observe,
                output: options.ui_perf_output.clone().or_else(|| {
                    if perf_observe {
                        default_ui_perf_output_path("rufin-ui-observe")
                    } else {
                        default_ui_perf_output_path("rufin-ui-perf")
                    }
                }),
            }))
        }),
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rufin")
        .build();

    let root_stack = gtk::Stack::new();
    root_stack.add_css_class("app-root");
    root_stack.set_hexpand(true);
    root_stack.set_vexpand(true);

    let app_root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    app_root.add_css_class("app-root");
    app_root.set_hexpand(true);
    app_root.set_vexpand(true);

    let login_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    login_host.add_css_class("login-root");
    login_host.set_hexpand(true);
    login_host.set_vexpand(true);

    let upper = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    upper.set_hexpand(true);
    upper.set_vexpand(true);

    let normal_nav = gtk::Box::new(gtk::Orientation::Vertical, 10);
    normal_nav.add_css_class("wide-sidebar");
    normal_nav.set_hexpand(false);
    normal_nav.set_width_request(NORMAL_SIDEBAR_WIDTH);

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 3);
    compact_nav.add_css_class("compact-rail");
    compact_nav.set_hexpand(false);
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
    let main_menu = content_chrome.main_menu;
    let right_panel_slot = content_chrome.right_panel_slot;
    let player_controls = build_bottom_player();

    upper.append(&normal_nav);
    upper.append(&compact_nav);
    upper.append(&content_chrome.root);

    app_root.append(&upper);
    app_root.append(&player_controls.root);

    root_stack.add_named(&login_host, Some("login"));
    root_stack.add_named(&app_root, Some("app"));
    window.set_content(Some(&root_stack));

    let shell = Rc::new(Shell {
        state,
        controller,
        application: app.clone(),
        window,
        root_stack,
        app_root,
        login_host,
        normal_nav,
        compact_nav,
        server_selector,
        route_title,
        route_host,
        normal_back_button,
        normal_forward_button,
        compact_back_button,
        compact_forward_button,
        right_panel_slot,
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
    #[cfg(unix)]
    install_mpris(&shell);
    shell.update_layout();
    prime_first_cached_cover(&shell);
    if defer_initial_route {
        shell.render_startup_loading_view();
    } else {
        shell.render_current_route();
        shell.refresh_home_for_current_visit();
    }
    shell.schedule_startup_cover_warm();
    shell.render_queue_panel();
    shell.render_lyrics_panel();
    shell.update_bottom_player();
    shell.update_discord_presence(&shell.state.player.borrow());
    shell.update_right_panel_button();
    shell.update_lyrics_panel_button();
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
    if defer_initial_route {
        shell.schedule_startup_route_reveal();
    }
    let layout_shell = Rc::clone(&shell);
    glib::idle_add_local_once(move || {
        layout_shell.update_layout();
    });
    shell.queue_responsive_route_render();

    if options.ui_perf_run {
        start_ui_perf_run(&shell, app);
    } else if perf_observe {
        start_ui_perf_observe(&shell, app);
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
    let mut sizes = Vec::from([
        preferred_size,
        GRID_COVER_SIZE,
        THUMB_COVER_SIZE,
        DETAIL_COVER_SIZE,
    ]);
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

impl Shell {
    fn register_home_section_view(
        &self,
        section_kind: HomeSectionKind,
        root: &gtk::Box,
        row: &gtk::Box,
        previous: &gtk::Button,
        next: &gtk::Button,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        self.state.home_section_views.borrow_mut().insert(
            section_kind,
            HomeSectionView {
                root: root.clone().upcast::<gtk::Widget>(),
                row: row.clone(),
                previous: previous.clone(),
                next: next.clone(),
            },
        );
    }

    fn refresh_visible_home_section(
        self: &Rc<Self>,
        section_kind: HomeSectionKind,
        sections: &[HomeSection],
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        if let Some(section) = sections.iter().find(|section| section.kind == section_kind) {
            self.render_visible_home_section(section);
        } else {
            self.hide_visible_home_section(section_kind);
        }
    }

    fn refresh_visible_home_sections(
        self: &Rc<Self>,
        sections: &[HomeSection],
        include_explore: bool,
    ) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }

        let section_kinds = self
            .state
            .home_section_views
            .borrow()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for section_kind in section_kinds {
            if !include_explore && section_kind == HomeSectionKind::Explore {
                continue;
            }
            self.refresh_visible_home_section(section_kind, sections);
        }
    }

    fn render_visible_home_section(self: &Rc<Self>, section: &HomeSection) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section.kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };

        view.root.set_visible(true);
        if !section.tracks.is_empty() {
            cards::render_home_track_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.tracks,
            );
        } else {
            cards::render_home_album_page(
                self,
                &view.row,
                &view.previous,
                &view.next,
                section.kind,
                &section.albums,
            );
        }
        true
    }

    fn hide_visible_home_section(&self, section_kind: HomeSectionKind) -> bool {
        let view = self
            .state
            .home_section_views
            .borrow()
            .get(&section_kind)
            .cloned();
        let Some(view) = view else {
            return false;
        };
        view.root.set_visible(false);
        true
    }

    fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        let previous = self.state.routes.borrow().current().clone();
        self.refresh_search_results_for_route(&route);
        self.state.routes.borrow_mut().navigate(route.clone());
        self.handle_home_route_transition(&previous, &route);
        self.render_current_route();
        if matches!(route, Route::Home) {
            self.refresh_home_for_current_visit();
        }
        if matches!(route, Route::Playlists) {
            self.refresh_playlists_for_current_visit();
        }
    }

    fn go_back(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.render_current_route();
            if matches!(route, Route::Home) {
                self.refresh_home_for_current_visit();
            }
            if matches!(route, Route::Playlists) {
                self.refresh_playlists_for_current_visit();
            }
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let previous = self.state.routes.borrow().current().clone();
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.refresh_search_results_for_route(&route);
            self.handle_home_route_transition(&previous, &route);
            self.render_current_route();
            if matches!(route, Route::Home) {
                self.refresh_home_for_current_visit();
            }
            if matches!(route, Route::Playlists) {
                self.refresh_playlists_for_current_visit();
            }
        }
    }

    fn refresh_search_results_for_route(&self, route: &Route) {
        if let Route::Search { query, .. } = route {
            self.controller.search(query.clone());
        }
    }

    fn start_folder_load(self: &Rc<Self>, path: Vec<FolderPathItem>) {
        let request_id = self.state.folder_request_generation.get().saturating_add(1);
        self.state.folder_request_generation.set(request_id);
        *self.state.folder_state.borrow_mut() = FolderRouteState {
            request_id,
            path: path.clone(),
            loading: true,
            detail: None,
            error: None,
        };
        self.controller.load_folder_for_active(request_id, path);
    }

    fn apply_folder_loaded(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        detail: FolderDetail,
    ) {
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = Some(detail);
            state.error = None;
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }

    fn apply_folder_load_failed(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        error: String,
    ) {
        warn!(%error, "folder load failed");
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = None;
            state.error = Some(error);
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }

    fn handle_home_route_transition(self: &Rc<Self>, previous: &Route, next: &Route) {
        let was_home = matches!(previous, Route::Home);
        let is_home = matches!(next, Route::Home);
        let was_playlists = matches!(previous, Route::Playlists);
        let is_playlists = matches!(next, Route::Playlists);

        if is_home && !was_home {
            self.state.home_refresh_started_for_visit.set(false);
            self.state.home_showcase_seed.set(next_home_showcase_seed());
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.promote_cached_prefetched_explore();
        }
        if is_playlists && !was_playlists {
            self.state.playlist_refresh_started_for_visit.set(false);
        }
    }

    fn refresh_home_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Home) {
            return;
        }
        if self.state.home_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller
            .refresh_home_sections_without_explore_for_active();
        self.controller.prefetch_explore_for_active();
    }

    fn refresh_playlists_for_current_visit(self: &Rc<Self>) {
        if !matches!(self.state.routes.borrow().current(), Route::Playlists) {
            return;
        }
        if self.state.playlist_refresh_started_for_visit.replace(true) {
            return;
        }
        self.controller.refresh_playlists_for_active();
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
        let prefetched = self.state.prefetched_explore.borrow().clone();
        let promoted = prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, true))
            .unwrap_or(false);
        if promoted {
            self.controller.prefetch_explore_for_active();
        }
        promoted
    }

    fn promote_cached_prefetched_explore(self: &Rc<Self>) -> bool {
        let prefetched = self.state.prefetched_explore.borrow().clone();
        prefetched
            .map(|prefetched| self.promote_prefetched_explore(prefetched, false))
            .unwrap_or(false)
    }

    fn promote_prefetched_explore(
        self: &Rc<Self>,
        prefetched: PrefetchedHomeSection,
        render_current_route: bool,
    ) -> bool {
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
        if prefetched.server_id != server_id {
            *self.state.prefetched_explore.borrow_mut() = Some(prefetched);
            return false;
        }

        let section = prefetched.section.clone();
        let mut changed = false;
        {
            let mut library = self.state.library.borrow_mut();
            let current = library
                .home_sections
                .iter()
                .find(|existing| existing.kind == section.kind);
            if current != Some(&section) {
                upsert_snapshot_home_section(&mut library.home_sections, section.clone());
                changed = true;
            }
        }
        if changed {
            reset_home_section_pages(&mut self.state.home_section_state.borrow_mut());
            self.controller
                .promote_prefetched_explore_for_active(section.clone());
        }
        if render_current_route {
            self.refresh_visible_home_section(section.kind, std::slice::from_ref(&section));
        }
        true
    }

    fn update_prefetched_explore_from_snapshot(
        &self,
        server_id: Option<rufin_core::ServerId>,
        prefetched: Option<PrefetchedHomeSection>,
        sections: &[HomeSection],
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
                    && !sections.iter().any(|section| {
                        section.kind == HomeSectionKind::Explore && section == &current.section
                    })
            })
        };
        if !keep_current {
            *self.state.prefetched_explore.borrow_mut() = None;
        }
    }

    fn update_layout(self: &Rc<Self>) -> bool {
        let width = self.layout_width().max(1);
        let settings = self.state.settings.borrow().layout.clone();
        let resolved = resolve_layout(&settings, width);
        self.apply_resolved_layout(resolved)
    }

    fn apply_resolved_layout(self: &Rc<Self>, resolved: ResolvedLayout) -> bool {
        let login_active = self.login_screen_active();
        if login_active {
            self.root_stack.set_visible_child(&self.login_host);
        } else {
            self.root_stack.set_visible_child(&self.app_root);
        }
        let previous_left = self
            .state
            .resolved_left_sidebar
            .replace(resolved.left_sidebar);
        let previous_right = self
            .state
            .resolved_right_sidebar
            .replace(resolved.right_sidebar);
        let previous_right_width = self
            .state
            .resolved_right_sidebar_width
            .replace(resolved.right_sidebar_width);
        let previous_main_width = self.state.main_content_width.replace(resolved.main_width);

        self.normal_nav
            .set_visible(!login_active && resolved.left_sidebar == LeftSidebarMode::Full);
        self.compact_nav
            .set_visible(!login_active && resolved.left_sidebar == LeftSidebarMode::Compact);
        self.right_panel_slot
            .set_visible(!login_active && resolved.right_sidebar.is_visible());
        self.right_panel_slot.set_min_content_width(0);
        self.right_panel_slot
            .set_max_content_width(resolved.right_sidebar_width);
        self.right_panel_slot.set_size_request(-1, -1);
        self.right_panel
            .set_width_request(resolved.right_sidebar_width);
        self.right_panel
            .set_visible(!login_active && resolved.right_sidebar.is_visible());
        self.player_controls.root.set_visible(!login_active);
        self.update_right_panel_button();
        self.update_lyrics_panel_button();

        let changed = previous_left != resolved.left_sidebar
            || previous_right != resolved.right_sidebar
            || previous_right_width != resolved.right_sidebar_width
            || previous_main_width != resolved.main_width;
        if changed {
            debug!(?resolved, "resolved layout changed");
            self.queue_responsive_route_render();
        }
        self.log_layout_snapshot("apply_resolved_layout");
        changed
    }

    fn layout_width(&self) -> i32 {
        self.window
            .surface()
            .map(|surface| surface.width())
            .filter(|width| *width > 1)
            .or_else(|| {
                let width = self.window.width();
                (width > 1).then_some(width)
            })
            .unwrap_or(1)
    }

    fn login_screen_active(&self) -> bool {
        self.state.library.borrow().first_run || self.state.first_run_connection_pending.get()
    }

    fn log_layout_snapshot(&self, stage: &'static str) {
        if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_none() {
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        info!(
            stage,
            ?route,
            login_active = self.login_screen_active(),
            first_run = self.state.library.borrow().first_run,
            first_run_connection_pending = self.state.first_run_connection_pending.get(),
            first_run_connection_ready = self.state.first_run_connection_ready.get(),
            window_width = self.layout_width(),
            root_stack_width = self.root_stack.width(),
            app_root_width = self.app_root.width(),
            login_host_width = self.login_host.width(),
            route_host_width = self.route_host.width(),
            resolved_main_width = self.state.main_content_width.get(),
            right_sidebar = ?self.state.resolved_right_sidebar.get(),
            right_panel_slot_visible = self.right_panel_slot.is_visible(),
            right_panel_slot_width = self.right_panel_slot.width(),
            right_panel_width = self.right_panel.width(),
            "layout snapshot"
        );
    }

    fn render_startup_loading_view(&self) {
        self.route_title.set_title("Rufin");
        self.set_history_buttons_sensitive(false, false);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }
        self.route_host
            .append(&route_boundary(self.startup_loading_view()));
    }

    fn startup_loading_view(&self) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.add_css_class("startup-loading-page");
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_halign(gtk::Align::Center);
        wrapper.set_valign(gtk::Align::Center);

        let spinner = gtk::Spinner::new();
        spinner.start();
        wrapper.append(&spinner);
        wrapper.upcast()
    }

    fn schedule_startup_route_reveal(self: &Rc<Self>) {
        if self.state.startup_route_revealed.get() || self.login_screen_active() {
            return;
        }

        let prime_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(STARTUP_TRACK_THUMB_PRIME_DELAY_MS),
            move || {
                if !prime_shell.state.startup_route_revealed.get()
                    && !prime_shell.login_screen_active()
                {
                    prime_first_track_thumbnail_covers(&prime_shell);
                }
            },
        );

        let started_at = Instant::now();
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
            move || {
                if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
                    return glib::ControlFlow::Break;
                }

                shell.update_layout();
                let elapsed = started_at.elapsed();
                let width_ready = shell.layout_width() > 1 && shell.route_host.width() > 1;
                let reveal_ready =
                    width_ready && elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MIN_MS);
                let reveal_expired = elapsed >= Duration::from_millis(STARTUP_ROUTE_REVEAL_MAX_MS);
                if reveal_ready || reveal_expired {
                    shell.reveal_startup_route();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }

    fn reveal_startup_route(self: &Rc<Self>) {
        if self.state.startup_route_revealed.replace(true) || self.login_screen_active() {
            return;
        }

        self.update_layout();
        self.render_current_route();
    }

    fn schedule_first_run_app_reveal(self: &Rc<Self>) {
        self.log_layout_snapshot("first_run_reveal_queued");

        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.state.first_run_connection_pending.set(false);
            shell.state.first_run_connection_ready.set(false);
            shell.log_layout_snapshot("first_run_reveal_before_stack_switch");
            shell.update_layout();
            shell.window.queue_resize();
            shell.app_root.queue_resize();
            shell.route_host.queue_resize();
            shell.right_panel_slot.queue_resize();
            shell.log_layout_snapshot("first_run_reveal_after_stack_switch");

            let shell = Rc::clone(&shell);
            glib::timeout_add_local_once(
                Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS),
                move || {
                    shell.log_layout_snapshot("first_run_reveal_before_render");
                    shell.state.startup_route_revealed.set(true);
                    shell.update_layout();
                    shell.render_current_route();
                    if matches!(shell.state.routes.borrow().current(), Route::Home) {
                        shell.refresh_home_for_current_visit();
                    }
                    shell.render_queue_panel();
                    shell.render_lyrics_panel();
                    shell.update_bottom_player();
                    shell.log_layout_snapshot("first_run_reveal_after_render");
                    shell.queue_post_layout_route_render();
                },
            );
        });
    }

    fn update_server_selector(self: &Rc<Self>) {
        source_selector::update_server_selector(self);
    }

    fn present_library_preferences_dialog(self: &Rc<Self>) {
        present_library_preferences_dialog(self);
    }

    fn rebuild_sidebar_navigation(self: &Rc<Self>) {
        rebuild_navigation(self);
        self.update_layout();
    }

    fn set_history_buttons_sensitive(&self, can_back: bool, can_forward: bool) {
        self.normal_back_button.set_sensitive(can_back);
        self.compact_back_button.set_sensitive(can_back);
        self.normal_forward_button.set_sensitive(can_forward);
        self.compact_forward_button.set_sensitive(can_forward);
    }

    fn queue_responsive_route_render(self: &Rc<Self>) {
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            return;
        }
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
                shell.update_layout();
                if route_uses_responsive_cards(shell.state.routes.borrow().current()) {
                    shell.render_current_route();
                }
            },
        );
    }

    fn queue_post_layout_route_render(self: &Rc<Self>) {
        if !route_uses_responsive_cards(self.state.routes.borrow().current()) {
            return;
        }

        self.window.queue_resize();
        self.app_root.queue_resize();
        self.route_host.queue_resize();
        self.right_panel_slot.queue_resize();
        self.queue_responsive_route_render();

        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(RESPONSIVE_RENDER_DELAY_MS * 4),
            move || {
                shell.state.responsive_render_queued.set(false);
                shell.update_layout();
                if route_uses_responsive_cards(shell.state.routes.borrow().current())
                    && !shell.login_screen_active()
                {
                    shell.render_current_route();
                }
            },
        );
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
        self.cancel_cover_warm();
        self.update_layout();
        self.state.home_section_views.borrow_mut().clear();
        if !self.state.startup_route_revealed.get() && !self.login_screen_active() {
            self.render_startup_loading_view();
            return;
        }
        if self.login_screen_active() {
            clear_favorite_controls(&self.state.favorite_controls);
            while let Some(child) = self.login_host.first_child() {
                self.login_host.remove(&child);
            }
            let route_name = "FirstRun".to_string();
            self.route_title.set_title(&tr("Connect to Music Server"));
            self.set_history_buttons_sensitive(false, false);
            let view = self.add_server_view();
            self.login_host.append(&view);
            self.observe_route_scroll(&route_name);
            self.record_perf_route_render(route_name, render_started.elapsed());
            return;
        }

        clear_favorite_controls(&self.state.favorite_controls);
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let route = self.state.routes.borrow().current().clone();
        let route_name = format!("{route:?}");
        self.route_title.set_title(&tr(route.title()));
        self.set_history_buttons_sensitive(
            self.state.routes.borrow().can_back(),
            self.state.routes.borrow().can_forward(),
        );
        update_navigation_selection(self.as_ref());

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
            Route::Folders { path } => self.folders_view(path),
            Route::Playlists => self.library_playlists_view(),
            Route::PlaylistDetail(playlist_id) => self.playlist_detail_view(playlist_id),
            Route::Search { query, .. } => {
                let library = self.state.library.borrow().clone();
                self.search_view(&query, library)
            }
        };

        self.route_host.append(&route_boundary(view));
        self.observe_route_scroll(&route_name);
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

    fn observe_route_scroll(&self, route: &str) {
        let Some(perf) = self
            .state
            .perf
            .as_ref()
            .filter(|perf| perf.options.observe_scroll)
            .cloned()
        else {
            return;
        };
        let host = if self.login_screen_active() {
            self.login_host.clone().upcast::<gtk::Widget>()
        } else {
            self.route_host.clone().upcast::<gtk::Widget>()
        };
        let route = route.to_string();
        glib::idle_add_local_once(move || {
            let Some(scroller) = find_largest_scrolled_window(&host) else {
                perf.record_scroll_note(&route, "no_scrolled_window");
                return;
            };
            let adjustment = scroller.vadjustment();
            adjustment.connect_value_changed(move |adjustment| {
                let max_adjustment = (adjustment.upper() - adjustment.page_size()).max(0.0);
                perf.record_manual_scroll_step(&route, adjustment.value(), max_adjustment);
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
        content.set_margin_top(20);
        content.set_margin_bottom(36);
        content.set_margin_start(32);
        content.set_margin_end(32);

        let content_width = route_content_width(self);
        let compact = content_width < 760;
        let cover_size = if compact { 164 } else { 204 };
        let header_orientation = if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        let header = gtk::Box::new(header_orientation, if compact { 16 } else { 24 });
        header.add_css_class("album-detail-showcase");
        add_album_seed_gradient_class(&header, album.color_seed);
        header.set_hexpand(true);
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            cover_size,
            DETAIL_COVER_SIZE,
        );
        cover.add_css_class("album-detail-cover");
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        metadata.set_hexpand(true);
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
        actions.add_css_class("album-detail-actions");
        let play_album = icon_button("media-playback-start-symbolic", "Play");
        play_album.add_css_class("album-detail-action-button");
        play_album.add_css_class("album-detail-play-button");
        let controller = self.controller.clone();
        let album_tracks = tracks.clone();
        play_album.connect_clicked(move |_| controller.play_tracks_now(album_tracks.clone()));
        actions.append(&play_album);

        let play_next = icon_button(PLAY_NEXT_ICON, "Play next");
        play_next.add_css_class("album-detail-action-button");
        let controller = self.controller.clone();
        let next_tracks = tracks.clone();
        play_next.connect_clicked(move |_| {
            for track in next_tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        });
        actions.append(&play_next);

        let favorite = favorite_icon_button("Favorite");
        favorite.add_css_class("album-detail-action-button");
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
        let cover_refs = grouped_cover_refs_for_items(&detail.albums, &detail.tracks);
        self.grouped_detail_view(GroupedDetailData {
            title: detail.genre.name,
            image_ref: detail.genre.image_ref,
            cover_refs,
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
        let cover_refs = track_cover_refs_for_items(&detail.tracks);
        let summary = format!(
            "{} {} • {}",
            detail.playlist.track_count,
            tr("tracks"),
            format_duration(detail.playlist.duration_seconds)
        );
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 20);
        wrapper.add_css_class("route-content");
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        header.append(&self.cover_group_tile_for(
            cover_refs,
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
        let entries = Rc::new(detail.entries.clone());
        let state = Rc::new(RefCell::new(PlaylistEntryListState::default()));
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 8);
        wrapper.set_hexpand(true);
        wrapper.set_halign(gtk::Align::Fill);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("track-toolbar");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some(&tr("Search")));
        search.set_hexpand(true);
        toolbar.append(&search);

        let sort_titles = PLAYLIST_ENTRY_SORTS
            .iter()
            .map(|sort| tr(sort.title()))
            .collect::<Vec<_>>();
        let sort_refs = sort_titles.iter().map(String::as_str).collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&sort_refs);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        toolbar.append(&sort_dropdown);

        let direction = gtk::Button::from_icon_name("view-sort-ascending-symbolic");
        direction.add_css_class("flat");
        direction.set_tooltip_text(Some(&tr("Change sort order")));
        toolbar.append(&direction);
        wrapper.append(&toolbar);

        wrapper.append(&playlist_entries_header_row());

        let list = gtk::ListBox::new();
        list.add_css_class("track-table");
        list.add_css_class("playlist-entry-list");
        list.set_hexpand(true);
        list.set_halign(gtk::Align::Fill);
        list.set_selection_mode(gtk::SelectionMode::None);

        rebuild_playlist_entries_list(self, &list, &entries, &state.borrow(), &detail.playlist.id);

        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            search.connect_search_changed(move |entry| {
                state.borrow_mut().query = entry.text().trim().to_string();
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            sort_dropdown.connect_selected_notify(move |dropdown| {
                let selected = PLAYLIST_ENTRY_SORTS
                    .get(dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(PlaylistEntrySort::Order);
                state.borrow_mut().sort = selected;
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        {
            let shell = Rc::clone(self);
            let list = list.clone();
            let entries = Rc::clone(&entries);
            let state = Rc::clone(&state);
            let playlist_id = detail.playlist.id.clone();
            direction.connect_clicked(move |button| {
                let descending = {
                    let mut state = state.borrow_mut();
                    state.descending = !state.descending;
                    state.descending
                };
                button.set_icon_name(if descending {
                    "view-sort-descending-symbolic"
                } else {
                    "view-sort-ascending-symbolic"
                });
                rebuild_playlist_entries_list(
                    &shell,
                    &list,
                    &entries,
                    &state.borrow(),
                    &playlist_id,
                );
            });
        }
        wrapper.append(&list);
        wrapper.upcast()
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
        let GroupedDetailData {
            title,
            image_ref,
            cover_refs,
            seed,
            summary,
            tracks,
            table_context,
        } = data;
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
        header.append(&self.cover_group_tile_for(
            cover_refs,
            image_ref.as_ref(),
            seed,
            160,
            DETAIL_COVER_SIZE,
        ));
        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let title_label = gtk::Label::new(Some(&title));
        title_label.add_css_class("detail-title");
        title_label.set_xalign(0.0);
        title_label.set_wrap(true);
        let summary_label = gtk::Label::new(Some(&summary));
        summary_label.add_css_class("muted");
        summary_label.set_xalign(0.0);
        metadata.append(&title_label);
        metadata.append(&summary_label);
        header.append(&metadata);
        wrapper.append(&header);

        if tracks.is_empty() {
            wrapper
                .append(&self.placeholder_view("Tracks", "No cached tracks are linked here yet."));
        } else {
            let key = if table_context == "genre-detail" {
                LibraryListKey::GenreTracks
            } else {
                LibraryListKey::Tracks
            };
            wrapper.append(&self.library_tracks_panel(tracks, key, table_context));
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
        self.cover_tile_for_dimensions(image_ref, seed, size, size, fetch_size)
    }

    fn cover_tile_for_dimensions(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new_sized(width, height, seed);
        let widget = tile.widget();
        let decode_size = width.max(height);

        if let Some(image_ref) = image_ref
            && let Some(key) = self.cover_cache_key(image_ref, fetch_size)
        {
            if let Some((cache_key, pixbuf)) = self.decoded_cover_for_ref(image_ref, fetch_size) {
                self.record_perf_cover_cache_hit(&cache_key);
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
                        decode_size,
                        fetch_size,
                    );
                });
            }
        } else if image_ref.is_none() {
            self.record_perf_coverless_tile();
        }
        widget
    }

    fn cover_group_tile_for(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fallback_image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let image_refs = unique_cover_refs(image_refs);
        match image_refs.len() {
            0 => self.cover_tile_for(fallback_image_ref, seed, size, fetch_size),
            1 => self.cover_tile_for(image_refs.first(), seed, size, fetch_size),
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_size_request(size, size);
                grid.set_width_request(size);
                grid.set_height_request(size);
                grid.set_row_homogeneous(true);
                grid.set_column_homogeneous(true);
                grid.set_hexpand(false);
                grid.set_vexpand(false);
                grid.set_halign(gtk::Align::Start);
                grid.set_valign(gtk::Align::Start);

                let cell_size = (size / 2).max(1);
                if image_refs.len() == 3 {
                    let tall = self.cover_tile_for_dimensions(
                        image_refs.first(),
                        seed,
                        cell_size,
                        size,
                        fetch_size,
                    );
                    let top = self.cover_tile_for(
                        image_refs.get(1),
                        seed.wrapping_add(0x9e37_79b9),
                        cell_size,
                        fetch_size,
                    );
                    let bottom = self.cover_tile_for(
                        image_refs.get(2),
                        seed.wrapping_add(0x3c6e_f372),
                        cell_size,
                        fetch_size,
                    );
                    grid.attach(&tall, 0, 0, 1, 2);
                    grid.attach(&top, 1, 0, 1, 1);
                    grid.attach(&bottom, 1, 1, 1, 1);
                } else {
                    for index in 0..4 {
                        let image_ref = image_refs.get(index % image_refs.len());
                        let child = self.cover_tile_for(
                            image_ref,
                            seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                            cell_size,
                            fetch_size,
                        );
                        grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                    }
                }
                grid.upcast()
            }
        }
    }

    fn request_cover_for_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        key: String,
        image_ref: ImageRef,
        size: i32,
        fetch_size: u32,
    ) {
        if let Some((cache_key, pixbuf)) = self.decoded_cover_for_ref(&image_ref, fetch_size) {
            self.record_perf_cover_cache_hit(&cache_key);
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
        if let Some(path) = self.controller.cached_cover_path(&image_ref, fetch_size) {
            let shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                shell.record_perf_cover_ready(&key);
                shell.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Visible);
            });
        } else {
            self.controller
                .request_cover_for_key(key, image_ref, fetch_size);
        }
    }

    fn warm_cover_refs(self: &Rc<Self>, image_refs: Vec<ImageRef>, fetch_size: u32, size: i32) {
        let generation = self.next_cover_warm_generation();
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self.decoded_cover_for_ref(&image_ref, fetch_size).is_some()
            {
                continue;
            }
            jobs.push_back((key, image_ref));
        }

        if jobs.is_empty() {
            return;
        }

        self.schedule_cover_warm_jobs(Rc::new(RefCell::new(jobs)), fetch_size, size, generation);
    }

    fn schedule_startup_cover_warm(self: &Rc<Self>) {
        let generation = self
            .state
            .startup_cover_warm_generation
            .get()
            .saturating_add(1);
        self.state.startup_cover_warm_generation.set(generation);

        let jobs = self.startup_cover_warm_jobs();
        if jobs.is_empty() {
            return;
        }

        info!(covers = jobs.len(), "scheduled startup cover warm");
        let jobs = Rc::new(RefCell::new(jobs));
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(STARTUP_COVER_WARM_DELAY_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() == generation {
                    shell.start_startup_cover_warm_jobs(jobs, generation);
                }
            },
        );
    }

    fn startup_cover_warm_jobs(&self) -> VecDeque<StartupCoverWarmJob> {
        let image_refs = startup_library_cover_refs(&self.state.library.borrow());
        let mut seen = HashSet::new();
        let mut jobs = VecDeque::new();

        for image_ref in image_refs {
            let fetch_size = GRID_COVER_SIZE;
            let Some(key) = self.cover_cache_key(&image_ref, fetch_size) else {
                continue;
            };
            if !seen.insert(key.clone())
                || self.decoded_cover_for_ref(&image_ref, fetch_size).is_some()
            {
                continue;
            }
            jobs.push_back(StartupCoverWarmJob {
                key,
                image_ref,
                fetch_size,
                size: GRID_COVER_SIZE as i32,
            });
        }

        jobs
    }

    fn start_startup_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<StartupCoverWarmJob>>>,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(
            Duration::from_millis(STARTUP_COVER_WARM_INTERVAL_MS),
            move || {
                if shell.state.startup_cover_warm_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }
                if jobs.borrow().is_empty() {
                    return glib::ControlFlow::Break;
                }

                let in_flight = shell.state.cover_decodes.borrow().len();
                if in_flight >= COVER_WARM_MAX_IN_FLIGHT {
                    return glib::ControlFlow::Continue;
                }

                let capacity = COVER_WARM_MAX_IN_FLIGHT.saturating_sub(in_flight);
                let mut processed = 0;
                while processed < STARTUP_COVER_WARM_BATCH_SIZE.min(capacity) {
                    let Some(job) = jobs.borrow_mut().pop_front() else {
                        break;
                    };
                    processed += 1;
                    if shell
                        .decoded_cover_for_ref(&job.image_ref, job.fetch_size)
                        .is_some()
                        || shell.state.cover_decodes.borrow().contains(&job.key)
                    {
                        continue;
                    }
                    if let Some(path) = shell
                        .controller
                        .cached_cover_path(&job.image_ref, job.fetch_size)
                    {
                        shell.start_cover_decode_from_path(
                            job.key,
                            path,
                            job.size,
                            CoverDecodePriority::Warm,
                        );
                    }
                }

                if jobs.borrow().is_empty() {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
    }

    fn next_cover_warm_generation(&self) -> u64 {
        let generation = self.state.cover_warm_generation.get().saturating_add(1);
        self.state.cover_warm_generation.set(generation);
        generation
    }

    fn cancel_cover_warm(&self) {
        self.state
            .cover_warm_generation
            .set(self.state.cover_warm_generation.get().saturating_add(1));
    }

    fn schedule_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(COVER_WARM_INITIAL_DELAY_MS),
            move || {
                if shell.state.cover_warm_generation.get() == generation {
                    shell.start_cover_warm_jobs(jobs, fetch_size, size, generation);
                }
            },
        );
    }

    fn start_cover_warm_jobs(
        self: &Rc<Self>,
        jobs: Rc<RefCell<VecDeque<(String, ImageRef)>>>,
        fetch_size: u32,
        size: i32,
        generation: u64,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(COVER_WARM_INTERVAL_MS), move || {
            if shell.state.cover_warm_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            if jobs.borrow().is_empty() {
                return glib::ControlFlow::Break;
            }

            let in_flight = shell.state.cover_decodes.borrow().len();
            if in_flight >= COVER_WARM_MAX_IN_FLIGHT {
                return glib::ControlFlow::Continue;
            }

            let capacity = COVER_WARM_MAX_IN_FLIGHT.saturating_sub(in_flight);
            let mut processed = 0;
            while processed < COVER_WARM_BATCH_SIZE.min(capacity) {
                let Some((key, image_ref)) = jobs.borrow_mut().pop_front() else {
                    break;
                };
                processed += 1;
                if shell.state.decoded_covers.borrow().contains_key(&key)
                    || shell.state.cover_decodes.borrow().contains(&key)
                {
                    continue;
                }
                if let Some(path) = shell.controller.cached_cover_path(&image_ref, fetch_size) {
                    shell.start_cover_decode_from_path(key, path, size, CoverDecodePriority::Warm);
                }
            }

            if jobs.borrow().is_empty() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
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

    fn decoded_cover_for_ref(
        &self,
        image_ref: &ImageRef,
        preferred_size: u32,
    ) -> Option<(String, Pixbuf)> {
        for size in decoded_cover_candidate_sizes(preferred_size) {
            let Some(key) = self.cover_cache_key(image_ref, size) else {
                continue;
            };
            if let Some(pixbuf) = self.state.decoded_covers.borrow().get(&key).cloned() {
                return Some((key, pixbuf));
            }
        }
        None
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
        self.start_cover_decode_from_path(
            key.to_string(),
            path.to_path_buf(),
            size,
            CoverDecodePriority::Visible,
        );
    }

    fn start_cover_decode_from_path(
        self: &Rc<Self>,
        key: String,
        path: PathBuf,
        size: i32,
        priority: CoverDecodePriority,
    ) {
        if self.apply_decoded_cover_if_available(&key) {
            return;
        }

        if self.state.cover_decodes.borrow().contains(&key) {
            return;
        }

        {
            let mut queue = self.state.cover_decode_queue.borrow_mut();
            if let Some(position) = queue.iter().position(|job| job.key == key) {
                let Some(mut job) = queue.remove(position) else {
                    return;
                };
                job.size = job.size.max(size);
                job.priority = if job.priority == CoverDecodePriority::Visible
                    || priority == CoverDecodePriority::Visible
                {
                    CoverDecodePriority::Visible
                } else {
                    CoverDecodePriority::Warm
                };
                if job.priority == CoverDecodePriority::Visible {
                    queue.push_front(job);
                } else {
                    queue.push_back(job);
                }
                drop(queue);
                self.drain_cover_decode_queue();
                return;
            }

            let job = CoverDecodeJob {
                key,
                path,
                size,
                priority,
            };
            if priority == CoverDecodePriority::Visible {
                queue.push_front(job);
            } else {
                queue.push_back(job);
            }
        }

        self.drain_cover_decode_queue();
    }

    fn apply_decoded_cover_if_available(&self, key: &str) -> bool {
        let Some(pixbuf) = self.state.decoded_covers.borrow().get(key).cloned() else {
            return false;
        };
        let bindings = self.take_live_cover_bindings(key);
        apply_pixbuf_to_bindings(bindings, pixbuf);
        true
    }

    fn drain_cover_decode_queue(self: &Rc<Self>) {
        loop {
            if self.state.cover_decodes.borrow().len() >= COVER_DECODE_MAX_IN_FLIGHT {
                break;
            }
            let Some(job) = self.state.cover_decode_queue.borrow_mut().pop_front() else {
                break;
            };
            if self.apply_decoded_cover_if_available(&job.key) {
                continue;
            }
            if !self
                .state
                .cover_decodes
                .borrow_mut()
                .insert(job.key.clone())
            {
                continue;
            }
            self.spawn_cover_decode_job(job);
        }
    }

    fn spawn_cover_decode_job(self: &Rc<Self>, job: CoverDecodeJob) {
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let CoverDecodeJob {
                key,
                path,
                size,
                priority,
            } = job;
            match load_cover_pixbuf(path.clone(), size, priority.glib_priority()).await {
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
            shell.drain_cover_decode_queue();
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
    connect_layout_resize(shell);
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

fn connect_layout_resize(shell: &Rc<Shell>) {
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            resize_shell.update_layout();
            resize_shell.queue_responsive_route_render();
        });

    let window = shell.window.clone();
    let resize_shell = Rc::clone(shell);
    window.connect_realize(move |window| {
        if let Some(surface) = window.surface() {
            let surface_resize_shell = Rc::clone(&resize_shell);
            surface.connect_width_notify(move |_| {
                surface_resize_shell.update_layout();
                surface_resize_shell.queue_responsive_route_render();
            });
        }
        resize_shell.update_layout();
        resize_shell.queue_responsive_route_render();
    });

    let route_shell = Rc::clone(shell);
    shell
        .route_host
        .connect_notify_local(Some("width"), move |_, _| {
            route_shell.queue_responsive_route_render();
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
        .comments(tr(
            "Thank you for trying out Rufin! If you have problems or suggestions, please open an issue in Github.",
        ))
        .build();
    dialog.add_link(&tr("Website"), "https://github.com/screwys/Rufin");
    dialog.add_link(&tr("Issues"), "https://github.com/screwys/Rufin/issues");
    dialog.present(Some(&shell.window));
}

fn schedule_startup_sync(shell: &Rc<Shell>) {
    let Some(delay_ms) = shell.controller.startup_sync_delay_ms() else {
        return;
    };

    let shell = Rc::clone(&shell);
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
        debug!(delay_ms, "starting deferred background sync");
        shell.controller.start_background_sync_for_active();
    });
}

fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(&shell);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        shell.controller.poll_playback_events();
        while let Ok(event) = receiver.try_recv() {
            match event {
                ControllerEvent::Snapshot(snapshot) => {
                    let entering_first_run =
                        snapshot.first_run && !shell.state.library.borrow().first_run;
                    let finishing_first_run_connection =
                        shell.state.first_run_connection_pending.get()
                            && shell.state.first_run_connection_ready.get()
                            && !snapshot.first_run;
                    let source_changed =
                        shell.state.library.borrow().selected_source != snapshot.selected_source;
                    let server_id = snapshot.server.as_ref().map(|server| server.id.clone());
                    let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
                    let sections = snapshot.home_sections.clone();
                    *shell.state.library.borrow_mut() = *snapshot;
                    if entering_first_run {
                        shell.state.server_discovery_started.set(false);
                        shell.state.server_discovery_running.set(false);
                        *shell.state.discovered_servers.borrow_mut() = Vec::new();
                        *shell.state.server_discovery_status.borrow_mut() =
                            "Searching will start automatically".to_string();
                    }
                    shell.update_prefetched_explore_from_snapshot(
                        server_id,
                        prefetched_explore,
                        &sections,
                    );
                    *shell.state.folder_state.borrow_mut() = FolderRouteState::default();
                    shell.update_server_selector();
                    if finishing_first_run_connection {
                        shell.log_layout_snapshot("first_run_final_snapshot");
                        shell.schedule_first_run_app_reveal();
                        continue;
                    }
                    if source_changed {
                        shell.navigate(Route::Home);
                    } else {
                        shell.render_current_route_preserving_scroll();
                    }
                    shell.schedule_startup_cover_warm();
                }
                ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore,
                } => {
                    let server_id = snapshot.server.as_ref().map(|server| server.id.clone());
                    let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
                    let snapshot = *snapshot;
                    let sections = snapshot.home_sections.clone();
                    *shell.state.library.borrow_mut() = snapshot;
                    shell.update_prefetched_explore_from_snapshot(
                        server_id,
                        prefetched_explore,
                        &sections,
                    );
                    if !include_explore {
                        shell.promote_cached_prefetched_explore();
                    }
                    shell.update_server_selector();
                    shell.refresh_visible_home_sections(&sections, include_explore);
                    shell.schedule_startup_cover_warm();
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
                ControllerEvent::PlaylistChanged {
                    playlist_id,
                    snapshot,
                } => {
                    *shell.state.library.borrow_mut() = *snapshot;
                    shell.update_server_selector();
                    let route = shell.state.routes.borrow().current().clone();
                    let playlist_route_changed = matches!(route, Route::Playlists)
                        || matches!(route, Route::PlaylistDetail(id) if id == playlist_id);
                    if playlist_route_changed {
                        shell.render_current_route_preserving_scroll();
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
                    shell.maybe_clear_player_seek_preview(
                        &next_snapshot,
                        previous_track != next_track,
                    );
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
                    #[cfg(unix)]
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
                ControllerEvent::FolderLoaded {
                    request_id,
                    path,
                    detail,
                } => {
                    shell.apply_folder_loaded(request_id, path, detail);
                }
                ControllerEvent::FolderLoadFailed {
                    request_id,
                    path,
                    error,
                } => {
                    shell.apply_folder_load_failed(request_id, path, error);
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
                    if status == "Library sync complete" {
                        shell.state.first_run_connection_ready.set(true);
                    }
                    let should_render = {
                        let mut library = shell.state.library.borrow_mut();
                        library.sync_status = status;
                        route_displays_sync_status(
                            shell.state.routes.borrow().current(),
                            library.first_run,
                        ) || shell.state.first_run_connection_pending.get()
                    };
                    if should_render {
                        shell.render_current_route();
                    }
                }
                ControllerEvent::Error(error) => {
                    warn!(%error, "controller error");
                    shell.state.first_run_connection_pending.set(false);
                    shell.state.first_run_connection_ready.set(false);
                    let mut library = shell.state.library.borrow_mut();
                    library.sync_status = "Action failed".to_string();
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

fn start_ui_perf_observe(shell: &Rc<Shell>, app: &adw::Application) {
    let Some(perf) = shell.state.perf.clone() else {
        return;
    };
    info!(
        max_gap_ms = perf.options.max_gap_ms,
        asset_ms = perf.options.asset_ms,
        output = perf
            .options
            .output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "stdout_only".to_string()),
        "manual UI perf observer started"
    );
    let heartbeat = Rc::new(RefCell::new(Some(start_ui_perf_heartbeat(Rc::clone(
        &perf,
    )))));
    let heartbeat_for_shutdown = Rc::clone(&heartbeat);
    app.connect_shutdown(move |_| {
        if let Some(source) = heartbeat_for_shutdown.borrow_mut().take() {
            source.remove();
        }
        perf.finish_scroll();
        let failed = write_ui_perf_report(&perf, false);
        info!(
            failed,
            output = perf
                .options
                .output
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "stdout_only".to_string()),
            "manual UI perf observer stopped"
        );
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
    let failed = write_ui_perf_report(&perf, true);
    app.quit();
    if failed {
        std::process::exit(1);
    }
}

fn write_ui_perf_report(perf: &UiPerfMonitor, print_stdout: bool) -> bool {
    let report = perf.report();
    if print_stdout {
        print!("{report}");
    }
    if let Some(path) = &perf.options.output {
        match std::fs::write(path, &report) {
            Ok(()) => info!(path = %path.display(), "wrote UI perf report"),
            Err(error) => eprintln!(
                "RUFIN_PERF failed_to_write_report path={} error={error}",
                path.display()
            ),
        }
    } else if !print_stdout {
        print!("{report}");
    }
    perf.failed()
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

fn route_boundary(view: gtk::Widget) -> gtk::Widget {
    let spec = route_boundary_spec();
    let scroller = gtk::ScrolledWindow::new();
    // this is necessary because route pages can contain tables, grids, and
    // toolbars wider than the visible pane. they may scroll inside the pane,
    // but they must never draw under the right sidebar.
    scroller.set_policy(spec.horizontal_policy, spec.vertical_policy);
    scroller.set_overflow(spec.overflow);
    scroller.set_min_content_width(spec.min_content_width);
    scroller.set_propagate_natural_width(spec.propagate_natural_width);
    scroller.set_hexpand(spec.hexpand);
    scroller.set_vexpand(spec.vexpand);
    scroller.set_child(Some(&view));
    scroller.upcast()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RouteBoundarySpec {
    horizontal_policy: gtk::PolicyType,
    vertical_policy: gtk::PolicyType,
    overflow: gtk::Overflow,
    min_content_width: i32,
    propagate_natural_width: bool,
    hexpand: bool,
    vexpand: bool,
}

fn route_boundary_spec() -> RouteBoundarySpec {
    RouteBoundarySpec {
        horizontal_policy: gtk::PolicyType::Automatic,
        vertical_policy: gtk::PolicyType::Never,
        overflow: gtk::Overflow::Hidden,
        min_content_width: 0,
        propagate_natural_width: false,
        hexpand: true,
        vexpand: true,
    }
}

fn route_displays_sync_status(_route: &Route, first_run: bool) -> bool {
    first_run
}

fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}

fn next_home_showcase_seed() -> u64 {
    let counter = HOME_SHOWCASE_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let time_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_else(|_| stable_seed("home-showcase") as u64);
    time_seed.rotate_left(17) ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

fn add_album_seed_gradient_class(widget: &impl IsA<gtk::Widget>, seed: u32) {
    let class_name = format!("album-seed-gradient-{:08x}", seed);
    widget.add_css_class(&class_name);

    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let (red, green, blue) = showcase_seed_rgb(seed);
    let (red_two, green_two, blue_two) = showcase_seed_rgb(seed.rotate_left(11) ^ 0x5bd1_e995);
    let (red_three, green_three, blue_three) =
        showcase_seed_rgb(seed.rotate_right(7) ^ 0x9e37_79b9);
    let css = format!(
        ".{class_name} {{
            background: linear-gradient(135deg,
                color-mix(in srgb, rgba({red}, {green}, {blue}, 0.78) 58%, @window_bg_color),
                color-mix(in srgb, rgba({red_two}, {green_two}, {blue_two}, 0.64) 44%, @card_bg_color) 58%,
                color-mix(in srgb, @window_bg_color 62%, rgba({red_three}, {green_three}, {blue_three}, 0.56)));
        }}"
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn showcase_seed_rgb(seed: u32) -> (u8, u8, u8) {
    (
        showcase_color_component(seed, 0),
        showcase_color_component(seed, 8),
        showcase_color_component(seed, 16),
    )
}

fn showcase_color_component(seed: u32, shift: u8) -> u8 {
    let value = ((seed >> shift) & 0xff) as f64;
    (value * 0.72 + 48.0).round().clamp(0.0, 232.0) as u8
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
        let track = boxed.borrow::<Track>().clone();
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
        install_track_context_menu(&row, &shell, track);
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
        let track = boxed.borrow::<Track>().clone();
        let (text, route) = value(&track);
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("table-link-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_width_chars(1);
        label.set_max_width_chars((width / 8).clamp(8, 32));

        let Some(route) = route else {
            install_track_context_menu(&label, &shell, track);
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
        install_track_context_menu(&button, &shell, track);

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

fn install_track_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, track: Track) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_track = track.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_track_context_menu(
            &target,
            &click_shell,
            context_track(&click_shell, &click_track),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_track = track;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_track_context_menu(
                &target,
                &key_shell,
                context_track(&key_shell, &key_track),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}

fn install_album_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, album: Album) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_album = album.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_album_context_menu(
            &target,
            &click_shell,
            context_album(&click_shell, &click_album),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_album = album;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_album_context_menu(
                &target,
                &key_shell,
                context_album(&key_shell, &key_album),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}

fn install_artist_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>, artist: Artist) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click_artist = artist.clone();
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        present_artist_context_menu(
            &target,
            &click_shell,
            context_artist(&click_shell, &click_artist),
            Some((x, y)),
        );
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key_artist = artist;
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade() {
            present_artist_context_menu(
                &target,
                &key_shell,
                context_artist(&key_shell, &key_artist),
                None,
            );
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}

fn install_current_track_context_menu(target: &impl IsA<gtk::Widget>, shell: &Rc<Shell>) {
    let target = target.as_ref();
    let target_weak = target.downgrade();
    let click_shell = Rc::clone(shell);
    let click = gtk::GestureClick::new();
    click.set_button(3);
    click.connect_pressed(move |_, _, x, y| {
        let Some(target) = target_weak.upgrade() else {
            return;
        };
        if let Some(track) = current_player_track(&click_shell) {
            present_track_context_menu(&target, &click_shell, track, Some((x, y)));
        }
    });
    target.add_controller(click);

    let target_weak = target.downgrade();
    let key_shell = Rc::clone(shell);
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, state| {
        let opens_menu = key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK));
        if !opens_menu {
            return glib::Propagation::Proceed;
        }
        if let Some(target) = target_weak.upgrade()
            && let Some(track) = current_player_track(&key_shell)
        {
            present_track_context_menu(&target, &key_shell, track, None);
        }
        glib::Propagation::Stop
    });
    target.add_controller(key);
}

fn present_track_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "track.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "track.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item("Play Later", "track.play-last", PLAY_LATER_ICON));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("track.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if track.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("track.favorite"),
    );
    menu.append(Some(&tr("Go to Album")), Some("track.go-album"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("track-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_now(action_track.clone());
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_next(action_track.clone());
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_last(vec![action_track.clone()]);
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let action_track = track.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            controller.add_tracks_to_playlist(playlist_id.clone(), vec![action_track.clone()]);
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let track_id = track.id.clone();
    let favorite = !track.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_track_favorite(track_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    let go_album = gio::SimpleAction::new("go-album", None);
    let shell = Rc::clone(shell);
    let album_id = track.album_id.clone();
    let action_popover = popover.downgrade();
    go_album.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        shell.navigate(Route::AlbumDetail(album_id.clone()));
    });
    actions.add_action(&go_album);

    target.insert_action_group("track", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}

fn present_album_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: Album,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "album.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "album.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item("Play Later", "album.play-last", PLAY_LATER_ICON));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("album.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if album.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("album.favorite"),
    );
    menu.append(Some(&tr("Go to Album")), Some("album.go-album"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("album-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_album_now(album_id.clone());
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            controller.play_last(tracks);
        }
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let album_id = album.id.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                controller.add_tracks_to_playlist(playlist_id.clone(), tracks);
            }
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let favorite = !album.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_album_favorite(album_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    let go_album = gio::SimpleAction::new("go-album", None);
    let shell = Rc::clone(shell);
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    go_album.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        shell.navigate(Route::AlbumDetail(album_id.clone()));
    });
    actions.add_action(&go_album);

    target.insert_action_group("album", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}

fn present_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: Artist,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "artist.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "artist.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item(
        "Play Later",
        "artist.play-last",
        PLAY_LATER_ICON,
    ));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("artist.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if artist.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("artist.favorite"),
    );
    menu.append(Some(&tr("Go to Artist")), Some("artist.go-artist"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("artist-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
            controller.play_tracks_now(tracks);
        }
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
            controller.play_last(tracks);
        }
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let artist_id = artist.id.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
                controller.add_tracks_to_playlist(playlist_id.clone(), tracks);
            }
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let favorite = !artist.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_artist_favorite(artist_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    let go_artist = gio::SimpleAction::new("go-artist", None);
    let shell = Rc::clone(shell);
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    go_artist.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        shell.navigate(Route::ArtistDetail(artist_id.clone()));
    });
    actions.add_action(&go_artist);

    target.insert_action_group("artist", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}

fn artist_tracks_for_context(
    controller: &AppController,
    artist_id: &ArtistId,
) -> Option<Vec<Track>> {
    controller
        .cached_artist_detail(artist_id)
        .ok()
        .flatten()
        .map(|detail| detail.tracks)
        .filter(|tracks| !tracks.is_empty())
}

fn menu_item(label: &str, action: &str, icon_name: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(&tr(label)), Some(action));
    item.set_icon(&gio::ThemedIcon::new(icon_name));
    item
}

fn context_menu_playlists(shell: &Rc<Shell>) -> Vec<Playlist> {
    shell
        .controller
        .cached_playlists_page(0, CONTEXT_MENU_PLAYLIST_LIMIT)
        .map(|page| page.items)
        .unwrap_or_else(|_| shell.state.library.borrow().playlists.clone())
}

fn context_track(shell: &Rc<Shell>, fallback: &Track) -> Track {
    shell
        .controller
        .cached_track(&fallback.id)
        .ok()
        .flatten()
        .or_else(|| {
            let library = shell.state.library.borrow();
            library_track(&library, &fallback.id)
        })
        .unwrap_or_else(|| fallback.clone())
}

fn library_track(library: &LibrarySnapshot, track_id: &TrackId) -> Option<Track> {
    library
        .tracks
        .iter()
        .chain(library.favorites.iter())
        .chain(library.search.tracks.iter())
        .chain(
            library
                .home_sections
                .iter()
                .flat_map(|section| section.tracks.iter()),
        )
        .find(|track| track.id == *track_id)
        .cloned()
}

fn context_album(shell: &Rc<Shell>, fallback: &Album) -> Album {
    {
        let library = shell.state.library.borrow();
        library_album(&library, &fallback.id)
    }
    .unwrap_or_else(|| fallback.clone())
}

fn context_artist(shell: &Rc<Shell>, fallback: &Artist) -> Artist {
    {
        let library = shell.state.library.borrow();
        library_artist(&library, &fallback.id)
    }
    .unwrap_or_else(|| fallback.clone())
}

fn library_album(library: &LibrarySnapshot, album_id: &AlbumId) -> Option<Album> {
    library
        .albums
        .iter()
        .chain(library.search.albums.iter())
        .chain(
            library
                .home_sections
                .iter()
                .flat_map(|section| section.albums.iter()),
        )
        .find(|album| album.id == *album_id)
        .cloned()
}

fn library_artist(library: &LibrarySnapshot, artist_id: &ArtistId) -> Option<Artist> {
    library
        .artists
        .iter()
        .chain(library.album_artists.iter())
        .chain(library.search.artists.iter())
        .find(|artist| artist.id == *artist_id)
        .cloned()
}

fn current_player_track(shell: &Rc<Shell>) -> Option<Track> {
    let entry = shell.state.player.borrow().current.clone()?;
    shell
        .controller
        .cached_track(&entry.track_id)
        .ok()
        .flatten()
        .or_else(|| track_from_queue_entry(&entry))
}

fn track_from_queue_entry(entry: &QueueEntry) -> Option<Track> {
    Some(Track {
        id: entry.track_id.clone(),
        album_id: entry.album_id.clone()?,
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        artist_id: entry.artist_id.clone(),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: entry.album.clone(),
        year: entry.year,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: entry.duration_seconds,
        favorite: entry.favorite,
        disc_number: 0,
        track_number: 0,
        image_ref: entry.image_ref.clone(),
        genres: Vec::new(),
        local_path: None,
    })
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
        install_track_context_menu(&button, &shell, track.clone());
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
        Self::new_sized(size, size, seed)
    }

    fn new_sized(width: i32, height: i32, seed: u32) -> Self {
        let area = gtk::DrawingArea::new();
        area.add_css_class("cover-tile");
        area.add_css_class("card");
        area.set_content_width(width);
        area.set_content_height(height);
        area.set_width_request(width);
        area.set_height_request(height);
        area.set_size_request(width, height);
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
            size: width.max(height),
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

    fn advance_generation(&self) {
        self.generation.set(self.generation.get().saturating_add(1));
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
        self.advance_generation();
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

async fn load_cover_pixbuf(
    path: PathBuf,
    size: i32,
    priority: glib::Priority,
) -> Result<Pixbuf, glib::Error> {
    let file = gio::File::for_path(path);
    let stream = file.read_future(priority).await?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaylistEntrySort {
    Order,
    Title,
    Artist,
    Album,
    Duration,
}

impl PlaylistEntrySort {
    fn title(self) -> &'static str {
        match self {
            Self::Order => "Playlist order",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Duration => "Duration",
        }
    }
}

const PLAYLIST_ENTRY_SORTS: [PlaylistEntrySort; 5] = [
    PlaylistEntrySort::Order,
    PlaylistEntrySort::Title,
    PlaylistEntrySort::Artist,
    PlaylistEntrySort::Album,
    PlaylistEntrySort::Duration,
];

#[derive(Clone, Debug)]
struct PlaylistEntryListState {
    query: String,
    sort: PlaylistEntrySort,
    descending: bool,
}

impl Default for PlaylistEntryListState {
    fn default() -> Self {
        Self {
            query: String::new(),
            sort: PlaylistEntrySort::Order,
            descending: false,
        }
    }
}

fn rebuild_playlist_entries_list(
    shell: &Rc<Shell>,
    list: &gtk::ListBox,
    entries: &Rc<Vec<PlaylistEntry>>,
    state: &PlaylistEntryListState,
    playlist_id: &PlaylistId,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let rows = playlist_entries_for_state(entries, state);
    if rows.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No tracks match the search.")));
        empty.add_css_class("muted");
        empty.set_margin_top(16);
        empty.set_margin_bottom(16);
        list.append(&empty);
        return;
    }

    for (display_index, (original_index, entry)) in rows.into_iter().enumerate() {
        list.append(&playlist_entry_row(
            shell,
            Rc::clone(entries),
            playlist_id,
            original_index,
            display_index,
            &entry,
        ));
    }
}

fn playlist_entries_for_state(
    entries: &[PlaylistEntry],
    state: &PlaylistEntryListState,
) -> Vec<(usize, PlaylistEntry)> {
    let query = state.query.trim().to_lowercase();
    let mut rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query.is_empty() || playlist_entry_matches_query(entry, &query))
        .map(|(index, entry)| (index, entry.clone()))
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        let ordering = compare_playlist_entry(left, right, state.sort);
        if state.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

fn playlist_entry_matches_query(entry: &PlaylistEntry, query: &str) -> bool {
    entry.track.title.to_lowercase().contains(query)
        || entry.track.artist.to_lowercase().contains(query)
        || entry.track.album.to_lowercase().contains(query)
}

fn compare_playlist_entry(
    left: &(usize, PlaylistEntry),
    right: &(usize, PlaylistEntry),
    sort: PlaylistEntrySort,
) -> std::cmp::Ordering {
    match sort {
        PlaylistEntrySort::Order => left.0.cmp(&right.0),
        PlaylistEntrySort::Title => cmp_text(&left.1.track.title, &right.1.track.title),
        PlaylistEntrySort::Artist => cmp_text(&left.1.track.artist, &right.1.track.artist),
        PlaylistEntrySort::Album => cmp_text(&left.1.track.album, &right.1.track.album),
        PlaylistEntrySort::Duration => left
            .1
            .track
            .duration_seconds
            .cmp(&right.1.track.duration_seconds),
    }
    .then_with(|| left.0.cmp(&right.0))
}

fn cmp_text(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

fn playlist_entries_header_row() -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    row.add_css_class("playlist-entry-header");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Center);
    row.append(&fixed_spacer(PLAYLIST_ENTRY_DRAG_WIDTH));
    row.append(&playlist_header_label(
        "#",
        PLAYLIST_ENTRY_NUMBER_WIDTH,
        false,
        PLAYLIST_ENTRY_NUMBER_XALIGN,
    ));
    row.append(&playlist_text_columns(
        playlist_header_text_label("Title", PLAYLIST_ENTRY_TITLE_MAX_CHARS).upcast(),
        playlist_header_album_label("Album", PLAYLIST_ENTRY_ALBUM_MAX_CHARS).upcast(),
    ));
    row.append(&playlist_header_label(
        "Duration",
        PLAYLIST_ENTRY_DURATION_WIDTH,
        false,
        0.5,
    ));
    row.append(&fixed_spacer(PLAYLIST_ENTRY_REMOVE_WIDTH));
    row.upcast()
}

fn playlist_header_label(text: &str, width: i32, expand: bool, xalign: f32) -> gtk::Label {
    let label = gtk::Label::new(Some(&tr(text)));
    label.add_css_class("muted");
    label.set_xalign(xalign);
    label.set_width_request(width);
    label.set_hexpand(expand);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if expand {
        label.set_width_chars(1);
        label.set_max_width_chars(PLAYLIST_ENTRY_TITLE_MAX_CHARS);
    }
    label
}

fn playlist_header_text_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(&tr(text)));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Fill);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn playlist_header_album_label(text: &str, max_width_chars: i32) -> gtk::Label {
    let label = playlist_header_text_label(text, max_width_chars);
    label.set_xalign(0.5);
    label
}

fn playlist_text_columns(title: gtk::Widget, album: gtk::Widget) -> gtk::Widget {
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_TEXT_COLUMN_GAP);
    columns.set_homogeneous(false);
    columns.set_hexpand(true);
    columns.set_halign(gtk::Align::Fill);
    columns.set_width_request(1);

    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    columns.append(&title);

    album.set_hexpand(false);
    album.set_halign(gtk::Align::Fill);
    album.set_width_request(PLAYLIST_ENTRY_ALBUM_COLUMN_WIDTH);
    columns.append(&album);

    columns.upcast()
}

fn playlist_title_cell(cover: gtk::Widget, labels: gtk::Widget) -> gtk::Widget {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.append(&cover);
    title.append(&labels);
    title.upcast()
}

fn playlist_entry_row(
    shell: &Rc<Shell>,
    entries: Rc<Vec<PlaylistEntry>>,
    playlist_id: &PlaylistId,
    original_index: usize,
    display_index: usize,
    entry: &PlaylistEntry,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PLAYLIST_ENTRY_COLUMN_GAP);
    row.add_css_class("playlist-entry-row");
    row.set_focusable(true);
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);
    row.set_valign(gtk::Align::Center);

    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    drag.set_width_request(PLAYLIST_ENTRY_DRAG_WIDTH);
    drag.set_halign(gtk::Align::Center);
    let drag_source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let drag_entry_id = entry.entry_id.clone();
    drag_source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(
            &drag_entry_id.to_value(),
        ))
    });
    drag.add_controller(drag_source);
    row.append(&drag);

    let number = gtk::Label::new(Some(&(display_index + 1).to_string()));
    number.add_css_class("muted");
    number.set_xalign(PLAYLIST_ENTRY_NUMBER_XALIGN);
    number.set_width_request(PLAYLIST_ENTRY_NUMBER_WIDTH);
    row.append(&number);

    let cover = shell.cover_tile_for(
        entry.track.image_ref.as_ref(),
        stable_seed(entry.track.id.as_str()),
        PLAYLIST_ENTRY_COVER_WIDTH,
        THUMB_COVER_SIZE,
    );

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_halign(gtk::Align::Fill);
    labels.set_width_request(1);
    labels.append(&playlist_entry_text_label(
        &entry.track.title,
        "",
        PLAYLIST_ENTRY_TITLE_MAX_CHARS,
    ));
    labels.append(&playlist_entry_text_label(
        &entry.track.artist,
        "muted",
        PLAYLIST_ENTRY_TITLE_MAX_CHARS,
    ));

    let album =
        playlist_entry_text_label(&entry.track.album, "muted", PLAYLIST_ENTRY_ALBUM_MAX_CHARS);
    album.set_xalign(0.5);
    album.set_valign(gtk::Align::Center);
    row.append(&playlist_text_columns(
        playlist_title_cell(cover, labels.upcast()),
        album.upcast(),
    ));

    let duration = gtk::Label::new(Some(&format_duration(entry.track.duration_seconds)));
    duration.add_css_class("muted");
    duration.set_xalign(0.5);
    duration.set_width_request(PLAYLIST_ENTRY_DURATION_WIDTH);
    row.append(&duration);

    let remove = gtk::Button::with_label("x");
    remove.add_css_class("icon-button");
    remove.add_css_class("flat");
    remove.add_css_class("circular");
    remove.set_tooltip_text(Some(&tr("Remove from playlist")));
    remove.set_width_request(PLAYLIST_ENTRY_REMOVE_WIDTH);
    let remove_shell = Rc::clone(shell);
    let remove_playlist_id = playlist_id.clone();
    let remove_entry_id = entry.entry_id.clone();
    let remove_title = entry.track.title.clone();
    remove.connect_clicked(move |_| {
        confirm_remove_playlist_entry(
            &remove_shell,
            remove_playlist_id.clone(),
            remove_entry_id.clone(),
            remove_title.clone(),
        );
    });
    row.append(&remove);

    let controller = shell.controller.clone();
    let track = entry.track.clone();
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, n_press, _, _| {
        if n_press == 2 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            controller.play_now(track.clone());
        }
    });
    row.add_controller(click);
    install_track_context_menu(&row, shell, entry.track.clone());

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let controller = shell.controller.clone();
    let playlist_id = playlist_id.clone();
    let entries_for_drop = Rc::clone(&entries);
    let row_for_drop = row.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(entry_id) = value.get::<String>() else {
            return false;
        };
        let after = y > f64::from(row_for_drop.height()) / 2.0;
        let Some(new_index) =
            playlist_drop_index(&entries_for_drop, &entry_id, original_index, after)
        else {
            return false;
        };
        controller.move_playlist_entry(playlist_id.clone(), entry_id, new_index);
        true
    });
    row.add_controller(drop_target);

    row.upcast()
}

fn playlist_drop_index(
    entries: &[PlaylistEntry],
    dragged_entry_id: &str,
    target_index: usize,
    after: bool,
) -> Option<usize> {
    let source_index = entries
        .iter()
        .position(|entry| entry.entry_id == dragged_entry_id)?;
    let mut new_index = if after {
        target_index.saturating_add(1)
    } else {
        target_index
    };
    if source_index < new_index {
        new_index = new_index.saturating_sub(1);
    }
    (source_index != new_index).then_some(new_index)
}

fn playlist_entry_text_label(text: &str, css_class: &str, max_width_chars: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    if !css_class.is_empty() {
        label.add_css_class(css_class);
    }
    label.set_xalign(0.0);
    label.set_width_chars(1);
    label.set_max_width_chars(max_width_chars);
    label.set_wrap(false);
    label.set_single_line_mode(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn fixed_spacer(width: i32) -> gtk::Widget {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_width_request(width);
    spacer.upcast()
}

fn confirm_remove_playlist_entry(
    shell: &Rc<Shell>,
    playlist_id: PlaylistId,
    entry_id: String,
    title: String,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove from Playlist"))
        .body(format!("Remove \"{title}\" from this playlist?"))
        .build();
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("remove", &tr("Remove"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    let controller = shell.controller.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "remove" {
            controller.remove_playlist_entry(playlist_id.clone(), entry_id.clone());
        }
    });
    dialog.present(Some(&shell.window));
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
        AutoLyricsRequest, PlaylistEntryListState, PlaylistEntrySort,
        auto_lyrics_request_for_settings, auto_lyrics_skip_action_enabled,
        current_playback_track_id, playlist_drop_index, playlist_entries_for_state,
        seekbar_target_seconds,
    };
    use rufin_core::{
        Album, AlbumId, AppSettings, ArtistId, HomeSectionKind, QueueEntry, QueueEntryId, Route,
        SearchKind, Track, TrackId, TrackSortKey, TrackTableSettings,
    };
    use rufin_provider::{LyricLine, Lyrics, LyricsSource, PlaylistEntry};
    use std::collections::HashMap;

    #[test]
    fn home_section_pages_reset_for_new_home_data() {
        let mut states = HashMap::from([(
            HomeSectionKind::Explore,
            super::HomeSectionState {
                page_start: 6,
                page_size: 3,
            },
        )]);

        super::reset_home_section_pages(&mut states);

        assert!(states.is_empty());
    }

    #[test]
    fn manual_ui_perf_observer_records_scrolls_by_route() {
        let monitor = super::UiPerfMonitor::new(super::UiPerfOptions {
            max_gap_ms: 120,
            route_ms: 650,
            duration_ms: 15_000,
            asset_ms: 300,
            require_assets: false,
            terminal_events: false,
            observe_scroll: true,
            output: None,
        });

        monitor.record_manual_scroll_step("Tracks", 10.0, 100.0);
        monitor.record_manual_scroll_step("Tracks", 40.0, 100.0);
        monitor.record_manual_scroll_step("Albums", 5.0, 50.0);
        monitor.finish_scroll();

        let report = monitor.report();
        assert!(report.contains("RUFIN_PERF_SCROLL route=Tracks scenario=manual"));
        assert!(report.contains("steps=2"));
        assert!(report.contains("max_adjustment=100"));
        assert!(report.contains("RUFIN_PERF_SCROLL route=Albums scenario=manual"));
    }

    #[test]
    fn queue_lyrics_position_clamps_to_available_height() {
        assert_eq!(clamp_queue_lyrics_position(800, 1701), 500);
        assert_eq!(clamp_queue_lyrics_position(800, 10), 120);
        assert_eq!(clamp_queue_lyrics_position(200, 1701), 120);
        assert_eq!(queue_lyrics_default_position(700), 400);
        assert_eq!(queue_lyrics_default_position(1400), 1000);
        assert_eq!(queue_lyrics_initial_position(700, None), 400);
        assert_eq!(queue_lyrics_initial_position(700, Some(0.5)), 350);
        assert_eq!(queue_lyrics_initial_position(700, Some(2.0)), 400);
        assert_eq!(queue_lyrics_initial_position(700, Some(f64::NAN)), 400);
        assert_eq!(queue_lyrics_position_from_ratio(700, 0.5), 350);
        assert_eq!(queue_lyrics_position_ratio(700, 350), 0.5);
        let saved_default_ratio = queue_lyrics_position_ratio(700, 400);
        assert_eq!(
            queue_lyrics_initial_position(1400, Some(saved_default_ratio)),
            800
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
    fn playlist_entry_search_and_sort_use_track_fields() {
        let mut first = test_track("Artist B", None);
        first.title = "Alpha".to_string();
        first.album = "Plain Album".to_string();
        first.duration_seconds = 240;
        let mut second = test_track("Artist A", None);
        second.id = TrackId::fake(2);
        second.title = "Beta".to_string();
        second.album = "Needle Album".to_string();
        second.duration_seconds = 120;
        let entries = vec![
            PlaylistEntry {
                entry_id: "entry-alpha".to_string(),
                track: first,
            },
            PlaylistEntry {
                entry_id: "entry-beta".to_string(),
                track: second,
            },
        ];

        let filtered = playlist_entries_for_state(
            &entries,
            &PlaylistEntryListState {
                query: "needle".to_string(),
                sort: PlaylistEntrySort::Order,
                descending: false,
            },
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.entry_id, "entry-beta");

        let sorted = playlist_entries_for_state(
            &entries,
            &PlaylistEntryListState {
                query: String::new(),
                sort: PlaylistEntrySort::Duration,
                descending: true,
            },
        );
        assert_eq!(sorted[0].1.entry_id, "entry-alpha");
        assert_eq!(sorted[1].1.entry_id, "entry-beta");
    }

    #[test]
    fn playlist_drop_index_accounts_for_removed_source_row() {
        let entries = ["a", "b", "c"]
            .into_iter()
            .enumerate()
            .map(|(index, entry_id)| {
                let mut track = test_track("Artist", None);
                track.id = TrackId::fake(index + 1);
                PlaylistEntry {
                    entry_id: entry_id.to_string(),
                    track,
                }
            })
            .collect::<Vec<_>>();

        assert_eq!(playlist_drop_index(&entries, "a", 2, false), Some(1));
        assert_eq!(playlist_drop_index(&entries, "a", 2, true), Some(2));
        assert_eq!(playlist_drop_index(&entries, "c", 0, false), Some(0));
        assert_eq!(playlist_drop_index(&entries, "b", 1, false), None);
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
    fn route_boundary_keeps_route_items_inside_main_pane() {
        let spec = super::route_boundary_spec();

        assert_eq!(spec.horizontal_policy, gtk::PolicyType::Automatic);
        assert_eq!(spec.vertical_policy, gtk::PolicyType::Never);
        assert_eq!(spec.overflow, gtk::Overflow::Hidden);
        assert_eq!(spec.min_content_width, 0);
        assert!(!spec.propagate_natural_width);
        assert!(spec.hexpand);
        assert!(spec.vexpand);
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
