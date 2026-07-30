use crate::{EqualizerSettings, PlaybackSettings, ReplayGainMode};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunId(u64);

impl RunId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceWindow {
    pub start_millis: u64,
    pub end_millis: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct PreparedStream {
    uri: String,
    redacted_uri: String,
    trust_invalid_certificate: bool,
    window: Option<SourceWindow>,
}

impl PreparedStream {
    pub fn new(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        Self {
            redacted_uri: redact_sensitive_uri(&uri),
            uri,
            trust_invalid_certificate: false,
            window: None,
        }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
            trust_invalid_certificate: false,
            window: None,
        }
    }

    pub fn with_trust_invalid_certificate(mut self, trust: bool) -> Self {
        self.trust_invalid_certificate = trust;
        self
    }

    pub fn with_source_window(mut self, start_millis: u64, end_millis: u64) -> Self {
        if end_millis > start_millis {
            self.window = Some(SourceWindow {
                start_millis,
                end_millis,
            });
        }
        self
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn redacted_uri(&self) -> &str {
        &self.redacted_uri
    }

    pub fn trust_invalid_certificate(&self) -> bool {
        self.trust_invalid_certificate
    }

    pub fn source_start_millis(&self) -> u64 {
        self.window
            .as_ref()
            .map(|window| window.start_millis)
            .unwrap_or(0)
    }

    pub fn source_end_millis(&self) -> Option<u64> {
        self.window.as_ref().map(|window| window.end_millis)
    }

    pub fn source_window(&self) -> Option<&SourceWindow> {
        self.window.as_ref()
    }
}

impl std::fmt::Debug for PreparedStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedStream")
            .field("uri", &self.redacted_uri)
            .field("window", &self.window)
            .finish()
    }
}

fn redact_sensitive_uri(uri: &str) -> String {
    let Some((base, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| {
            let Some((key, value)) = pair.split_once('=') else {
                return pair.to_string();
            };
            let lower = key.to_ascii_lowercase();
            if lower.contains("token") || lower.contains("key") {
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NextTransition {
    #[default]
    Default,
    Gapless,
    Crossfade {
        duration_millis: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedNext {
    pub run: RunId,
    pub stream: PreparedStream,
    pub transition: NextTransition,
}

impl PreparedNext {
    pub fn new(run: RunId, stream: PreparedStream, transition: NextTransition) -> Self {
        Self {
            run,
            stream,
            transition,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackendAudioSettings {
    pub replay_gain: ReplayGainMode,
    pub audio_output: Option<String>,
    pub equalizer: EqualizerSettings,
    pub volume: f64,
    pub muted: bool,
    pub fade_on_status_change: bool,
}

impl Default for BackendAudioSettings {
    fn default() -> Self {
        Self::from(PlaybackSettings::default())
    }
}

impl From<PlaybackSettings> for BackendAudioSettings {
    fn from(mut settings: PlaybackSettings) -> Self {
        settings.sanitize();
        Self {
            replay_gain: settings.replay_gain,
            audio_output: settings.audio_output,
            equalizer: settings.equalizer,
            volume: settings.volume,
            muted: settings.muted,
            fade_on_status_change: settings.audio_fade_on_status_change,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackendState {
    #[default]
    Stopped,
    Buffering,
    Paused,
    Playing,
}
#[derive(Clone, Debug, PartialEq)]
pub enum BackendCommand {
    Start {
        run: RunId,
        current: PreparedStream,
        next: Option<PreparedNext>,
        start_position_millis: u64,
    },
    PrepareNext {
        current_run: RunId,
        next: Option<PreparedNext>,
    },
    Play {
        run: RunId,
    },
    Pause {
        run: RunId,
    },
    Stop {
        run: RunId,
    },
    Seek {
        run: RunId,
        position_millis: u64,
    },
    SetOutputVolume {
        volume: f64,
        muted: bool,
    },
    ConfigureAudio(BackendAudioSettings),
    SetVisualizerEnabled(bool),
}

impl BackendCommand {
    pub fn run(&self) -> Option<RunId> {
        match self {
            Self::Start { run, .. }
            | Self::Play { run }
            | Self::Pause { run }
            | Self::Stop { run }
            | Self::Seek { run, .. } => Some(*run),
            Self::PrepareNext { current_run, .. } => Some(*current_run),
            Self::SetOutputVolume { .. }
            | Self::ConfigureAudio(_)
            | Self::SetVisualizerEnabled(_) => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    Started {
        run: RunId,
    },
    State {
        run: RunId,
        state: BackendState,
    },
    Position {
        run: RunId,
        millis: u64,
    },
    Duration {
        run: RunId,
        millis: u64,
    },
    Buffering {
        run: RunId,
        percent: u8,
    },
    Ended {
        run: RunId,
    },
    Transitioned {
        old_run: RunId,
        new_run: RunId,
    },
    NextNeeded {
        run: RunId,
    },
    NextUnavailable {
        current_run: RunId,
        next_run: RunId,
        error: BackendFailure,
    },
    AudioApplied {
        volume: f64,
        muted: bool,
        output: Option<String>,
    },
    Visualizer {
        run: RunId,
        levels: Vec<f64>,
    },
    Error {
        run: RunId,
        error: BackendFailure,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("playback backend failed: {0}")]
    Backend(String),
    #[error("playback command channel closed")]
    ChannelClosed,
}
#[derive(Clone, Debug, Error, PartialEq)]
#[error("{message}")]
pub struct BackendFailure {
    message: String,
}

impl BackendFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub trait PlaybackBackend: Send {
    fn send(&mut self, command: BackendCommand) -> Result<(), BackendError>;
    fn drain_events(&mut self) -> Vec<BackendEvent>;

    fn shutdown(&mut self) -> Result<(), BackendError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_stream_debug_uses_redacted_uri() {
        let stream = PreparedStream::with_redacted(
            "https://music.test/audio?token=secret",
            "https://music.test/audio?token=<redacted>",
        );
        let debug = format!("{stream:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
    }
}
