use crate::player::lyrics::search::LyricsSearchDialog;
use crate::player::state::current_playback_media_key;
use crate::shell::Shell;
use gtk::glib;
use localization::tr;
use metadata::{Lyrics, LyricsSource};
use playback::MediaKey;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

pub(crate) struct LyricsState {
    pub(crate) current: RefCell<Option<Lyrics>>,
    pub(crate) loading_media: RefCell<Option<MediaKey>>,
    pub(crate) auto_search_attempted: RefCell<HashSet<MediaKey>>,
    pub(crate) timing_generation: Cell<u64>,
    pub(crate) timing_source: RefCell<Option<glib::SourceId>>,
    pub(crate) panel_visible: Cell<bool>,
    pub(crate) search_dialog: RefCell<Option<LyricsSearchDialog>>,
}

impl Shell {
    pub(crate) fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }
    pub(crate) fn apply_loaded_lyrics(self: &Rc<Self>, lyrics: Option<Lyrics>) {
        self.restart_lyrics_follow_tracking();
        *self.lyrics.current.borrow_mut() = lyrics;
        self.render_lyrics_panel();
        let shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            shell.restart_lyrics_follow_tracking();
            shell.update_lyrics_highlight();
        });
    }
    pub(crate) fn apply_loaded_lyrics_for_media(
        self: &Rc<Self>,
        media_key: playback::MediaKey,
        lyrics: Option<Lyrics>,
    ) {
        clear_matching_lyrics_loading(&mut self.lyrics.loading_media.borrow_mut(), &media_key);
        if !loaded_lyrics_matches_current(
            current_playback_media_key(&self.playback.player.borrow()).as_ref(),
            &media_key,
            lyrics.as_ref(),
        ) {
            return;
        }
        allow_loaded_lyrics_cache_revisit(
            &mut self.lyrics.auto_search_attempted.borrow_mut(),
            &media_key,
            lyrics.as_ref(),
        );
        let has_lyrics = lyrics.is_some();
        self.apply_loaded_lyrics(lyrics);
        if let Some(dialog) = self.lyrics.search_dialog.borrow().as_ref()
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
        self.right_panel.lyrics_pane.restart_follow_tracking();
        self.player_view
            .fullscreen_player
            .lyrics_pane
            .restart_follow_tracking();
    }
    pub(crate) fn request_initial_lyrics_if_needed(&self) {
        if current_playback_media_key(&self.playback.player.borrow()).is_none() {
            return;
        }
        self.request_auto_lyrics_if_needed();
    }
    pub(crate) fn request_auto_lyrics_if_needed(&self) {
        let Some(media_key) = current_playback_media_key(&self.playback.player.borrow()) else {
            return;
        };
        if self.lyrics.current.borrow().is_some() {
            return;
        }
        let lyrics_surface_visible =
            self.lyrics.panel_visible.get() || self.fullscreen_player_visible();
        if !lyrics_surface_visible {
            return;
        }
        let settings = self.settings.current.borrow();
        let request = settings
            .metadata
            .automatic_lyrics_request(settings.private_mode, &media_key.track_id);
        drop(settings);
        let requested_media = media_key.clone();
        if !self
            .lyrics
            .auto_search_attempted
            .borrow_mut()
            .insert(media_key)
        {
            return;
        }
        *self.lyrics.loading_media.borrow_mut() = Some(requested_media.clone());
        self.products.lyrics.request(requested_media, request);
    }
    pub(crate) fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
        let Some(media_key) = current_playback_media_key(&self.playback.player.borrow()) else {
            return;
        };
        let track_id = &media_key.track_id;
        {
            let mut attempted = self.lyrics.auto_search_attempted.borrow_mut();
            attempted.remove(&media_key);
        }
        self.update_app_settings("lyrics auto-search setting", |settings| {
            settings.metadata.suppress_auto_lyrics(track_id)
        });
        if self.lyrics.current.borrow().as_ref().is_some_and(|lyrics| {
            &lyrics.track_id == track_id && lyrics.source == LyricsSource::Remote
        }) {
            *self.lyrics.current.borrow_mut() = None;
            self.products.lyrics.clear_remote_current();
        }
        self.render_lyrics_panel();
    }
    pub(crate) fn lyrics_empty_status(&self) -> String {
        let settings = self.settings.current.borrow();
        if settings.private_mode {
            tr("No server lyrics for the current track. Private mode is on.")
        } else if !settings.metadata.external_lyrics_enabled {
            tr("No server lyrics for the current track. External lyric lookup is off.")
        } else {
            tr("No lyrics for the current track.")
        }
    }
    pub(crate) fn update_lyrics_highlight_at(self: &Rc<Self>, position_millis: u64) {
        let lyrics = self.lyrics.current.borrow();
        self.right_panel
            .lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.player_view
            .fullscreen_player
            .lyrics_pane
            .update_highlight(lyrics.as_ref(), position_millis);
        self.schedule_next_lyrics_highlight(position_millis);
    }
    pub(crate) fn current_position_millis(&self) -> u64 {
        self.playback
            .player
            .borrow()
            .as_ref()
            .map_or(0, |player| player.transport.position_millis)
    }
    pub(crate) fn seek_to_lyrics_position(self: &Rc<Self>, position_millis: u64) {
        self.right_panel.lyrics_pane.clear_follow_scroll_pause();
        self.player_view
            .fullscreen_player
            .lyrics_pane
            .clear_follow_scroll_pause();
        self.products
            .playback
            .transport
            .seek_millis(position_millis);
        self.update_lyrics_highlight_at(position_millis);
    }
}

fn allow_loaded_lyrics_cache_revisit(
    attempted: &mut HashSet<playback::MediaKey>,
    media_key: &playback::MediaKey,
    lyrics: Option<&Lyrics>,
) {
    if lyrics.is_some_and(|lyrics| lyrics.track_id == media_key.track_id) {
        attempted.remove(media_key);
    }
}
fn loaded_lyrics_matches_current(
    current_media: Option<&playback::MediaKey>,
    media_key: &playback::MediaKey,
    lyrics: Option<&Lyrics>,
) -> bool {
    current_media == Some(media_key)
        && lyrics.is_none_or(|lyrics| lyrics.track_id == media_key.track_id)
}
pub(super) fn lyrics_loading_matches_current(
    current_media: Option<&playback::MediaKey>,
    loading_media: Option<&playback::MediaKey>,
    lyrics: Option<&Lyrics>,
) -> bool {
    lyrics.is_none() && current_media.is_some() && current_media == loading_media
}
fn clear_matching_lyrics_loading(
    loading_media: &mut Option<playback::MediaKey>,
    media_key: &playback::MediaKey,
) {
    if loading_media.as_ref() == Some(media_key) {
        *loading_media = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        allow_loaded_lyrics_cache_revisit, clear_matching_lyrics_loading,
        loaded_lyrics_matches_current, lyrics_loading_matches_current,
    };
    use library::{SourceId, TrackId};
    use metadata::{Lyrics, LyricsSource};
    use playback::MediaKey;
    use std::collections::HashSet;

    fn media(source: &str, track: usize) -> MediaKey {
        MediaKey {
            source_id: SourceId::new(source),
            track_id: TrackId::fake(track),
        }
    }

    fn lyrics(track: usize) -> Lyrics {
        Lyrics {
            track_id: TrackId::fake(track),
            source: LyricsSource::Server,
            external_provider: None,
            lines: Vec::new(),
        }
    }

    #[test]
    fn loaded_lyrics_require_the_exact_media_and_track() {
        let current = media("current", 13);
        let old = media("current", 12);
        let other_source = media("other", 13);
        let current_lyrics = lyrics(13);
        let old_lyrics = lyrics(12);

        assert!(loaded_lyrics_matches_current(
            Some(&current),
            &current,
            Some(&current_lyrics)
        ));
        assert!(loaded_lyrics_matches_current(
            Some(&current),
            &current,
            None
        ));
        assert!(!loaded_lyrics_matches_current(
            Some(&current),
            &old,
            Some(&old_lyrics)
        ));
        assert!(!loaded_lyrics_matches_current(
            Some(&current),
            &current,
            Some(&old_lyrics)
        ));
        assert!(!loaded_lyrics_matches_current(
            Some(&current),
            &other_source,
            Some(&current_lyrics)
        ));
        assert!(!loaded_lyrics_matches_current(None, &current, None));
    }

    #[test]
    fn lyrics_request_state_is_scoped_to_the_exact_media() {
        let current = media("current", 13);
        let old = media("current", 12);
        let other_source = media("other", 13);
        let current_lyrics = lyrics(13);

        assert!(lyrics_loading_matches_current(
            Some(&current),
            Some(&current),
            None
        ));
        assert!(!lyrics_loading_matches_current(
            Some(&current),
            Some(&old),
            None
        ));
        assert!(!lyrics_loading_matches_current(
            Some(&current),
            Some(&other_source),
            None
        ));
        assert!(!lyrics_loading_matches_current(
            Some(&current),
            Some(&current),
            Some(&current_lyrics)
        ));

        let mut loading = Some(old.clone());
        clear_matching_lyrics_loading(&mut loading, &current);
        assert_eq!(loading.as_ref(), Some(&old));
        clear_matching_lyrics_loading(&mut loading, &old);
        assert_eq!(loading, None);

        let mut attempted = HashSet::from([current.clone(), old.clone(), other_source.clone()]);
        allow_loaded_lyrics_cache_revisit(&mut attempted, &current, Some(&current_lyrics));
        assert!(!attempted.contains(&current));
        assert!(attempted.contains(&old));
        assert!(attempted.contains(&other_source));
        allow_loaded_lyrics_cache_revisit(&mut attempted, &old, None);
        assert!(attempted.contains(&old));
    }
}
