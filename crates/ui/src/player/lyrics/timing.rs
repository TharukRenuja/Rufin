use super::view::next_lyrics_line_start_after;
use crate::shell::Shell;
use gtk::glib;
use playback::TransportStatus;
use std::rc::Rc;
use std::time::Duration;

impl Shell {
    pub(crate) fn cancel_scheduled_lyrics_highlight(&self) {
        let Some(lyrics) = self.selected_lyrics() else {
            return;
        };
        lyrics
            .timing_generation
            .set(lyrics.timing_generation.get().saturating_add(1));
        if let Some(source) = lyrics.timing_source.borrow_mut().take() {
            source.remove();
        }
    }
    pub(crate) fn schedule_next_lyrics_highlight(self: &Rc<Self>, position_millis: u64) {
        if !self.lyrics_surface_visible() {
            return;
        }
        let playing = self
            .selected_playback()
            .as_deref()
            .is_some_and(|player| matches!(player.transport.state, TransportStatus::Playing));
        if !playing {
            return;
        }

        let Some(next_position_millis) = self.visible_lyrics().as_ref().and_then(|lyrics| {
            next_lyrics_line_start_after(
                &lyrics.lines,
                self.lyrics_position_millis(position_millis),
            )
        }) else {
            return;
        };
        let lyrics_position_millis = self.lyrics_position_millis(position_millis);
        let Ok(delay_millis) =
            u64::try_from(i128::from(next_position_millis) - lyrics_position_millis)
        else {
            return;
        };
        let next_playback_position_millis = position_millis.saturating_add(delay_millis);
        let Some(lyrics) = self.selected_lyrics() else {
            return;
        };
        let generation = lyrics.timing_generation.get().saturating_add(1);
        lyrics.timing_generation.set(generation);
        drop(lyrics);

        let shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(delay_millis), move || {
            let Some(lyrics) = shell.selected_lyrics() else {
                return;
            };
            if lyrics.timing_generation.get() != generation {
                return;
            }
            let _source = lyrics.timing_source.borrow_mut().take();
            drop(lyrics);
            shell.update_lyrics_highlight_at(next_playback_position_millis);
        });
        let Some(lyrics) = self.selected_lyrics() else {
            source.remove();
            return;
        };
        if let Some(previous_source) = lyrics.timing_source.borrow_mut().replace(source) {
            previous_source.remove();
        }
    }
}
