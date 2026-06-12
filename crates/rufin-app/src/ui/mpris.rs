use std::fmt::Write as _;
use std::rc::Rc;

use gtk::glib;
use mpris_server::{
    LoopStatus, Metadata, PlaybackStatus, Player as MprisPlayer, Time, TrackId as MprisTrackId,
};
use rufin_core::{QueueEntry, RepeatMode};
use rufin_playback::PlaybackState;
use tracing::warn;

use crate::controller::PlaybackSnapshot;

use super::{Shell, THUMB_COVER_SIZE};

impl Shell {
    pub(super) fn update_mpris_player(&self) {
        let Some(player) = self.state.mpris_player.borrow().as_ref().cloned() else {
            return;
        };
        let snapshot = self.state.player.borrow().clone();
        let metadata = self.mpris_metadata_update(&snapshot);
        let playback_status = match snapshot.state {
            PlaybackState::Playing | PlaybackState::Buffering => PlaybackStatus::Playing,
            PlaybackState::Paused => PlaybackStatus::Paused,
            PlaybackState::Stopped => PlaybackStatus::Stopped,
        };
        let loop_status = match snapshot.repeat_mode {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::One => LoopStatus::Track,
            RepeatMode::All => LoopStatus::Playlist,
        };
        let has_current = snapshot.current.is_some();
        let position = Time::from_millis(snapshot.position_millis.min(i64::MAX as u64) as i64);
        let volume = snapshot.volume.clamp(0.0, 1.0);

        glib::spawn_future_local(async move {
            let _updated = player.set_playback_status(playback_status).await;
            let _updated = player.set_loop_status(loop_status).await;
            let _updated = player.set_shuffle(snapshot.shuffle_enabled).await;
            if let Some(metadata) = metadata {
                let _updated = player.set_metadata(metadata).await;
            }
            let _updated = player.set_volume(volume).await;
            let _updated = player.set_can_play(has_current).await;
            let _updated = player.set_can_pause(has_current).await;
            let _updated = player.set_can_seek(has_current).await;
            let _updated = player.set_can_go_next(has_current).await;
            let _updated = player.set_can_go_previous(has_current).await;
            player.set_position(position);
        });
    }

    fn mpris_metadata_update(&self, snapshot: &PlaybackSnapshot) -> Option<Metadata> {
        let key = mpris_metadata_key(snapshot);
        if self.state.mpris_metadata_key.borrow().as_deref() == Some(key.as_str()) {
            return None;
        }
        *self.state.mpris_metadata_key.borrow_mut() = Some(key);
        let art_url = snapshot
            .current
            .as_ref()
            .and_then(|entry| self.current_art_url(entry));
        Some(self.mpris_metadata(snapshot, art_url))
    }

    fn mpris_metadata(&self, snapshot: &PlaybackSnapshot, art_url: Option<String>) -> Metadata {
        let Some(entry) = snapshot.current.as_ref() else {
            return Metadata::builder().trackid(MprisTrackId::NO_TRACK).build();
        };
        let mut builder = Metadata::builder()
            .trackid(mpris_track_id(entry.track_id.as_str()))
            .title(entry.title.clone())
            .artist([entry.artist.clone()])
            .album(entry.album.clone())
            .length(Time::from_secs(i64::from(entry.duration_seconds)));
        if let Some(art_url) = art_url {
            builder = builder.art_url(art_url);
        }
        builder.build()
    }

    fn current_art_url(&self, entry: &QueueEntry) -> Option<String> {
        let artwork = self.current_playback_artwork_path(entry, THUMB_COVER_SIZE)?;
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
        let play_shell = Rc::clone(&shell);
        player.connect_play(move |_| {
            let state = play_shell.state.player.borrow().state;
            if !matches!(state, PlaybackState::Playing | PlaybackState::Buffering) {
                play_shell.controller.play_pause();
            }
        });
        let pause_shell = Rc::clone(&shell);
        player.connect_pause(move |_| {
            let state = pause_shell.state.player.borrow().state;
            if matches!(state, PlaybackState::Playing | PlaybackState::Buffering) {
                pause_shell.controller.play_pause();
            }
        });
        let controller = shell.controller.clone();
        player.connect_stop(move |_| controller.stop());
        let controller = shell.controller.clone();
        player.connect_next(move |_| controller.next_track());
        let controller = shell.controller.clone();
        player.connect_previous(move |_| controller.previous_track());
        let controller = shell.controller.clone();
        let seek_shell = Rc::clone(&shell);
        player.connect_seek(move |_, offset| {
            let current = seek_shell.state.player.borrow().position_millis;
            let offset_millis = offset.as_micros() / 1_000;
            let target = if offset_millis.is_negative() {
                current.saturating_sub(offset_millis.unsigned_abs())
            } else {
                current.saturating_add(offset_millis as u64)
            };
            controller.seek_millis(target);
        });
        let controller = shell.controller.clone();
        player.connect_set_position(move |_, _, position| {
            controller.seek_millis((position.as_micros() / 1_000).max(0) as u64);
        });

        let run_player = Rc::clone(&player);
        glib::spawn_future_local(async move {
            run_player.run().await;
        });
        *shell.state.mpris_player.borrow_mut() = Some(player);
        shell.update_mpris_player();
    });
}

fn mpris_track_id(track_id: &str) -> MprisTrackId {
    let mut encoded = String::with_capacity(track_id.len() * 2);
    for byte in track_id.as_bytes() {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    MprisTrackId::try_from(format!("/io/github/screwys/Rufin/track/{encoded}"))
        .unwrap_or(MprisTrackId::NO_TRACK)
}

fn mpris_metadata_key(snapshot: &PlaybackSnapshot) -> String {
    let Some(entry) = snapshot.current.as_ref() else {
        return "none".to_string();
    };
    let image = entry
        .image_ref
        .as_ref()
        .map(|image| format!("{}:{}", image.item_id, image.tag.as_deref().unwrap_or("")))
        .unwrap_or_default();
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        entry.track_id.as_str(),
        entry.title,
        entry.artist,
        entry.album,
        entry.duration_seconds,
        image,
    )
}
