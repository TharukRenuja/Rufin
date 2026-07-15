use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ::library::{LibraryDelta, LibraryReset, SourceId};
use gtk::glib;
use playback::{PlaybackView, QueuePageQuery, WaveformProjection};
use sources::{
    LibraryCacheState, LibrarySourceSelection, ServerDiscoveryStatus, SourceNotice,
    SourcePresentationState,
};
use tracing::warn;

use crate::player::fullscreen::{FullscreenPlaybackRefresh, fullscreen_playback_refresh};
use crate::player::state::current_playback_media_key;
use crate::player::{now_playing_notification_can_send, now_playing_notification_should_withdraw};
use crate::preferences::source::{
    LibraryLoad, selector::update_source_selector, source_sync_progress_text,
};
use crate::routes::playlist_picker::refresh_context_playlist_picker;
use crate::routes::route::Route;
use crate::runtime::ProductReceivers;
use localization::{tr, tr_with};

use super::Shell;
use super::cover::THUMB_COVER_SIZE;
use super::route::route_current_track;

const PLAYBACK_BACKEND_POLL_INTERVAL: Duration = Duration::from_millis(33);
const SLOW_PLAYBACK_BACKEND_POLL_MS: u64 = 100;

fn source_notice_message(notice: &SourceNotice) -> String {
    match notice {
        SourceNotice::Checking { source_name } => tr_with(
            "Checking {provider} server...",
            &[("provider", source_name.as_str())],
        ),
        SourceNotice::Connected => tr("Connected. Loading cached library..."),
        SourceNotice::SettingsSaved => tr("Source settings saved."),
        SourceNotice::NoChanges => tr("No changes to save."),
        SourceNotice::CacheCleared => tr("Cached library cleared."),
    }
}
pub(crate) fn queue_source_waits_for_presentation(
    player: Option<&PlaybackView>,
    active_source_id: Option<&::library::SourceId>,
) -> bool {
    player.is_some_and(|player| active_source_id != Some(&player.transport.source_id))
}
pub(crate) fn queue_ready_for_library(
    player: Option<&PlaybackView>,
    library: &SourcePresentationState,
) -> bool {
    let Some(player) = player else {
        return true;
    };
    library
        .source
        .as_ref()
        .is_some_and(|server| server.id == player.transport.source_id)
}
pub(crate) fn install_product_event_receivers(shell: &Rc<Shell>, receivers: ProductReceivers) {
    let ProductReceivers {
        source_presentation,
        source_local_access,
        source_selection,
        source_discovery,
        source_notice,
        source_transition_failure,
        library_sync,
        library_fact,
        playback_projection,
        waveform,
        metadata_lyrics,
        artwork,
    } = receivers;

    install_playback_backend_poll(shell);

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(change) = source_selection.recv().await {
            apply_source_selection(&event_shell, change);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(update) = source_discovery.recv().await {
            apply_source_discovery(&event_shell, update);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(notice) = source_notice.recv().await {
            event_shell.apply_source_notice(notice);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(presentation) = source_presentation.recv().await {
            event_shell.apply_source_presentation(presentation);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(presentation) = source_local_access.recv().await {
            event_shell.apply_source_local_access_presentation(presentation);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(failure) = source_transition_failure.recv().await {
            event_shell.apply_source_transition_failed(failure.source_id, failure.error);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(event) = library_sync.recv().await {
            apply_library_sync_event(&event_shell, event);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(event) = library_fact.recv().await {
            apply_library_fact(&event_shell, event);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(projection) = playback_projection.recv().await {
            apply_playback_projection(&event_shell, projection);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(waveform) = waveform.recv().await {
            apply_waveform(&event_shell, waveform);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(event) = metadata_lyrics.recv().await {
            apply_metadata_event(&event_shell, event);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(event) = artwork.recv().await {
            apply_artwork_event(&event_shell, event);
        }
    });
}

fn install_playback_backend_poll(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(PLAYBACK_BACKEND_POLL_INTERVAL, move || {
        let playback_poll_started = Instant::now();
        shell.products.playback.transport.poll_events();
        let playback_poll_ms = playback_poll_started.elapsed().as_millis() as u64;
        if playback_poll_ms >= SLOW_PLAYBACK_BACKEND_POLL_MS {
            warn!(playback_poll_ms, "slow playback backend poll");
        }
        glib::ControlFlow::Continue
    });
}

fn apply_source_selection(shell: &Rc<Shell>, change: sources::SourceSelectionChanged) {
    let selected_source = change.selected_source;
    {
        let mut library = shell.source.presentation.borrow_mut();
        library.selected_source = Some(selected_source.clone());
        library.music_folders.clear();
        library.selected_music_folder_id = None;
    }
    *shell.source.load.borrow_mut() = LibraryLoad::Switching {
        target: selected_source,
    };
    shell.startup.route_revealed.set(false);
    update_source_selector(shell);
    shell.enter_startup_loading();
}

fn apply_source_discovery(shell: &Rc<Shell>, update: sources::ServerDiscoveryUpdate) {
    *shell.source.discovered_servers.borrow_mut() = update.servers;
    *shell.source.discovery_status.borrow_mut() = update.status;
    shell.source.discovery_running.set(update.running);
    if shell.source.presentation.borrow().first_run {
        shell.render_current_route();
    }
    shell.refresh_add_server_dialog();
}

fn apply_library_sync_event(shell: &Rc<Shell>, event: library_sync::LibrarySyncEvent) {
    match event {
        library_sync::LibrarySyncEvent::Committed(update) => {
            shell.apply_library_committed(update);
        }
        library_sync::LibrarySyncEvent::SyncChanged(change) => {
            shell.apply_source_sync_changed(change);
        }
    }
}

fn apply_library_fact(shell: &Rc<Shell>, event: ::library::LibraryEvent) {
    match event {
        ::library::LibraryEvent::Delta(delta) => {
            if !delta.playlists.is_empty() {
                refresh_context_playlist_picker(shell);
            }
            shell.apply_library_delta(*delta);
        }
        ::library::LibraryEvent::HomeSectionsChanged { source_id } => {
            if shell
                .library
                .query
                .borrow()
                .clone()
                .as_ref()
                .is_none_or(|query| query.source_id() != &source_id)
            {
                return;
            }
            let delta = LibraryDelta {
                home_changed: true,
                ..LibraryDelta::default()
            };
            shell.apply_library_delta(delta);
        }
        ::library::LibraryEvent::HomeSectionPrefetched { source_id, section } => {
            let active_source_id = shell
                .source
                .presentation
                .borrow()
                .source
                .as_ref()
                .map(|server| server.id.clone());
            if active_source_id.as_ref() == Some(&source_id) {
                shell.remember_prefetched_home_explore(source_id, section);
                if matches!(shell.navigation.routes.borrow().current(), Route::Home)
                    && !shell.startup.route_revealed.get()
                {
                    shell.prepare_startup_route_content();
                }
            }
        }
        ::library::LibraryEvent::HomeSectionProjectionFinished { source_id, section } => {
            if shell
                .library
                .query
                .borrow()
                .as_ref()
                .is_some_and(|query| query.source_id() == &source_id)
            {
                shell.finish_home_explore_promotion(&source_id, &section);
            }
        }
        ::library::LibraryEvent::FavoriteChanged { item_id, favorite } => {
            if shell.apply_favorite_changed(item_id.clone(), favorite) {
                shell.apply_library_delta(LibraryDelta::favorite_changed(&item_id));
            }
        }
        ::library::LibraryEvent::FavoriteChangeFailed {
            item_id,
            previous_favorite,
            error,
        } => {
            shell.restore_failed_favorite_change(&item_id, previous_favorite);
            warn!(%error, "favorite change failed");
        }
    }
}

fn apply_playback_projection(shell: &Rc<Shell>, projection: playback::PlaybackProjection) {
    let previous_player = shell.playback.player.borrow().clone();
    let previous_media = current_playback_media_key(&previous_player);
    let previous_route_track = route_current_track(previous_player.as_ref());
    let next_player = projection.view;
    let next_media = next_player
        .transport
        .current
        .as_ref()
        .map(|entry| playback::MediaKey {
            source_id: next_player.transport.source_id.clone(),
            track_id: entry.track.id.clone(),
        });
    let next_route_track = route_current_track(Some(&next_player));
    let notification_became_sendable = !now_playing_notification_can_send(
        &shell.settings.current.borrow(),
        previous_player.as_ref(),
    ) && now_playing_notification_can_send(
        &shell.settings.current.borrow(),
        Some(&next_player),
    );
    let lyrics_timing_changed = previous_media != next_media
        || previous_player
            .as_ref()
            .map(|player| player.transport.state)
            != Some(next_player.transport.state)
        || previous_player
            .as_ref()
            .map(|player| player.transport.position_millis)
            != Some(next_player.transport.position_millis);
    let fullscreen_refresh = fullscreen_playback_refresh(previous_player.as_ref(), &next_player);
    if let Some(queue_page) = projection.queue_page {
        shell.apply_queue_page_projection(queue_page);
    }
    if shell.queue.filter.borrow().trim().is_empty()
        && let Some(current_index) = next_player.queue.current_index
        && shell.queue.page.borrow().as_ref().is_none_or(|page| {
            page.query.follows_current()
                && (page.revision != next_player.queue.revision
                    || !page
                        .rows
                        .iter()
                        .any(|row| row.absolute_index == current_index))
        })
    {
        shell.request_queue_page(QueuePageQuery::current());
    }
    *shell.playback.player.borrow_mut() = Some(next_player.clone());
    if previous_route_track != next_route_track {
        shell.refresh_current_route_now_playing_selections();
    }
    if previous_media != next_media {
        shell.sync_bottom_player_favorite();
    }
    shell.maybe_clear_player_seek_preview(&next_player, previous_media != next_media);
    shell.update_bottom_player();
    #[cfg(unix)]
    let mut mpris_discontinuity = None;
    let mut notification_started_run = None;
    let mut media_changed_key = None;
    for notice in projection.notices {
        match notice {
            playback::PlaybackNotice::MediaChanged(media) => {
                media_changed_key = Some(media.key);
                shell.products.playback.waveform.request_current();
                shell.products.playback.waveform.warm_queue();
            }
            playback::PlaybackNotice::Visualizer { levels, .. } => {
                shell.apply_fullscreen_visualizer_levels(levels);
            }
            playback::PlaybackNotice::PositionDiscontinuity(discontinuity) => {
                #[cfg(unix)]
                {
                    mpris_discontinuity = Some(discontinuity);
                }
                #[cfg(not(unix))]
                {
                    let _ = discontinuity;
                }
            }
            playback::PlaybackNotice::RunStarted(run) => {
                notification_started_run = Some(run);
            }
        }
    }
    let lyrics_media_changed = match media_changed_key.as_ref() {
        Some(media_key) => previous_media.as_ref() != Some(media_key),
        None => previous_media.is_some() && next_media.is_none(),
    };
    if now_playing_notification_should_withdraw(
        &shell.settings.current.borrow(),
        Some(&next_player),
    ) {
        shell.withdraw_now_playing_notification();
    }
    let waits_for_source_presentation = {
        let library = shell.source.presentation.borrow();
        queue_source_waits_for_presentation(
            Some(&next_player),
            library.source.as_ref().map(|server| &server.id),
        )
    };
    if waits_for_source_presentation {
        if let Some(target) = shell.source.presentation.borrow().selected_source.clone() {
            *shell.source.load.borrow_mut() = LibraryLoad::Switching { target };
        }
        shell.startup.route_revealed.set(false);
        shell.enter_startup_loading();
        return;
    }
    let switch_ready = {
        let load = shell.source.load.borrow();
        let library = shell.source.presentation.borrow();
        matches!(
            &*load,
            LibraryLoad::Switching { target }
                if library.selected_source.as_ref() == Some(target)
                    && library.cache.is_committed()
                    && queue_ready_for_library(Some(&next_player), &library)
        )
    };
    if switch_ready {
        shell.finish_source_switch();
        return;
    }
    if matches!(&*shell.source.load.borrow(), LibraryLoad::Switching { .. }) {
        if lyrics_media_changed {
            *shell.lyrics.current.borrow_mut() = None;
            *shell.lyrics.loading_media.borrow_mut() = None;
            shell.lyrics.offset_millis.set(0);
            shell.right_panel.lyrics_pane.clear_follow_scroll_pause();
            shell
                .player_view
                .fullscreen_player
                .lyrics_pane
                .clear_follow_scroll_pause();
            shell.cancel_scheduled_lyrics_highlight();
        }
        return;
    }
    if lyrics_media_changed {
        *shell.lyrics.current.borrow_mut() = None;
        *shell.lyrics.loading_media.borrow_mut() = None;
        shell.lyrics.offset_millis.set(0);
        shell.right_panel.lyrics_pane.clear_follow_scroll_pause();
        shell
            .player_view
            .fullscreen_player
            .lyrics_pane
            .clear_follow_scroll_pause();
        shell.cancel_scheduled_lyrics_highlight();
        shell.render_lyrics_panel();
    }
    if notification_started_run.is_some_and(|run| next_player.transport.run == Some(run))
        || notification_became_sendable
    {
        shell.notify_now_playing(Some(&next_player));
    }
    match fullscreen_refresh {
        FullscreenPlaybackRefresh::Static => shell.update_fullscreen_player(),
        FullscreenPlaybackRefresh::Visualizer => shell.sync_fullscreen_visualizer_state(),
        FullscreenPlaybackRefresh::None => {}
    }
    if lyrics_timing_changed {
        shell.update_lyrics_highlight();
    }
    #[cfg(unix)]
    shell.update_mpris_player_after(mpris_discontinuity);
    shell.schedule_queue_panel_render();
}

fn apply_waveform(shell: &Rc<Shell>, waveform: WaveformProjection) {
    *shell.playback.waveform.borrow_mut() = waveform;
    shell.update_bottom_player();
}

fn apply_metadata_event(shell: &Rc<Shell>, event: metadata::LyricsEvent) {
    match event {
        metadata::LyricsEvent::Loaded {
            media_key,
            generation,
            lyrics,
        } => {
            if shell.products.lyrics.accepts_generation(generation) {
                shell.apply_loaded_lyrics_for_media(media_key, *lyrics);
            }
        }
        metadata::LyricsEvent::SearchResults {
            media_key,
            generation,
            artist_name,
            track_name,
            results,
        } => {
            if shell.products.lyrics.accepts_generation(generation) {
                shell.apply_lyrics_search_results(media_key, artist_name, track_name, results);
            }
        }
        metadata::LyricsEvent::SearchFailed {
            media_key,
            generation,
            artist_name,
            track_name,
            error,
        } => {
            if shell.products.lyrics.accepts_generation(generation) {
                shell.apply_lyrics_search_failed(media_key, artist_name, track_name, error);
            }
        }
        metadata::LyricsEvent::Saved {
            media_key,
            generation,
            path,
            lyrics,
        } => {
            if shell.products.lyrics.accepts_generation(generation) {
                shell.apply_lyrics_saved(media_key, path, lyrics);
            }
        }
        metadata::LyricsEvent::FileSaved {
            media_key,
            generation,
            path,
        } => {
            if shell.products.lyrics.accepts_generation(generation) {
                shell.apply_lyrics_file_saved(media_key, path);
            }
        }
    }
}

fn apply_artwork_event(shell: &Rc<Shell>, event: artwork::ArtworkEvent) {
    if let artwork::ArtworkEvent::Changed(projection) = &event
        && let artwork::Readiness::Failed(error) = &projection.readiness
    {
        warn!(
            request_id = projection.request_id.get(),
            %error,
            "artwork request failed"
        );
    }
    let ready_path = match &event {
        artwork::ArtworkEvent::Changed(projection) => match &projection.readiness {
            artwork::Readiness::Ready(image) => Some(image.cache_path().to_path_buf()),
            _ => None,
        },
        artwork::ArtworkEvent::Invalidated(_) => None,
    };
    let update_playback_art = ready_path.as_ref().is_some_and(|ready_path| {
        shell
            .playback
            .player
            .borrow()
            .as_ref()
            .and_then(|player| {
                player.transport.current.as_ref().and_then(|entry| {
                    shell.current_playback_cached_artwork_path(
                        &player.transport.source_id,
                        entry,
                        THUMB_COVER_SIZE,
                    )
                })
            })
            .is_some_and(|artwork| artwork.path == *ready_path)
    });
    shell.apply_artwork_event(event);
    if update_playback_art {
        let player = shell.playback.player.borrow().clone();
        shell.refresh_now_playing_notification(player.as_ref());
    }
    #[cfg(unix)]
    if update_playback_art {
        shell.update_mpris_player();
    }
}

impl Shell {
    fn apply_source_local_access_presentation(
        &self,
        presentation: sources::SourceLocalAccessPresentation,
    ) {
        let mut source = self.source.presentation.borrow_mut();
        if let Some(current) = source
            .source_local_access
            .iter_mut()
            .find(|current| current.source_id == presentation.source_id)
        {
            *current = presentation.clone();
        } else {
            source.source_local_access.push(presentation.clone());
        }
        if source
            .source
            .as_ref()
            .is_some_and(|active| active.id == presentation.source_id)
        {
            source.local_access = presentation.access;
            source.local_access_status = presentation.status;
        }
    }

    pub(crate) fn replace_source_presentation(
        self: &Rc<Self>,
        presentation: SourcePresentationState,
    ) {
        let next_source_id = presentation.source.as_ref().map(|source| &source.id);
        let (source_changed, scope_changed) = {
            let current = self.source.presentation.borrow();
            let current_source_id = current.source.as_ref().map(|source| &source.id);
            (
                current_source_id != next_source_id,
                current_source_id == next_source_id
                    && current.selected_music_folder_id != presentation.selected_music_folder_id,
            )
        };
        let current_query = self.library.query.borrow().clone();
        let query = match (current_query, next_source_id) {
            (Some(query), Some(source_id)) if query.source_id() == source_id => Some(query),
            (_, Some(source_id)) => Some(self.products.library.query(source_id.clone())),
            (_, None) => None,
        };
        *self.source.presentation.borrow_mut() = presentation;
        *self.library.query.borrow_mut() = query;
        if source_changed || scope_changed {
            self.cancel_source_artwork_warm();
        }
        if source_changed {
            self.clear_home_projection_state();
        }
        if scope_changed {
            self.apply_library_delta(LibraryDelta {
                reset: Some(LibraryReset::Scope),
                ..LibraryDelta::default()
            });
        }
        if source_changed || scope_changed {
            self.schedule_prepared_library_warm();
        }
        self.sync_bottom_player_favorite();
    }

    fn apply_source_notice(self: &Rc<Self>, notice: SourceNotice) {
        let message = source_notice_message(&notice);
        match notice {
            SourceNotice::Checking { .. } | SourceNotice::Connected
                if matches!(&*self.source.load.borrow(), LibraryLoad::Connecting { .. }) =>
            {
                let first_run = match &*self.source.load.borrow() {
                    LibraryLoad::Connecting { first_run, .. } => *first_run,
                    _ => false,
                };
                *self.source.load.borrow_mut() = LibraryLoad::Connecting {
                    stage: message,
                    first_run,
                };
                self.render_current_route();
            }
            _ => self.show_notice_toast(&message),
        }
    }

    fn apply_source_transition_failed(self: &Rc<Self>, source_id: Option<SourceId>, error: String) {
        warn!(%error, "source transition failed");
        let load = self.source.load.borrow().clone();
        match load {
            LibraryLoad::Connecting {
                first_run: true, ..
            } => {
                *self.source.load.borrow_mut() = LibraryLoad::Failed {
                    source_id,
                    message: error,
                };
                self.cancel_startup_route_reveal();
                self.update_layout();
                self.render_current_route();
            }
            LibraryLoad::Connecting {
                first_run: false, ..
            } => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.update_layout();
                self.render_current_route();
            }
            LibraryLoad::Switching { .. } | LibraryLoad::WaitingForFirstCommit { .. } => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
            }
            LibraryLoad::Ready | LibraryLoad::Failed { .. } => {}
        }
    }

    fn apply_source_presentation(self: &Rc<Self>, presentation: SourcePresentationState) {
        let (previous_first_run, previous_source) = {
            let current = self.source.presentation.borrow();
            (current.first_run, current.selected_source.clone())
        };
        let entered_first_run = presentation.first_run && !previous_first_run;
        let source_changed = previous_source != presentation.selected_source;
        let source_id = presentation.source.as_ref().map(|source| source.id.clone());
        let selected_source = presentation.selected_source.clone();
        let first_run = presentation.first_run;
        let has_cache = presentation.cache.is_committed();
        self.replace_source_presentation(presentation);

        if entered_first_run {
            self.source.discovery_started.set(false);
            self.source.discovery_running.set(false);
            *self.source.discovered_servers.borrow_mut() = Vec::new();
            *self.source.discovery_status.borrow_mut() = ServerDiscoveryStatus::Idle;
        }
        refresh_context_playlist_picker(self);
        update_source_selector(self);

        let load = self.source.load.borrow().clone();
        let recovers_failed_projection =
            failed_source_load_recovers_from_presentation(&load, source_id.as_ref(), has_cache);
        match load {
            LibraryLoad::Connecting { .. } if has_cache && source_id.is_some() => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_first_run_app_reveal();
                return;
            }
            LibraryLoad::Connecting { .. } => {
                self.update_layout();
                self.render_current_route();
                return;
            }
            LibraryLoad::Switching { target }
                if selected_source.as_ref() == Some(&target)
                    && first_run
                    && source_id.is_some() =>
            {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.startup.route_revealed.set(true);
                self.update_layout();
                self.render_current_route();
                self.schedule_source_artwork_warm();
                self.show_reconnect_notice_if_needed();
                return;
            }
            LibraryLoad::Switching { target }
                if selected_source.as_ref() == Some(&target) && has_cache =>
            {
                if queue_ready_for_library(
                    self.playback.player.borrow().as_ref(),
                    &self.source.presentation.borrow(),
                ) {
                    self.finish_source_switch();
                } else {
                    self.render_startup_loading_view();
                }
                return;
            }
            LibraryLoad::Switching { .. } | LibraryLoad::WaitingForFirstCommit { .. } => {
                self.render_startup_loading_view();
                return;
            }
            LibraryLoad::Ready if !has_cache && !first_run => {
                if let Some(source_id) = source_id {
                    *self.source.load.borrow_mut() =
                        LibraryLoad::WaitingForFirstCommit { source_id };
                    self.startup.route_revealed.set(false);
                    self.enter_startup_loading();
                    return;
                }
            }
            LibraryLoad::Failed { .. } if recovers_failed_projection => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
                return;
            }
            LibraryLoad::Failed { .. } => {
                self.update_layout();
                self.render_current_route();
                return;
            }
            LibraryLoad::Ready => {}
        }

        self.update_layout();
        if source_changed {
            self.clear_mounted_routes();
            self.reset_cover_pipeline_state();
            self.navigate(Route::Home);
        }
        self.schedule_source_artwork_warm();
    }

    fn finish_source_switch(self: &Rc<Self>) {
        *self.source.load.borrow_mut() = LibraryLoad::Ready;
        self.clear_mounted_routes();
        self.prepare_home_route();
        self.render_queue_panel();
        self.render_lyrics_panel();
        self.update_bottom_player();
        self.update_fullscreen_player();
        #[cfg(unix)]
        self.update_mpris_player();
        self.schedule_startup_route_reveal();
    }

    fn apply_source_sync_changed(self: &Rc<Self>, change: library_sync::SourceSyncChanged) {
        apply_source_sync_presentation(&mut self.source.syncs.borrow_mut(), &change);
        if matches!(
            change.phase,
            library_sync::SyncPhase::Idle | library_sync::SyncPhase::Failed
        ) {
            self.dismiss_source_sync_toast(&change.source_id);
        }
        let active = self
            .source
            .presentation
            .borrow()
            .source
            .as_ref()
            .is_some_and(|source| source.id == change.source_id);
        match change.phase {
            library_sync::SyncPhase::Running => {
                if active && self.source.load.borrow().blocks_library() {
                    if self.source.login_screen_active() {
                        self.render_current_route();
                    } else {
                        self.render_startup_loading_view();
                    }
                }
                if change.manual && !self.library_sync_status_visible_fullscreen() {
                    let status = source_sync_progress_text(&change);
                    self.show_or_update_source_sync_toast(&change.source_id, change.epoch, &status);
                }
            }
            library_sync::SyncPhase::Failed => {
                if let Some(failure) = change.failure {
                    warn!(
                        source_id = %change.source_id,
                        error = %failure,
                        "source sync failed"
                    );
                    if active && !self.source.presentation.borrow().cache.is_committed() {
                        *self.source.load.borrow_mut() = LibraryLoad::Failed {
                            source_id: Some(change.source_id),
                            message: failure.clone(),
                        };
                        self.cancel_startup_route_reveal();
                        self.update_layout();
                        self.render_current_route();
                    }
                }
            }
            library_sync::SyncPhase::Idle => {}
        }
    }

    fn apply_library_committed(self: &Rc<Self>, commit: library_sync::LibraryCommitted) {
        if !self.commit_matches_selected_source(&commit)
            || !apply_commit_revision(&mut self.source.presentation.borrow_mut(), &commit)
        {
            return;
        }
        let commit_source_id = commit.source_id.clone();
        update_source_selector(self);
        self.apply_committed_library_delta(commit.revision, commit.delta);
        self.schedule_prepared_library_warm();
        let load = self.source.load.borrow().clone();
        match load {
            LibraryLoad::Connecting { .. } => self.schedule_first_run_app_reveal(),
            LibraryLoad::Switching { .. } => self.finish_source_switch(),
            LibraryLoad::WaitingForFirstCommit { source_id } if source_id == commit.source_id => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
            }
            LibraryLoad::Failed {
                source_id: Some(failed_source_id),
                ..
            } if failed_source_id == commit_source_id => {
                *self.source.load.borrow_mut() = LibraryLoad::Ready;
                self.schedule_startup_route_reveal();
            }
            LibraryLoad::Ready
            | LibraryLoad::WaitingForFirstCommit { .. }
            | LibraryLoad::Failed { .. } => {}
        }
    }

    fn commit_matches_selected_source(&self, commit: &library_sync::LibraryCommitted) -> bool {
        let library = self.source.presentation.borrow();
        let Some(source) = library.source.as_ref() else {
            return false;
        };
        if source.id != commit.source_id {
            return false;
        }
        match library.selected_source.as_ref() {
            Some(LibrarySourceSelection::Source(source_id)) => source_id == &commit.source_id,
            Some(LibrarySourceSelection::Local) => true,
            None => false,
        }
    }
}

fn failed_projection_matches_source(load: &LibraryLoad, source_id: &SourceId) -> bool {
    matches!(
        load,
        LibraryLoad::Failed {
            source_id: Some(failed_source_id),
            ..
        } if failed_source_id == source_id
    )
}

fn failed_source_load_recovers_from_presentation(
    load: &LibraryLoad,
    source_id: Option<&SourceId>,
    has_cache: bool,
) -> bool {
    has_cache
        && source_id.is_some_and(|source_id| failed_projection_matches_source(load, source_id))
}

fn apply_source_sync_presentation(
    presentations: &mut HashMap<SourceId, library_sync::SourceSyncChanged>,
    change: &library_sync::SourceSyncChanged,
) {
    match change.phase {
        library_sync::SyncPhase::Running => {
            presentations.insert(change.source_id.clone(), change.clone());
        }
        library_sync::SyncPhase::Idle | library_sync::SyncPhase::Failed => {
            presentations.remove(&change.source_id);
        }
    }
}

pub(crate) fn apply_commit_revision(
    library: &mut SourcePresentationState,
    commit: &library_sync::LibraryCommitted,
) -> bool {
    let active = library
        .source
        .as_ref()
        .is_some_and(|source| source.id == commit.source_id);
    if !active || commit.revision <= library.cache.revision() {
        return false;
    }

    library.cache = LibraryCacheState::Committed {
        revision: commit.revision,
    };
    true
}

impl Shell {
    pub(crate) fn show_notice_toast(&self, message: &str) {
        self.chrome
            .toast_overlay
            .add_toast(adw::Toast::new(message));
    }

    fn library_sync_status_visible_fullscreen(&self) -> bool {
        self.source.login_screen_active() || !self.startup.route_revealed.get()
    }

    fn show_or_update_source_sync_toast(
        self: &Rc<Self>,
        source_id: &SourceId,
        _epoch: u64,
        message: &str,
    ) {
        if let Some(toast) = self.source.sync_toasts.borrow().get(source_id) {
            toast.set_title(message);
            toast.set_timeout(0);
            return;
        }

        let toast = adw::Toast::new(message);
        toast.set_timeout(0);
        self.chrome.toast_overlay.add_toast(toast.clone());
        self.source
            .sync_toasts
            .borrow_mut()
            .insert(source_id.clone(), toast);
    }

    fn dismiss_source_sync_toast(&self, source_id: &SourceId) {
        let toast = self.source.sync_toasts.borrow_mut().remove(source_id);
        if let Some(toast) = toast {
            toast.dismiss();
        }
    }
}

#[cfg(test)]
mod source_sync_handoff_tests {
    use std::collections::HashMap;

    use ::library::SourceId;

    use super::{
        apply_source_sync_presentation, failed_projection_matches_source,
        failed_source_load_recovers_from_presentation,
    };
    use crate::preferences::source::LibraryLoad;

    fn sync_change(
        source_id: &SourceId,
        epoch: u64,
        phase: library_sync::SyncPhase,
        manual: bool,
    ) -> library_sync::SourceSyncChanged {
        library_sync::SourceSyncChanged {
            source_id: source_id.clone(),
            epoch,
            phase,
            progress: None,
            failure: None,
            manual,
        }
    }

    #[test]
    fn failed_projection_retries_and_recovers_only_for_its_cached_source() {
        let failed_source = SourceId::new("source-a");
        let other_source = SourceId::new("source-b");
        let load = LibraryLoad::Failed {
            source_id: Some(failed_source.clone()),
            message: "snapshot read failed".to_string(),
        };

        assert!(failed_projection_matches_source(&load, &failed_source));
        assert!(!failed_projection_matches_source(&load, &other_source));
        assert!(failed_source_load_recovers_from_presentation(
            &load,
            Some(&failed_source),
            true,
        ));
        assert!(!failed_source_load_recovers_from_presentation(
            &load,
            Some(&failed_source),
            false,
        ));
        assert!(!failed_source_load_recovers_from_presentation(
            &load,
            Some(&other_source),
            true,
        ));
    }

    #[test]
    fn automatic_idle_removes_only_its_source_presentation() {
        let automatic_source = SourceId::new("source-a");
        let manual_source = SourceId::new("source-b");
        let mut presentations = HashMap::new();
        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(
                &automatic_source,
                3,
                library_sync::SyncPhase::Running,
                false,
            ),
        );
        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(&manual_source, 7, library_sync::SyncPhase::Running, true),
        );

        apply_source_sync_presentation(
            &mut presentations,
            &sync_change(&automatic_source, 3, library_sync::SyncPhase::Idle, false),
        );

        assert!(!presentations.contains_key(&automatic_source));
        assert!(
            presentations
                .get(&manual_source)
                .is_some_and(|change| change.manual && change.epoch == 7)
        );
    }
}
