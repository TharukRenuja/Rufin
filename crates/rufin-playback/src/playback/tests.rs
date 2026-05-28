    use super::{
        CrossfadeState, FakePlaybackBackend, GstEngine, PendingSeek, PlaybackBackend,
        PlaybackCommand, PlaybackEvent, PlaybackState, PlaybackTrack, PlayerPipeline,
        PreparedPlaybackItem, SEEK_SETTLE_WINDOW, STARTUP_SEEK_SETTLE_WINDOW,
        SharedPlaybackState, Slot, StreamDescriptor,
    };
    use rufin_core::{PlaybackSettings, PlaybackTransitionMode, TrackId};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    #[test]
    fn fake_backend_reports_basic_state_transitions() {
        let mut backend = FakePlaybackBackend::new();
        let track = track(1);

        backend
            .send(PlaybackCommand::Play {
                track,
                stream: StreamDescriptor::new("fake://track/1"),
                start_position_seconds: 12,
            })
            .expect("play");
        backend.send(PlaybackCommand::Pause).expect("pause");
        backend.send(PlaybackCommand::Resume).expect("resume");
        backend.send(PlaybackCommand::Seek(42)).expect("seek");
        backend
            .send(PlaybackCommand::SeekMillis(42_500))
            .expect("seek millis");
        backend.send(PlaybackCommand::Stop).expect("stop");

        let events = backend.drain_events();
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Playing)));
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Paused)));
        assert!(events.contains(&PlaybackEvent::PositionChanged {
            seconds: 42,
            millis: 42_000,
        }));
        assert!(events.contains(&PlaybackEvent::PositionChanged {
            seconds: 42,
            millis: 42_500,
        }));
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Stopped)));
    }
    #[test]
    fn fake_backend_tracks_prepared_next_item() {
        let mut backend = FakePlaybackBackend::new();
        let current = PreparedPlaybackItem::new(track(1), StreamDescriptor::new("fake://track/1"));
        let next = PreparedPlaybackItem::new(track(2), StreamDescriptor::new("fake://track/2"));
        let settings = PlaybackSettings {
            transition_mode: PlaybackTransitionMode::Gapless,
            ..PlaybackSettings::default()
        };

        backend
            .send(PlaybackCommand::PlayPrepared {
                item: current,
                next: Some(next.clone()),
                start_position_seconds: 0,
                settings,
            })
            .expect("play prepared");
        backend.emit_prepared_track_started_for_test();

        let events = backend.drain_events();
        assert!(events.contains(&PlaybackEvent::PreparedTrackStarted(next.track)));
    }
    #[test]
    fn fake_backend_updates_prepared_next_item() {
        let mut backend = FakePlaybackBackend::new();
        let next = PreparedPlaybackItem::new(track(3), StreamDescriptor::new("fake://track/3"));

        backend
            .send(PlaybackCommand::PrepareNext(Some(next.clone())))
            .expect("prepare next");
        backend.emit_prepared_track_started_for_test();

        let events = backend.drain_events();
        assert_eq!(
            events,
            vec![PlaybackEvent::PreparedTrackStarted(next.track)]
        );
    }
    #[test]
    fn stream_descriptor_redacts_sensitive_query_values() {
        let stream = StreamDescriptor::new(
            "https://music.example/Audio/track/stream?UserId=user&api_key=secret-token&DeviceId=device",
        );

        assert_eq!(
            stream.redacted_uri(),
            "https://music.example/Audio/track/stream?UserId=user&api_key=<redacted>&DeviceId=device"
        );
        assert!(!format!("{stream:?}").contains("secret-token"));
    }
    #[test]
    fn pending_seek_rejects_stale_positions_until_target_or_timeout() {
        let now = Instant::now();
        let pending = PendingSeek::interactive(42_000, PlaybackState::Playing, now);

        assert!(!pending.accepts_position(12_000, now));
        assert!(pending.accepts_position(43_000, now));
        assert!(pending.accepts_position(12_000, now + SEEK_SETTLE_WINDOW));
    }
    #[test]
    fn pending_seek_suppresses_transient_state_changes() {
        let now = Instant::now();
        let pending = PendingSeek::interactive(42_000, PlaybackState::Playing, now);

        assert!(pending.suppresses_state(PlaybackState::Paused, now));
        assert!(pending.suppresses_state(PlaybackState::Buffering, now));
        assert!(!pending.suppresses_state(PlaybackState::Playing, now));
        assert!(pending.suppresses_state(PlaybackState::Stopped, now));
        assert!(!pending.suppresses_state(PlaybackState::Paused, now + SEEK_SETTLE_WINDOW));
    }
    #[test]
    fn startup_seek_waits_for_async_done_before_resuming() {
        let now = Instant::now();
        let pending = PendingSeek::startup(42_000, PlaybackState::Buffering, now);

        assert!(pending.retry_on_async_done);
        assert!(pending.resume_after_seek);
        assert!(pending.suppresses_state(PlaybackState::Paused, now));
        assert!(pending.suppresses_state(PlaybackState::Stopped, now));
        assert!(!pending.suppresses_state(PlaybackState::Buffering, now));
        assert!(pending.suppresses_buffering(now));
        assert!(!pending.suppresses_state(PlaybackState::Paused, now + STARTUP_SEEK_SETTLE_WINDOW));
    }
    #[test]
    fn track_start_rejects_previous_position_and_stopped_state() {
        let mut engine = test_engine_with_pending_seek(42_000);
        engine.pending_seek = Some(PendingSeek::track_start(Instant::now()));
        let events = Arc::clone(&engine.events);

        engine.handle_state_changed(PlaybackState::Stopped);
        engine.push_position(78_000);
        assert!(events.lock().expect("events").is_empty());

        engine.handle_state_changed(PlaybackState::Playing);
        assert!(engine.pending_seek.is_some());
        engine.push_position(0);

        let events = events.lock().expect("events");
        assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Playing)));
        assert!(events.contains(&PlaybackEvent::PositionChanged {
            seconds: 0,
            millis: 0
        }));
        assert!(!events.contains(&PlaybackEvent::PositionChanged {
            seconds: 78,
            millis: 78_000
        }));
    }
    #[test]
    fn ordinary_stream_start_preserves_startup_seek() {
        let mut engine = test_engine_with_pending_seek(42_000);
        let events = Arc::clone(&engine.events);

        engine.handle_stream_started_track(None);
        assert!(engine.pending_seek.is_some());
        assert!(events.lock().expect("events").is_empty());

        engine
            .pending_seek
            .as_mut()
            .expect("pending seek")
            .retry_on_async_done = false;
        engine.handle_stream_started_track(Some(track(2)));
        assert!(engine.pending_seek.is_none());
        let events = events.lock().expect("events");
        assert!(events.contains(&PlaybackEvent::PositionChanged {
            seconds: 0,
            millis: 0
        }));
    }
    #[test]
    fn seek_during_crossfade_promotes_incoming_track_before_targeting_active_pipeline() {
        let mut engine = test_engine_with_pending_seek(0);
        engine.pending_seek = None;
        let outgoing = PreparedPlaybackItem::new(track(1), StreamDescriptor::new("fake://track/1"));
        let incoming = PreparedPlaybackItem::new(track(2), StreamDescriptor::new("fake://track/2"));

        {
            let mut shared = engine.shared.lock().expect("shared");
            shared.current = Some(outgoing);
            shared.gapless_pending = Some(incoming.clone());
            shared.active = Slot::Primary;
            shared.crossfade = Some(CrossfadeState {
                from: Slot::Primary,
                to: Slot::Secondary,
                started_at: Instant::now(),
                duration: Duration::from_secs(5),
                item: incoming.clone(),
            });
        }

        engine.finish_crossfade_for_seek();

        let shared = engine.shared.lock().expect("shared");
        assert_eq!(shared.active, Slot::Secondary);
        assert_eq!(shared.current, Some(incoming));
        assert!(shared.crossfade.is_none());
        assert!(shared.gapless_pending.is_none());
    }
    fn track(number: u32) -> PlaybackTrack {
        PlaybackTrack {
            id: TrackId::fake(number),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_seconds: 180,
        }
    }
    fn test_engine_with_pending_seek(target_millis: u64) -> GstEngine {
        let shared = Arc::new(Mutex::new(SharedPlaybackState::new()));
        let events = Arc::new(Mutex::new(VecDeque::new()));
        GstEngine {
            primary: test_pipeline(
                Slot::Primary,
                "rufin-test-player-primary",
                Arc::clone(&shared),
                Arc::clone(&events),
            ),
            secondary: test_pipeline(
                Slot::Secondary,
                "rufin-test-player-secondary",
                Arc::clone(&shared),
                Arc::clone(&events),
            ),
            shared,
            events,
            last_position_tick: Instant::now(),
            state: PlaybackState::Buffering,
            pending_seek: Some(PendingSeek::startup(
                target_millis,
                PlaybackState::Buffering,
                Instant::now(),
            )),
        }
    }
    fn test_pipeline(
        slot: Slot,
        name: &str,
        shared: Arc<Mutex<SharedPlaybackState>>,
        events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
    ) -> PlayerPipeline {
        gstreamer::init().expect("gst init");
        PlayerPipeline::new(slot, name, shared, events).expect("test pipeline")
    }
