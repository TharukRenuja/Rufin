const UI_PERF_ROUTE_GATE_POLL_MS: u64 = 33;
const UI_PERF_ROUTE_GATE_TIMEOUT_MS: u64 = 3_500;

struct UiPerfRouteScrollRun {
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    runs: Rc<RefCell<VecDeque<(Route, UiPerfScenario)>>>,
    heartbeat: Rc<RefCell<Option<glib::SourceId>>>,
    route_name: String,
    scenario: UiPerfScenario,
    wait_started_at: Instant,
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
fn initial_window_size(width: Option<i32>, height: Option<i32>) -> (i32, i32) {
    sanitized_window_size(width, height).unwrap_or((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))
}
fn install_window_state_persistence(shell: &Rc<Shell>) {
    let save_shell = Rc::clone(shell);
    shell.application.connect_shutdown(move |_| {
        save_shell.save_window_state();
    });
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
                    if previous_track != next_track {
                        *shell.state.lyrics.borrow_mut() = None;
                        *shell.state.lyrics_track_id.borrow_mut() = next_track.clone();
                        shell.lyrics_pane.clear_follow_scroll_pause();
                        shell.fullscreen_player.lyrics_pane.clear_follow_scroll_pause();
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
                    let sync_complete = status == LIBRARY_SYNC_COMPLETE_STATUS;
                    if sync_complete {
                        shell.state.first_run_connection_ready.set(true);
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
    let plan = ui_perf_plan(
        shell,
        perf.options.duration_ms,
        perf.options.route_ms,
    );
    println!("RUFIN_PERF route_plan {}", ui_perf_plan_summary(&plan));
    let runs = Rc::new(RefCell::new(plan));
    let shell = Rc::clone(shell);
    let app = app.clone();
    wait_for_ui_perf_startup_reveal(shell, app, perf, runs, Instant::now());
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
fn wait_for_ui_perf_startup_reveal(
    shell: Rc<Shell>,
    app: adw::Application,
    perf: Rc<UiPerfMonitor>,
    runs: Rc<RefCell<VecDeque<(Route, UiPerfScenario)>>>,
    started_at: Instant,
) {
    if shell.state.startup_route_revealed.get() || shell.login_screen_active() {
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

    glib::timeout_add_local_once(Duration::from_millis(STARTUP_ROUTE_REVEAL_POLL_MS), move || {
        wait_for_ui_perf_startup_reveal(shell, app, perf, runs, started_at);
    });
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
            wait_started_at: Instant::now(),
        });
    });
}
fn begin_ui_perf_route_scroll(run: UiPerfRouteScrollRun) {
    if route_cover_gate_active_for_current_route(&run.shell)
        && run.wait_started_at.elapsed() < Duration::from_millis(UI_PERF_ROUTE_GATE_TIMEOUT_MS)
    {
        glib::timeout_add_local_once(Duration::from_millis(UI_PERF_ROUTE_GATE_POLL_MS), move || {
            begin_ui_perf_route_scroll(run);
        });
        return;
    }

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

    glib::timeout_add_local_once(Duration::from_millis(run.perf.options.route_ms), move || {
        if let Some(source) = scroll_source.borrow_mut().take() {
            source.remove();
        }
        run.perf.finish_scroll();
        run_next_ui_perf_route(run.shell, run.app, run.perf, run.runs, run.heartbeat);
    });
}
fn route_cover_gate_active_for_current_route(shell: &Shell) -> bool {
    let route_key = match shell.state.routes.borrow().current() {
        Route::Tracks => "tracks",
        Route::Albums => "albums",
        Route::Artists => "artists",
        Route::AlbumArtists => "album_artists",
        _ => return false,
    };
    shell
        .state
        .route_cover_gate_started
        .borrow()
        .contains_key(route_key)
        && !shell
            .state
            .route_cover_gate_timed_out
            .borrow()
            .contains(route_key)
}
fn reset_ui_perf_route_scroll_position(shell: &Shell) {
    if let Some(scroller) = find_largest_scrolled_window(&shell.route_host.clone().upcast()) {
        scroller.vadjustment().set_value(0.0);
    }
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
fn ui_perf_plan_summary(plan: &VecDeque<(Route, UiPerfScenario)>) -> String {
    let mut summary = String::new();
    for (index, (route, scenario)) in plan.iter().enumerate() {
        if index > 0 {
            summary.push_str(" -> ");
        }
        let _ = write!(
            summary,
            "{}:{route:?}:{}",
            index + 1,
            scenario.name()
        );
    }
    summary
}
fn ui_perf_critical_plan(shell: &Shell) -> Vec<(Route, UiPerfScenario)> {
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
        runs.push((
            Route::AlbumDetail(album_id),
            UiPerfScenario::HumanScroll,
        ));
    }
    runs
}
fn ui_perf_broad_routes(shell: &Shell) -> Vec<Route> {
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
fn ui_perf_plan(shell: &Shell, duration_ms: u64, route_ms: u64) -> VecDeque<(Route, UiPerfScenario)> {
    ui_perf_take_plan(
        ui_perf_critical_plan(shell),
        ui_perf_broad_routes(shell),
        duration_ms,
        route_ms,
    )
}
fn ui_perf_take_plan(
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
