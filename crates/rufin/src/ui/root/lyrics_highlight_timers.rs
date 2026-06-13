use super::*;

impl Shell {
    pub(in crate::ui) fn cancel_scheduled_lyrics_highlight(&self) {
        self.state
            .lyrics_timing_generation
            .set(self.state.lyrics_timing_generation.get().saturating_add(1));
        if let Some(source) = self.state.lyrics_timing_source.borrow_mut().take() {
            source.remove();
        }
    }
    pub(in crate::ui) fn schedule_next_lyrics_highlight(self: &Rc<Self>, position_millis: u64) {
        let playing = matches!(self.state.player.borrow().state, PlaybackState::Playing);
        if !playing {
            return;
        }

        let Some(next_position_millis) = self
            .state
            .lyrics
            .borrow()
            .as_ref()
            .and_then(|lyrics| next_lyrics_line_start_after(&lyrics.lines, position_millis))
        else {
            return;
        };
        let delay_millis = next_position_millis.saturating_sub(position_millis);
        let generation = self.state.lyrics_timing_generation.get().saturating_add(1);
        self.state.lyrics_timing_generation.set(generation);

        let shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(delay_millis), move || {
            if shell.state.lyrics_timing_generation.get() != generation {
                return;
            }
            let _source = shell.state.lyrics_timing_source.borrow_mut().take();
            shell.update_lyrics_highlight_at(next_position_millis);
        });
        if let Some(previous_source) = self.state.lyrics_timing_source.borrow_mut().replace(source)
        {
            previous_source.remove();
        }
    }
}
