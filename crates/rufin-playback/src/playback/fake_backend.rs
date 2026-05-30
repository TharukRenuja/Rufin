use super::*;

#[derive(Default)]
pub struct FakePlaybackBackend {
    state: PlaybackState,
    current: Option<PlaybackTrack>,
    next: Option<PreparedPlaybackItem>,
    settings: PlaybackSettings,
    position_seconds: u32,
    position_millis: u64,
    duration_seconds: u32,
    volume: f64,
    muted: bool,
    events: VecDeque<PlaybackEvent>,
}
impl FakePlaybackBackend {
    pub fn new() -> Self {
        let settings = PlaybackSettings::default();
        Self {
            state: PlaybackState::Stopped,
            current: None,
            next: None,
            volume: settings.volume,
            muted: settings.muted,
            settings,
            position_seconds: 0,
            position_millis: 0,
            duration_seconds: 0,
            events: VecDeque::new(),
        }
    }

    pub fn emit_end_of_stream_for_test(&mut self) {
        self.events.push_back(PlaybackEvent::EndOfStream);
    }

    pub fn emit_prepared_track_started_for_test(&mut self) {
        if let Some(next) = self.next.take() {
            self.current = Some(next.track.clone());
            self.duration_seconds = next.track.duration_seconds;
            self.position_seconds = 0;
            self.position_millis = 0;
            self.events
                .push_back(PlaybackEvent::PreparedTrackStarted(next.track));
        }
    }

    fn set_state(&mut self, state: PlaybackState) {
        self.state = state;
        self.events.push_back(PlaybackEvent::StateChanged(state));
    }

    fn play_item(&mut self, item: PreparedPlaybackItem, start_position_seconds: u32) {
        self.duration_seconds = item.track.duration_seconds;
        self.position_seconds = start_position_seconds.min(self.duration_seconds);
        self.position_millis = u64::from(self.position_seconds) * 1_000;
        self.current = Some(item.track);
        let track_id = self.current.as_ref().map(|track| track.id.clone());
        self.events.push_back(PlaybackEvent::DurationChanged {
            track_id: track_id.clone(),
            seconds: self.duration_seconds,
        });
        self.events
            .push_back(position_event_for_track(self.position_millis, track_id));
        self.set_state(PlaybackState::Playing);
    }
}
impl PlaybackBackend for FakePlaybackBackend {
    fn send(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        match command {
            PlaybackCommand::WarmUp(settings) => {
                self.settings = settings;
                self.volume = self.settings.volume;
                self.muted = self.settings.muted;
            }
            PlaybackCommand::Play {
                track,
                stream,
                start_position_seconds,
            } => self.play_item(
                PreparedPlaybackItem::new(track, stream),
                start_position_seconds,
            ),
            PlaybackCommand::PlayPrepared {
                item,
                next,
                start_position_seconds,
                settings,
            } => {
                self.settings = settings;
                self.volume = self.settings.volume;
                self.muted = self.settings.muted;
                self.next = next;
                self.play_item(item, start_position_seconds);
            }
            PlaybackCommand::PrepareNext(next) => self.next = next,
            PlaybackCommand::UpdateSettings(settings) => {
                self.settings = settings;
                self.volume = self.settings.volume;
                self.muted = self.settings.muted;
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
            PlaybackCommand::Resume => self.set_state(PlaybackState::Playing),
            PlaybackCommand::Pause => self.set_state(PlaybackState::Paused),
            PlaybackCommand::Stop => {
                self.position_seconds = 0;
                self.position_millis = 0;
                self.next = None;
                self.set_state(PlaybackState::Stopped);
                self.events.push_back(position_event(0));
            }
            PlaybackCommand::Seek(seconds) => {
                self.position_seconds = seconds.min(self.duration_seconds);
                self.position_millis = u64::from(self.position_seconds) * 1_000;
                self.events.push_back(position_event_for_track(
                    self.position_millis,
                    self.current.as_ref().map(|track| track.id.clone()),
                ));
            }
            PlaybackCommand::SeekMillis(millis) => {
                self.position_millis =
                    millis.min(u64::from(self.duration_seconds).saturating_mul(1_000));
                self.position_seconds = clock_seconds_from_millis(self.position_millis);
                self.events.push_back(position_event_for_track(
                    self.position_millis,
                    self.current.as_ref().map(|track| track.id.clone()),
                ));
            }
            PlaybackCommand::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
            PlaybackCommand::SetMuted(muted) => {
                self.muted = muted;
                self.events.push_back(PlaybackEvent::VolumeChanged {
                    volume: self.volume,
                    muted: self.muted,
                });
            }
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        self.events.drain(..).collect()
    }
}
