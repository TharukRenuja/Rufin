use super::*;

pub(in crate::ui) const UI_PERF_ROUTE_READY_POLL_MS: u64 = 8;
pub(in crate::ui) const UI_PERF_ROUTE_READY_TIMEOUT_MS: u64 = 3_500;
pub(in crate::ui) const UI_PERF_ROUTE_PROBE_MID_DRAG_SETTLE_MS: u64 = 64;
pub(in crate::ui) const UI_PERF_ROUTE_PROBE_SCROLL_SETTLE_MS: u64 = 300;

pub(in crate::ui) struct UiPerfRouteScrollRun {
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    runs: Rc<RefCell<VecDeque<(Route, UiPerfScenario)>>>,
    heartbeat: Rc<RefCell<Option<glib::SourceId>>>,
    route_name: String,
    scenario: UiPerfScenario,
}

pub(in crate::ui) struct UiPerfRouteProbeRun {
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    routes: Rc<RefCell<VecDeque<Route>>>,
    heartbeat: Rc<RefCell<Option<glib::SourceId>>>,
    route: Route,
    route_name: String,
    route_started_at: Instant,
    wait_started_at: Instant,
    route_ready_recorded: bool,
}

pub(in crate::ui) fn connect_shell_actions(shell: &Rc<Shell>, main_menu: gtk::MenuButton) {
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
pub(in crate::ui) fn connect_lyrics_search_controls(shell: &Rc<Shell>) {
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

    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .lyrics_pane
        .connect_search_clicked(move || {
            if current_playback_track_id(&fullscreen_lyrics_shell.state.player.borrow()).is_none() {
                return;
            }
            fullscreen_lyrics_shell.present_lyrics_search_dialog();
        });
    let fullscreen_lyrics_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .lyrics_pane
        .connect_clear_auto_search_clicked(move || {
            fullscreen_lyrics_shell.suppress_auto_lyrics_for_current()
        });
}
pub(in crate::ui) fn submit_lyrics_search(shell: &Rc<Shell>) {
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
pub(in crate::ui) fn auto_lyrics_search_is_suppressed(
    settings: &AppSettings,
    track_id: &rufin_core::TrackId,
) -> bool {
    settings
        .suppressed_auto_lyrics_track_ids
        .iter()
        .any(|stored| stored == track_id.as_str())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum AutoLyricsRequest {
    Default,
    ServerOnly,
}
pub(in crate::ui) fn preferences_login_status_toast_message(status: &str) -> Option<&str> {
    let status = status.trim();
    let server_check = status.starts_with("Checking ") && status.ends_with(" server...");
    let server_saved = status.starts_with("Server settings saved.");
    if server_check || server_saved || status == "No changes to save." {
        Some(status)
    } else {
        None
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum SnapshotRenderDecision {
    SourceChanged,
    FirstRunFinished,
    PreserveScroll,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum LocalSourceCacheGateAction {
    None,
    Enter,
    Wait,
    Reveal,
    Cancel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct SnapshotEventOutcome {
    pub entered_first_run: bool,
    pub render: SnapshotRenderDecision,
}
pub(in crate::ui) fn snapshot_event_outcome(
    previous_first_run: bool,
    next_first_run: bool,
    previous_source: &Option<rufin_core::LibrarySourceSelection>,
    next_source: &Option<rufin_core::LibrarySourceSelection>,
    first_run_connection_pending: bool,
    first_run_connection_ready: bool,
) -> SnapshotEventOutcome {
    let first_run_finished =
        first_run_connection_pending && first_run_connection_ready && !next_first_run;
    let render = if first_run_finished {
        SnapshotRenderDecision::FirstRunFinished
    } else if previous_source != next_source {
        SnapshotRenderDecision::SourceChanged
    } else {
        SnapshotRenderDecision::PreserveScroll
    };

    SnapshotEventOutcome {
        entered_first_run: next_first_run && !previous_first_run,
        render,
    }
}
pub(in crate::ui) fn local_source_cache_gate_action(
    local_folders_changed: bool,
    next_source: &Option<rufin_core::LibrarySourceSelection>,
    has_local_folders: bool,
    preparing: bool,
    sync_seen: bool,
    sync_status: &str,
) -> LocalSourceCacheGateAction {
    if !library_source_is_local(next_source) {
        return if preparing {
            LocalSourceCacheGateAction::Cancel
        } else {
            LocalSourceCacheGateAction::None
        };
    }

    if !preparing
        && has_local_folders
        && (local_folders_changed || local_source_snapshot_is_syncing(sync_status))
    {
        return LocalSourceCacheGateAction::Enter;
    }

    if !preparing {
        return LocalSourceCacheGateAction::None;
    }

    if local_source_snapshot_is_syncing(sync_status) || !sync_seen {
        LocalSourceCacheGateAction::Wait
    } else {
        LocalSourceCacheGateAction::Reveal
    }
}
pub(in crate::ui) fn snapshot_local_source_cache_gate_action(
    render: SnapshotRenderDecision,
    local_folders_changed: bool,
    next_source: &Option<rufin_core::LibrarySourceSelection>,
    has_local_folders: bool,
    preparing: bool,
    sync_seen: bool,
    sync_status: &str,
) -> LocalSourceCacheGateAction {
    if matches!(render, SnapshotRenderDecision::FirstRunFinished) {
        return LocalSourceCacheGateAction::None;
    }

    local_source_cache_gate_action(
        local_folders_changed,
        next_source,
        has_local_folders,
        preparing,
        sync_seen,
        sync_status,
    )
}
pub(in crate::ui) fn library_source_is_local(
    source: &Option<rufin_core::LibrarySourceSelection>,
) -> bool {
    matches!(source, Some(rufin_core::LibrarySourceSelection::Local))
}
pub(in crate::ui) fn local_source_snapshot_is_syncing(sync_status: &str) -> bool {
    sync_status == "Syncing library..."
}
pub(in crate::ui) fn queue_source_waits_for_snapshot(
    queue: Option<&QueueSnapshot>,
    active_server_id: Option<&rufin_core::ServerId>,
) -> bool {
    queue.is_some_and(|queue| active_server_id != Some(&queue.server_id))
}
pub(in crate::ui) fn queue_source_matches_library(
    queue: Option<&QueueSnapshot>,
    library: &LibrarySnapshot,
) -> bool {
    let Some(queue) = queue else {
        return false;
    };
    library
        .server
        .as_ref()
        .is_some_and(|server| server.id == queue.server_id)
}
pub(in crate::ui) fn auto_lyrics_request_for_settings(
    settings: &AppSettings,
    track_id: &rufin_core::TrackId,
    lyrics_surface_visible: bool,
) -> Option<AutoLyricsRequest> {
    if !lyrics_surface_visible {
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
pub(in crate::ui) fn auto_lyrics_skip_action_enabled(
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
pub(in crate::ui) fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
pub(in crate::ui) fn lyrics_search_result_has_content(result: &LyricsSearchResult) -> bool {
    result
        .synced_lyrics
        .as_deref()
        .is_some_and(|lyrics| !lyrics.trim().is_empty())
        || result
            .plain_lyrics
            .as_deref()
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
}
pub(in crate::ui) fn lyrics_result_subtitle(result: &LyricsSearchResult) -> String {
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
pub(in crate::ui) fn initial_window_size(width: Option<i32>, height: Option<i32>) -> (i32, i32) {
    sanitized_window_size(width, height).unwrap_or((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))
}
pub(in crate::ui) fn install_window_state_persistence(shell: &Rc<Shell>) {
    let save_shell = Rc::clone(shell);
    shell.application.connect_shutdown(move |_| {
        save_shell.save_window_state();
    });
}
pub(in crate::ui) fn connect_layout_resize(shell: &Rc<Shell>) {
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
pub(in crate::ui) fn install_window_actions(shell: &Rc<Shell>) {
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
pub(in crate::ui) fn install_main_menu_shortcut(shell: &Rc<Shell>, main_menu: gtk::MenuButton) {
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
pub(in crate::ui) fn show_shortcuts_dialog(shell: &Shell) {
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
pub(in crate::ui) fn show_about_dialog(shell: &Shell) {
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
pub(in crate::ui) fn schedule_startup_sync(shell: &Rc<Shell>) {
    let Some(delay_ms) = shell.controller.startup_sync_delay_ms() else {
        return;
    };

    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
        debug!(delay_ms, "starting deferred background sync");
        shell.controller.start_background_sync_for_active();
    });
}
pub(in crate::ui) fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(Duration::from_millis(33), move || {
        shell.controller.poll_playback_events();
        while let Ok(event) = receiver.try_recv() {
            match event {
                ControllerEvent::Snapshot(snapshot) => {
                    let (snapshot_outcome, local_folders_changed) = {
                        let current = shell.state.library.borrow();
                        (
                            snapshot_event_outcome(
                                current.first_run,
                                snapshot.first_run,
                                &current.selected_source,
                                &snapshot.selected_source,
                                shell.state.first_run_connection_pending.get(),
                                shell.state.first_run_connection_ready.get(),
                            ),
                            current.local_folders != snapshot.local_folders,
                        )
                    };
                    let local_gate_action = snapshot_local_source_cache_gate_action(
                        snapshot_outcome.render,
                        local_folders_changed,
                        &snapshot.selected_source,
                        !snapshot.local_folders.is_empty(),
                        shell.state.local_source_preparing.get(),
                        shell.state.local_source_sync_seen.get(),
                        &snapshot.sync_status,
                    );
                    let local_snapshot_syncing =
                        local_source_snapshot_is_syncing(&snapshot.sync_status);
                    let server_id = snapshot.server.as_ref().map(|server| server.id.clone());
                    let prefetched_explore = prefetched_explore_from_snapshot(&snapshot);
                    let sections = snapshot.home_sections.clone();
                    *shell.state.library.borrow_mut() = *snapshot;
                    if snapshot_outcome.entered_first_run {
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
                    match local_gate_action {
                        LocalSourceCacheGateAction::Enter => {
                            shell.state.local_source_preparing.set(true);
                            shell.state.source_switch_preparing.set(false);
                            shell
                                .state
                                .local_source_sync_seen
                                .set(local_snapshot_syncing);
                            shell.state.startup_route_render_pending.set(false);
                            shell.state.startup_route_revealed.set(false);
                            shell.prepare_home_route_for_source_change();
                            shell.render_startup_loading_view();
                            continue;
                        }
                        LocalSourceCacheGateAction::Wait => {
                            if local_snapshot_syncing {
                                shell.state.local_source_sync_seen.set(true);
                            }
                            shell.render_startup_loading_view();
                            continue;
                        }
                        LocalSourceCacheGateAction::Reveal => {
                            shell.state.local_source_preparing.set(false);
                            shell.state.local_source_sync_seen.set(false);
                            shell.state.source_switch_preparing.set(false);
                            shell.log_layout_snapshot("local_source_final_snapshot");
                            shell.schedule_startup_route_reveal();
                            continue;
                        }
                        LocalSourceCacheGateAction::Cancel => {
                            shell.state.local_source_preparing.set(false);
                            shell.state.local_source_sync_seen.set(false);
                            shell.state.source_switch_preparing.set(false);
                            shell.state.startup_route_render_pending.set(false);
                            shell.state.startup_route_revealed.set(true);
                        }
                        LocalSourceCacheGateAction::None => {}
                    }
                    if shell.state.source_switch_preparing.get() {
                        let queue_matches_library = {
                            let queue = shell.state.queue.borrow();
                            let library = shell.state.library.borrow();
                            queue_source_matches_library(queue.as_ref(), &library)
                        };
                        if queue_matches_library {
                            shell.state.source_switch_preparing.set(false);
                            shell.prepare_home_route_for_source_change();
                            shell.render_queue_panel();
                            shell.render_lyrics_panel();
                            shell.update_bottom_player();
                            shell.update_fullscreen_player();
                            let player = shell.state.player.borrow().clone();
                            #[cfg(unix)]
                            shell.update_mpris_player();
                            shell.update_discord_presence(&player);
                            shell.schedule_startup_route_reveal();
                            continue;
                        }
                    }
                    match snapshot_outcome.render {
                        SnapshotRenderDecision::FirstRunFinished => {
                            shell.state.local_source_preparing.set(false);
                            shell.state.local_source_sync_seen.set(false);
                            shell.state.source_switch_preparing.set(false);
                            shell.log_layout_snapshot("first_run_final_snapshot");
                            shell.schedule_first_run_app_reveal();
                            continue;
                        }
                        SnapshotRenderDecision::SourceChanged => {
                            shell.reset_cover_pipeline_for_source_change();
                            shell.navigate(Route::Home);
                        }
                        SnapshotRenderDecision::PreserveScroll => {
                            shell.render_current_route_preserving_scroll();
                        }
                    }
                }
                ControllerEvent::HomeSectionsUpdated {
                    snapshot,
                    include_explore,
                } => {
                    let previous_sections = shell.state.library.borrow().home_sections.clone();
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
                    shell.refresh_changed_visible_home_sections(
                        &previous_sections,
                        &sections,
                        include_explore,
                    );
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
                ControllerEvent::SmartPlaylistChanged {
                    smart_playlist_id,
                    snapshot,
                } => {
                    *shell.state.library.borrow_mut() = *snapshot;
                    shell.update_server_selector();
                    let route = shell.state.routes.borrow().current().clone();
                    if matches!(route, Route::SmartPlaylists) {
                        shell.navigate(Route::SmartPlaylistDetail(smart_playlist_id));
                    } else if matches!(
                        route,
                        Route::SmartPlaylistDetail(id) if id == smart_playlist_id
                    ) {
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
                    let waits_for_source_snapshot = {
                        let library = shell.state.library.borrow();
                        queue_source_waits_for_snapshot(
                            queue.as_ref().as_ref(),
                            library.server.as_ref().map(|server| &server.id),
                        )
                    };
                    *shell.state.queue.borrow_mut() = *queue;
                    if waits_for_source_snapshot {
                        shell.state.source_switch_preparing.set(true);
                        shell.state.startup_route_render_pending.set(false);
                        shell.state.startup_route_revealed.set(false);
                        shell.render_startup_loading_view();
                        continue;
                    }
                    shell.render_queue_panel();
                    shell.update_bottom_player();
                    shell.update_fullscreen_player();
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
                    if shell.state.source_switch_preparing.get() {
                        if previous_track != next_track {
                            *shell.state.lyrics.borrow_mut() = None;
                            *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                            shell.lyrics_pane.clear_follow_scroll_pause();
                            shell
                                .fullscreen_player
                                .lyrics_pane
                                .clear_follow_scroll_pause();
                            shell.cancel_scheduled_lyrics_highlight();
                        }
                        continue;
                    }
                    if previous_track != next_track {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                        shell.lyrics_pane.clear_follow_scroll_pause();
                        shell
                            .fullscreen_player
                            .lyrics_pane
                            .clear_follow_scroll_pause();
                        shell.cancel_scheduled_lyrics_highlight();
                        shell.render_queue_panel();
                        shell.render_lyrics_panel();
                        shell.request_auto_lyrics_if_needed();
                        shell.notify_now_playing(&next_snapshot);
                    }
                    shell.update_bottom_player();
                    shell.update_fullscreen_player();
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
                    #[cfg(unix)]
                    let update_mpris_art = shell.current_mpris_art_key_is(&key);
                    shell.apply_cover_ready(&key, &path);
                    #[cfg(unix)]
                    if update_mpris_art {
                        shell.update_mpris_player();
                    }
                }
                ControllerEvent::CoverUnavailable {
                    key,
                    external_retry_generation,
                } => {
                    if external_retry_generation.is_some_and(|generation| {
                        !shell
                            .controller
                            .external_cover_retry_generation_is_current(generation)
                    }) {
                        continue;
                    }
                    shell.apply_cover_unavailable(&key);
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
                    if let Some(message) = preferences_login_status_toast_message(&status) {
                        shell.show_preferences_toast(message);
                    }
                    let sync_complete = status == LIBRARY_SYNC_COMPLETE_STATUS;
                    if sync_complete {
                        shell.state.first_run_connection_ready.set(true);
                        if shell.state.local_source_preparing.get() {
                            shell.state.local_source_sync_seen.set(true);
                        }
                    }
                    let first_run_connection_pending =
                        shell.state.first_run_connection_pending.get();
                    let display_status = if sync_complete && first_run_connection_pending {
                        tr(LIBRARY_PREPARING_STATUS)
                    } else {
                        status
                    };
                    let should_render = {
                        let mut library = shell.state.library.borrow_mut();
                        library.sync_status = display_status;
                        route_displays_sync_status(
                            shell.state.routes.borrow().current(),
                            library.first_run,
                        ) || shell.state.first_run_connection_pending.get()
                            || shell.state.local_source_preparing.get()
                    };
                    if should_render {
                        if shell.state.local_source_preparing.get() {
                            shell.render_startup_loading_view();
                        } else {
                            shell.render_current_route();
                        }
                    }
                }
                ControllerEvent::PlaybackPerf(event) => {
                    if let Some(perf) = shell.state.perf.as_ref() {
                        perf.record_playback_event(&event);
                    }
                }
                ControllerEvent::Error(error) => {
                    warn!(%error, "controller error");
                    shell.show_preferences_toast(&error);
                    shell.state.first_run_connection_pending.set(false);
                    shell.state.first_run_connection_ready.set(false);
                    shell.state.local_source_preparing.set(false);
                    shell.state.local_source_sync_seen.set(false);
                    shell.state.source_switch_preparing.set(false);
                    shell.state.startup_route_render_pending.set(false);
                    shell.state.startup_route_revealed.set(true);
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
pub(in crate::ui) fn start_ui_perf_run(shell: &Rc<Shell>, app: &adw::Application) {
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
    let plan = ui_perf_plan(shell, perf.options.duration_ms, perf.options.route_ms);
    println!("RUFIN_PERF route_plan {}", ui_perf_plan_summary(&plan));
    let runs = Rc::new(RefCell::new(plan));
    let shell = Rc::clone(shell);
    let app = app.clone();
    wait_for_ui_perf_startup_reveal(shell, app, perf, runs, Instant::now());
}
pub(in crate::ui) fn start_ui_perf_observe(shell: &Rc<Shell>, app: &adw::Application) {
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
pub(in crate::ui) fn start_ui_perf_route_probe(shell: &Rc<Shell>, app: &adw::Application) {
    let Some(perf) = shell.state.perf.clone() else {
        return;
    };
    println!(
        "RUFIN_ROUTE_PROBE start max_gap_ms={} route_ready_ms={} drag_ms={} route_ms={} asset_ms={} launch_elapsed_ms={} output={}",
        perf.options.max_gap_ms,
        perf.options.route_ready_ms,
        perf.options.drag_ms,
        perf.options.route_ms,
        perf.options.asset_ms,
        duration_ms(perf.started_at().elapsed()),
        perf.options
            .output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "stdout_only".to_string())
    );
    let routes = ui_perf_route_probe_plan(shell);
    println!(
        "RUFIN_ROUTE_PROBE route_plan {}",
        ui_perf_route_probe_plan_summary(&routes)
    );
    wait_for_ui_perf_route_probe_startup_reveal(
        Rc::clone(shell),
        app.clone(),
        Rc::clone(&perf),
        Rc::new(RefCell::new(routes)),
        perf.started_at(),
    );
}
pub(in crate::ui) fn wait_for_ui_perf_route_probe_startup_reveal(
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    routes: Rc<RefCell<VecDeque<Route>>>,
    started_at: Instant,
) {
    if (shell.state.startup_route_revealed.get() && !shell.state.startup_route_render_pending.get())
        || shell.login_screen_active()
    {
        perf.record_startup_reveal();
        println!(
            "RUFIN_ROUTE_PROBE startup_reveal elapsed_ms={}",
            duration_ms(started_at.elapsed())
        );
        let heartbeat = Rc::new(RefCell::new(Some(start_ui_perf_heartbeat(Rc::clone(
            &perf,
        )))));
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            run_next_ui_perf_route_probe(shell, app, perf, routes, heartbeat);
        });
        return;
    }
    if startup_reveal_wait_timed_out(started_at) {
        perf.record_startup_reveal();
        println!(
            "RUFIN_ROUTE_PROBE startup_reveal_timeout elapsed_ms={}",
            duration_ms(started_at.elapsed())
        );
        finish_ui_perf_run(perf, app);
        return;
    }

    glib::timeout_add_local_once(
        Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
        move || {
            wait_for_ui_perf_route_probe_startup_reveal(shell, app, perf, routes, started_at);
        },
    );
}
pub(in crate::ui) fn run_next_ui_perf_route_probe(
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    routes: Rc<RefCell<VecDeque<Route>>>,
    heartbeat: Rc<RefCell<Option<glib::SourceId>>>,
) {
    let Some(route) = routes.borrow_mut().pop_front() else {
        if let Some(source) = heartbeat.borrow_mut().take() {
            source.remove();
        }
        if perf.pending_assets() > 0 {
            glib::timeout_add_local_once(
                Duration::from_millis(perf.options.asset_ms.saturating_mul(2)),
                move || finish_ui_perf_run(perf, app),
            );
            return;
        }
        finish_ui_perf_run(perf, app);
        return;
    };

    let route_name = format!("{route:?}");
    println!("RUFIN_ROUTE_PROBE route_begin route={route_name}");
    let route_started_at = Instant::now();
    if shell.state.routes.borrow().current() == &route {
        reset_ui_perf_route_scroll_position(&shell);
        if ui_perf_route_probe_should_rerender_current_route(&route) {
            shell.render_current_route_preserving_scroll();
        } else {
            shell.prime_route_visible_cover_window(&route);
        }
    } else {
        shell.navigate(route.clone());
    }

    let shell_for_probe = Rc::clone(&shell);
    let app_for_probe = app.clone();
    let perf_for_probe = Rc::clone(&perf);
    let routes_for_probe = Rc::clone(&routes);
    let heartbeat_for_probe = Rc::clone(&heartbeat);
    wait_for_ui_perf_route_probe_ready(UiPerfRouteProbeRun {
        shell: shell_for_probe,
        app: app_for_probe,
        perf: perf_for_probe,
        routes: routes_for_probe,
        heartbeat: heartbeat_for_probe,
        route,
        route_name,
        route_started_at,
        wait_started_at: Instant::now(),
        route_ready_recorded: false,
    });
}
pub(in crate::ui) fn wait_for_ui_perf_route_probe_ready(mut run: UiPerfRouteProbeRun) {
    let scroll_max = route_scroll_max(&run.shell);
    let visible_contract = ui_perf_route_visible_contract(
        &run.shell,
        run.route_name.clone(),
        &run.route,
        "route_ready",
    );
    let has_pending_counters = route_visible_contract_has_pending_work(
        visible_contract.pending,
        visible_contract.fallback_after_reveal,
        visible_contract.pending_assets,
        visible_contract.active_decodes,
        visible_contract.queued_decodes,
        visible_contract.path_lookups,
    );
    let has_rendered_work = route_visible_contract_has_rendered_work(
        visible_contract.expected_visible,
        visible_contract.rendered_expected,
        visible_contract.rendered_fallback,
        visible_contract.rendered_ready,
        visible_contract.rendered_final_missing,
    );
    let has_unaccounted_visible_work = route_visible_contract_has_unaccounted_visible_work(
        visible_contract.expected_visible,
        visible_contract.ready,
        visible_contract.final_missing,
        visible_contract.pending,
    );
    let has_route_ready_work =
        has_pending_counters || has_unaccounted_visible_work || has_rendered_work;
    let wait_elapsed = run.wait_started_at.elapsed();
    if run.perf.options.terminal_events
        && duration_ms(wait_elapsed) % 128 < UI_PERF_ROUTE_READY_POLL_MS
        && has_route_ready_work
    {
        println!(
            "RUFIN_ROUTE_PROBE route_wait route={} gate_wait_ms={} pending={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={} expected_visible={} ready={} final_missing={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} scroll_max={:.0}",
            run.route_name,
            duration_ms(wait_elapsed),
            visible_contract.pending,
            visible_contract.fallback_after_reveal,
            visible_contract.pending_assets,
            visible_contract.active_decodes,
            visible_contract.queued_decodes,
            visible_contract.path_lookups,
            visible_contract.expected_visible,
            visible_contract.ready,
            visible_contract.final_missing,
            visible_contract.rendered_expected,
            visible_contract.rendered_ready,
            visible_contract.rendered_final_missing,
            visible_contract.rendered_fallback,
            scroll_max,
        );
    }
    if !run.route_ready_recorded
        && ui_perf_route_probe_waits_for_route_ready(
            ui_perf_route_probe_expects_scroll(&run.shell, &run.route),
            visible_contract
                .visible_end
                .saturating_sub(visible_contract.visible_start),
            scroll_max,
            wait_elapsed,
            has_route_ready_work,
        )
    {
        glib::timeout_add_local_once(
            Duration::from_millis(UI_PERF_ROUTE_READY_POLL_MS),
            move || wait_for_ui_perf_route_probe_ready(run),
        );
        return;
    }

    if !run.route_ready_recorded {
        println!(
            "RUFIN_ROUTE_PROBE route_ready route={} elapsed_ms={} gate_wait_ms={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={} scroll_max={:.0}",
            run.route_name,
            duration_ms(run.route_started_at.elapsed()),
            duration_ms(wait_elapsed),
            visible_contract.layout,
            visible_contract.visible_start,
            visible_contract.visible_end,
            visible_contract.expected_visible,
            visible_contract.ready,
            visible_contract.final_missing,
            visible_contract.pending,
            visible_contract.rendered_expected,
            visible_contract.rendered_ready,
            visible_contract.rendered_final_missing,
            visible_contract.rendered_fallback,
            visible_contract.fallback_after_reveal,
            visible_contract.pending_assets,
            visible_contract.active_decodes,
            visible_contract.queued_decodes,
            visible_contract.path_lookups,
            scroll_max,
        );
        run.perf.record_route_ready(
            run.route_name.clone(),
            run.route_started_at.elapsed(),
            wait_elapsed,
        );
        run.route_ready_recorded = true;
    }

    if ui_perf_route_probe_waits_for_rendered_ready(
        ui_perf_route_probe_expects_scroll(&run.shell, &run.route),
        visible_contract
            .visible_end
            .saturating_sub(visible_contract.visible_start),
        scroll_max,
        wait_elapsed,
        has_rendered_work,
    ) {
        glib::timeout_add_local_once(
            Duration::from_millis(UI_PERF_ROUTE_READY_POLL_MS),
            move || wait_for_ui_perf_route_probe_ready(run),
        );
        return;
    }

    println!(
        "RUFIN_ROUTE_PROBE route_visible_ready route={} elapsed_ms={} gate_wait_ms={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={} scroll_max={:.0}",
        run.route_name,
        duration_ms(run.route_started_at.elapsed()),
        duration_ms(run.wait_started_at.elapsed()),
        visible_contract.layout,
        visible_contract.visible_start,
        visible_contract.visible_end,
        visible_contract.expected_visible,
        visible_contract.ready,
        visible_contract.final_missing,
        visible_contract.pending,
        visible_contract.rendered_expected,
        visible_contract.rendered_ready,
        visible_contract.rendered_final_missing,
        visible_contract.rendered_fallback,
        visible_contract.fallback_after_reveal,
        visible_contract.pending_assets,
        visible_contract.active_decodes,
        visible_contract.queued_decodes,
        visible_contract.path_lookups,
        scroll_max,
    );
    run.perf.record_route_visible_contract(visible_contract);
    begin_ui_perf_route_probe_drag(run);
}

pub(in crate::ui) fn ui_perf_route_probe_waits_for_scroll_geometry(
    expects_scroll: bool,
    visible_count: usize,
    scroll_max: f64,
    elapsed: Duration,
) -> bool {
    expects_scroll
        && elapsed < Duration::from_millis(UI_PERF_ROUTE_READY_TIMEOUT_MS)
        && (visible_count <= 1 || scroll_max <= 1.0)
}

pub(in crate::ui) fn ui_perf_route_probe_waits_for_route_ready(
    _expects_scroll: bool,
    _visible_count: usize,
    _scroll_max: f64,
    elapsed: Duration,
    has_route_ready_work: bool,
) -> bool {
    has_route_ready_work && elapsed < Duration::from_millis(UI_PERF_ROUTE_READY_TIMEOUT_MS)
}

pub(in crate::ui) fn ui_perf_route_probe_waits_for_rendered_ready(
    _expects_scroll: bool,
    _visible_count: usize,
    _scroll_max: f64,
    elapsed: Duration,
    has_rendered_work: bool,
) -> bool {
    has_rendered_work && elapsed < Duration::from_millis(UI_PERF_ROUTE_READY_TIMEOUT_MS)
}

pub(in crate::ui) fn ui_perf_route_probe_should_rerender_current_route(route: &Route) -> bool {
    !matches!(route, Route::Home)
}

pub(in crate::ui) fn ui_perf_route_probe_mid_drag_sample_delay_ms(drag_ms: u64) -> u64 {
    UI_PERF_ROUTE_PROBE_MID_DRAG_SETTLE_MS.min(drag_ms.saturating_div(4).max(1))
}

pub(in crate::ui) fn ui_perf_route_probe_drag_checkpoint_phase(
    progress: f64,
    next_checkpoint: usize,
) -> Option<(&'static str, usize, f64)> {
    let (threshold, phase) = UI_PERF_ROUTE_DRAG_CHECKPOINTS.get(next_checkpoint)?;
    if progress >= *threshold {
        Some((*phase, next_checkpoint.saturating_add(1), *threshold))
    } else {
        None
    }
}

const UI_PERF_ROUTE_DRAG_CHECKPOINTS: [(f64, &str); 3] =
    [(0.25, "drag_25"), (0.50, "drag_50"), (0.75, "drag_75")];

struct UiPerfVisibleCoverRef {
    image_ref: ImageRef,
    fetch_size: u32,
    size: i32,
}

struct UiPerfVisibleCoverWindow {
    layout: &'static str,
    visible_start: usize,
    visible_end: usize,
    coverless: usize,
    refs: Vec<UiPerfVisibleCoverRef>,
}

impl Shell {
    fn show_preferences_toast(&self, message: &str) {
        if let Some(overlay) = self.state.preferences_toast_overlay.borrow().as_ref() {
            overlay.add_toast(adw::Toast::new(message));
        }
    }

    pub(in crate::ui) fn prime_route_visible_cover_window(self: &Rc<Self>, route: &Route) -> usize {
        let window = ui_perf_visible_cover_window(self, route);
        self.prime_visible_cover_window(window)
    }

    pub(in crate::ui) fn prime_route_leading_and_warm_anchor_cover_windows(
        self: &Rc<Self>,
        route: &Route,
        leading_rows: usize,
    ) -> usize {
        let window = match route {
            Route::Tracks => ui_perf_track_leading_cover_window(self, leading_rows),
            _ => ui_perf_visible_cover_window(self, route),
        };
        let mut refs = self.prime_visible_cover_window(window);
        if matches!(route, Route::Tracks) {
            let anchors = ui_perf_track_anchor_cover_window(self);
            refs = refs.saturating_add(self.warm_visible_cover_window(anchors));
        }
        refs
    }

    fn prime_visible_cover_window(self: &Rc<Self>, window: UiPerfVisibleCoverWindow) -> usize {
        let mut groups = HashMap::<(u32, i32), Vec<ImageRef>>::new();
        for cover_ref in window.refs {
            groups
                .entry((cover_ref.fetch_size, cover_ref.size))
                .or_default()
                .push(cover_ref.image_ref);
        }
        let mut refs = 0_usize;
        for ((fetch_size, size), image_refs) in groups {
            refs = refs.saturating_add(image_refs.len());
            self.prime_cover_refs_now(image_refs, fetch_size, size);
        }
        refs
    }

    fn warm_visible_cover_window(self: &Rc<Self>, window: UiPerfVisibleCoverWindow) -> usize {
        let mut groups = HashMap::<(u32, i32), Vec<ImageRef>>::new();
        for cover_ref in window.refs {
            groups
                .entry((cover_ref.fetch_size, cover_ref.size))
                .or_default()
                .push(cover_ref.image_ref);
        }
        let mut refs = 0_usize;
        for ((fetch_size, size), image_refs) in groups {
            refs = refs.saturating_add(image_refs.len());
            self.warm_cover_refs_now(image_refs, fetch_size, size);
        }
        refs
    }
}

pub(in crate::ui) fn ui_perf_route_visible_contract(
    shell: &Shell,
    route_name: String,
    route: &Route,
    phase: &'static str,
) -> UiPerfRouteVisibleContract {
    let window = ui_perf_visible_cover_window(shell, route);
    let mut seen = HashSet::new();
    let mut expected_visible = window.coverless;
    let mut ready = 0_usize;
    let mut final_missing = window.coverless;
    let mut pending = 0_usize;
    let mut pending_samples = Vec::new();
    let mut visible_candidate_keys = HashSet::new();
    let rendered = ui_perf_rendered_cover_tile_snapshot(shell);
    let provider = shell
        .state
        .library
        .borrow()
        .server
        .as_ref()
        .map(|server| server.provider.clone());

    for cover_ref in &window.refs {
        let Some(key) = shell.cover_cache_key(&cover_ref.image_ref, cover_ref.fetch_size) else {
            expected_visible = expected_visible.saturating_add(1);
            final_missing = final_missing.saturating_add(1);
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        expected_visible = expected_visible.saturating_add(1);
        let candidate_keys =
            shell.cover_cache_candidate_keys(&cover_ref.image_ref, cover_ref.fetch_size);
        visible_candidate_keys.extend(candidate_keys.iter().cloned());
        let decode_size = cover_decode_size(cover_ref.size, cover_ref.fetch_size);
        if shell
            .decoded_cover_for_ref(&cover_ref.image_ref, cover_ref.fetch_size, decode_size)
            .is_some()
        {
            ready = ready.saturating_add(1);
        } else if ui_perf_cover_has_active_work(shell, &candidate_keys) {
            pending = pending.saturating_add(1);
            if pending_samples.len() < 12 {
                pending_samples.push(UiPerfRouteVisiblePendingSample {
                    key_hash: super::ui_perf_hash_label(&key),
                    kind: ui_perf_image_ref_kind(&cover_ref.image_ref),
                    state: ui_perf_pending_cover_state(shell, &candidate_keys),
                    fetch_size: cover_ref.fetch_size,
                    decode_size,
                });
            }
        } else if cover::visible_cover_cache_miss_action(
            provider.as_deref(),
            &cover_ref.image_ref,
            candidate_keys.iter().all(|candidate_key| {
                shell
                    .state
                    .cover_unavailable
                    .borrow()
                    .contains(candidate_key)
            }),
            shell
                .controller
                .external_cover_lookup_known_missing(&cover_ref.image_ref, cover_ref.fetch_size),
        ) == cover::VisibleCoverCacheMissAction::FinalMissing
        {
            final_missing = final_missing.saturating_add(1);
        } else {
            pending = pending.saturating_add(1);
            if pending_samples.len() < 12 {
                pending_samples.push(UiPerfRouteVisiblePendingSample {
                    key_hash: super::ui_perf_hash_label(&key),
                    kind: ui_perf_image_ref_kind(&cover_ref.image_ref),
                    state: ui_perf_pending_cover_state(shell, &candidate_keys),
                    fetch_size: cover_ref.fetch_size,
                    decode_size,
                });
            }
        }
    }
    let active_decodes = shell
        .state
        .cover_decodes
        .borrow()
        .keys()
        .filter(|key| visible_candidate_keys.contains(*key))
        .count();
    let queued_decodes = shell
        .state
        .cover_decode_queue
        .borrow()
        .iter()
        .filter(|job| visible_candidate_keys.contains(&job.key))
        .count();
    let path_lookups = shell
        .state
        .cover_path_lookups
        .borrow()
        .keys()
        .filter(|key| visible_candidate_keys.contains(*key))
        .count();
    let pending_assets = shell.state.perf.as_ref().map_or(0, |perf| {
        perf.pending_assets_for_keys(&visible_candidate_keys)
    });

    UiPerfRouteVisibleContract {
        phase,
        route: route_name,
        layout: window.layout,
        visible_start: window.visible_start,
        visible_end: window.visible_end,
        expected_visible,
        ready,
        final_missing,
        pending,
        rendered_expected: rendered.expected,
        rendered_ready: rendered.ready,
        rendered_final_missing: rendered.final_missing,
        rendered_fallback: rendered.fallback,
        fallback_after_reveal: pending,
        pending_assets,
        active_decodes,
        queued_decodes,
        path_lookups,
        pending_samples,
    }
}

#[derive(Default)]
struct UiPerfRenderedCoverTileSnapshot {
    expected: usize,
    ready: usize,
    final_missing: usize,
    fallback: usize,
}

fn ui_perf_rendered_cover_tile_snapshot(shell: &Shell) -> UiPerfRenderedCoverTileSnapshot {
    let root = shell.route_host.clone().upcast::<gtk::Widget>();
    let scroller = find_largest_scrolled_window(&root);
    let mut snapshot = UiPerfRenderedCoverTileSnapshot::default();
    collect_ui_perf_rendered_cover_tiles(&root, scroller.as_ref(), &mut snapshot);
    snapshot
}

fn collect_ui_perf_rendered_cover_tiles(
    widget: &gtk::Widget,
    scroller: Option<&gtk::ScrolledWindow>,
    snapshot: &mut UiPerfRenderedCoverTileSnapshot,
) {
    if !widget.is_visible() || !widget.is_mapped() {
        return;
    }
    let viewport_intersection = ui_perf_widget_viewport_intersection(widget, scroller);
    if viewport_intersection == Some(false) {
        return;
    }

    let expected = widget.has_css_class("cover-tile-expected");
    let final_missing = widget.has_css_class("cover-tile-final-missing");
    let intersects_viewport = matches!(
        (scroller, viewport_intersection),
        (None, _) | (Some(_), Some(true))
    );
    if (expected || final_missing) && intersects_viewport {
        snapshot.expected = snapshot.expected.saturating_add(1);
        if widget.has_css_class("cover-tile-resolved") {
            snapshot.ready = snapshot.ready.saturating_add(1);
        } else if final_missing {
            snapshot.final_missing = snapshot.final_missing.saturating_add(1);
        } else if widget.has_css_class("cover-tile-fallback") {
            snapshot.fallback = snapshot.fallback.saturating_add(1);
        }
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect_ui_perf_rendered_cover_tiles(&widget, scroller, snapshot);
        child = widget.next_sibling();
    }
}

fn ui_perf_widget_viewport_intersection(
    widget: &gtk::Widget,
    scroller: Option<&gtk::ScrolledWindow>,
) -> Option<bool> {
    let scroller = scroller?;
    let bounds = widget.compute_bounds(scroller)?;
    let viewport_width = scroller.width().max(1) as f32;
    let viewport_height = scroller.height().max(1) as f32;
    Some(
        bounds.x() < viewport_width
            && bounds.x() + bounds.width() > 0.0
            && bounds.y() < viewport_height
            && bounds.y() + bounds.height() > 0.0,
    )
}

fn ui_perf_image_ref_kind(image_ref: &ImageRef) -> &'static str {
    let item_id = image_ref.item_id.as_str();
    if item_id.starts_with("local:cover:") {
        "local_cover"
    } else if item_id.starts_with("local:track:") {
        "local_track"
    } else if item_id.starts_with("local:album:") {
        "local_album"
    } else if item_id.starts_with("local:artist:") {
        "local_artist"
    } else if item_id.starts_with("local:") {
        "local_other"
    } else if item_id.starts_with("external:album:") {
        "external_album"
    } else if item_id.starts_with("external:artist:") {
        "external_artist"
    } else if item_id.starts_with("external:") {
        "external_other"
    } else {
        "provider"
    }
}

fn ui_perf_cover_has_active_work(shell: &Shell, candidate_keys: &[String]) -> bool {
    candidate_keys
        .iter()
        .any(|key| shell.state.cover_decodes.borrow().contains_key(key))
        || candidate_keys.iter().any(|key| {
            shell
                .state
                .cover_decode_queue
                .borrow()
                .iter()
                .any(|job| &job.key == key)
        })
        || candidate_keys
            .iter()
            .any(|key| shell.state.cover_path_lookups.borrow().contains_key(key))
        || candidate_keys
            .iter()
            .any(|key| shell.state.cover_fetches.borrow().contains(key))
        || candidate_keys.iter().any(|key| {
            shell
                .state
                .startup_cover_prime_pending
                .borrow()
                .contains(key)
        })
        || candidate_keys.iter().any(|key| {
            shell
                .state
                .first_run_cover_prime_pending
                .borrow()
                .contains(key)
        })
}

fn ui_perf_pending_cover_state(shell: &Shell, candidate_keys: &[String]) -> &'static str {
    if candidate_keys
        .iter()
        .any(|key| shell.state.cover_decodes.borrow().contains_key(key))
    {
        return "decoding";
    }
    if candidate_keys.iter().any(|key| {
        shell
            .state
            .cover_decode_queue
            .borrow()
            .iter()
            .any(|job| &job.key == key)
    }) {
        return "queued_decode";
    }
    if candidate_keys
        .iter()
        .any(|key| shell.state.cover_path_lookups.borrow().contains_key(key))
    {
        return "path_lookup";
    }
    if candidate_keys
        .iter()
        .any(|key| shell.state.cover_fetches.borrow().contains(key))
    {
        return "fetching";
    }
    if candidate_keys.iter().any(|key| {
        shell
            .state
            .startup_cover_prime_pending
            .borrow()
            .contains(key)
    }) {
        return "startup_prime";
    }
    if candidate_keys.iter().any(|key| {
        shell
            .state
            .first_run_cover_prime_pending
            .borrow()
            .contains(key)
    }) {
        return "first_run_prime";
    }
    "idle_unresolved"
}

fn ui_perf_visible_cover_window(shell: &Shell, route: &Route) -> UiPerfVisibleCoverWindow {
    match route {
        Route::Home => ui_perf_home_visible_cover_window(shell),
        Route::Favorites => ui_perf_track_visible_cover_window(
            shell,
            LibraryListKey::FavoriteTracks,
            shell.state.library.borrow().favorites.clone(),
            true,
        ),
        Route::Tracks => ui_perf_track_visible_cover_window(
            shell,
            LibraryListKey::Tracks,
            shell.state.library.borrow().tracks.clone(),
            false,
        ),
        Route::Albums => ui_perf_album_visible_cover_window(shell),
        Route::Artists => ui_perf_artist_visible_cover_window(shell, false),
        Route::AlbumArtists => ui_perf_artist_visible_cover_window(shell, true),
        Route::Genres => ui_perf_genre_visible_cover_window(shell),
        Route::Playlists => ui_perf_playlist_visible_cover_window(shell),
        Route::SmartPlaylists => ui_perf_smart_playlist_visible_cover_window(shell),
        _ => UiPerfVisibleCoverWindow {
            layout: "none",
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        },
    }
}

fn ui_perf_home_visible_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let refs = startup_home_cover_prime_targets(shell)
        .into_iter()
        .map(|target| UiPerfVisibleCoverRef {
            image_ref: target.image_ref,
            fetch_size: target.fetch_size,
            size: target.size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout: "home",
        visible_start: 0,
        visible_end: refs.len(),
        coverless: 0,
        refs,
    }
}

fn ui_perf_track_visible_cover_window(
    shell: &Shell,
    key: LibraryListKey,
    mut tracks: Vec<Track>,
    favorite_first: bool,
) -> UiPerfVisibleCoverWindow {
    let settings = shell.library_settings(key);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let tracks = if key == LibraryListKey::Tracks {
        let route_tracks = shell.state.route_tracks.borrow();
        if route_tracks.is_empty() {
            library::sort_tracks(&mut tracks, &settings, favorite_first);
            tracks
        } else {
            route_tracks.clone()
        }
    } else {
        library::sort_tracks(&mut tracks, &settings, favorite_first);
        tracks
    };
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, tracks.len(), settings.layout);
    let visible_tracks = &tracks[visible_start..visible_end];
    let coverless = visible_tracks
        .iter()
        .filter(|track| track.image_ref.is_none())
        .count();
    let refs = visible_tracks
        .iter()
        .filter_map(|track| track.image_ref.clone())
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_track_leading_cover_window(
    shell: &Shell,
    leading_rows: usize,
) -> UiPerfVisibleCoverWindow {
    let key = LibraryListKey::Tracks;
    let settings = shell.library_settings(key);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut tracks = shell.state.library.borrow().tracks.clone();
    library::sort_tracks(&mut tracks, &settings, false);
    let visible_end = leading_rows.min(tracks.len());
    let visible_tracks = &tracks[..visible_end];
    let coverless = visible_tracks
        .iter()
        .filter(|track| track.image_ref.is_none())
        .count();
    let refs = visible_tracks
        .iter()
        .filter_map(|track| track.image_ref.clone())
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start: 0,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_track_anchor_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let key = LibraryListKey::Tracks;
    let settings = shell.library_settings(key);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut tracks = shell.state.library.borrow().tracks.clone();
    library::sort_tracks(&mut tracks, &settings, false);
    let total = tracks.len();
    if total == 0 {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    }

    let visible_rows = ui_perf_initial_visible_count(shell, settings.layout)
        .max(1)
        .min(total);
    let mut refs = Vec::new();
    let mut coverless = 0_usize;
    for numerator in [1_usize, 2, 3, 4] {
        let start = total.saturating_sub(visible_rows).saturating_mul(numerator) / 4;
        let end = start.saturating_add(visible_rows).min(total);
        for track in &tracks[start..end] {
            if let Some(image_ref) = track.image_ref.clone() {
                refs.push(UiPerfVisibleCoverRef {
                    image_ref,
                    fetch_size,
                    size,
                });
            } else {
                coverless = coverless.saturating_add(1);
            }
        }
    }

    UiPerfVisibleCoverWindow {
        layout,
        visible_start: 0,
        visible_end: total,
        coverless,
        refs,
    }
}

fn ui_perf_album_visible_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let settings = shell.library_settings(LibraryListKey::Albums);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut albums = shell.state.library.borrow().albums.clone();
    library::sort_albums(&mut albums, &settings);
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, albums.len(), settings.layout);
    let visible_albums = &albums[visible_start..visible_end];
    let coverless = visible_albums
        .iter()
        .filter(|album| album.image_ref.is_none())
        .count();
    let refs = visible_albums
        .iter()
        .filter_map(|album| album.image_ref.clone())
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_artist_visible_cover_window(
    shell: &Shell,
    album_artist: bool,
) -> UiPerfVisibleCoverWindow {
    let key = if album_artist {
        LibraryListKey::AlbumArtists
    } else {
        LibraryListKey::Artists
    };
    let settings = shell.library_settings(key);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut artists = if album_artist {
        shell.state.library.borrow().album_artists.clone()
    } else {
        shell.state.library.borrow().artists.clone()
    };
    library::sort_artists(&mut artists, &settings);
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, artists.len(), settings.layout);
    let visible_artists = &artists[visible_start..visible_end];
    let coverless = visible_artists
        .iter()
        .filter(|artist| artist.image_ref.is_none())
        .count();
    let refs = visible_artists
        .iter()
        .filter_map(|artist| artist.image_ref.clone())
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_genre_visible_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let settings = shell.library_settings(LibraryListKey::Genres);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let library = shell.state.library.borrow();
    let mut genres = library.genres.clone();
    library::sort_genres(&mut genres, &settings);
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, genres.len(), settings.layout);
    let mut coverless = 0_usize;
    let mut image_refs = Vec::new();
    for genre in &genres[visible_start..visible_end] {
        let mut refs = genre.image_refs.clone();
        refs.extend(genre.image_ref.iter().cloned());
        if refs.is_empty() {
            coverless = coverless.saturating_add(1);
        } else {
            image_refs.extend(refs);
        }
    }
    let refs = image_refs
        .into_iter()
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_playlist_visible_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let settings = shell.library_settings(LibraryListKey::Playlists);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut playlists = shell.state.library.borrow().playlists.clone();
    library::sort_playlists(&mut playlists, &settings);
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, playlists.len(), settings.layout);
    let visible_playlists = &playlists[visible_start..visible_end];
    let mut coverless = 0_usize;
    let mut image_refs = Vec::new();
    for playlist in visible_playlists {
        let mut refs = playlist.image_refs.clone();
        refs.extend(playlist.image_ref.iter().cloned());
        if refs.is_empty() {
            coverless = coverless.saturating_add(1);
        } else {
            image_refs.extend(refs);
        }
    }
    let refs = image_refs
        .into_iter()
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_smart_playlist_visible_cover_window(shell: &Shell) -> UiPerfVisibleCoverWindow {
    let settings = shell.library_settings(LibraryListKey::SmartPlaylists);
    let layout = ui_perf_library_layout_name(settings.layout);
    let Some((fetch_size, size)) = ui_perf_cover_sizes(shell, &settings) else {
        return UiPerfVisibleCoverWindow {
            layout,
            visible_start: 0,
            visible_end: 0,
            coverless: 0,
            refs: Vec::new(),
        };
    };
    let mut playlists = shell.state.smart_playlists.borrow().clone();
    library::sort_smart_playlists(&mut playlists, &settings);
    let (visible_start, visible_end) =
        ui_perf_visible_index_range(shell, playlists.len(), settings.layout);
    let visible_playlists = &playlists[visible_start..visible_end];
    let mut coverless = 0_usize;
    let mut image_refs = Vec::new();
    for playlist in visible_playlists {
        let mut refs = playlist.image_refs.clone();
        refs.extend(playlist.image_ref.iter().cloned());
        if refs.is_empty() {
            coverless = coverless.saturating_add(1);
        } else {
            image_refs.extend(refs);
        }
    }
    let refs = image_refs
        .into_iter()
        .map(|image_ref| UiPerfVisibleCoverRef {
            image_ref,
            fetch_size,
            size,
        })
        .collect::<Vec<_>>();
    UiPerfVisibleCoverWindow {
        layout,
        visible_start,
        visible_end,
        coverless,
        refs,
    }
}

fn ui_perf_cover_sizes(shell: &Shell, settings: &LibraryListSettings) -> Option<(u32, i32)> {
    match settings.layout {
        LibraryLayout::Grid => Some((GRID_COVER_SIZE, shell.responsive_card_grid_metrics().1)),
        LibraryLayout::Detail => Some((GRID_COVER_SIZE, GRID_COVER_SIZE as i32)),
        LibraryLayout::Row if row_layout_uses_cover(settings) => Some((THUMB_COVER_SIZE, 48)),
        LibraryLayout::Row => None,
    }
}

fn ui_perf_visible_index_range(
    shell: &Shell,
    total: usize,
    layout: LibraryLayout,
) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let Some(scroller) = find_largest_scrolled_window(&shell.route_host.clone().upcast()) else {
        return (0, ui_perf_initial_visible_count(shell, layout).min(total));
    };
    let adjustment = scroller.vadjustment();
    let offset = adjustment.value().max(0.0);
    let page_size = ui_perf_effective_page_size(shell, &scroller, &adjustment);
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    ui_perf_visible_index_range_from_metrics(
        total,
        layout,
        offset,
        page_size,
        library::LIBRARY_TABLE_ROW_HEIGHT.max(1),
        columns,
        card_size,
    )
}

fn ui_perf_effective_page_size(
    shell: &Shell,
    scroller: &gtk::ScrolledWindow,
    adjustment: &gtk::Adjustment,
) -> f64 {
    let fallback_height = scroller
        .height()
        .max(shell.route_host.height())
        .max(shell.app_root.height())
        .max(1);
    adjustment.page_size().max(f64::from(fallback_height))
}

pub(in crate::ui) fn ui_perf_visible_index_range_from_metrics(
    total: usize,
    layout: LibraryLayout,
    offset: f64,
    page_size: f64,
    row_height: i32,
    grid_columns: usize,
    grid_card_size: i32,
) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    match layout {
        LibraryLayout::Row => {
            let row_height = f64::from(row_height.max(1));
            let raw_start = (offset.max(0.0) / row_height).floor() as usize;
            let count = (page_size.max(1.0) / row_height).ceil().max(1.0) as usize;
            let count = count.min(total);
            let start = raw_start.min(total.saturating_sub(count));
            (start, start.saturating_add(count).min(total))
        }
        LibraryLayout::Grid | LibraryLayout::Detail => {
            let columns = grid_columns.max(1);
            let item_extent = f64::from(grid_card_size.saturating_add(88).max(1));
            let first_row = (offset.max(0.0) / item_extent).floor() as usize;
            let rows = (page_size.max(1.0) / item_extent).ceil().max(1.0) as usize + 1;
            let count = rows.saturating_mul(columns).max(columns).min(total);
            let raw_start = first_row.saturating_mul(columns);
            let start = raw_start.min(total.saturating_sub(count));
            (start, start.saturating_add(count).min(total))
        }
    }
}

fn ui_perf_initial_visible_count(shell: &Shell, layout: LibraryLayout) -> usize {
    let (columns, card_size) = shell.responsive_card_grid_metrics();
    ui_perf_initial_visible_count_from_metrics(
        layout,
        shell.route_host.height(),
        shell.app_root.height(),
        columns,
        card_size,
    )
}

pub(in crate::ui) fn ui_perf_initial_visible_count_from_metrics(
    layout: LibraryLayout,
    route_height: i32,
    app_height: i32,
    grid_columns: usize,
    grid_card_size: i32,
) -> usize {
    let viewport_height = route_height.max(app_height).max(1);
    match layout {
        LibraryLayout::Row => {
            let row_height = library::LIBRARY_TABLE_ROW_HEIGHT.max(1);
            (viewport_height / row_height).saturating_add(2).max(1) as usize
        }
        LibraryLayout::Grid | LibraryLayout::Detail => {
            let columns = grid_columns.max(1);
            let item_extent = grid_card_size.saturating_add(88).max(1);
            let rows = (viewport_height / item_extent).saturating_add(2).max(1) as usize;
            rows.saturating_mul(columns)
        }
    }
}

fn ui_perf_library_layout_name(layout: LibraryLayout) -> &'static str {
    match layout {
        LibraryLayout::Row => "row",
        LibraryLayout::Grid => "grid",
        LibraryLayout::Detail => "detail",
    }
}
pub(in crate::ui) fn ui_perf_route_probe_expects_scroll(shell: &Shell, route: &Route) -> bool {
    let library = shell.state.library.borrow();
    match route {
        Route::Home => !library.home_sections.is_empty(),
        Route::Albums => library.albums.len() > 12,
        Route::Tracks => library.tracks.len() > 20,
        Route::Artists => library.artists.len() > 12,
        Route::AlbumArtists => library.album_artists.len() > 12,
        Route::Genres => library.genres.len() > 12,
        Route::Playlists => library.playlists.len() > 12,
        _ => false,
    }
}
pub(in crate::ui) fn begin_ui_perf_route_probe_drag(run: UiPerfRouteProbeRun) {
    if ui_perf_route_probe_waits_for_scroll_geometry(
        ui_perf_route_probe_expects_scroll(&run.shell, &run.route),
        2,
        route_scroll_max(&run.shell),
        run.wait_started_at.elapsed(),
    ) {
        glib::timeout_add_local_once(
            Duration::from_millis(UI_PERF_ROUTE_READY_POLL_MS),
            move || begin_ui_perf_route_probe_drag(run),
        );
        return;
    }

    record_ui_perf_route_probe_drag_contract(&run, "ready_before_drag");
    run.perf
        .begin_scroll(run.route_name.clone(), UiPerfScenario::DragSweep);
    let route_name = run.route_name.clone();
    let scroll_source = Rc::new(RefCell::new(None::<glib::SourceId>));
    let next_drag_checkpoint = Rc::new(Cell::new(0_usize));
    let drag_checkpoint_hold_until = Rc::new(RefCell::new(None::<Instant>));
    let drag_checkpoint_hold_started = Rc::new(RefCell::new(None::<Instant>));
    let paused_drag_duration = Rc::new(Cell::new(Duration::ZERO));
    let sample_delay_ms = ui_perf_route_probe_mid_drag_sample_delay_ms(run.perf.options.drag_ms);
    let drag_run_duration = Duration::from_millis(run.perf.options.drag_ms.max(1)).saturating_add(
        Duration::from_millis(sample_delay_ms)
            .saturating_mul(UI_PERF_ROUTE_DRAG_CHECKPOINTS.len() as u32),
    );

    if let Some(scroller) = find_largest_scrolled_window(&run.shell.route_host.clone().upcast()) {
        let perf_for_tick = Rc::clone(&run.perf);
        let route_for_tick = route_name.clone();
        let shell_for_tick = Rc::clone(&run.shell);
        let route_for_contract = run.route.clone();
        let route_name_for_contract = route_name.clone();
        let next_drag_checkpoint_for_tick = Rc::clone(&next_drag_checkpoint);
        let drag_checkpoint_hold_until_for_tick = Rc::clone(&drag_checkpoint_hold_until);
        let drag_checkpoint_hold_started_for_tick = Rc::clone(&drag_checkpoint_hold_started);
        let paused_drag_duration_for_tick = Rc::clone(&paused_drag_duration);
        let duration = Duration::from_millis(run.perf.options.drag_ms.max(1));
        let started_at = Instant::now();
        let id = glib::timeout_add_local(Duration::from_millis(16), move || {
            let hold_until = *drag_checkpoint_hold_until_for_tick.borrow();
            if let Some(hold_until) = hold_until {
                if Instant::now() < hold_until {
                    return glib::ControlFlow::Continue;
                }
                *drag_checkpoint_hold_until_for_tick.borrow_mut() = None;
                if let Some(hold_started) =
                    drag_checkpoint_hold_started_for_tick.borrow_mut().take()
                {
                    paused_drag_duration_for_tick.set(
                        paused_drag_duration_for_tick
                            .get()
                            .saturating_add(hold_started.elapsed()),
                    );
                }
            }
            let adjustment = scroller.vadjustment();
            let page_size = adjustment.page_size().max(1.0);
            let max_value = (adjustment.upper() - page_size).max(0.0);
            if max_value > 1.0 {
                let elapsed = started_at
                    .elapsed()
                    .saturating_sub(paused_drag_duration_for_tick.get());
                let progress = elapsed.as_secs_f64() / duration.as_secs_f64();
                let checkpoint = ui_perf_route_probe_drag_checkpoint_phase(
                    progress,
                    next_drag_checkpoint_for_tick.get(),
                );
                let next_progress = checkpoint
                    .map(|(_, _, threshold)| threshold)
                    .unwrap_or(progress)
                    .clamp(0.0, 1.0);
                let next = max_value * next_progress;
                adjustment.set_value(next);
                perf_for_tick.record_scroll_step(&route_for_tick, next, max_value);
                if let Some((phase, next_checkpoint, _threshold)) = checkpoint {
                    next_drag_checkpoint_for_tick.set(next_checkpoint);
                    *drag_checkpoint_hold_started_for_tick.borrow_mut() = Some(Instant::now());
                    *drag_checkpoint_hold_until_for_tick.borrow_mut() =
                        Some(Instant::now() + Duration::from_millis(sample_delay_ms));
                    let shell_for_sample = Rc::clone(&shell_for_tick);
                    let perf_for_sample = Rc::clone(&perf_for_tick);
                    let route_for_sample = route_for_contract.clone();
                    let route_name_for_sample = route_name_for_contract.clone();
                    glib::timeout_add_local_once(
                        Duration::from_millis(sample_delay_ms),
                        move || {
                            let visible_contract = ui_perf_route_visible_contract(
                                &shell_for_sample,
                                route_name_for_sample.clone(),
                                &route_for_sample,
                                phase,
                            );
                            println!(
                                "RUFIN_ROUTE_PROBE route_drag route={} phase={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={}",
                                route_name_for_sample,
                                phase,
                                visible_contract.layout,
                                visible_contract.visible_start,
                                visible_contract.visible_end,
                                visible_contract.expected_visible,
                                visible_contract.ready,
                                visible_contract.final_missing,
                                visible_contract.pending,
                                visible_contract.rendered_expected,
                                visible_contract.rendered_ready,
                                visible_contract.rendered_final_missing,
                                visible_contract.rendered_fallback,
                                visible_contract.fallback_after_reveal,
                                visible_contract.pending_assets,
                                visible_contract.active_decodes,
                                visible_contract.queued_decodes,
                                visible_contract.path_lookups,
                            );
                            perf_for_sample.record_route_visible_contract(visible_contract);
                        },
                    );
                }
            }
            glib::ControlFlow::Continue
        });
        *scroll_source.borrow_mut() = Some(id);
    } else {
        run.perf
            .record_scroll_note(&route_name, "no_scrolled_window");
    }

    glib::timeout_add_local_once(drag_run_duration, move || {
        if let Some(source) = scroll_source.borrow_mut().take() {
            source.remove();
        }
        run.perf.finish_scroll();
        glib::timeout_add_local_once(
            Duration::from_millis(UI_PERF_ROUTE_PROBE_SCROLL_SETTLE_MS),
            move || {
                let visible_contract = ui_perf_route_visible_contract(
                    &run.shell,
                    run.route_name.clone(),
                    &run.route,
                    "drag_done",
                );
                println!(
                    "RUFIN_ROUTE_PROBE route_done route={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={}",
                    run.route_name,
                    visible_contract.layout,
                    visible_contract.visible_start,
                    visible_contract.visible_end,
                    visible_contract.expected_visible,
                    visible_contract.ready,
                    visible_contract.final_missing,
                    visible_contract.pending,
                    visible_contract.rendered_expected,
                    visible_contract.rendered_ready,
                    visible_contract.rendered_final_missing,
                    visible_contract.rendered_fallback,
                    visible_contract.fallback_after_reveal,
                    visible_contract.pending_assets,
                    visible_contract.active_decodes,
                    visible_contract.queued_decodes,
                    visible_contract.path_lookups,
                );
                run.perf.record_route_visible_contract(visible_contract);
                run_next_ui_perf_route_probe(
                    run.shell,
                    run.app,
                    run.perf,
                    run.routes,
                    run.heartbeat,
                );
            },
        );
    });
}

fn record_ui_perf_route_probe_drag_contract(run: &UiPerfRouteProbeRun, phase: &'static str) {
    let visible_contract =
        ui_perf_route_visible_contract(&run.shell, run.route_name.clone(), &run.route, phase);
    println!(
        "RUFIN_ROUTE_PROBE route_drag route={} phase={} layout={} visible_start={} visible_end={} expected_visible={} ready={} final_missing={} pending={} rendered_expected={} rendered_ready={} rendered_final_missing={} rendered_fallback={} fallback_after_reveal={} pending_assets={} active_decodes={} queued_decodes={} path_lookups={}",
        run.route_name,
        phase,
        visible_contract.layout,
        visible_contract.visible_start,
        visible_contract.visible_end,
        visible_contract.expected_visible,
        visible_contract.ready,
        visible_contract.final_missing,
        visible_contract.pending,
        visible_contract.rendered_expected,
        visible_contract.rendered_ready,
        visible_contract.rendered_final_missing,
        visible_contract.rendered_fallback,
        visible_contract.fallback_after_reveal,
        visible_contract.pending_assets,
        visible_contract.active_decodes,
        visible_contract.queued_decodes,
        visible_contract.path_lookups,
    );
    run.perf.record_route_visible_contract(visible_contract);
}
pub(in crate::ui) fn start_ui_perf_heartbeat(perf: Rc<UiPerfMonitor>) -> glib::SourceId {
    let last_tick = Rc::new(RefCell::new(Instant::now()));
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let now = Instant::now();
        let gap = now.saturating_duration_since(*last_tick.borrow());
        *last_tick.borrow_mut() = now;
        perf.record_tick_gap(gap);
        glib::ControlFlow::Continue
    })
}
pub(in crate::ui) fn wait_for_ui_perf_startup_reveal(
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    runs: Rc<RefCell<VecDeque<(Route, UiPerfScenario)>>>,
    started_at: Instant,
) {
    if (shell.state.startup_route_revealed.get() && !shell.state.startup_route_render_pending.get())
        || shell.login_screen_active()
    {
        println!(
            "RUFIN_PERF startup_reveal elapsed_ms={}",
            duration_ms(started_at.elapsed())
        );
        let heartbeat = Rc::new(RefCell::new(Some(start_ui_perf_heartbeat(Rc::clone(
            &perf,
        )))));
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            run_next_ui_perf_route(shell, app, perf, runs, heartbeat);
        });
        return;
    }
    if startup_reveal_wait_timed_out(started_at) {
        perf.record_startup_reveal();
        println!(
            "RUFIN_PERF startup_reveal_timeout elapsed_ms={}",
            duration_ms(started_at.elapsed())
        );
        finish_ui_perf_run(perf, app);
        return;
    }

    glib::timeout_add_local_once(
        Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS),
        move || {
            wait_for_ui_perf_startup_reveal(shell, app, perf, runs, started_at);
        },
    );
}
fn startup_reveal_wait_timed_out(started_at: Instant) -> bool {
    started_at.elapsed()
        >= Duration::from_millis(
            STARTUP_ROUTE_REVEAL_MAX_MS
                .saturating_add(RESPONSIVE_RENDER_DELAY_MS)
                .saturating_add(1_000),
        )
}
pub(in crate::ui) fn run_next_ui_perf_route(
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
    if shell.state.routes.borrow().current() == &route {
        reset_ui_perf_route_scroll_position(&shell);
    } else {
        shell.navigate(route);
    }

    let shell_for_scroll = Rc::clone(&shell);
    let app_for_next = app.clone();
    let perf_for_scroll = Rc::clone(&perf);
    let runs_for_next = Rc::clone(&runs);
    let heartbeat_for_next = Rc::clone(&heartbeat);
    glib::timeout_add_local_once(Duration::from_millis(120), move || {
        begin_ui_perf_route_scroll(UiPerfRouteScrollRun {
            shell: shell_for_scroll,
            app: app_for_next,
            perf: perf_for_scroll,
            runs: runs_for_next,
            heartbeat: heartbeat_for_next,
            route_name,
            scenario,
        });
    });
}
pub(in crate::ui) fn begin_ui_perf_route_scroll(run: UiPerfRouteScrollRun) {
    run.perf.begin_scroll(run.route_name.clone(), run.scenario);
    let scroll_source = Rc::new(RefCell::new(None::<glib::SourceId>));
    if let Some(scroller) = find_largest_scrolled_window(&run.shell.route_host.clone().upcast()) {
        let direction = Rc::new(Cell::new(1.0_f64));
        let jump_index = Rc::new(Cell::new(0_usize));
        let perf_for_tick = Rc::clone(&run.perf);
        let route_for_tick = run.route_name.clone();
        let direction_for_tick = Rc::clone(&direction);
        let jump_index_for_tick = Rc::clone(&jump_index);
        let scenario = run.scenario;
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
        run.perf
            .record_scroll_note(&run.route_name, "no_scrolled_window");
    }

    glib::timeout_add_local_once(
        Duration::from_millis(run.perf.options.route_ms),
        move || {
            if let Some(source) = scroll_source.borrow_mut().take() {
                source.remove();
            }
            run.perf.finish_scroll();
            run_next_ui_perf_route(run.shell, run.app, run.perf, run.runs, run.heartbeat);
        },
    );
}
pub(in crate::ui) fn reset_ui_perf_route_scroll_position(shell: &Shell) {
    if let Some(scroller) = find_largest_scrolled_window(&shell.route_host.clone().upcast()) {
        scroller.vadjustment().set_value(0.0);
    }
}
pub(in crate::ui) fn route_scroll_max(shell: &Shell) -> f64 {
    let Some(scroller) = find_largest_scrolled_window(&shell.route_host.clone().upcast()) else {
        return 0.0;
    };
    let adjustment = scroller.vadjustment();
    (adjustment.upper() - adjustment.page_size()).max(0.0)
}
pub(in crate::ui) fn finish_ui_perf_run(perf: Rc<UiPerfMonitor>, app: adw::Application) {
    let failed = write_ui_perf_report(&perf, true);
    app.quit();
    if failed {
        std::process::exit(1);
    }
}
pub(in crate::ui) fn write_ui_perf_report(perf: &UiPerfMonitor, print_stdout: bool) -> bool {
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
pub(in crate::ui) fn ui_perf_plan_summary(plan: &VecDeque<(Route, UiPerfScenario)>) -> String {
    let mut summary = String::new();
    for (index, (route, scenario)) in plan.iter().enumerate() {
        if index > 0 {
            summary.push_str(" -> ");
        }
        let _ = write!(summary, "{}:{route:?}:{}", index + 1, scenario.name());
    }
    summary
}
pub(in crate::ui) fn ui_perf_route_probe_plan(shell: &Shell) -> VecDeque<Route> {
    let settings = shell.state.settings.borrow().clone();
    let mut routes = VecDeque::new();
    routes.push_back(Route::Home);
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Tracks,
        Route::Tracks,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Albums,
        Route::Albums,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Artists,
        Route::Artists,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Genres,
        Route::Genres,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::AlbumArtists,
        Route::AlbumArtists,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Favorites,
        Route::Favorites,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Folders,
        Route::Folders { path: Vec::new() },
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::Playlists,
        Route::Playlists,
    );
    push_ui_perf_probe_route(
        &mut routes,
        &settings,
        SidebarRouteItem::SmartPlaylists,
        Route::SmartPlaylists,
    );
    routes
}
pub(in crate::ui) fn push_ui_perf_probe_route(
    routes: &mut VecDeque<Route>,
    settings: &AppSettings,
    item: SidebarRouteItem,
    route: Route,
) {
    if sidebar_route_visible(settings, item) {
        routes.push_back(route);
    }
}
pub(in crate::ui) fn ui_perf_route_probe_plan_summary(plan: &VecDeque<Route>) -> String {
    let mut summary = String::new();
    for (index, route) in plan.iter().enumerate() {
        if index > 0 {
            summary.push_str(" -> ");
        }
        let _ = write!(summary, "{}:{route:?}", index + 1);
    }
    summary
}
pub(in crate::ui) fn ui_perf_critical_plan(shell: &Shell) -> Vec<(Route, UiPerfScenario)> {
    let mut runs = vec![
        (Route::Tracks, UiPerfScenario::HumanScroll),
        (Route::Tracks, UiPerfScenario::FastScroll),
        (Route::Tracks, UiPerfScenario::FullSweep),
        (Route::Tracks, UiPerfScenario::DragSweep),
        (Route::Albums, UiPerfScenario::HumanScroll),
        (Route::Albums, UiPerfScenario::FastScroll),
        (Route::Albums, UiPerfScenario::DragSweep),
        (Route::Tracks, UiPerfScenario::HumanScroll),
        (Route::Albums, UiPerfScenario::HumanScroll),
    ];
    let album_id = {
        let library = shell.state.library.borrow();
        library
            .albums
            .iter()
            .find(|album| album.image_ref.is_some())
            .or_else(|| library.albums.first())
            .map(|album| album.id.clone())
    };
    if let Some(album_id) = album_id {
        runs.push((Route::AlbumDetail(album_id), UiPerfScenario::HumanScroll));
    }
    runs
}
pub(in crate::ui) fn ui_perf_broad_routes(shell: &Shell) -> Vec<Route> {
    let library = shell.state.library.borrow();
    let settings = shell.state.settings.borrow().clone();
    let artists_visible = sidebar_route_visible(&settings, SidebarRouteItem::Artists);
    let album_artists_visible = sidebar_route_visible(&settings, SidebarRouteItem::AlbumArtists);
    let genres_visible = sidebar_route_visible(&settings, SidebarRouteItem::Genres);
    let playlists_visible = sidebar_route_visible(&settings, SidebarRouteItem::Playlists);
    let mut routes = Vec::new();
    if artists_visible {
        routes.push(Route::Artists);
    }
    if sidebar_route_visible(&settings, SidebarRouteItem::Favorites) {
        routes.push(Route::Favorites);
    }
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
    if sidebar_route_visible(&settings, SidebarRouteItem::Favorites) {
        routes.push(Route::Favorites);
    }
    if artists_visible {
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
    }
    if album_artists_visible {
        routes.push(Route::AlbumArtists);
        if let Some(artist) = library
            .album_artists
            .iter()
            .find(|artist| artist.image_ref.is_some())
            .or_else(|| library.album_artists.first())
        {
            routes.push(Route::ArtistDetail(artist.id.clone()));
        }
    }
    if genres_visible {
        routes.push(Route::Genres);
    }
    if playlists_visible {
        routes.push(Route::Playlists);
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
    routes.push(Route::Home);
    routes
}
pub(in crate::ui) fn ui_perf_plan(
    shell: &Shell,
    duration_ms: u64,
    route_ms: u64,
) -> VecDeque<(Route, UiPerfScenario)> {
    ui_perf_take_plan(
        ui_perf_critical_plan(shell),
        ui_perf_broad_routes(shell),
        duration_ms,
        route_ms,
    )
}
pub(in crate::ui) fn ui_perf_take_plan(
    mut critical: Vec<(Route, UiPerfScenario)>,
    broad_routes: Vec<Route>,
    duration_ms: u64,
    route_ms: u64,
) -> VecDeque<(Route, UiPerfScenario)> {
    critical.extend(
        broad_routes
            .into_iter()
            .map(|route| (route, UiPerfScenario::HumanScroll)),
    );
    if critical.is_empty() {
        critical.push((Route::Home, UiPerfScenario::HumanScroll));
    }
    let run_ms = route_ms.saturating_add(140).max(1);
    let needed = ((duration_ms.saturating_add(run_ms - 1)) / run_ms).max(1);
    critical
        .iter()
        .cloned()
        .cycle()
        .take(needed as usize)
        .collect()
}
