use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::AdwDialogExt;
use gtk::glib;
use localization::tr;
use playback::QueuePageQuery;
use tracing::warn;

use crate::player::fullscreen::{FullscreenPlaybackRefresh, fullscreen_playback_refresh};
use crate::player::state::current_playback_media_id;
use crate::player::{now_playing_notification_can_send, now_playing_notification_should_withdraw};
use crate::preferences::dialogs::release_notes::apply_release_update;
use crate::routes::playlist_picker::refresh_context_playlist_picker;
use crate::routes::route::Route;
use crate::runtime::source::{DiscoveryStatus, DiscoveryUpdate, SourceOperation};
use crate::runtime::{
    HomePublication, ProductReceivers, SelectedLibraryUpdate, SourceEvent, SourceNotice,
    SourceNoticeKind, WaveformProjection,
};

use super::Shell;
use super::navigation::update_sidebar_pin_playback;
use super::route::route_current_track;

pub(crate) fn install_product_event_receivers(shell: &Rc<Shell>, receivers: ProductReceivers) {
    let ProductReceivers {
        source,
        source_discovery,
        downloads,
        playback,
        waveform,
        lyrics,
        release_updates,
    } = receivers;

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(event) = source.recv().await {
            apply_source_event(&event_shell, event);
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
        while let Ok(event) = downloads.recv().await {
            event_shell.apply_download_event(event);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(publication) = playback.recv().await {
            apply_playback_publication(&event_shell, publication);
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
        while let Ok(event) = lyrics.recv().await {
            apply_lyrics_event(&event_shell, event);
        }
    });

    let event_shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        while let Ok(update) = release_updates.recv().await {
            apply_release_update(&event_shell, update);
        }
    });
}

fn apply_source_event(shell: &Rc<Shell>, event: SourceEvent) {
    match event {
        SourceEvent::Configured(configured) => apply_configured_sources(shell, configured),
        SourceEvent::Selected {
            configured,
            selected,
            playback,
        } => {
            apply_selected_source(shell, configured, selected, *playback);
        }
        SourceEvent::LibraryReplaced {
            configured,
            selected,
        } => {
            apply_selected_library_replacement(shell, configured, selected);
        }
        SourceEvent::Operation(operation) => apply_source_operation(shell, operation),
        SourceEvent::Home(publication) => apply_home_publication(shell, publication),
        SourceEvent::HomeReplaced {
            source_id,
            source_session_epoch,
            home,
        } => apply_home_replacement(shell, source_id, source_session_epoch, home),
        SourceEvent::LibraryUpdate(update) => apply_selected_library_update(shell, update),
        SourceEvent::Notice(notice) => apply_source_notice(shell, notice),
        SourceEvent::ReleaseSelected { acknowledged } => {
            release_selected_source(shell);
            let _ = acknowledged.try_send(());
        }
    }
}

fn apply_playback_publication(shell: &Rc<Shell>, publication: crate::runtime::PlaybackPublication) {
    let matches_selected = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .is_some_and(|selected| {
            selected.source_id == publication.source_id
                && selected.source_session_epoch == publication.source_session_epoch
        });
    if matches_selected {
        apply_playback_projection(shell, publication.projection);
    }
}

fn release_selected_source(shell: &Rc<Shell>) {
    let released_source = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .map(|selected| selected.source_id.clone());
    shell.clear_mounted_routes();
    shell.reset_cover_pipeline_state();
    shell.library.selected.borrow_mut().take();
    if let Some(source_id) = released_source.as_ref() {
        shell.release_artwork_textures(source_id);
    }
    shell.playback.player.borrow_mut().take();
    *shell.playback.waveform.borrow_mut() = WaveformProjection::default();
    shell.playback.seek_preview_seconds.set(None);
    shell
        .playback
        .seek_generation
        .set(shell.playback.seek_generation.get().saturating_add(1));

    shell.queue.page.borrow_mut().take();
    shell.queue.page_request.borrow_mut().take();
    shell.queue.filter.borrow_mut().clear();
    if let Some(source) = shell.queue.search_source.borrow_mut().take() {
        source.remove();
    }
    shell.invalidate_queue_panel_render_state();

    *shell.lyrics.projection.borrow_mut() = lyrics::CurrentLyrics::Cleared;
    shell.lyrics.offset_millis.set(0);
    shell.cancel_scheduled_lyrics_highlight();
    if let Some(dialog) = shell.lyrics.search_dialog.borrow_mut().take() {
        if let Some(source) = dialog.search_debounce_source.borrow_mut().take() {
            source.remove();
        }
        dialog.dialog.close();
    }
    let settings_dialog = shell.lyrics.settings_dialog.borrow_mut().take();
    if let Some(dialog) = settings_dialog {
        dialog.dialog.close();
    }

    shell.favorites.clear_all();
    shell.sync_bottom_player_favorite();
    shell.rebuild_sidebar_navigation();
    shell.update_bottom_player();
    shell.render_queue_panel();
    shell.render_lyrics_panel();
    shell.apply_fullscreen_visualizer_levels(Vec::new());
    shell.clear_fullscreen_player_cover();
    shell.close_fullscreen_player();
    shell.withdraw_now_playing_notification();
    shell.update_media_controls();
}

#[derive(Clone)]
struct SourcePresentation {
    first_run: bool,
    selected: Option<crate::runtime::SelectedLibrary>,
}

fn apply_configured_sources(
    shell: &Rc<Shell>,
    configured: crate::runtime::source::ConfiguredSources,
) {
    let previous = current_source_presentation(shell);
    let next = SourcePresentation {
        first_run: configured.first_run,
        selected: previous.selected.clone(),
    };
    *shell.source.configured.borrow_mut() = configured;
    finish_source_assignment(shell, previous, next);
}

fn current_source_presentation(shell: &Shell) -> SourcePresentation {
    SourcePresentation {
        first_run: shell.source.configured.borrow().first_run,
        selected: shell.library.selected.borrow().clone(),
    }
}

fn finish_source_assignment(
    shell: &Rc<Shell>,
    previous: SourcePresentation,
    next: SourcePresentation,
) {
    let previous_source_id = previous
        .selected
        .as_ref()
        .map(|selected| selected.source_id.clone());
    let next_source_id = next
        .selected
        .as_ref()
        .map(|selected| selected.source_id.clone());
    let previous_epoch = previous
        .selected
        .as_ref()
        .map(|selected| selected.source_session_epoch);
    let next_epoch = next
        .selected
        .as_ref()
        .map(|selected| selected.source_session_epoch);
    let previous_scope = previous
        .selected
        .as_ref()
        .and_then(|selected| selected.music_folder_id.clone());
    let next_scope = next
        .selected
        .as_ref()
        .and_then(|selected| selected.music_folder_id.clone());
    let previous_loaded = previous
        .selected
        .as_ref()
        .map(|selected| Arc::clone(&selected.library));
    let next_loaded = next
        .selected
        .as_ref()
        .map(|selected| Arc::clone(&selected.library));
    let source_changed = previous_source_id != next_source_id;
    let session_changed = previous_epoch != next_epoch;
    let scope_changed = previous_scope != next_scope;
    let library_changed = match (&previous_loaded, &next_loaded) {
        (Some(previous), Some(next)) => !Arc::ptr_eq(previous, next),
        (None, None) => false,
        _ => true,
    };
    let entered_first_run = next.first_run && !previous.first_run;
    let left_first_run = previous.first_run && !next.first_run;

    if entered_first_run {
        shell.close_preferences_dialog();
        shell.source.discovery_started.set(false);
        shell.source.discovery_running.set(false);
        shell.source.discovered_servers.borrow_mut().clear();
        *shell.source.discovery_status.borrow_mut() = DiscoveryStatus::Idle;
    }
    if left_first_run {
        shell.release_first_run_setup();
    }

    refresh_context_playlist_picker(shell);
    shell.sync_bottom_player_favorite();
    let rebuild_sidebar = source_changed || session_changed || scope_changed || library_changed;

    if next.selected.is_none() {
        if rebuild_sidebar {
            shell.rebuild_sidebar_navigation();
        }
        shell.clear_mounted_routes();
        shell.update_layout();
        shell.render_current_route();
        return;
    }

    if session_changed && !source_changed {
        shell.reset_cover_pipeline_state();
    }

    if source_changed {
        shell.clear_mounted_routes();
        shell.reset_cover_pipeline_state();
        shell.navigate(Route::Home);
    } else if scope_changed {
        shell.replace_current_route_when_ready();
    } else if session_changed || library_changed {
        // A mounted Home owns its current snapshot for the duration of the
        // visit. Other routes keep their current page until its replacement
        // projection is ready.
        if !matches!(shell.navigation.routes.borrow().current(), Route::Home) {
            shell.replace_current_route_when_ready();
        }
    }

    if session_changed && !source_changed {
        shell.refresh_artwork_bindings();
    }
    if rebuild_sidebar {
        shell.rebuild_sidebar_navigation();
    }

    if !shell.startup.route_revealed.get()
        && !shell.source.operation.borrow().blocks_library()
        && !shell.source.login_screen_active()
    {
        shell.schedule_startup_route_reveal();
    }
}

fn apply_selected_source(
    shell: &Rc<Shell>,
    configured: crate::runtime::source::ConfiguredSources,
    selected: crate::runtime::SelectedLibrary,
    playback: playback::PlaybackProjection,
) {
    let previous_source = current_source_presentation(shell);
    let next_source = SourcePresentation {
        first_run: configured.first_run,
        selected: Some(selected.clone()),
    };
    let previous_player = shell.playback.player.borrow().clone();
    let playback::PlaybackProjection {
        view,
        queue_page,
        notices,
    } = playback;

    *shell.source.configured.borrow_mut() = configured;
    *shell.library.selected.borrow_mut() = Some(selected);
    *shell.playback.player.borrow_mut() = Some(view.clone());
    shell.queue.page_request.borrow_mut().take();
    *shell.queue.page.borrow_mut() = queue_page;
    shell.invalidate_queue_panel_render_state();

    finish_source_assignment(shell, previous_source, next_source);
    finish_playback_projection(shell, previous_player, view, notices, true);
}

fn apply_selected_library_replacement(
    shell: &Rc<Shell>,
    configured: crate::runtime::source::ConfiguredSources,
    selected: crate::runtime::SelectedLibrary,
) {
    let previous = current_source_presentation(shell);
    let next = SourcePresentation {
        first_run: configured.first_run,
        selected: Some(selected.clone()),
    };
    *shell.source.configured.borrow_mut() = configured;
    *shell.library.selected.borrow_mut() = Some(selected);
    finish_source_assignment(shell, previous, next);
}

fn apply_home_publication(shell: &Rc<Shell>, publication: HomePublication) {
    let matches_selected = {
        let mut selected = shell.library.selected.borrow_mut();
        let Some(selected) = selected.as_mut() else {
            return;
        };
        if selected.source_id != publication.source_id
            || selected.source_session_epoch != publication.source_session_epoch
        {
            false
        } else {
            selected.home = Arc::clone(&publication.home);
            true
        }
    };
    if matches_selected && matches!(shell.navigation.routes.borrow().current(), Route::Home) {
        shell.apply_home_section_to_mounted_route(publication.kind, publication.home);
    }
}

fn apply_home_replacement(
    shell: &Rc<Shell>,
    source_id: library::SourceId,
    source_session_epoch: playback::SourceSessionEpoch,
    home: Arc<library::HomeSnapshot>,
) {
    let mut selected = shell.library.selected.borrow_mut();
    if let Some(selected) = selected.as_mut().filter(|selected| {
        selected.source_id == source_id && selected.source_session_epoch == source_session_epoch
    }) {
        selected.home = home;
    }
}

fn apply_selected_library_update(shell: &Rc<Shell>, update: SelectedLibraryUpdate) {
    {
        let mut selected = shell.library.selected.borrow_mut();
        let Some(selected) = selected.as_mut() else {
            return;
        };
        if selected.source_id != update.source_id
            || selected.source_session_epoch != update.source_session_epoch
        {
            return;
        }
        if let Some(home) = &update.home {
            selected.home = Arc::clone(home);
        }
    }

    if let Some(acknowledgement) = &update.change.favorite {
        shell.apply_favorite_changed(acknowledgement.item.clone(), acknowledgement.favorite);
    }
    shell.apply_queue_track_replacements(&update.change.tracks);

    if shell.sidebar_pins_changed(&update.change) {
        shell.rebuild_sidebar_navigation();
    }
    if !update.change.playlists.is_empty() {
        refresh_context_playlist_picker(shell);
    }
    shell.apply_library_update_to_mounted_route(&update);
}

fn apply_source_notice(shell: &Rc<Shell>, notice: SourceNotice) {
    let matches_selected = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .is_some_and(|selected| {
            selected.source_id == notice.source_id
                && selected.source_session_epoch == notice.source_session_epoch
        });
    if !matches_selected {
        return;
    }
    shell.show_feedback_toast(match notice.kind {
        SourceNoticeKind::ServerUnreachable => tr("Server is unreachable"),
        SourceNoticeKind::FavoriteRejected => tr("Could not update favorites"),
    });
}

fn apply_source_operation(shell: &Rc<Shell>, operation: SourceOperation) {
    let previous_operation = shell.source.operation.borrow().clone();
    let previously_blocked = previous_operation.blocks_library();
    let completed_add = source_add_completed(&previous_operation, &operation);
    *shell.source.operation.borrow_mut() = operation.clone();

    match &operation {
        SourceOperation::Adding { .. } => {
            let first_run = shell.source.configured.borrow().first_run;
            if first_run {
                shell.cancel_startup_route_reveal();
                if !shell.first_run_setup_mounted() {
                    shell.update_layout();
                    shell.render_current_route();
                }
                shell.update_add_server_dialog();
            } else {
                shell.close_preferences_dialog();
                if !shell.startup.route_revealed.get() {
                    shell.render_startup_loading_view();
                } else {
                    shell.enter_startup_loading();
                }
            }
        }
        SourceOperation::Switching { .. } => {
            shell.close_preferences_dialog();
            shell.startup.route_revealed.set(false);
            if previously_blocked {
                shell.render_startup_loading_view();
            } else {
                shell.enter_startup_loading();
            }
        }
        SourceOperation::Refreshing { .. } => {}
        SourceOperation::Failed {
            message, add_form, ..
        } => {
            warn!(error = %message, "source operation failed");
            if *add_form {
                let first_run = shell.source.configured.borrow().first_run;
                let setup_was_mounted = shell.first_run_setup_mounted();
                if first_run {
                    shell.cancel_startup_route_reveal();
                    if !setup_was_mounted {
                        shell.update_layout();
                        shell.render_current_route();
                    }
                }
                if first_run {
                    shell.update_add_server_dialog();
                } else {
                    shell.restore_add_server_dialog_after_failure();
                }
                if !first_run && previously_blocked {
                    shell.schedule_startup_route_reveal();
                }
            } else if previously_blocked {
                shell.schedule_startup_route_reveal();
            }
        }
        SourceOperation::Idle => {
            if completed_add {
                shell.complete_add_server_dialog();
            }
            shell.update_add_server_dialog();
            if shell.library.selected.borrow().is_some()
                && !shell.startup.route_revealed.get()
                && !shell.source.login_screen_active()
            {
                shell.schedule_startup_route_reveal();
            }
        }
    }
}

fn source_add_completed(previous: &SourceOperation, next: &SourceOperation) -> bool {
    matches!(previous, SourceOperation::Adding { .. }) && matches!(next, SourceOperation::Idle)
}

fn apply_source_discovery(shell: &Rc<Shell>, update: DiscoveryUpdate) {
    *shell.source.discovered_servers.borrow_mut() = update.servers.to_vec();
    *shell.source.discovery_status.borrow_mut() = update.status.clone();
    shell
        .source
        .discovery_running
        .set(matches!(update.status, DiscoveryStatus::Searching));
    if shell.source.configured.borrow().first_run && !shell.first_run_setup_mounted() {
        shell.render_current_route();
    }
    shell.update_add_server_discovery();
}

fn apply_playback_projection(shell: &Rc<Shell>, projection: playback::PlaybackProjection) {
    let previous_player = shell.playback.player.borrow().clone();
    let playback::PlaybackProjection {
        view,
        queue_page,
        notices,
    } = projection;
    let queue_page_changed = queue_page
        .map(|queue_page| shell.apply_queue_page_projection(queue_page))
        .unwrap_or(false);
    *shell.playback.player.borrow_mut() = Some(view.clone());
    finish_playback_projection(shell, previous_player, view, notices, queue_page_changed);
}

fn finish_playback_projection(
    shell: &Rc<Shell>,
    previous_player: Option<playback::PlaybackView>,
    next_player: playback::PlaybackView,
    notices: Vec<playback::PlaybackNotice>,
    queue_page_changed: bool,
) {
    let previous_media = previous_player
        .as_ref()
        .and_then(|player| player.transport.current.as_ref())
        .map(|current| &current.id);
    let next_media = next_player
        .transport
        .current
        .as_ref()
        .map(|current| &current.id);
    let media_changed = previous_media != next_media;
    let notification_became_sendable = !now_playing_notification_can_send(
        &shell.settings.current.borrow(),
        previous_player.as_ref(),
    ) && now_playing_notification_can_send(
        &shell.settings.current.borrow(),
        Some(&next_player),
    );
    let lyrics_timing_changed = media_changed
        || previous_player
            .as_ref()
            .map(|player| player.transport.state)
            != Some(next_player.transport.state)
        || previous_player
            .as_ref()
            .map(|player| player.transport.position_millis)
            != Some(next_player.transport.position_millis);
    let fullscreen_refresh = fullscreen_playback_refresh(previous_player.as_ref(), &next_player);
    let static_playback_changed = matches!(fullscreen_refresh, FullscreenPlaybackRefresh::Static);
    let position_only =
        bottom_player_can_update_position_only(previous_player.as_ref(), &next_player);
    let media_controls_static_changed =
        media_controls_static_state_changed(previous_player.as_ref(), &next_player);
    let queue_panel_changed = queue_panel_refresh_needed(
        queue_page_changed,
        previous_player
            .as_ref()
            .and_then(|player| player.queue.current_occurrence.as_ref()),
        next_player.queue.current_occurrence.as_ref(),
    );

    if shell.queue.filter.borrow().trim().is_empty()
        && let Some(current_index) = next_player.queue.current_index
        && shell.queue.page.borrow().as_ref().is_none_or(|page| {
            page.query.follows_current()
                && !page
                    .rows
                    .iter()
                    .any(|row| row.absolute_index == current_index)
        })
    {
        shell.request_queue_page(QueuePageQuery::current());
    }

    let previous_route_track = route_current_track(previous_player.as_ref());
    let next_route_track = route_current_track(Some(&next_player));
    if previous_route_track != next_route_track {
        shell.refresh_current_route_now_playing_selections();
        update_sidebar_pin_playback(shell);
    }
    if static_playback_changed {
        shell.sync_bottom_player_favorite();
    }
    shell.maybe_clear_player_seek_preview(&next_player, media_changed);
    if static_playback_changed {
        shell.update_bottom_player();
    } else if position_only {
        shell.update_bottom_player_position();
    } else {
        shell.update_bottom_player_transport();
    }

    let mut media_controls_discontinuity = None;
    let mut notification_started_run = None;
    for notice in notices {
        match notice {
            playback::PlaybackNotice::Visualizer { levels, .. } => {
                shell.apply_fullscreen_visualizer_levels(levels);
            }
            playback::PlaybackNotice::PositionDiscontinuity(discontinuity) => {
                media_controls_discontinuity = Some(discontinuity);
            }
            playback::PlaybackNotice::RunStarted(run) => {
                notification_started_run = Some(run);
            }
        }
    }

    if now_playing_notification_should_withdraw(
        &shell.settings.current.borrow(),
        Some(&next_player),
    ) {
        shell.withdraw_now_playing_notification();
    }
    if media_changed {
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
    if notification_started_run.is_some_and(|run| {
        next_player
            .transport
            .current
            .as_ref()
            .is_some_and(|media| media.id.run == Some(run))
    }) || notification_became_sendable
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
    if media_controls_static_changed {
        shell.update_media_controls_after(media_controls_discontinuity);
    } else {
        shell.update_media_controls_position_after(
            next_player
                .transport
                .current
                .as_ref()
                .map(|_| next_player.transport.position_millis),
            media_controls_discontinuity,
        );
    }
    if queue_panel_changed {
        shell.schedule_queue_panel_render();
    }
}

fn queue_panel_refresh_needed(
    page_changed: bool,
    previous_current: Option<&playback::OccurrenceId>,
    next_current: Option<&playback::OccurrenceId>,
) -> bool {
    page_changed || previous_current != next_current
}

fn bottom_player_can_update_position_only(
    previous: Option<&playback::PlaybackView>,
    next: &playback::PlaybackView,
) -> bool {
    previous.is_some_and(|previous| {
        previous.transport.source_id == next.transport.source_id
            && previous.transport.current == next.transport.current
            && previous.transport.effective_state() == next.transport.effective_state()
            && previous.transport.duration_millis == next.transport.duration_millis
            && previous.transport.buffering_percent == next.transport.buffering_percent
            && previous.transport.error == next.transport.error
            && previous.controls == next.controls
    })
}

fn media_controls_static_state_changed(
    previous: Option<&playback::PlaybackView>,
    next: &playback::PlaybackView,
) -> bool {
    previous.is_none_or(|previous| {
        previous.transport.current != next.transport.current
            || previous.transport.effective_state() != next.transport.effective_state()
            || previous.transport.duration_millis != next.transport.duration_millis
            || previous.controls.repeat_mode != next.controls.repeat_mode
            || previous.controls.shuffle_enabled != next.controls.shuffle_enabled
            || previous.controls.auto_dj_enabled != next.controls.auto_dj_enabled
            || previous.controls.volume != next.controls.volume
            || previous.queue.next_occurrence != next.queue.next_occurrence
    })
}

fn apply_waveform(shell: &Rc<Shell>, waveform: WaveformProjection) {
    *shell.playback.waveform.borrow_mut() = waveform;
    shell.update_bottom_player_transport();
}

fn apply_lyrics_event(shell: &Rc<Shell>, event: lyrics::LyricsEvent) {
    match event {
        lyrics::LyricsEvent::Current(projection) => shell.apply_current_lyrics(projection),
        lyrics::LyricsEvent::SearchFinished {
            media_id,
            query,
            result,
        } => match result {
            Ok(results) => shell.apply_lyrics_search_results(
                media_id,
                query.artist_name,
                query.track_name,
                results,
            ),
            Err(error) => shell.apply_lyrics_search_failed(
                media_id,
                query.artist_name,
                query.track_name,
                error,
            ),
        },
        lyrics::LyricsEvent::Saved { media_id, path } => {
            if current_playback_media_id(&shell.playback.player.borrow()).as_ref()
                == Some(&media_id)
            {
                shell.apply_lyrics_saved(media_id, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use library::SourceId;
    use playback::{
        ControlsView, PlaybackView, QueueSummaryView, RepeatMode, TransportStatus, TransportView,
    };

    use super::{
        bottom_player_can_update_position_only, media_controls_static_state_changed,
        queue_panel_refresh_needed, source_add_completed,
    };
    use crate::runtime::source::{SourceOperation, SourceProgress, SourceProgressStage};

    fn adding() -> SourceOperation {
        SourceOperation::Adding {
            progress: SourceProgress {
                stage: SourceProgressStage::Connecting,
                completed: 0,
                total: None,
            },
        }
    }

    #[test]
    fn a_failed_add_keeps_its_retry_form_until_an_add_reaches_idle() {
        assert!(!source_add_completed(
            &adding(),
            &SourceOperation::Failed {
                source_id: None,
                message: "Connection failed".to_string(),
                add_form: true,
            }
        ));
        assert!(source_add_completed(&adding(), &SourceOperation::Idle));
    }

    #[test]
    fn queue_panel_refresh_ignores_unchanged_playback_ticks() {
        let current = playback::OccurrenceId::new("current");
        let next = playback::OccurrenceId::new("next");

        assert!(!queue_panel_refresh_needed(
            false,
            Some(&current),
            Some(&current)
        ));
        assert!(queue_panel_refresh_needed(
            false,
            Some(&current),
            Some(&next)
        ));
        assert!(queue_panel_refresh_needed(
            true,
            Some(&current),
            Some(&current)
        ));
    }

    #[test]
    fn playback_ticks_only_update_position_owned_surfaces() {
        let previous = idle_playback_view();
        let mut tick = previous.clone();
        tick.transport.position_millis = 500;

        assert!(bottom_player_can_update_position_only(
            Some(&previous),
            &tick
        ));
        assert!(!media_controls_static_state_changed(Some(&previous), &tick));

        let mut state_change = tick.clone();
        state_change.transport.state = TransportStatus::Playing;
        state_change.transport.desired_playing = true;
        assert!(!bottom_player_can_update_position_only(
            Some(&tick),
            &state_change
        ));
        assert!(media_controls_static_state_changed(
            Some(&tick),
            &state_change
        ));

        let mut duration_change = tick.clone();
        duration_change.transport.duration_millis = 2_000;
        assert!(media_controls_static_state_changed(
            Some(&tick),
            &duration_change
        ));

        let mut queue_change = tick.clone();
        queue_change.queue.next_occurrence = Some(playback::OccurrenceId::new("next"));
        assert!(bottom_player_can_update_position_only(
            Some(&tick),
            &queue_change
        ));
        assert!(media_controls_static_state_changed(
            Some(&tick),
            &queue_change
        ));
    }

    fn idle_playback_view() -> PlaybackView {
        PlaybackView {
            queue: QueueSummaryView {
                revision: 0,
                total: 0,
                current_occurrence: None,
                current_index: None,
                next_occurrence: None,
            },
            transport: TransportView {
                source_id: SourceId::new("tick-source"),
                current: None,
                state: TransportStatus::Stopped,
                desired_playing: false,
                position_millis: 0,
                duration_millis: 0,
                buffering_percent: None,
                error: None,
            },
            controls: ControlsView {
                repeat_mode: RepeatMode::Off,
                shuffle_enabled: false,
                auto_dj_enabled: false,
                volume: 1.0,
                muted: false,
                audio_output: None,
            },
        }
    }
}
