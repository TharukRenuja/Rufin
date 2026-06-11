use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use rufin_core::{
    AlbumId, EQUALIZER_BAND_COUNT, EqualizerSettings, PlaybackSettings, PlaybackTransitionMode,
    ReplayGainMode, TrackId,
};
use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;
use std::fmt;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};

mod fake_backend;
mod gstreamer_backend;
mod waveform;

pub use fake_backend::FakePlaybackBackend;
pub use gstreamer_backend::{GStreamerPlaybackBackend, LazyGStreamerPlaybackBackend};
pub use waveform::generate_waveform_peaks;

#[cfg(test)]
use gstreamer_backend::{
    AboutToFinishAction, CrossfadeState, GstEngine, PendingSeek, PlayerPipeline,
    SharedPlaybackState, Slot, about_to_finish_action, cancel_crossfade_next,
    cancel_gapless_pending, same_album_crossfade_is_skipped,
};
use gstreamer_backend::{
    clock_seconds_from_millis, position_event, position_event_for_track, redact_sensitive_uri,
};

#[cfg(test)]
mod tests;

const SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(1_000);
const TRACK_START_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const STARTUP_SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const SEEK_POSITION_TOLERANCE_MILLIS: u64 = 1_500;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackTrack {
    pub id: TrackId,
    pub album_id: Option<AlbumId>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: u32,
}
#[derive(Clone, Eq, PartialEq)]
pub struct StreamDescriptor {
    uri: String,
    redacted_uri: String,
    source_start_millis: Option<u64>,
    source_end_millis: Option<u64>,
}
impl StreamDescriptor {
    pub fn new(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        let redacted_uri = redact_sensitive_uri(&uri);
        Self {
            uri,
            redacted_uri,
            source_start_millis: None,
            source_end_millis: None,
        }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
            source_start_millis: None,
            source_end_millis: None,
        }
    }

    pub fn with_source_window(mut self, start_millis: u64, end_millis: u64) -> Self {
        if end_millis > start_millis {
            self.source_start_millis = Some(start_millis);
            self.source_end_millis = Some(end_millis);
        }
        self
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn redacted_uri(&self) -> &str {
        &self.redacted_uri
    }

    pub fn source_start_millis(&self) -> u64 {
        self.source_start_millis.unwrap_or(0)
    }

    pub fn source_end_millis(&self) -> Option<u64> {
        self.source_end_millis
    }
}
impl fmt::Debug for StreamDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamDescriptor")
            .field("uri", &self.redacted_uri)
            .finish()
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPlaybackItem {
    pub track: PlaybackTrack,
    pub stream: StreamDescriptor,
}
impl PreparedPlaybackItem {
    pub fn new(track: PlaybackTrack, stream: StreamDescriptor) -> Self {
        Self { track, stream }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Buffering,
    Paused,
    Playing,
}
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PlaybackCommand {
    WarmUp(PlaybackSettings),
    Play {
        track: PlaybackTrack,
        stream: StreamDescriptor,
        start_position_seconds: u32,
    },
    PlayPrepared {
        item: PreparedPlaybackItem,
        next: Option<PreparedPlaybackItem>,
        start_position_seconds: u32,
        settings: PlaybackSettings,
    },
    PrepareNext(Option<PreparedPlaybackItem>),
    UpdateSettings(PlaybackSettings),
    Resume,
    Pause,
    Stop,
    Seek(u32),
    SeekMillis(u64),
    SetVolume(f64),
    SetMuted(bool),
    SetVisualizerEnabled(bool),
}
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    StateChanged(PlaybackState),
    PositionChanged {
        track_id: Option<TrackId>,
        seconds: u32,
        millis: u64,
    },
    DurationChanged {
        track_id: Option<TrackId>,
        seconds: u32,
    },
    Buffering(u8),
    EndOfStream,
    PreparedTrackStarted(PlaybackTrack),
    VolumeChanged {
        volume: f64,
        muted: bool,
    },
    Visualizer(Vec<f64>),
    Error(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}
pub fn available_audio_outputs() -> Vec<AudioOutput> {
    let _ = gst::init();
    let candidates = [
        ("autoaudiosink", "System default"),
        ("pipewiresink", "PipeWire"),
        ("pulsesink", "PulseAudio"),
        ("alsasink", "ALSA"),
        ("jackaudiosink", "JACK"),
        ("osxaudiosink", "macOS"),
        ("wasapisink", "WASAPI"),
        ("directsoundsink", "DirectSound"),
    ];
    candidates
        .into_iter()
        .filter(|(id, _)| gst::ElementFactory::find(id).is_some())
        .map(|(id, name)| AudioOutput {
            id: id.to_string(),
            name: name.to_string(),
        })
        .collect()
}
#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("playback backend failed: {0}")]
    Backend(String),
    #[error("playback command channel closed")]
    ChannelClosed,
}
pub trait PlaybackBackend: Send {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError>;
    fn drain_events(&mut self) -> Vec<PlaybackEvent>;
}
