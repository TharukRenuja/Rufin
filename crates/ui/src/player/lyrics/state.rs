use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;
use localization::tr;
use lyrics::{CurrentLyrics, CurrentLyricsContent, LyricsDocument, LyricsOrigin};

use crate::player::lyrics::search::LyricsSearchDialog;
use crate::player::lyrics::settings::LyricsSettingsDialog;
use crate::player::state::{current_playback_media_id, current_playback_track_id};
use crate::shell::Shell;

pub(crate) struct LyricsState {
    pub(crate) projection: RefCell<CurrentLyrics>,
    pub(crate) offset_millis: Cell<i64>,
    pub(crate) timing_generation: Cell<u64>,
    pub(crate) timing_source: RefCell<Option<glib::SourceId>>,
    pub(crate) panel_visible: Cell<bool>,
    pub(crate) right_pane_dirty: Cell<bool>,
    pub(crate) fullscreen_pane_dirty: Cell<bool>,
    pub(crate) search_dialog: RefCell<Option<LyricsSearchDialog>>,
    pub(crate) settings_dialog: RefCell<Option<LyricsSettingsDialog>>,
}

impl Shell {
    pub(crate) fn right_lyrics_surface_visible(&self) -> bool {
        self.right_sidebar_visible() && self.lyrics.panel_visible.get()
    }

    pub(crate) fn fullscreen_lyrics_surface_visible(&self) -> bool {
        self.fullscreen_player_visible()
            && self
                .player_view
                .fullscreen_player
                .stack
                .visible_child_name()
                .as_deref()
                == Some("lyrics")
    }

    pub(crate) fn lyrics_surface_visible(&self) -> bool {
        self.right_lyrics_surface_visible() || self.fullscreen_lyrics_surface_visible()
    }

    pub(crate) fn visible_lyrics(&self) -> Option<Arc<LyricsDocument>> {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        match &*self.lyrics.projection.borrow() {
            CurrentLyrics::Ready {
                media_id,
                content: Some(CurrentLyricsContent::Document { document, .. }),
                ..
            } if current_media.as_ref() == Some(media_id) => Some(document.clone()),
            CurrentLyrics::Cleared
            | CurrentLyrics::Loading { .. }
            | CurrentLyrics::Ready { .. } => None,
        }
    }

    pub(crate) fn visible_lyrics_are_instrumental(&self) -> bool {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        matches!(
            &*self.lyrics.projection.borrow(),
            CurrentLyrics::Ready {
                media_id,
                content: Some(CurrentLyricsContent::Instrumental),
                ..
            } if current_media.as_ref() == Some(media_id)
        )
    }

    fn current_lyrics_resolved(&self) -> bool {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        matches!(
            &*self.lyrics.projection.borrow(),
            CurrentLyrics::Ready {
                media_id,
                content: Some(_),
                ..
            } if current_media.as_ref() == Some(media_id)
        )
    }

    pub(crate) fn visible_lyrics_have_word_timing(&self) -> bool {
        self.visible_lyrics()
            .is_some_and(|document| document.has_word_timing())
    }

    pub(crate) fn visible_lyrics_origin(&self) -> Option<LyricsOrigin> {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        match &*self.lyrics.projection.borrow() {
            CurrentLyrics::Ready {
                media_id, origin, ..
            } if current_media.as_ref() == Some(media_id) => *origin,
            _ => None,
        }
    }

    pub(crate) fn visible_lyrics_pronunciation(&self) -> Option<Arc<LyricsDocument>> {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        match &*self.lyrics.projection.borrow() {
            CurrentLyrics::Ready {
                media_id, content, ..
            } if current_media.as_ref() == Some(media_id) => match content {
                Some(CurrentLyricsContent::Document { pronunciation, .. }) => pronunciation.clone(),
                Some(CurrentLyricsContent::Instrumental) | None => None,
            },
            _ => None,
        }
    }

    pub(crate) fn current_lyrics_loading(&self) -> bool {
        let current_media = current_playback_media_id(&self.playback.player.borrow());
        matches!(
            &*self.lyrics.projection.borrow(),
            CurrentLyrics::Loading { media_id }
                if current_media.as_ref() == Some(media_id)
        )
    }

    pub(crate) fn apply_current_lyrics(self: &Rc<Self>, projection: CurrentLyrics) {
        let document_changed = match (&*self.lyrics.projection.borrow(), &projection) {
            (
                CurrentLyrics::Ready {
                    content:
                        Some(CurrentLyricsContent::Document {
                            document: previous, ..
                        }),
                    ..
                },
                CurrentLyrics::Ready {
                    content: Some(CurrentLyricsContent::Document { document: next, .. }),
                    ..
                },
            ) => !Arc::ptr_eq(previous, next),
            (
                _,
                CurrentLyrics::Ready {
                    content: Some(CurrentLyricsContent::Document { .. }),
                    ..
                },
            ) => true,
            _ => false,
        };
        let (media_id, has_lyrics) = match &projection {
            CurrentLyrics::Ready {
                media_id, content, ..
            } => (Some(media_id.clone()), content.is_some()),
            CurrentLyrics::Loading { media_id } => (Some(media_id.clone()), false),
            CurrentLyrics::Cleared => (None, false),
        };
        if document_changed {
            self.restart_lyrics_follow_tracking();
            self.lyrics.offset_millis.set(0);
        }
        *self.lyrics.projection.borrow_mut() = projection;
        crate::player::lyrics::settings::refresh_word_highlighting_availability(self);
        self.render_lyrics_panel();
        if let Some(media_id) = media_id
            && let Some(dialog) = self.lyrics.search_dialog.borrow().as_ref()
            && dialog.media_id == media_id
            && dialog.status.text().as_str() == tr("Searching...")
            && !self.current_lyrics_loading()
        {
            dialog.status.set_text(&if has_lyrics {
                tr("Loaded in lyrics panel.")
            } else {
                tr("No lyrics found.")
            });
        }
        if document_changed {
            self.refocus_current_lyrics_highlight();
            let shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                shell.refocus_current_lyrics_highlight();
            });
        } else {
            let shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                shell.restart_lyrics_follow_tracking();
                shell.update_lyrics_highlight();
            });
        }
    }

    fn restart_lyrics_follow_tracking(&self) {
        self.right_panel.lyrics_pane.restart_follow_tracking();
        self.player_view
            .fullscreen_player
            .lyrics_pane
            .restart_follow_tracking();
    }

    pub(crate) fn refocus_current_lyrics_highlight(&self) {
        let lyrics = self.visible_lyrics();
        let position_millis = self.lyrics_position_millis(self.current_position_millis());
        if self.right_lyrics_surface_visible() {
            self.right_panel
                .lyrics_pane
                .refocus_highlight(lyrics.as_deref(), position_millis);
        }
        if self.fullscreen_lyrics_surface_visible() {
            self.player_view
                .fullscreen_player
                .lyrics_pane
                .refocus_highlight(lyrics.as_deref(), position_millis);
        }
    }

    pub(crate) fn request_initial_lyrics_if_needed(&self) {
        self.request_auto_lyrics_if_needed();
    }

    pub(crate) fn request_auto_lyrics_if_needed(&self) {
        let Some(media_id) = current_playback_media_id(&self.playback.player.borrow()) else {
            return;
        };
        if self.current_lyrics_resolved() || self.current_lyrics_loading() {
            return;
        }
        if !self.lyrics_surface_visible() {
            return;
        }
        self.products.lyrics.load(media_id);
    }

    pub(crate) fn suppress_auto_lyrics_for_current(self: &Rc<Self>) {
        let Some(media_id) = current_playback_media_id(&self.playback.player.borrow()) else {
            return;
        };
        let Some(track_id) = current_playback_track_id(&self.playback.player.borrow()) else {
            return;
        };
        self.products.lyrics.clear_fetched(media_id);
        self.update_app_settings("lyrics auto-search setting", |settings| {
            settings.lyrics.suppress_auto_lyrics(&track_id)
        });
        self.render_lyrics_panel();
    }

    pub(crate) fn lyrics_empty_status(&self) -> String {
        let settings = self.settings.current.borrow();
        if settings.private_mode {
            tr("No server lyrics for the current track. Private mode is on.")
        } else if !settings.lyrics.external_lyrics_enabled {
            tr("No server lyrics for the current track. External lyric lookup is off.")
        } else {
            tr("No lyrics for the current track.")
        }
    }

    pub(crate) fn update_lyrics_highlight(self: &Rc<Self>) {
        self.cancel_scheduled_lyrics_highlight();
        self.update_lyrics_highlight_at(self.current_position_millis());
    }

    pub(crate) fn update_lyrics_highlight_at(self: &Rc<Self>, position_millis: u64) {
        if !self.lyrics_surface_visible() {
            return;
        }
        let lyrics = self.visible_lyrics();
        let lyrics_position_millis = self.lyrics_position_millis(position_millis);
        if self.right_lyrics_surface_visible() {
            self.right_panel
                .lyrics_pane
                .update_highlight(lyrics.as_deref(), lyrics_position_millis);
        }
        if self.fullscreen_lyrics_surface_visible() {
            self.player_view
                .fullscreen_player
                .lyrics_pane
                .update_highlight(lyrics.as_deref(), lyrics_position_millis);
        }
        self.schedule_next_lyrics_highlight(position_millis);
    }

    pub(crate) fn lyrics_position_millis(&self, position_millis: u64) -> i128 {
        i128::from(position_millis) + i128::from(self.lyrics.offset_millis.get())
    }

    pub(crate) fn adjust_lyrics_offset(self: &Rc<Self>, delta_millis: i64) {
        self.set_lyrics_offset(self.lyrics.offset_millis.get().saturating_add(delta_millis));
    }

    pub(crate) fn set_lyrics_offset_from_text(self: &Rc<Self>, value: &str) {
        let Some(offset_millis) = parse_lyrics_offset_millis(value) else {
            self.update_lyrics_offset_controls();
            return;
        };
        self.set_lyrics_offset(offset_millis);
    }

    pub(crate) fn apply_lyrics_offset_from_text(self: &Rc<Self>, value: &str) {
        let Some(offset_millis) = parse_lyrics_offset_millis(value) else {
            return;
        };
        if self.lyrics.offset_millis.replace(offset_millis) != offset_millis {
            self.update_lyrics_highlight();
        }
    }

    fn set_lyrics_offset(self: &Rc<Self>, offset_millis: i64) {
        let changed = self.lyrics.offset_millis.replace(offset_millis) != offset_millis;
        self.update_lyrics_offset_controls();
        if changed {
            self.update_lyrics_highlight();
        }
    }

    fn update_lyrics_offset_controls(&self) {
        let label = tr("Lyrics offset (ms)");
        let decrease_label = tr("Decrease");
        let increase_label = tr("Increase");
        let offset_millis = self.lyrics.offset_millis.get();
        for pane in [
            &self.right_panel.lyrics_pane,
            &self.player_view.fullscreen_player.lyrics_pane,
        ] {
            pane.set_offset_action(
                &label,
                &decrease_label,
                &increase_label,
                offset_millis,
                true,
            );
        }
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
        let position_millis =
            playback_position_for_lyrics_position(position_millis, self.lyrics.offset_millis.get());
        self.products
            .playback
            .transport
            .seek_millis(position_millis);
        self.update_lyrics_highlight_at(position_millis);
    }
}

fn playback_position_for_lyrics_position(position_millis: u64, offset_millis: i64) -> u64 {
    if offset_millis >= 0 {
        position_millis.saturating_sub(offset_millis.unsigned_abs())
    } else {
        position_millis.saturating_add(offset_millis.unsigned_abs())
    }
}

fn parse_lyrics_offset_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    let number = ["ms", "MS", "Ms", "mS"]
        .into_iter()
        .find_map(|suffix| value.strip_suffix(suffix))
        .unwrap_or(value)
        .trim();
    number.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{parse_lyrics_offset_millis, playback_position_for_lyrics_position};

    #[test]
    fn lyrics_offset_input_accepts_milliseconds() {
        assert_eq!(parse_lyrics_offset_millis("100ms"), Some(100));
        assert_eq!(parse_lyrics_offset_millis(" -250 ms "), Some(-250));
        assert_eq!(parse_lyrics_offset_millis("50"), Some(50));
        assert_eq!(parse_lyrics_offset_millis("later"), None);
    }

    #[test]
    fn lyrics_row_seek_applies_the_inverse_offset() {
        assert_eq!(playback_position_for_lyrics_position(5_000, 250), 4_750);
        assert_eq!(playback_position_for_lyrics_position(5_000, -250), 5_250);
        assert_eq!(playback_position_for_lyrics_position(100, 250), 0);
    }
}
