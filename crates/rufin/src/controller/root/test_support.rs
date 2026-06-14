use super::*;

pub(in crate::controller) fn library_track(
    number: u32,
    artist_id: Option<ArtistId>,
    album_id: AlbumId,
    artist: &str,
    genres: &[&str],
) -> Track {
    Track {
        id: TrackId::fake(number),
        album_id,
        title: format!("Track {number}"),
        artist: artist.to_string(),
        artist_id,
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: "Album".to_string(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180,
        favorite: false,
        disc_number: 1,
        track_number: number as u16,
        image_ref: None,
        genres: genres.iter().map(|genre| genre.to_string()).collect(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
    }
}
pub(in crate::controller) fn wait_for_snapshot(
    events: &Receiver<ControllerEvent>,
) -> LibrarySnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Snapshot(snapshot)
        | ControllerEvent::HomeSectionsUpdated { snapshot, .. }
        | ControllerEvent::PlaylistChanged { snapshot, .. }
        | ControllerEvent::SmartPlaylistChanged { snapshot, .. } => Some(*snapshot),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_favorite_changed(
    events: &Receiver<ControllerEvent>,
) -> (FavoriteItemId, bool, LibrarySnapshot) {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::FavoriteChanged {
            item_id,
            favorite,
            snapshot,
        } => Some((item_id, favorite, *snapshot)),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playlist_changed(
    events: &Receiver<ControllerEvent>,
) -> (PlaylistId, LibrarySnapshot) {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::PlaylistChanged {
            playlist_id,
            snapshot,
        } => Some((playlist_id, *snapshot)),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_status(events: &Receiver<ControllerEvent>) -> String {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::LoginStatus(status) => Some(status),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_queue(
    events: &Receiver<ControllerEvent>,
) -> Option<domain::QueueSnapshot> {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Queue(queue) => Some(*queue),
        _ => None,
    })
}
fn wait_for_event<T>(
    events: &Receiver<ControllerEvent>,
    context: &str,
    mut select: impl FnMut(ControllerEvent) -> Option<T>,
) -> T {
    loop {
        let event = events.recv_timeout(Duration::from_secs(5)).expect(context);
        match event {
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            event => {
                if let Some(value) = select(event) {
                    return value;
                }
            }
        }
    }
}
pub(in crate::controller) fn random_request(
    action: RandomPlayAction,
    limit: usize,
) -> RandomPlayRequest {
    RandomPlayRequest {
        action,
        limit,
        min_year: None,
        max_year: None,
        genre_id: None,
        genre_name: None,
        played_filter: PlayedFilter::All,
    }
}
pub(in crate::controller) fn random_track_ids(tracks: &[Track], limit: usize) -> Vec<TrackId> {
    let mut ids = tracks
        .iter()
        .map(|track| track.id.clone())
        .collect::<Vec<_>>();
    ids.sort_by_key(|id| id.as_str().to_string());
    ids.truncate(limit);
    ids
}
pub(in crate::controller) fn wait_for_cover_ready(
    events: &Receiver<ControllerEvent>,
    expected_key: &str,
) -> PathBuf {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::CoverReady { key, path } if key == expected_key => Some(path),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_lyrics(
    events: &Receiver<ControllerEvent>,
) -> Option<source::Lyrics> {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Lyrics { lyrics, .. } => Some(*lyrics),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_recorded_command(
    commands: &Arc<Mutex<Vec<PlaybackCommand>>>,
    predicate: impl Fn(&PlaybackCommand) -> bool,
) -> PlaybackCommand {
    for _ in 0..50 {
        if let Some(command) = commands
            .lock()
            .expect("commands")
            .iter()
            .find(|command| predicate(command))
            .cloned()
        {
            return command;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for playback command");
}
pub(in crate::controller) fn wait_for_playback_state(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    state: PlaybackState,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(controller, events, "playback state", |event| match event {
        ControllerEvent::Playback(playback) if playback.state == state => Some(*playback),
        ControllerEvent::Error(error) => panic!("controller error: {error}"),
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_track_position(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    track_id: &TrackId,
    position_millis: u64,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(
        controller,
        events,
        "playback track position",
        |event| match event {
            ControllerEvent::Playback(playback)
                if playback.position_millis == position_millis
                    && playback
                        .current
                        .as_ref()
                        .is_some_and(|entry| &entry.track_id == track_id) =>
            {
                Some(*playback)
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => None,
        },
    )
}
pub(in crate::controller) fn wait_for_playback_auto_dj(
    events: &Receiver<ControllerEvent>,
    enabled: bool,
) -> super::PlaybackSnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Playback(playback) if playback.auto_dj_enabled == enabled => {
            Some(*playback)
        }
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_repeat(
    events: &Receiver<ControllerEvent>,
    repeat_mode: RepeatMode,
) -> super::PlaybackSnapshot {
    wait_for_event(events, "controller event", |event| match event {
        ControllerEvent::Playback(playback) if playback.repeat_mode == repeat_mode => {
            Some(*playback)
        }
        _ => None,
    })
}
pub(in crate::controller) fn wait_for_playback_current_favorite(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    favorite: bool,
) -> super::PlaybackSnapshot {
    wait_for_polled_event(
        controller,
        events,
        "playback favorite",
        |event| match event {
            ControllerEvent::Playback(playback)
                if playback
                    .current
                    .as_ref()
                    .is_some_and(|entry| entry.favorite == favorite) =>
            {
                Some(*playback)
            }
            ControllerEvent::Error(error) => panic!("controller error: {error}"),
            _ => None,
        },
    )
}
pub(in crate::controller) fn wait_for_polled_event<T>(
    controller: &AppController,
    events: &Receiver<ControllerEvent>,
    context: &str,
    mut select: impl FnMut(ControllerEvent) -> Option<T>,
) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {context}"
        );
        controller.poll_playback_events();
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                if let Some(value) = select(event) {
                    return value;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("controller event channel closed")
            }
        }
    }
}
pub(in crate::controller) fn assert_playlist_order(
    controller: &AppController,
    playlist_id: &PlaylistId,
    ids: &[&str],
) {
    let detail = controller
        .cached_playlist_detail(playlist_id)
        .expect("playlist detail")
        .expect("playlist detail");
    assert_eq!(
        detail
            .entries
            .iter()
            .map(|entry| entry.track.id.as_str())
            .collect::<Vec<_>>(),
        ids
    );
}
