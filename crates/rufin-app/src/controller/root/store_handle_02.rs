impl AppController {
    pub fn toggle_auto_dj(&self) {
        let enabled = self
            .auto_dj_enabled
            .lock()
            .map(|mut current| {
                *current = !*current;
                *current
            })
            .unwrap_or(false);
        let mut settings = self.load_settings();
        settings.auto_dj_enabled = enabled;
        if let Err(error) = self.save_settings(&settings) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.auto_dj_enabled = enabled;
        });
        if enabled && self.auto_dj_top_up_or_emit_error() {
            self.persist_and_emit_queue();
        } else {
            self.emit_playback_snapshot();
        }
    }
    pub fn play_pause(&self) {
        let state = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.state)
            .unwrap_or(PlaybackState::Stopped);
        match state {
            PlaybackState::Playing | PlaybackState::Buffering => {
                if let Err(error) = self.send_playback_command(PlaybackCommand::Pause) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Paused;
                        snapshot.buffering_percent = None;
                    });
                    self.persist_current_queue_snapshot();
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Paused => {
                if let Err(error) = self.send_playback_command(PlaybackCommand::Resume) {
                    let _sent = self.events.send(ControllerEvent::Error(error));
                } else {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Playing;
                        snapshot.buffering_percent = None;
                    });
                    self.emit_playback_snapshot();
                    self.report_playback(PlaybackReportKind::Progress, false);
                }
            }
            PlaybackState::Stopped => self.start_current_track(),
        }
    }
    pub fn stop(&self) {
        self.report_playback(PlaybackReportKind::Stopped, false);
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(0);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::Stop) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.state = PlaybackState::Stopped;
            snapshot.position_seconds = 0;
            snapshot.position_millis = 0;
            snapshot.buffering_percent = None;
        });
        self.persist_and_emit_queue();
    }
    pub fn next_track(&self) {
        self.auto_dj_top_up_or_emit_error();
        let mut moved = false;
        let mut had_current = false;
        let result = self.with_queue_mut(|queue| {
            had_current = queue.current().is_some();
            moved = queue.next_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            if had_current {
                self.seek(0);
            } else {
                self.stop();
            }
            return;
        }
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn previous_track(&self) {
        let should_restart_current = self
            .playback_snapshot
            .lock()
            .map(|snapshot| snapshot.position_seconds > 10)
            .unwrap_or(false);
        if should_restart_current {
            self.seek(0);
            return;
        }
        let mut moved = false;
        let result = self.with_queue_mut(|queue| {
            moved = queue.previous_track().is_some();
            Ok(())
        });
        if let Err(error) = result {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        if !moved {
            self.seek(0);
            return;
        }
        self.auto_dj_top_up_or_emit_error();
        self.persist_and_emit_queue();
        self.start_current_track();
    }
    pub fn seek(&self, seconds: u32) {
        self.seek_millis(u64::from(seconds) * 1_000);
    }
    pub fn seek_millis(&self, millis: u64) {
        let seconds = (millis / 1_000).min(u64::from(u32::MAX)) as u32;
        let _result = self.with_queue_mut(|queue| {
            queue.set_progress_seconds(seconds);
            Ok(())
        });
        if let Err(error) = self.send_playback_command(PlaybackCommand::SeekMillis(millis)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
            return;
        }
        let queue_snapshot = self.queue_snapshot();
        if let Some(snapshot) = &queue_snapshot {
            self.persist_queue_snapshot(snapshot);
        }
        self.sync_playback_snapshot_from_queue();
        self.update_playback_snapshot(|snapshot| {
            snapshot.position_seconds = seconds;
            snapshot.position_millis = millis;
        });
        self.emit_playback_snapshot();
    }
    pub fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetVolume(volume)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.persist_playback_settings(|settings| {
                settings.volume = volume;
            });
            self.update_playback_snapshot(|snapshot| {
                snapshot.volume = volume;
            });
            self.emit_playback_snapshot();
        }
    }
    pub fn toggle_mute(&self) {
        let muted = self
            .playback_snapshot
            .lock()
            .map(|snapshot| !snapshot.muted)
            .unwrap_or(true);
        if let Err(error) = self.send_playback_command(PlaybackCommand::SetMuted(muted)) {
            let _sent = self.events.send(ControllerEvent::Error(error));
        } else {
            self.persist_playback_settings(|settings| {
                settings.muted = muted;
            });
            self.update_playback_snapshot(|snapshot| {
                snapshot.muted = muted;
            });
            self.emit_playback_snapshot();
        }
    }
    pub fn update_playback_settings(&self, mut playback_settings: PlaybackSettings) {
        playback_settings.sanitize();
        let mut settings = self.load_settings();
        if settings.playback != playback_settings {
            settings.playback = playback_settings.clone();
            if let Err(error) = self.save_settings(&settings) {
                let _sent = self.events.send(ControllerEvent::Error(error));
                return;
            }
        }
        if let Err(error) =
            self.send_playback_command(PlaybackCommand::UpdateSettings(playback_settings.clone()))
        {
            let _sent = self.events.send(ControllerEvent::Error(error));
        }
        self.update_playback_snapshot(|snapshot| {
            snapshot.volume = playback_settings.volume;
            snapshot.muted = playback_settings.muted;
        });
        self.prepare_next_stream();
        self.emit_playback_snapshot();
    }
    pub fn poll_playback_events(&self) {
        let events = self
            .playback
            .lock()
            .map(|mut playback| playback.drain_events())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                PlaybackEvent::StateChanged(state) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = state;
                        snapshot.buffering_percent = None;
                    });
                }
                PlaybackEvent::PositionChanged { seconds, millis } => {
                    let _result = self.with_queue_mut(|queue| {
                        queue.set_progress_seconds(seconds);
                        Ok(())
                    });
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.position_seconds = seconds;
                        snapshot.position_millis = millis;
                    });
                    self.persist_progress_if_needed(seconds);
                    self.report_playback_progress_if_needed(seconds);
                }
                PlaybackEvent::DurationChanged(seconds) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.duration_seconds = seconds;
                    });
                }
                PlaybackEvent::Buffering(percent) => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.state = PlaybackState::Buffering;
                        snapshot.buffering_percent = Some(percent);
                    });
                }
                PlaybackEvent::EndOfStream => self.advance_after_end_of_stream(),
                PlaybackEvent::PreparedTrackStarted(track) => {
                    self.advance_after_prepared_track_started(track);
                }
                PlaybackEvent::VolumeChanged { volume, muted } => {
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.volume = volume;
                        snapshot.muted = muted;
                    });
                }
                PlaybackEvent::Error(error) => {
                    self.report_playback(PlaybackReportKind::Stopped, true);
                    self.update_playback_snapshot(|snapshot| {
                        snapshot.last_error = Some(error.clone());
                        snapshot.state = PlaybackState::Stopped;
                    });
                    let _sent = self.events.send(ControllerEvent::Error(error));
                }
            }
        }
        self.emit_playback_snapshot();
    }
    #[cfg(test)]
    pub fn clear_active_server_cache(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Error(
                    "No active music server is saved.".to_string(),
                ));
                return;
            };
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before clearing cache."
                        .to_string(),
                ));
                return;
            }
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Cached library cleared.".to_string(),
            ));
            match load_snapshot(&store) {
                Ok(snapshot) => {
                    let _sent = events.send(ControllerEvent::Snapshot(Box::new(snapshot)));
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                }
            }
        });
    }
    pub fn clear_server_cache(&self, server_id: ServerId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                Ok(store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id))
            }) {
                Ok(Some(saved)) => saved,
                Ok(None) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before clearing cache."
                        .to_string(),
                ));
                return;
            }
            let result = store.with_store(|store| {
                store.clear_library_cache(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let _sent = events.send(ControllerEvent::LoginStatus(
                "Cached library cleared.".to_string(),
            ));
            emit_snapshot(&store, &events);
        });
    }
    #[cfg(test)]
    pub fn forget_active_server(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let Some(saved) = store
                .with_store(|store| store.active_server())
                .unwrap_or(None)
            else {
                let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                    LibrarySnapshot::first_run(),
                )));
                return;
            };
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before forgetting the server."
                        .to_string(),
                ));
                return;
            }
            if let Err(error) = secrets.delete_token(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error.to_string()));
                return;
            }
            let result = store.with_store(|store| {
                store.forget_server(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Ok(mut queue) = queue.lock() {
                *queue = None;
            }
            if let Ok(mut playback) = playback.lock() {
                let _result = playback.send(PlaybackCommand::Stop);
            }
            if let Ok(mut snapshot) = playback_snapshot.lock() {
                *snapshot = PlaybackSnapshot {
                    auto_dj_enabled: auto_dj_enabled
                        .lock()
                        .map(|enabled| *enabled)
                        .unwrap_or_default(),
                    ..PlaybackSnapshot::default()
                };
            }
            let _sent = events.send(ControllerEvent::Queue(Box::new(None)));
            let _sent = events.send(ControllerEvent::Playback(Box::new(PlaybackSnapshot {
                auto_dj_enabled: auto_dj_enabled
                    .lock()
                    .map(|enabled| *enabled)
                    .unwrap_or_default(),
                ..PlaybackSnapshot::default()
            })));
            let _sent = events.send(ControllerEvent::Snapshot(Box::new(
                LibrarySnapshot::first_run(),
            )));
        });
    }
    pub fn forget_server(&self, server_id: ServerId) {
        let store = self.store.clone();
        let events = self.events.clone();
        let secrets = Arc::clone(&self.secrets);
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        let sync_in_flight = Arc::clone(&self.sync_in_flight);
        thread::spawn(move || {
            let saved = match store.with_store(|store| {
                let active_id = store.active_server()?.map(|saved| saved.server.id);
                let saved = store
                    .list_servers()?
                    .into_iter()
                    .find(|saved| saved.server.id == server_id);
                Ok((saved, active_id))
            }) {
                Ok((Some(saved), active_id)) => (saved, active_id),
                Ok((None, _)) => {
                    let _sent = events.send(ControllerEvent::Error(
                        "The selected server is no longer saved.".to_string(),
                    ));
                    return;
                }
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let (saved, active_id) = saved;
            if sync_is_running(&sync_in_flight, &saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(
                    "Wait for the current library sync to finish before forgetting the server."
                        .to_string(),
                ));
                return;
            }
            if let Err(error) = secrets.delete_token(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error.to_string()));
                return;
            }
            let mut settings = load_settings_from_store(&store);
            if settings.sources.selected
                == Some(LibrarySourceSelection::Server(saved.server.id.clone()))
            {
                settings.sources.selected = None;
                if let Err(error) = store.save_settings(&settings) {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            }
            let result = store.with_store(|store| {
                store.forget_server(&saved.server.id)?;
                Ok(())
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if let Err(error) = clear_disk_cover_cache(&saved.server.id) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if active_id.as_ref() == Some(&saved.server.id) {
                if let Ok(mut queue) = queue.lock() {
                    *queue = None;
                }
                if let Ok(mut playback) = playback.lock() {
                    let _result = playback.send(PlaybackCommand::Stop);
                }
                if let Ok(mut snapshot) = playback_snapshot.lock() {
                    *snapshot = PlaybackSnapshot {
                        auto_dj_enabled: auto_dj_enabled
                            .lock()
                            .map(|enabled| *enabled)
                            .unwrap_or_default(),
                        ..PlaybackSnapshot::default()
                    };
                }
                let _sent = events.send(ControllerEvent::Queue(Box::new(None)));
                let _sent = events.send(ControllerEvent::Playback(Box::new(PlaybackSnapshot {
                    auto_dj_enabled: auto_dj_enabled
                        .lock()
                        .map(|enabled| *enabled)
                        .unwrap_or_default(),
                    ..PlaybackSnapshot::default()
                })));
            }
            emit_snapshot(&store, &events);
        });
    }
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, password), fields(provider = provider.provider_id(), server_url = %server_url, username = %username, trust_invalid_cert = trust_invalid_cert))]
    pub fn login(
        &self,
        provider: StreamingProvider,
        server_url: String,
        username: String,
        password: String,
        trust_invalid_cert: bool,
        local_access_root: Option<PathBuf>,
        path_replace_from: Option<String>,
    ) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let runtime = Arc::clone(&sync_context.runtime);
        let secrets = Arc::clone(&sync_context.secrets);
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let provider_name = provider.title();
            let _sent = events.send(ControllerEvent::LoginStatus(format!(
                "Checking {provider_name} server..."
            )));
            let result = runtime.block_on(login_provider(
                provider,
                server_url,
                username,
                password,
                trust_invalid_cert,
            ));

            let session = match result {
                Ok(session) => session,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error.to_string()));
                    return;
                }
            };

            let saved = match activate_logged_in_server(
                &store,
                &queue,
                &playback,
                &playback_snapshot,
                &auto_dj_enabled,
                &events,
                &session,
                trust_invalid_cert,
                local_access_root.as_deref(),
                path_replace_from.as_deref(),
            ) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if let Err(error) = secrets.save_token(&saved.server.id, &session.access_token) {
                let _sent = events.send(ControllerEvent::Error(error.to_string()));
                return;
            }

            start_sync_thread(sync_context, saved);
        });
    }
    pub fn add_local_server(&self, root_path: PathBuf) {
        self.add_local_server_folders(vec![root_path]);
    }
    pub fn add_local_server_folders(&self, root_paths: Vec<PathBuf>) {
        self.add_local_library_folders_with_selection(root_paths, true);
    }
    pub fn add_local_library_folder(&self, root_path: PathBuf) {
        self.add_local_library_folders_with_selection(vec![root_path], false);
    }
    fn add_local_library_folders_with_selection(
        &self,
        root_paths: Vec<PathBuf>,
        select_local: bool,
    ) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            if root_paths.is_empty() {
                let _sent = events.send(ControllerEvent::Error(
                    "Choose at least one local music folder.".to_string(),
                ));
                return;
            }
            let mut local_paths = Vec::new();
            for root_path in root_paths {
                match LocalProvider::identity_for_root(&root_path) {
                    Ok(identity) => {
                        if !local_paths.iter().any(|path| path == &identity.base_url) {
                            local_paths.push(identity.base_url);
                        }
                    }
                    Err(error) => {
                        let _sent = events.send(ControllerEvent::Error(error.to_string()));
                        return;
                    }
                }
            }
            let mut settings = load_settings_from_store(&store);
            for path in local_paths {
                if !settings
                    .sources
                    .local_folders
                    .iter()
                    .any(|folder| folder.path == path)
                {
                    settings
                        .sources
                        .local_folders
                        .push(LocalLibraryFolder { path });
                }
            }
            if select_local {
                settings.sources.selected = Some(LibrarySourceSelection::Local);
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            if select_local
                && let Err(error) =
                    store.with_store(|store| store.set_active_server(&saved.server.id))
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            if select_local
                && let Err(error) = activate_queue_for_saved_and_emit(
                    &store,
                    &queue,
                    &playback,
                    &playback_snapshot,
                    &auto_dj_enabled,
                    &events,
                    &saved,
                )
            {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if select_local {
                start_sync_thread(sync_context, saved);
            }
        });
    }
    pub fn remove_local_library_folder(&self, path: String) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            let before = settings.sources.local_folders.len();
            settings
                .sources
                .local_folders
                .retain(|folder| folder.path != path);
            if settings.sources.local_folders.len() == before {
                return;
            }
            let selected_local = matches!(
                settings.sources.selected,
                Some(LibrarySourceSelection::Local)
            );
            if selected_local && settings.sources.local_folders.is_empty() {
                settings.sources.selected = None;
            }
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            let saved = match ensure_local_source_server(&store) {
                Ok(saved) => saved,
                Err(error) => {
                    let _sent = events.send(ControllerEvent::Error(error));
                    return;
                }
            };
            let result = store.with_store(|store| {
                if selected_local && !settings.sources.local_folders.is_empty() {
                    store.set_active_server(&saved.server.id)?;
                }
                store.clear_library_cache(&saved.server.id)
            });
            if let Err(error) = result {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }
            emit_snapshot(&store, &events);
            if selected_local && !settings.sources.local_folders.is_empty() {
                start_sync_thread(sync_context, saved);
            }
        });
    }
    pub fn select_source(&self, source: LibrarySourceSelection) {
        let sync_context = self.sync_context();
        let store = sync_context.store.clone();
        let events = sync_context.events.clone();
        let queue = Arc::clone(&self.queue);
        let playback = Arc::clone(&self.playback);
        let playback_snapshot = Arc::clone(&self.playback_snapshot);
        let auto_dj_enabled = Arc::clone(&self.auto_dj_enabled);
        thread::spawn(move || {
            let mut settings = load_settings_from_store(&store);
            settings.sources.selected = Some(source.clone());
            settings.migrate_defaults();
            if let Err(error) = store.save_settings(&settings) {
                let _sent = events.send(ControllerEvent::Error(error));
                return;
            }

            let sync_saved = match source {
                LibrarySourceSelection::Local => {
                    let saved = match ensure_local_source_server(&store) {
                        Ok(saved) => saved,
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) =
                        store.with_store(|store| store.set_active_server(&saved.server.id))
                    {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    if let Err(error) = activate_queue_for_saved_and_emit(
                        &store,
                        &queue,
                        &playback,
                        &playback_snapshot,
                        &auto_dj_enabled,
                        &events,
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    (!settings.sources.local_folders.is_empty()).then_some(saved)
                }
                LibrarySourceSelection::Server(server_id) => {
                    let saved = match store.with_store(|store| {
                        let saved = store
                            .list_servers()?
                            .into_iter()
                            .find(|saved| saved.server.id == server_id);
                        if saved.is_some() {
                            store.set_active_server(&server_id)?;
                        }
                        Ok(saved)
                    }) {
                        Ok(Some(saved)) => saved,
                        Ok(None) => {
                            let _sent = events.send(ControllerEvent::Error(
                                "The selected source is no longer saved.".to_string(),
                            ));
                            return;
                        }
                        Err(error) => {
                            let _sent = events.send(ControllerEvent::Error(error));
                            return;
                        }
                    };
                    if let Err(error) = activate_queue_for_saved_and_emit(
                        &store,
                        &queue,
                        &playback,
                        &playback_snapshot,
                        &auto_dj_enabled,
                        &events,
                        &saved,
                    ) {
                        let _sent = events.send(ControllerEvent::Error(error));
                        return;
                    }
                    active_server_needs_sync(&store, &saved.server.id).then_some(saved)
                }
            };

            emit_snapshot(&store, &events);
            if let Some(saved) = sync_saved {
                start_sync_thread(sync_context, saved);
            }
        });
    }
}
