use super::*;

impl Shell {
    pub(in crate::ui) fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }
    pub(in crate::ui) fn apply_loaded_lyrics(self: &Rc<Self>, lyrics: Option<Lyrics>) {
        self.restart_lyrics_follow_tracking();
        *self.state.lyrics.borrow_mut() = lyrics;
        self.render_lyrics_panel();
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.restart_lyrics_follow_tracking();
            shell.update_lyrics_highlight();
        });
    }
    pub(in crate::ui) fn apply_loaded_lyrics_for_media(
        self: &Rc<Self>,
        media_key: playback::MediaKey,
        lyrics: Option<Lyrics>,
    ) {
        clear_matching_lyrics_loading(
            &mut self.state.lyrics_loading_media.borrow_mut(),
            &media_key,
        );
        if !loaded_lyrics_matches_current(
            current_playback_media_key(&self.state.player.borrow()).as_ref(),
            &media_key,
            lyrics.as_ref(),
        ) {
            return;
        }
        allow_loaded_lyrics_cache_revisit(
            &mut self.state.lyrics_auto_search_attempted.borrow_mut(),
            &media_key,
            lyrics.as_ref(),
        );
        let has_lyrics = lyrics.is_some();
        self.apply_loaded_lyrics(lyrics);
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.media_key == media_key
            && dialog.status.text().as_str() == tr("Searching...")
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
        if current_playback_media_key(&self.state.player.borrow()).is_none() {
            return;
        }
        self.request_auto_lyrics_if_needed();
    }
    pub(in crate::ui) fn request_auto_lyrics_if_needed(&self) {
        let Some(media_key) = current_playback_media_key(&self.state.player.borrow()) else {
            return;
        };
        if self.state.lyrics.borrow().is_some() {
            return;
        }
        let lyrics_surface_visible = self.lyrics_surface_visible();
        if !lyrics_surface_visible {
            return;
        }
        let settings = self.state.settings.borrow();
        let request = settings
            .metadata
            .automatic_lyrics_request(settings.private_mode, &media_key.track_id);
        drop(settings);
        let requested_media = media_key.clone();
        if !self
            .state
            .lyrics_auto_search_attempted
            .borrow_mut()
            .insert(media_key)
        {
            return;
        }
        *self.state.lyrics_loading_media.borrow_mut() = Some(requested_media.clone());
        self.controller
            .request_lyrics_for_media(requested_media, request);
    }
    pub(in crate::ui) fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
        let Some(media_key) = current_playback_media_key(&self.state.player.borrow()) else {
            return;
        };
        let track_id = &media_key.track_id;
        {
            let mut attempted = self.state.lyrics_auto_search_attempted.borrow_mut();
            attempted.remove(&media_key);
        }
        {
            let mut settings = self.state.settings.borrow_mut();
            if settings.metadata.suppress_auto_lyrics(track_id) {
                if let Err(error) = self.controller.save_settings(&settings) {
                    warn!(%error, "failed to save lyrics auto-search setting");
                }
            }
        }
        if self.state.lyrics.borrow().as_ref().is_some_and(|lyrics| {
            &lyrics.track_id == track_id && lyrics.source == LyricsSource::Remote
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
        } else if !settings.metadata.external_lyrics_enabled {
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
        self.state
            .player
            .borrow()
            .as_ref()
            .map_or(0, |player| player.transport.position_millis)
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
    attempted: &mut HashSet<playback::MediaKey>,
    media_key: &playback::MediaKey,
    lyrics: Option<&Lyrics>,
) {
    if lyrics.is_some_and(|lyrics| lyrics.track_id == media_key.track_id) {
        attempted.remove(media_key);
    }
}
pub(in crate::ui) fn loaded_lyrics_matches_current(
    current_media: Option<&playback::MediaKey>,
    media_key: &playback::MediaKey,
    lyrics: Option<&Lyrics>,
) -> bool {
    current_media == Some(media_key)
        && lyrics.is_none_or(|lyrics| lyrics.track_id == media_key.track_id)
}
pub(in crate::ui) fn lyrics_loading_matches_current(
    current_media: Option<&playback::MediaKey>,
    loading_media: Option<&playback::MediaKey>,
    lyrics: Option<&Lyrics>,
) -> bool {
    lyrics.is_none() && current_media.is_some() && current_media == loading_media
}
pub(in crate::ui) fn clear_matching_lyrics_loading(
    loading_media: &mut Option<playback::MediaKey>,
    media_key: &playback::MediaKey,
) {
    if loading_media.as_ref() == Some(media_key) {
        *loading_media = None;
    }
}
