use super::{
    AboutToFinishAction, CrossfadeState, FakePlaybackBackend, GstEngine, PendingSeek,
    PlaybackBackend, PlaybackCommand, PlaybackEvent, PlaybackState, PlaybackTrack, PlayerPipeline,
    PreparedPlaybackItem, SEEK_SETTLE_WINDOW, STARTUP_SEEK_SETTLE_WINDOW, SharedPlaybackState,
    Slot, StreamDescriptor, about_to_finish_action, cancel_crossfade_next, cancel_gapless_pending,
    same_album_crossfade_is_skipped,
};
use rufin_core::{AlbumId, PlaybackSettings, PlaybackTransitionMode, TrackId};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[test]
fn playback_report_transitions() {
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
        track_id: Some(TrackId::fake(1)),
        seconds: 42,
        millis: 42_000,
    }));
    assert!(events.contains(&PlaybackEvent::PositionChanged {
        track_id: Some(TrackId::fake(1)),
        seconds: 42,
        millis: 42_500,
    }));
    assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Stopped)));
}
#[test]
fn playback_track_next() {
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
    backend.emit_prepared_track();

    let events = backend.drain_events();
    assert!(events.contains(&PlaybackEvent::PreparedTrackStarted(next.track)));
}
#[test]
fn playback_update_next() {
    let mut backend = FakePlaybackBackend::new();
    let next = PreparedPlaybackItem::new(track(3), StreamDescriptor::new("fake://track/3"));

    backend
        .send(PlaybackCommand::PrepareNext(Some(next.clone())))
        .expect("prepare next");
    backend.emit_prepared_track();

    let events = backend.drain_events();
    assert_eq!(
        events,
        vec![PlaybackEvent::PreparedTrackStarted(next.track)]
    );
}
#[test]
fn playback_redact_query() {
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
fn default_about_finish_waits_for_eos() {
    let next =
        PreparedPlaybackItem::new(track(2), StreamDescriptor::new("file:///music/next.flac"));
    let mut shared = SharedPlaybackState::new();
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Ignore);
    assert_eq!(shared.next, Some(next));
    assert!(shared.gapless_pending.is_none());
    assert!(!shared.about_to_finish_pending);
}
#[test]
fn gapless_about_finish() {
    let next =
        PreparedPlaybackItem::new(track(2), StreamDescriptor::new("file:///music/next.flac"));
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Gapless;
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Preload(Box::new(next.clone())));
    assert!(shared.next.is_none());
    assert_eq!(shared.gapless_pending, Some(next));
}
#[test]
fn gapless_about_finish_remote() {
    let next = PreparedPlaybackItem::new(
        track(2),
        StreamDescriptor::new("https://music.example/Audio/track/stream?api_key=secret-token"),
    );
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Gapless;
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Preload(Box::new(next.clone())));
    assert!(shared.next.is_none());
    assert_eq!(shared.gapless_pending, Some(next));
}
#[test]
fn playback_finish_eos_unsupported() {
    let next = PreparedPlaybackItem::new(track(2), StreamDescriptor::new("fake://track/2"));
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Gapless;
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Ignore);
    assert_eq!(shared.next, Some(next));
    assert!(shared.gapless_pending.is_none());
}
#[test]
fn playback_wait_eos() {
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Gapless;

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Ignore);
    assert!(shared.next.is_none());
    assert!(shared.gapless_pending.is_none());
    assert!(shared.about_to_finish_pending);
}
#[test]
fn playback_finish_eos_unsupported_clears_late_window() {
    let next = PreparedPlaybackItem::new(track(2), StreamDescriptor::new("fake://track/2"));
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Gapless;
    shared.about_to_finish_pending = true;
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Ignore);
    assert_eq!(shared.next, Some(next));
    assert!(!shared.about_to_finish_pending);
}
#[test]
fn gapless_cancel_restores_next() {
    let current = PreparedPlaybackItem::new(
        track(1),
        StreamDescriptor::new("https://music.example/current"),
    );
    let pending = PreparedPlaybackItem::new(
        track(2),
        StreamDescriptor::new("https://music.example/next"),
    );
    let mut shared = SharedPlaybackState::new();
    shared.current = Some(current.clone());
    shared.gapless_pending = Some(pending.clone());
    shared.about_to_finish_pending = true;

    let cancelled = cancel_gapless_pending(&mut shared);

    assert_eq!(cancelled, Some((current, pending.clone())));
    assert_eq!(shared.next, Some(pending));
    assert!(shared.gapless_pending.is_none());
    assert!(!shared.about_to_finish_pending);
}
#[test]
fn crossfade_cancel_restores_next() {
    let current = PreparedPlaybackItem::new(
        track(1),
        StreamDescriptor::new("https://music.example/current"),
    );
    let incoming = PreparedPlaybackItem::new(
        track(2),
        StreamDescriptor::new("https://music.example/next"),
    );
    let mut shared = SharedPlaybackState::new();
    shared.current = Some(current);
    shared.active = Slot::Primary;
    shared.crossfade = Some(CrossfadeState {
        from: Slot::Primary,
        to: Slot::Secondary,
        started_at: Instant::now(),
        duration: Duration::from_secs(5),
        item: incoming.clone(),
    });
    shared.gapless_pending = Some(incoming.clone());
    shared.about_to_finish_pending = true;

    let cancelled = cancel_crossfade_next(&mut shared, Slot::Secondary);

    assert!(cancelled.is_some());
    assert_eq!(shared.next, Some(incoming));
    assert_eq!(shared.active, Slot::Primary);
    assert!(shared.crossfade.is_none());
    assert!(shared.gapless_pending.is_none());
    assert!(!shared.about_to_finish_pending);
}
#[test]
fn album_crossfade_about() {
    let current = PreparedPlaybackItem::new(
        track_on_album(1, 7),
        StreamDescriptor::new("file:///music/one.flac"),
    );
    let next = PreparedPlaybackItem::new(
        track_on_album(2, 7),
        StreamDescriptor::new("file:///music/two.flac"),
    );
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Crossfade;
    shared.settings.skip_same_album_crossfade = true;
    shared.current = Some(current);
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Preload(Box::new(next.clone())));
    assert!(shared.next.is_none());
    assert_eq!(shared.gapless_pending, Some(next));
}
#[test]
fn playback_ignore_next() {
    let current = PreparedPlaybackItem::new(
        track_on_album(1, 7),
        StreamDescriptor::new("file:///music/one.flac"),
    );
    let next = PreparedPlaybackItem::new(
        track_on_album(2, 8),
        StreamDescriptor::new("file:///music/two.flac"),
    );
    let mut shared = SharedPlaybackState::new();
    shared.settings.transition_mode = PlaybackTransitionMode::Crossfade;
    shared.settings.skip_same_album_crossfade = true;
    shared.current = Some(current);
    shared.next = Some(next.clone());

    let action = about_to_finish_action(&mut shared);

    assert_eq!(action, AboutToFinishAction::Ignore);
    assert_eq!(shared.next, Some(next));
    assert!(shared.gapless_pending.is_none());
}
#[test]
fn playback_match_text() {
    let current = PreparedPlaybackItem::new(
        PlaybackTrack {
            album: "Album".to_string(),
            ..track_on_album(1, 7)
        },
        StreamDescriptor::new("file:///music/one.flac"),
    );
    let next = PreparedPlaybackItem::new(
        PlaybackTrack {
            album: "Different title".to_string(),
            ..track_on_album(2, 7)
        },
        StreamDescriptor::new("file:///music/two.flac"),
    );
    let settings = PlaybackSettings {
        transition_mode: PlaybackTransitionMode::Crossfade,
        skip_same_album_crossfade: true,
        ..PlaybackSettings::default()
    };

    assert!(same_album_crossfade_is_skipped(
        &settings,
        Some(&current),
        &next
    ));
}
#[test]
fn playback_reject_timeout() {
    let now = Instant::now();
    let pending = PendingSeek::interactive(42_000, PlaybackState::Playing, now);

    assert!(!pending.accepts_position(12_000, now));
    assert!(pending.accepts_position(43_000, now));
    assert!(pending.accepts_position(12_000, now + SEEK_SETTLE_WINDOW));
}
#[test]
fn playback_change_state() {
    let now = Instant::now();
    let pending = PendingSeek::interactive(42_000, PlaybackState::Playing, now);

    assert!(pending.suppresses_state(PlaybackState::Paused, now));
    assert!(pending.suppresses_state(PlaybackState::Buffering, now));
    assert!(!pending.suppresses_state(PlaybackState::Playing, now));
    assert!(pending.suppresses_state(PlaybackState::Stopped, now));
    assert!(!pending.suppresses_state(PlaybackState::Paused, now + SEEK_SETTLE_WINDOW));
}
#[test]
fn playback_wait_resuming() {
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
fn playback_reject_state() {
    let mut engine = test_engine_with_pending_seek(42_000);
    engine.pending_seek = Some(PendingSeek::track_start(Instant::now()));
    let events = Arc::clone(&engine.events);

    engine.handle_state_changed(PlaybackState::Stopped);
    engine.push_position(78_000);
    assert!(events.lock().expect("events").is_empty());

    engine.handle_state_changed(PlaybackState::Playing);
    assert!(engine.pending_seek.is_none());
    engine.push_position(0);

    let events = events.lock().expect("events");
    assert!(events.contains(&PlaybackEvent::StateChanged(PlaybackState::Playing)));
    assert!(events.contains(&PlaybackEvent::PositionChanged {
        track_id: None,
        seconds: 0,
        millis: 0
    }));
    assert!(!events.contains(&PlaybackEvent::PositionChanged {
        track_id: None,
        seconds: 78,
        millis: 78_000
    }));
}
#[test]
fn playback_source_window_reports_relative_position_and_ends() {
    let mut engine = test_engine_with_pending_seek(0);
    engine.pending_seek = None;
    {
        let mut shared = engine.shared.lock().expect("shared");
        shared.current = Some(PreparedPlaybackItem::new(
            track(1),
            StreamDescriptor::new("fake://track/1").with_source_window(10_000, 20_000),
        ));
    }

    engine.push_position(12_500);
    engine.push_position(20_000);

    let events = engine.events.lock().expect("events");
    assert!(events.contains(&PlaybackEvent::PositionChanged {
        track_id: Some(TrackId::new("track-1")),
        seconds: 2,
        millis: 2_500
    }));
    assert!(events.contains(&PlaybackEvent::EndOfStream));
}
#[test]
fn playback_start_stream() {
    let mut engine = test_engine_with_pending_seek(0);
    engine.pending_seek = None;
    let current = PreparedPlaybackItem::new(track(1), StreamDescriptor::new("fake://track/1"));
    let next = PreparedPlaybackItem::new(track(2), StreamDescriptor::new("fake://track/2"));
    {
        let mut shared = engine.shared.lock().expect("shared");
        shared.current = Some(current.clone());
        shared.gapless_pending = Some(next.clone());
    }

    engine.push_position(42_000);
    engine.push_duration(current.track.duration_seconds);
    {
        let mut events = engine.events.lock().expect("events");
        assert!(events.contains(&PlaybackEvent::PositionChanged {
            track_id: Some(current.track.id.clone()),
            seconds: 42,
            millis: 42_000,
        }));
        assert!(events.contains(&PlaybackEvent::DurationChanged {
            track_id: Some(current.track.id.clone()),
            seconds: current.track.duration_seconds,
        }));
        events.clear();
    }

    {
        let mut shared = engine.shared.lock().expect("shared");
        shared.current = Some(next.clone());
        shared.gapless_pending = None;
    }
    engine.handle_stream_started_track(Some(next.track.clone()));

    let events: Vec<_> = engine
        .events
        .lock()
        .expect("events")
        .iter()
        .cloned()
        .collect();
    assert_eq!(
        events.first(),
        Some(&PlaybackEvent::PreparedTrackStarted(next.track.clone()))
    );
    assert!(events.contains(&PlaybackEvent::PositionChanged {
        track_id: Some(next.track.id.clone()),
        seconds: 0,
        millis: 0,
    }));
    assert!(events.contains(&PlaybackEvent::DurationChanged {
        track_id: Some(next.track.id.clone()),
        seconds: next.track.duration_seconds,
    }));
}
#[test]
fn playback_preserve_seek() {
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
    let events: Vec<_> = events.lock().expect("events").iter().cloned().collect();
    assert_eq!(
        events.first(),
        Some(&PlaybackEvent::PreparedTrackStarted(track(2)))
    );
    assert!(events.contains(&PlaybackEvent::PositionChanged {
        track_id: Some(TrackId::fake(2)),
        seconds: 0,
        millis: 0
    }));
}
#[test]
fn playback_track_pipeline() {
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
    track_on_album(number, 1)
}
fn track_on_album(number: u32, album_number: u32) -> PlaybackTrack {
    PlaybackTrack {
        id: TrackId::fake(number),
        album_id: Some(AlbumId::fake(album_number)),
        title: format!("Track {number}"),
        artist: "Artist".to_string(),
        album: format!("Album {album_number}"),
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
        play_command_started_at: None,
    }
}
fn test_pipeline(
    slot: Slot,
    name: &str,
    shared: Arc<Mutex<SharedPlaybackState>>,
    _events: Arc<Mutex<VecDeque<PlaybackEvent>>>,
) -> PlayerPipeline {
    gstreamer::init().expect("gst init");
    PlayerPipeline::new(slot, name, shared).expect("test pipeline")
}
