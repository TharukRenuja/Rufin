use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;

use gtk::glib;
use library::SourceId;
use mpris_server::{
    LoopStatus, Metadata, PlaybackStatus, Player as MprisPlayer, Time, TrackId as MprisTrackId,
};
use playback::{
    PlaybackView, PositionDiscontinuity, RepeatMode, RunId, SequenceEntry, TransportStatus,
};
use tracing::warn;

use super::{Shell, THUMB_COVER_SIZE};

#[derive(Clone, Debug, PartialEq)]
struct MprisDesiredState {
    run: Option<RunId>,
    playback_status: PlaybackStatus,
    loop_status: LoopStatus,
    shuffle: bool,
    metadata: Metadata,
    volume: f64,
    can_play: bool,
    can_pause: bool,
    can_seek: bool,
    can_go_next: bool,
    can_go_previous: bool,
    position: Option<Time>,
}

pub(super) struct MprisAdapter {
    player: RefCell<Option<Rc<MprisPlayer>>>,
    desired: RefCell<Option<MprisDesiredState>>,
    applied: RefCell<Option<MprisDesiredState>>,
    pending_seeked: Cell<Option<PositionDiscontinuity>>,
    generation: Cell<u64>,
    running: Cell<bool>,
}

impl MprisAdapter {
    pub(super) fn new() -> Self {
        Self {
            player: RefCell::new(None),
            desired: RefCell::new(None),
            applied: RefCell::new(None),
            pending_seeked: Cell::new(None),
            generation: Cell::new(0),
            running: Cell::new(false),
        }
    }

    fn install_player(self: &Rc<Self>, player: Rc<MprisPlayer>) {
        *self.player.borrow_mut() = Some(player);
        self.applied.borrow_mut().take();
        self.pending_seeked.set(None);
        self.start();
    }

    fn queue(
        self: &Rc<Self>,
        desired: MprisDesiredState,
        discontinuity: Option<PositionDiscontinuity>,
    ) {
        if self
            .pending_seeked
            .get()
            .is_some_and(|pending| Some(pending.run) != desired.run)
        {
            self.pending_seeked.set(None);
        }
        if let Some(discontinuity) = discontinuity
            && desired.run == Some(discontinuity.run)
        {
            self.pending_seeked.set(Some(discontinuity));
        }
        *self.desired.borrow_mut() = Some(desired);
        self.generation.set(self.generation.get().saturating_add(1));
        self.start();
    }

    fn start(self: &Rc<Self>) {
        if self.player.borrow().is_none() || self.running.replace(true) {
            return;
        }
        let adapter = Rc::clone(self);
        glib::spawn_future_local(async move {
            adapter.drain().await;
            adapter.running.set(false);
        });
    }

    async fn drain(&self) {
        loop {
            let Some(player) = self.player.borrow().as_ref().cloned() else {
                return;
            };
            let Some(desired) = self.desired.borrow().clone() else {
                return;
            };
            let applied = self.applied.borrow().clone();
            let generation = self.generation.get();
            let discontinuity = self.pending_seeked.take();

            apply_mpris_desired(&player, applied.as_ref(), &desired).await;
            let mut applied = desired.clone();
            if let Some(discontinuity) = discontinuity
                && self
                    .desired
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| current.run == Some(discontinuity.run))
            {
                let position = mpris_time(discontinuity.position_millis);
                player.set_position(position);
                let _sent = player.seeked(position).await;
                applied.position = Some(position);
            }
            *self.applied.borrow_mut() = Some(applied);

            if self.generation.get() == generation && self.pending_seeked.get().is_none() {
                return;
            }
        }
    }
}

async fn apply_mpris_desired(
    player: &MprisPlayer,
    applied: Option<&MprisDesiredState>,
    desired: &MprisDesiredState,
) {
    if applied.is_none_or(|applied| applied.playback_status != desired.playback_status) {
        let _updated = player.set_playback_status(desired.playback_status).await;
    }
    if applied.is_none_or(|applied| applied.loop_status != desired.loop_status) {
        let _updated = player.set_loop_status(desired.loop_status).await;
    }
    if applied.is_none_or(|applied| applied.shuffle != desired.shuffle) {
        let _updated = player.set_shuffle(desired.shuffle).await;
    }
    if applied.is_none_or(|applied| applied.metadata != desired.metadata) {
        let _updated = player.set_metadata(desired.metadata.clone()).await;
    }
    if applied.is_none_or(|applied| applied.volume != desired.volume) {
        let _updated = player.set_volume(desired.volume).await;
    }
    if applied.is_none_or(|applied| applied.can_play != desired.can_play) {
        let _updated = player.set_can_play(desired.can_play).await;
    }
    if applied.is_none_or(|applied| applied.can_pause != desired.can_pause) {
        let _updated = player.set_can_pause(desired.can_pause).await;
    }
    if applied.is_none_or(|applied| applied.can_seek != desired.can_seek) {
        let _updated = player.set_can_seek(desired.can_seek).await;
    }
    if applied.is_none_or(|applied| applied.can_go_next != desired.can_go_next) {
        let _updated = player.set_can_go_next(desired.can_go_next).await;
    }
    if applied.is_none_or(|applied| applied.can_go_previous != desired.can_go_previous) {
        let _updated = player.set_can_go_previous(desired.can_go_previous).await;
    }
    if applied.is_none_or(|applied| applied.position != desired.position)
        && let Some(position) = desired.position
    {
        player.set_position(position);
    }
}

impl Shell {
    pub(super) fn update_mpris_player(&self) {
        self.update_mpris_player_after(None);
    }

    pub(super) fn update_mpris_player_after(&self, discontinuity: Option<PositionDiscontinuity>) {
        let playback = self.state.player.borrow();
        let art_url = playback.as_ref().and_then(|playback| {
            playback
                .transport
                .current
                .as_deref()
                .and_then(|entry| self.current_art_url(&playback.transport.source_id, entry))
        });
        let desired = mpris_desired_state(playback.as_ref(), art_url);
        self.state.mpris.queue(desired, discontinuity);
    }

    fn current_art_url(&self, source_id: &SourceId, entry: &SequenceEntry) -> Option<String> {
        let artwork =
            self.current_playback_cached_artwork_path(source_id, entry, THUMB_COVER_SIZE)?;
        glib::filename_to_uri(artwork.path, None)
            .ok()
            .map(|uri| uri.to_string())
    }
}

pub(super) fn install_mpris(shell: &Rc<Shell>) {
    let shell = Rc::clone(shell);
    glib::spawn_future_local(async move {
        let player = match MprisPlayer::builder("io.github.screwys.Rufin")
            .identity("Rufin")
            .desktop_entry("io.github.screwys.Rufin")
            .supported_uri_schemes(["http", "https", "file"])
            .supported_mime_types(["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"])
            .can_play(true)
            .can_pause(true)
            .can_go_next(true)
            .can_go_previous(true)
            .can_seek(true)
            .can_control(true)
            .build()
            .await
        {
            Ok(player) => Rc::new(player),
            Err(error) => {
                warn!(%error, "failed to start MPRIS server");
                return;
            }
        };

        let controller = shell.controller.clone();
        player.connect_play_pause(move |_| controller.play_pause());
        let controller = shell.controller.clone();
        player.connect_play(move |_| controller.play());
        let controller = shell.controller.clone();
        player.connect_pause(move |_| controller.pause());
        let controller = shell.controller.clone();
        player.connect_stop(move |_| controller.stop());
        let controller = shell.controller.clone();
        player.connect_next(move |_| controller.next_track());
        let controller = shell.controller.clone();
        player.connect_previous(move |_| controller.previous_track());
        let controller = shell.controller.clone();
        let seek_shell = Rc::clone(&shell);
        player.connect_seek(move |_, offset| {
            let current = seek_shell
                .state
                .player
                .borrow()
                .as_ref()
                .map_or(0, |playback| playback.transport.position_millis);
            let offset_millis = offset.as_micros() / 1_000;
            let target = if offset_millis.is_negative() {
                current.saturating_sub(offset_millis.unsigned_abs())
            } else {
                current.saturating_add(offset_millis as u64)
            };
            controller.seek_millis(target);
        });
        let position_shell = Rc::clone(&shell);
        player.connect_set_position(move |_, track_id, position| {
            let current_matches =
                mpris_set_position_matches(position_shell.state.player.borrow().as_ref(), track_id);
            if current_matches {
                position_shell
                    .controller
                    .seek_millis((position.as_micros() / 1_000).max(0) as u64);
            }
        });
        let volume_shell = Rc::clone(&shell);
        player.connect_set_volume(move |_, volume| volume_shell.apply_user_volume(volume));
        let controller = shell.controller.clone();
        player.connect_set_shuffle(move |_, enabled| controller.set_shuffle(enabled));
        let controller = shell.controller.clone();
        player.connect_set_loop_status(move |_, status| {
            controller.set_repeat(repeat_mode_from_mpris(status));
        });

        shell.state.mpris.install_player(Rc::clone(&player));
        shell.update_mpris_player();
        glib::spawn_future_local(async move {
            player.run().await;
        });
    });
}

fn mpris_desired_state(
    playback: Option<&PlaybackView>,
    art_url: Option<String>,
) -> MprisDesiredState {
    let has_current = playback.is_some_and(|playback| playback.transport.current.is_some());
    let has_active_run = playback.is_some_and(|playback| {
        playback.transport.run.is_some()
            && !matches!(
                playback.transport.state,
                TransportStatus::Stopped | TransportStatus::Failed
            )
    });
    let can_go_next = playback.is_some_and(|playback| {
        has_current
            && (playback.queue.next_occurrence.is_some() || playback.controls.auto_dj_enabled)
    });
    MprisDesiredState {
        run: playback.and_then(|playback| playback.transport.run),
        playback_status: playback.map_or(PlaybackStatus::Stopped, |playback| {
            mpris_playback_status(playback.transport.state)
        }),
        loop_status: mpris_loop_status(
            playback.map_or(RepeatMode::Off, |playback| playback.controls.repeat_mode),
        ),
        shuffle: playback.is_some_and(|playback| playback.controls.shuffle_enabled),
        metadata: mpris_metadata(playback, art_url),
        volume: playback.map_or(1.0, |playback| playback.controls.volume.clamp(0.0, 1.0)),
        can_play: has_current,
        can_pause: has_active_run,
        can_seek: has_current,
        can_go_next,
        can_go_previous: has_current,
        position: playback
            .filter(|playback| playback.transport.current.is_some())
            .map(|playback| mpris_time(playback.transport.position_millis)),
    }
}

fn mpris_metadata(playback: Option<&PlaybackView>, art_url: Option<String>) -> Metadata {
    let Some(entry) = playback.and_then(|playback| playback.transport.current.as_ref()) else {
        return Metadata::builder().trackid(MprisTrackId::NO_TRACK).build();
    };
    let mut builder = Metadata::builder()
        .trackid(mpris_track_id(entry.occurrence.as_str()))
        .title(entry.track.title.clone())
        .artist([entry.track.artist.clone()])
        .album(entry.track.album.clone())
        .length(Time::from_secs(i64::from(entry.track.duration_seconds)));
    if let Some(art_url) = art_url {
        builder = builder.art_url(art_url);
    }
    builder.build()
}

fn mpris_track_id(occurrence: &str) -> MprisTrackId {
    let mut encoded = String::with_capacity(occurrence.len() * 2);
    for byte in occurrence.as_bytes() {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    MprisTrackId::try_from(format!("/io/github/screwys/Rufin/track/{encoded}"))
        .unwrap_or(MprisTrackId::NO_TRACK)
}

fn mpris_set_position_matches(playback: Option<&PlaybackView>, track_id: &MprisTrackId) -> bool {
    playback
        .and_then(|playback| playback.transport.current.as_ref())
        .is_some_and(|entry| &mpris_track_id(entry.occurrence.as_str()) == track_id)
}

fn mpris_time(position_millis: u64) -> Time {
    Time::from_millis(position_millis.min(i64::MAX as u64) as i64)
}

fn mpris_loop_status(repeat_mode: RepeatMode) -> LoopStatus {
    match repeat_mode {
        RepeatMode::Off => LoopStatus::None,
        RepeatMode::One => LoopStatus::Track,
        RepeatMode::All => LoopStatus::Playlist,
    }
}

fn repeat_mode_from_mpris(status: LoopStatus) -> RepeatMode {
    match status {
        LoopStatus::None => RepeatMode::Off,
        LoopStatus::Track => RepeatMode::One,
        LoopStatus::Playlist => RepeatMode::All,
    }
}

fn mpris_playback_status(state: TransportStatus) -> PlaybackStatus {
    match state {
        TransportStatus::Playing => PlaybackStatus::Playing,
        TransportStatus::Resolving | TransportStatus::Buffering | TransportStatus::Paused => {
            PlaybackStatus::Paused
        }
        TransportStatus::Stopped | TransportStatus::Failed => PlaybackStatus::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::super::shell_tests::{test_image_ref, test_playback_view, test_sequence_entry};
    use super::*;
    use library::{SourceId, TrackId};
    use playback::OccurrenceId;

    #[test]
    fn mpris_occurrence_paths_distinguish_duplicate_tracks_and_guard_set_position() {
        let mut first = sequence_entry();
        let mut second = first.clone();
        first.occurrence = OccurrenceId::new("occurrence:first");
        second.occurrence = OccurrenceId::new("occurrence:second");
        let first_id = mpris_track_id(first.occurrence.as_str());
        let second_id = mpris_track_id(second.occurrence.as_str());
        assert_ne!(first_id, second_id);

        let playback = test_playback_view(
            Some(first),
            SourceId::fake(1),
            TransportStatus::Playing,
            1_000,
        );
        assert!(mpris_set_position_matches(Some(&playback), &first_id));
        assert!(!mpris_set_position_matches(Some(&playback), &second_id));
    }

    #[test]
    fn mpris_exact_repeat_mapping_round_trips() {
        for repeat in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
            assert_eq!(repeat_mode_from_mpris(mpris_loop_status(repeat)), repeat);
        }
    }

    #[test]
    fn mpris_capabilities_follow_queue_summary() {
        let mut playback = test_playback_view(
            Some(sequence_entry()),
            SourceId::fake(1),
            TransportStatus::Playing,
            0,
        );
        let exhausted = mpris_desired_state(Some(&playback), None);
        assert!(!exhausted.can_go_next);
        assert!(exhausted.can_go_previous);

        playback.queue.next_occurrence = Some(OccurrenceId::new("occurrence:next"));
        let with_next = mpris_desired_state(Some(&playback), None);
        assert!(with_next.can_go_next);
    }

    #[test]
    fn mpris_metadata_updates_when_cached_art_arrives() {
        let playback = test_playback_view(
            Some(sequence_entry()),
            SourceId::fake(1),
            TransportStatus::Paused,
            0,
        );
        assert_ne!(
            mpris_metadata(Some(&playback), None),
            mpris_metadata(Some(&playback), Some("file:///tmp/cover.png".to_string()))
        );
    }

    fn sequence_entry() -> SequenceEntry {
        let mut entry = test_sequence_entry("Track", test_image_ref("mpris"));
        entry.track.id = TrackId::new("track-one");
        entry.track.image_ref = None;
        entry
    }
}
