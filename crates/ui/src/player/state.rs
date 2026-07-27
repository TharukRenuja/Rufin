use std::cell::{Cell, RefCell};
use std::time::Instant;

use gtk::glib;
use playback::{CurrentMediaId, PlaybackView};

use crate::runtime::WaveformProjection;
pub(crate) struct PlaybackState {
    pub(crate) player: RefCell<Option<PlaybackView>>,
    pub(crate) waveform: RefCell<WaveformProjection>,
    pub(crate) updating_controls: Cell<bool>,
    pub(crate) volume_persist_source: RefCell<Option<glib::SourceId>>,
    pub(crate) seek_preview_seconds: Cell<Option<u32>>,
    pub(crate) seek_generation: Cell<u64>,
    pub(crate) audio_output_options: RefCell<Vec<(Option<String>, String)>>,
    pub(crate) audio_output_refresh_running: Cell<bool>,
    pub(crate) audio_output_refresh_generation: Cell<u64>,
    pub(crate) audio_output_refreshed_at: Cell<Option<Instant>>,
}

pub(crate) fn current_playback_track(player: &Option<PlaybackView>) -> Option<::library::Track> {
    player
        .as_ref()?
        .transport
        .current
        .as_ref()
        .map(|entry| entry.track.clone())
}

pub(crate) fn current_playback_track_id(
    player: &Option<PlaybackView>,
) -> Option<::library::TrackId> {
    current_playback_track(player).map(|track| track.id.clone())
}

pub(crate) fn current_playback_media_id(player: &Option<PlaybackView>) -> Option<CurrentMediaId> {
    player
        .as_ref()?
        .transport
        .current
        .as_ref()
        .map(|current| current.id.clone())
}
