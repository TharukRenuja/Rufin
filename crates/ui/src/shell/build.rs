use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use tracing::info;

use crate::favorites::FavoriteState;
use crate::interactions::connect_transient_entry_focus_dismissal;
use crate::player::desktop::DesktopState;
use crate::player::desktop::lifecycle::install_playback_shutdown;
use crate::player::lyrics::search::connect_lyrics_search_controls;
use crate::player::lyrics::state::LyricsState;
use crate::player::queue::QueueState;
use crate::player::right_panel::RightPanelWidgets;
use crate::player::state::PlaybackState;
use crate::player::{
    PlayerDesktopWidgets, apply_lyrics_panel_visibility, build_bottom_player,
    build_fullscreen_player, build_right_panel, connect_fullscreen_player_controls,
    connect_player_controls, connect_queue_lyrics_overlay, connect_queue_panel_controls,
    default_audio_output_options, warm_audio_output_cache,
};
#[cfg(unix)]
use crate::player::{install_tray, present_initial_window};
use crate::preferences::PreferencesState;
use crate::preferences::dialogs::release_notes::schedule_release_check;
use crate::preferences::source::SourceState;
use crate::routes::LibraryState;
use crate::routes::playlist_picker::PlaylistPickerState;
use crate::routes::route::Route;
use crate::runtime::RuntimeInputs;
use crate::runtime::WaveformProjection;
use crate::settings::SettingsState;
use localization::{effective_language_preference, set_language_preference, tr};
use lyrics::CurrentLyrics;

use super::Shell;
use super::actions::{ControlFeedbackState, connect_shell_actions};
use super::chrome::{
    WindowChrome, build_content_chrome, build_main_area, window_drag_handle_with_child,
};
use super::cover::ArtworkState;
use super::events::install_product_event_receivers;
use super::layout::{
    COMPACT_RAIL_WIDTH, MIN_APP_WINDOW_HEIGHT, MIN_APP_WINDOW_WIDTH, NORMAL_SIDEBAR_WIDTH,
    ShellLayoutState,
};
use super::localization::LocalizationState;
use super::navigation::{
    NavigationState, NavigationWidgets, PrimaryMenuWidgets, build_compact_navigation,
    build_normal_navigation,
};
use super::route::{RouteStack, RouteViewport};
use super::startup::StartupState;
use super::window_state::{initial_window_size, install_window_state_persistence};

fn sidebar_scroll_slot(width: i32, child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let slot = gtk::ScrolledWindow::new();
    slot.set_policy(gtk::PolicyType::Never, gtk::PolicyType::External);
    slot.set_width_request(width);
    slot.set_min_content_width(width);
    slot.set_max_content_width(width);
    slot.set_propagate_natural_width(false);
    slot.set_propagate_natural_height(false);
    slot.set_hexpand(false);
    slot.set_vexpand(true);
    slot.set_child(Some(child));
    slot
}

fn sidebar_resize_handle() -> gtk::Box {
    let handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    handle.add_css_class("sidebar-resize-handle");
    handle.set_width_request(8);
    handle.set_halign(gtk::Align::Start);
    handle.set_valign(gtk::Align::Fill);
    handle.set_vexpand(true);
    handle.set_focusable(false);
    handle.set_cursor_from_name(Some("col-resize"));
    let label = tr("Hold and drag to resize");
    handle.update_property(&[gtk::accessible::Property::Label(&label)]);
    handle
}

pub fn build(app: &adw::Application, inputs: RuntimeInputs) {
    crate::application::style::install_css();

    let loaded_at = std::time::Instant::now();
    let RuntimeInputs {
        diagnostics,
        products,
        settings: settings_handle,
        receivers,
        configured_sources,
        source_operation,
        release_notes,
    } = inputs;
    let settings = settings_handle.load();
    info!(
        first_run = configured_sources.first_run,
        elapsed_ms = loaded_at.elapsed().as_millis(),
        "loaded music source presentation"
    );
    let first_run = configured_sources.first_run;
    let defer_initial_route = !first_run;
    let language_preference = effective_language_preference(&settings.language);
    set_language_preference(&language_preference);
    let settings_state = SettingsState {
        current: RefCell::new(settings.clone()),
        persistence: settings_handle,
    };
    let navigation = NavigationState {
        routes: RefCell::new(RouteStack::new(Route::Home)),
    };
    let library_state = LibraryState {
        selected: RefCell::new(None),
    };
    let source = SourceState {
        configured: RefCell::new(configured_sources),
        operation: RefCell::new(source_operation),
        discovered_servers: RefCell::new(Vec::new()),
        discovery_status: RefCell::new(crate::runtime::source::DiscoveryStatus::Idle),
        discovery_running: Cell::new(false),
        discovery_started: Cell::new(false),
        add_server: RefCell::new(None),
        progress_toast: RefCell::new(None),
    };
    let startup = StartupState {
        route_revealed: Cell::new(!defer_initial_route),
        initial_launch: Cell::new(defer_initial_route),
        route_allocated: Cell::new(false),
        reveal_deadline: RefCell::new(None),
    };
    let playback_state = PlaybackState {
        player: RefCell::new(None),
        waveform: RefCell::new(WaveformProjection::default()),
        updating_controls: Cell::new(false),
        volume_persist_source: RefCell::new(None),
        seek_preview_seconds: Cell::new(None),
        seek_generation: Cell::new(0),
        audio_output_options: RefCell::new(default_audio_output_options()),
        audio_output_refresh_running: Cell::new(false),
        audio_output_refresh_generation: Cell::new(0),
        audio_output_refreshed_at: Cell::new(None),
    };
    let queue_state = QueueState::new(None);
    let lyrics_state = LyricsState {
        projection: RefCell::new(CurrentLyrics::Cleared),
        offset_millis: Cell::new(0),
        timing_generation: Cell::new(0),
        timing_source: RefCell::new(None),
        panel_visible: Cell::new(settings.lyrics_panel_visible),
        right_pane_dirty: Cell::new(true),
        fullscreen_pane_dirty: Cell::new(true),
        search_dialog: RefCell::new(None),
        settings_dialog: RefCell::new(None),
    };
    let preferences = PreferencesState {
        dialog: RefCell::new(None),
        release_notes: RefCell::new(release_notes),
    };
    let playlist_picker = PlaylistPickerState {
        active: RefCell::new(None),
    };
    let downloads = crate::downloads::DownloadsState::default();
    let control_feedback = ControlFeedbackState {
        generation: Rc::new(Cell::new(0)),
    };
    let localization = LocalizationState {
        bindings: RefCell::new(Vec::new()),
    };
    let desktop = DesktopState::new(app, products.playback.transport.clone());
    let artwork = ArtworkState {
        startup_prime: Default::default(),
        thumbnail_warm: Default::default(),
        live_bindings: RefCell::new(HashMap::new()),
        route_interaction: Rc::new(Default::default()),
        textures: RefCell::new(Default::default()),
    };
    let favorites = FavoriteState::default();

    let (window_width, window_height) =
        initial_window_size(settings.window_width, settings.window_height);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rufin")
        .default_width(window_width)
        .default_height(window_height)
        .build();

    let root_stack = gtk::Stack::new();
    root_stack.add_css_class("app-root");
    root_stack.set_width_request(MIN_APP_WINDOW_WIDTH);
    root_stack.set_height_request(MIN_APP_WINDOW_HEIGHT);
    root_stack.set_hhomogeneous(false);
    root_stack.set_vhomogeneous(false);
    root_stack.set_interpolate_size(false);
    root_stack.set_hexpand(true);
    root_stack.set_vexpand(true);

    let app_root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    app_root.add_css_class("app-root");
    app_root.set_hexpand(true);
    app_root.set_vexpand(true);

    let app_content_stack = gtk::Stack::new();
    app_content_stack.add_css_class("app-content-stack");
    app_content_stack.set_hhomogeneous(false);
    app_content_stack.set_vhomogeneous(false);
    app_content_stack.set_interpolate_size(false);
    app_content_stack.set_hexpand(true);
    app_content_stack.set_vexpand(true);

    let app_content_overlay = gtk::Overlay::new();
    app_content_overlay.set_hexpand(true);
    app_content_overlay.set_vexpand(true);

    let login_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    login_host.add_css_class("login-root");
    login_host.set_hexpand(true);
    login_host.set_vexpand(true);

    let startup_loading_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    startup_loading_host.add_css_class("startup-loading-root");
    startup_loading_host.set_hexpand(true);
    startup_loading_host.set_vexpand(true);
    startup_loading_host.set_visible(false);

    let upper = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    upper.set_hexpand(true);
    upper.set_vexpand(true);

    let normal_nav = gtk::Box::new(gtk::Orientation::Vertical, 4);
    normal_nav.add_css_class("wide-sidebar");
    normal_nav.set_hexpand(true);
    normal_nav.set_vexpand(true);
    normal_nav.set_width_request(1);
    let normal_nav_handle = window_drag_handle_with_child("sidebar-drag-handle", &normal_nav);
    normal_nav_handle.set_vexpand(true);
    normal_nav_handle.set_valign(gtk::Align::Fill);
    let normal_nav_slot = sidebar_scroll_slot(NORMAL_SIDEBAR_WIDTH, &normal_nav_handle);
    normal_nav_slot.set_width_request(1);
    normal_nav_slot.set_min_content_width(1);
    normal_nav_slot.set_max_content_width(-1);
    normal_nav_slot.add_css_class("sidebar-pane");
    normal_nav_slot.add_css_class("wide-sidebar-slot");

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 1);
    compact_nav.add_css_class("compact-rail");
    compact_nav.set_hexpand(false);
    compact_nav.set_vexpand(true);
    compact_nav.set_width_request(COMPACT_RAIL_WIDTH);
    let compact_nav_handle = window_drag_handle_with_child("sidebar-drag-handle", &compact_nav);
    compact_nav_handle.set_vexpand(true);
    compact_nav_handle.set_valign(gtk::Align::Fill);
    let compact_nav_slot = sidebar_scroll_slot(COMPACT_RAIL_WIDTH, &compact_nav_handle);
    compact_nav_slot.add_css_class("sidebar-pane");
    compact_nav_slot.add_css_class("compact-rail-slot");
    let normal_main_menu = gtk::Button::new();
    let compact_main_menu = gtk::Button::new();

    let main_area_parts = build_main_area();
    let main_area = main_area_parts.root;
    let route_host = main_area_parts.route_host;

    let right_panel_parts = build_right_panel();
    let right_panel = right_panel_parts.root;
    let queue_panel = right_panel_parts.queue_panel;
    let queue_search = right_panel_parts.queue_search;
    let queue_clear_button = right_panel_parts.queue_clear_button;
    let queue_lyrics_overlay = right_panel_parts.queue_lyrics_overlay;
    let lyrics_surface = right_panel_parts.lyrics_surface;
    let lyrics_resize_handle = right_panel_parts.lyrics_resize_handle;
    let lyrics_pane = right_panel_parts.lyrics_pane;

    let content_chrome = build_content_chrome(&main_area, &right_panel);
    let right_split = content_chrome.right_split;
    let right_panel_slot = content_chrome.right_panel_slot;
    let right_resize_handle = content_chrome.right_resize_handle;
    let tiny_nav_button = gtk::Button::from_icon_name("sidebar-show-symbolic");
    tiny_nav_button.add_css_class("icon-button");
    tiny_nav_button.add_css_class("flat");
    tiny_nav_button.add_css_class("circular");
    tiny_nav_button.add_css_class("tiny-sidebar-button");
    tiny_nav_button.set_tooltip_text(Some(&tr("Show sidebar")));
    tiny_nav_button.update_property(&[gtk::accessible::Property::Label(&tr("Show sidebar"))]);
    tiny_nav_button.set_halign(gtk::Align::Start);
    tiny_nav_button.set_valign(gtk::Align::End);
    tiny_nav_button.set_margin_start(8);
    tiny_nav_button.set_margin_bottom(8);
    tiny_nav_button.set_visible(false);
    content_chrome.root.add_overlay(&tiny_nav_button);
    content_chrome
        .root
        .set_measure_overlay(&tiny_nav_button, false);
    let fullscreen_player = build_fullscreen_player();
    let player_controls = build_bottom_player();

    let content_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content_row.set_hexpand(true);
    content_row.set_vexpand(true);
    content_row.append(&compact_nav_slot);
    content_row.append(&content_chrome.root);

    let split_view = adw::OverlaySplitView::new();
    split_view.set_hexpand(true);
    split_view.set_vexpand(true);
    split_view.set_enable_hide_gesture(false);
    split_view.set_enable_show_gesture(false);
    split_view.set_min_sidebar_width(NORMAL_SIDEBAR_WIDTH as f64);
    split_view.set_max_sidebar_width(NORMAL_SIDEBAR_WIDTH as f64);
    split_view.set_sidebar_width_unit(adw::LengthUnit::Px);
    split_view.set_sidebar(Some(&normal_nav_slot));
    split_view.set_content(Some(&content_row));

    let shell_layout = gtk::Overlay::new();
    shell_layout.set_hexpand(true);
    shell_layout.set_vexpand(true);
    shell_layout.set_child(Some(&split_view));
    let left_resize_handle = sidebar_resize_handle();
    shell_layout.add_overlay(&left_resize_handle);
    shell_layout.set_measure_overlay(&left_resize_handle, false);
    upper.append(&shell_layout);

    app_content_stack.add_named(&upper, Some("main"));
    app_content_overlay.set_child(Some(&app_content_stack));
    app_content_overlay.add_overlay(&fullscreen_player.root);
    app_content_overlay.set_measure_overlay(&fullscreen_player.root, false);

    app_root.append(&app_content_overlay);
    let bottom_player_handle =
        window_drag_handle_with_child("bottom-player-drag-handle", &player_controls.root);
    app_root.append(&bottom_player_handle);

    let app_root_overlay = gtk::Overlay::new();
    app_root_overlay.set_hexpand(true);
    app_root_overlay.set_vexpand(true);
    app_root_overlay.set_child(Some(&app_root));
    let control_feedback_label = gtk::Label::new(None);
    control_feedback_label.add_css_class("control-feedback-toast");
    control_feedback_label.set_halign(gtk::Align::Center);
    control_feedback_label.set_valign(gtk::Align::End);
    control_feedback_label.set_visible(false);
    app_root_overlay.add_overlay(&control_feedback_label);
    app_root_overlay.set_measure_overlay(&control_feedback_label, false);
    let operation_feedback = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    operation_feedback.add_css_class("operation-feedback");
    operation_feedback.set_halign(gtk::Align::Center);
    operation_feedback.set_valign(gtk::Align::End);
    operation_feedback.set_margin_bottom(96);
    operation_feedback.set_visible(false);
    operation_feedback.set_focusable(true);
    let operation_feedback_artwork = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    operation_feedback_artwork.set_size_request(48, 48);
    operation_feedback.append(&operation_feedback_artwork);
    let operation_feedback_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    operation_feedback_text.set_valign(gtk::Align::Center);
    operation_feedback_text.set_hexpand(true);
    let operation_feedback_title = gtk::Label::new(None);
    operation_feedback_title.add_css_class("heading");
    operation_feedback_title.set_xalign(0.0);
    operation_feedback_title.set_max_width_chars(34);
    operation_feedback_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    operation_feedback_title.set_single_line_mode(true);
    let operation_feedback_subtitle = gtk::Label::new(None);
    operation_feedback_subtitle.add_css_class("dim-label");
    operation_feedback_subtitle.set_xalign(0.0);
    operation_feedback_subtitle.set_max_width_chars(34);
    operation_feedback_subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    operation_feedback_subtitle.set_single_line_mode(true);
    operation_feedback_text.append(&operation_feedback_title);
    operation_feedback_text.append(&operation_feedback_subtitle);
    operation_feedback.append(&operation_feedback_text);
    let operation_feedback_close = gtk::Button::from_icon_name("window-close-symbolic");
    operation_feedback_close.add_css_class("flat");
    operation_feedback_close.set_valign(gtk::Align::Center);
    operation_feedback_close.set_tooltip_text(Some(&tr("Close")));
    operation_feedback.append(&operation_feedback_close);
    app_root_overlay.add_overlay(&operation_feedback);
    app_root_overlay.set_measure_overlay(&operation_feedback, false);
    app_root_overlay.add_overlay(&startup_loading_host);
    app_root_overlay.set_measure_overlay(&startup_loading_host, false);

    root_stack.add_named(&login_host, Some("login"));
    root_stack.add_named(&app_root_overlay, Some("app"));
    let layout_state = ShellLayoutState::new(&root_stack);
    let quick_toast_overlay = adw::ToastOverlay::new();
    quick_toast_overlay.add_css_class("quick-toast-overlay");
    quick_toast_overlay.set_child(Some(&layout_state.owner));
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.add_css_class("app-toast-overlay");
    toast_overlay.set_child(Some(&quick_toast_overlay));
    window.set_content(Some(&toast_overlay));

    let chrome = WindowChrome {
        application: app.clone(),
        window,
        toast_overlay,
        quick_toast_overlay,
        control_feedback_label,
        operation_feedback,
        operation_feedback_artwork,
        operation_feedback_title,
        operation_feedback_subtitle,
        operation_feedback_close,
        root_stack,
        app_root_overlay,
        app_content_stack,
        login_host,
        startup_loading_host,
    };
    let navigation_view = NavigationWidgets {
        split_view,
        left_resize_handle,
        normal_nav_slot,
        compact_nav_slot,
        tiny_nav_button,
        normal_nav,
        compact_nav,
        normal_main_menu: PrimaryMenuWidgets {
            button: normal_main_menu,
            popover: RefCell::new(None),
            click_handler: RefCell::new(None),
            unmap_handler: RefCell::new(None),
        },
        compact_main_menu: PrimaryMenuWidgets {
            button: compact_main_menu,
            popover: RefCell::new(None),
            click_handler: RefCell::new(None),
            unmap_handler: RefCell::new(None),
        },
    };
    let route_viewport = RouteViewport::new(route_host);
    let right_panel = RightPanelWidgets {
        right_split,
        right_panel_slot,
        right_resize_handle,
        root: right_panel,
        queue_panel,
        queue_search,
        queue_clear_button,
        queue_lyrics_overlay,
        lyrics_surface,
        lyrics_resize_handle,
        lyrics_pane,
    };
    let player_view = PlayerDesktopWidgets {
        fullscreen_player,
        player_controls,
    };

    let shell = Rc::new(Shell {
        diagnostics,
        settings: settings_state,
        navigation,
        library: library_state,
        source,
        startup,
        playback: playback_state,
        queue: queue_state,
        lyrics: lyrics_state,
        preferences,
        playlist_picker,
        downloads,
        control_feedback,
        localization,
        desktop,
        artwork,
        favorites,
        products,
        chrome,
        layout_state,
        navigation_view,
        route_viewport,
        right_panel,
        player_view,
    });

    shell.connect_operation_feedback();
    shell.connect_artwork_scale_refresh();
    {
        let source = Arc::clone(&shell.products.source);
        let was_active = Cell::new(shell.chrome.window.is_active());
        shell.chrome.window.connect_is_active_notify(move |window| {
            let active = window.is_active();
            let previous = was_active.replace(active);
            if active && !previous {
                source.check_for_source_changes();
            }
        });
    }
    build_normal_navigation(&shell);
    build_compact_navigation(&shell);
    shell.install_locale_bindings();
    {
        let split_view = shell.navigation_view.split_view.clone();
        shell
            .navigation_view
            .tiny_nav_button
            .connect_clicked(move |_| split_view.set_show_sidebar(true));
    }
    connect_shell_actions(&shell);
    install_playback_shutdown(
        &shell.chrome.application,
        &shell.products.playback.transport,
    );
    install_window_state_persistence(&shell);
    #[cfg(unix)]
    install_tray(&shell);
    connect_queue_panel_controls(&shell);
    connect_queue_lyrics_overlay(&shell);
    shell.connect_route_keyboard();
    connect_transient_entry_focus_dismissal(&shell);
    connect_lyrics_search_controls(&shell);
    connect_fullscreen_player_controls(&shell);
    connect_player_controls(&shell);
    warm_audio_output_cache(&shell);
    shell.update_layout();
    if defer_initial_route {
        shell.render_startup_loading_view();
    } else {
        shell.render_current_route();
    }
    shell.render_queue_panel();
    shell.render_lyrics_panel();
    shell.sync_bottom_player_favorite();
    shell.update_bottom_player();
    shell.update_fullscreen_player();
    shell.update_right_panel_button();
    shell.update_lyrics_panel_button();
    if !shell.lyrics.panel_visible.get() {
        apply_lyrics_panel_visibility(Rc::clone(&shell), false);
    }
    shell.request_initial_lyrics_if_needed();
    install_product_event_receivers(&shell, receivers);

    #[cfg(unix)]
    present_initial_window(&shell);
    #[cfg(not(unix))]
    shell.chrome.window.present();
    schedule_release_check(&shell);
    if defer_initial_route && !shell.source.operation.borrow().blocks_library() {
        shell.schedule_startup_route_reveal();
    }
}
