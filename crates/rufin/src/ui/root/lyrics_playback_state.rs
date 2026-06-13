use super::*;

impl Shell {
    pub(in crate::ui) fn notify_now_playing(&self, snapshot: &PlaybackSnapshot) {
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
        if let Some(artwork) = self.current_playback_artwork_path(entry, THUMB_COVER_SIZE)
            && let Some(bytes) = notification_icon_path(&artwork.path)
        {
            let bytes = glib::Bytes::from_owned(bytes);
            notification.set_icon(&gio::BytesIcon::new(&bytes));
        }
        self.application
            .send_notification(Some("now-playing"), &notification);
    }
    pub(in crate::ui) fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }
    pub(in crate::ui) fn apply_loaded_lyrics(self: &Rc<Self>, lyrics: Option<Lyrics>) {
        self.restart_lyrics_follow_tracking();
        allow_loaded_lyrics_cache_revisit(
            &mut self.state.lyrics_auto_search_attempted.borrow_mut(),
            lyrics.as_ref(),
        );
        *self.state.lyrics.borrow_mut() = lyrics;
        self.render_lyrics_panel();
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.restart_lyrics_follow_tracking();
            shell.update_lyrics_highlight();
        });
    }
    pub(in crate::ui) fn apply_loaded_lyrics_for_track(
        self: &Rc<Self>,
        track_id: TrackId,
        lyrics: Option<Lyrics>,
    ) {
        if !loaded_lyrics_matches_current(
            current_playback_track_id(&self.state.player.borrow()).as_ref(),
            &track_id,
            lyrics.as_ref(),
        ) {
            return;
        }
        let has_lyrics = lyrics.is_some();
        self.apply_loaded_lyrics(lyrics);
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.track_id == track_id
            && dialog.status.text().as_str() == tr("Searching…")
        {
            let status = if has_lyrics {
                tr("Loaded in lyrics panel.")
            } else {
                tr("No lyrics found.")
            };
            dialog.status.set_text(&status);
        }
    }
    fn restart_lyrics_follow_tracking(&self) {
        self.lyrics_pane.restart_follow_tracking();
        self.fullscreen_player.lyrics_pane.restart_follow_tracking();
    }
    pub(in crate::ui) fn request_initial_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        *self.state.lyrics_track_id.borrow_mut() = Some(track_id);
        self.request_auto_lyrics_if_needed();
    }
    pub(in crate::ui) fn request_auto_lyrics_if_needed(&self) {
        let Some(track_id) = current_playback_track_id(&self.state.player.borrow()) else {
            return;
        };
        if self.state.lyrics.borrow().is_some() {
            return;
        }
        let settings = self.state.settings.borrow();
        let lyrics_surface_visible = self.lyrics_surface_visible();
        let request =
            auto_lyrics_request_for_settings(&settings, &track_id, lyrics_surface_visible);
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
    pub(in crate::ui) fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
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
        if self.state.lyrics.borrow().as_ref().is_some_and(|lyrics| {
            lyrics.track_id == track_id && lyrics.source == LyricsSource::Remote
        }) {
            *self.state.lyrics.borrow_mut() = None;
            self.controller.clear_remote_lyrics_for_current();
        }
        self.render_lyrics_panel();
    }
    pub(in crate::ui) fn lyrics_empty_status(&self) -> String {
        let settings = self.state.settings.borrow();
        if settings.private_mode {
            tr("No server lyrics for the current track. Private mode is on.")
        } else if !settings.external_lyrics_enabled {
            tr("No server lyrics for the current track. External lyric lookup is off.")
        } else {
            tr("No lyrics for the current track.")
        }
    }
    pub(in crate::ui) fn update_lyrics_highlight_at(self: &Rc<Self>, position_millis: u64) {
        let lyrics = self.state.lyrics.borrow();
        self.lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.fullscreen_player
            .lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.schedule_next_lyrics_highlight(position_millis);
    }
    pub(in crate::ui) fn lyrics_surface_visible(&self) -> bool {
        self.state.lyrics_panel_visible.get() || self.state.fullscreen_player_visible.get()
    }
    pub(in crate::ui) fn current_position_millis(&self) -> u64 {
        self.state.player.borrow().position_millis
    }
    pub(in crate::ui) fn seek_to_lyrics_position(self: &Rc<Self>, position_millis: u64) {
        self.lyrics_pane.clear_follow_scroll_pause();
        self.fullscreen_player
            .lyrics_pane
            .clear_follow_scroll_pause();
        self.controller.seek_millis(position_millis);
        self.update_lyrics_highlight_at(position_millis);
    }
}

pub(in crate::ui) fn allow_loaded_lyrics_cache_revisit(
    attempted: &mut HashSet<TrackId>,
    lyrics: Option<&Lyrics>,
) {
    if let Some(lyrics) = lyrics {
        attempted.remove(&lyrics.track_id);
    }
}
pub(in crate::ui) fn loaded_lyrics_matches_current(
    current_track: Option<&TrackId>,
    track_id: &TrackId,
    lyrics: Option<&Lyrics>,
) -> bool {
    current_track.is_some_and(|current| current == track_id)
        && lyrics.is_none_or(|lyrics| &lyrics.track_id == track_id)
}
