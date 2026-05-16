use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use rufin_core::{Route, SearchKind, format_duration};
use rufin_playback::PlaybackState;

use super::{
    ArtworkTile, BOTTOM_PLAYER_BUTTON_OFFSET_Y, BOTTOM_PLAYER_BUTTON_ROW_HEIGHT,
    BOTTOM_PLAYER_BUTTON_STEP, BOTTOM_PLAYER_COVER_SIZE, BOTTOM_PLAYER_IDENTITY_WIDTH,
    BOTTOM_PLAYER_PLAY_BUTTON_SIZE, BOTTOM_PLAYER_PROGRESS_WIDTH, BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    BOTTOM_PLAYER_TRANSPORT_ICON_SIZE, BOTTOM_PLAYER_TRANSPORT_MARGIN_TOP,
    BOTTOM_PLAYER_TRANSPORT_OFFSET, BOTTOM_PLAYER_TRANSPORT_WIDTH, CoverBinding, Shell,
    THUMB_COVER_SIZE, add_label_click, favorite_icon_button, icon_button, icon_button_with_image,
    playback_state_label, player_link, queue_sidebar_button, repeat_icon_button, repeat_label,
    seekbar_target_seconds, set_active_class, set_favorite_button_active, set_repeat_button_icon,
    skip_icon_button, stop_icon_button,
};

pub(super) struct PlayerControls {
    pub(super) root: gtk::Overlay,
    pub(super) cover: ArtworkTile,
    pub(super) cover_key: RefCell<Option<String>>,
    pub(super) title: gtk::Label,
    pub(super) artist: gtk::Label,
    pub(super) album: gtk::Label,
    pub(super) stop_button: gtk::Button,
    pub(super) previous_button: gtk::Button,
    pub(super) play_button: gtk::Button,
    pub(super) play_icon: gtk::Image,
    pub(super) next_button: gtk::Button,
    pub(super) shuffle_button: gtk::Button,
    pub(super) repeat_button: gtk::Button,
    pub(super) dj_button: gtk::Button,
    pub(super) queue_button: gtk::Button,
    pub(super) queue_icon: gtk::DrawingArea,
    pub(super) queue_icon_open: Rc<Cell<bool>>,
    pub(super) favorite_button: gtk::Button,
    pub(super) elapsed: gtk::Label,
    pub(super) progress: gtk::Scale,
    pub(super) duration: gtk::Label,
    pub(super) mute_button: gtk::Button,
    pub(super) mute_icon: gtk::Image,
    pub(super) volume: gtk::Scale,
}

struct NowPlayingControls {
    root: gtk::Box,
    cover: ArtworkTile,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
}

struct TransportControls {
    root: gtk::Box,
    stop_button: gtk::Button,
    previous_button: gtk::Button,
    play_button: gtk::Button,
    play_icon: gtk::Image,
    next_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    dj_button: gtk::Button,
    elapsed: gtk::Label,
    progress: gtk::Scale,
    duration: gtk::Label,
}

struct PlayerActionControls {
    root: gtk::Box,
    queue_button: gtk::Button,
    queue_icon: gtk::DrawingArea,
    queue_icon_open: Rc<Cell<bool>>,
    favorite_button: gtk::Button,
    mute_button: gtk::Button,
    mute_icon: gtk::Image,
    volume: gtk::Scale,
}

impl Shell {
    pub(super) fn update_bottom_player(self: &Rc<Self>) {
        let player = self.state.player.borrow().clone();
        let controls = &self.player_controls;
        self.state.updating_player_controls.set(true);

        let cover_seed = player
            .current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or(42);
        controls.cover.set_seed(cover_seed);
        if let Some(image_ref) = player
            .current
            .as_ref()
            .and_then(|entry| entry.image_ref.as_ref())
        {
            if let Some(key) = self.cover_cache_key(image_ref, THUMB_COVER_SIZE) {
                if controls.cover_key.borrow().as_deref() != Some(key.as_str()) {
                    controls.cover.clear_image();
                    let generation = controls.cover.generation();
                    self.state
                        .cover_bindings
                        .borrow_mut()
                        .entry(key.clone())
                        .or_default()
                        .push(CoverBinding {
                            tile: controls.cover.clone(),
                            generation,
                        });
                    self.controller.request_cover_for_key(
                        key.clone(),
                        image_ref.clone(),
                        THUMB_COVER_SIZE,
                    );
                    *controls.cover_key.borrow_mut() = Some(key);
                }
            } else {
                let mut current_key = controls.cover_key.borrow_mut();
                if current_key.is_some() {
                    controls.cover.clear_image();
                    *current_key = None;
                }
            }
        } else {
            controls.cover.clear_image();
            *controls.cover_key.borrow_mut() = None;
        }

        let play_icon = match player.state {
            PlaybackState::Playing | PlaybackState::Buffering => "media-playback-pause-symbolic",
            PlaybackState::Paused | PlaybackState::Stopped => "media-playback-start-symbolic",
        };
        controls.play_icon.set_icon_name(Some(play_icon));
        controls
            .play_button
            .set_tooltip_text(Some(&crate::i18n::tr(playback_state_label(player.state))));

        let title = player
            .current
            .as_ref()
            .map(|entry| entry.title.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| crate::i18n::tr("Nothing playing"));
        let artist = player
            .current
            .as_ref()
            .map(|entry| entry.artist.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| crate::i18n::tr("Queue a track to begin"));
        let album = player
            .current
            .as_ref()
            .map(|entry| entry.album.as_str())
            .unwrap_or("");
        controls.title.set_text(&title);
        controls.artist.set_text(&artist);
        controls.album.set_text(album);
        controls.title.set_sensitive(player.current.is_some());
        controls.artist.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.artist.is_empty()),
        );
        controls.album.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.album.is_empty()),
        );

        set_active_class(&controls.shuffle_button, player.shuffle_enabled);
        set_active_class(
            &controls.repeat_button,
            player.repeat_mode != rufin_core::RepeatMode::Off,
        );
        set_repeat_button_icon(&controls.repeat_button, player.repeat_mode);
        set_active_class(&controls.dj_button, player.auto_dj_enabled);
        controls
            .dj_button
            .set_tooltip_text(Some(&crate::i18n::tr(if player.auto_dj_enabled {
                "Auto DJ on"
            } else {
                "Auto DJ"
            })));
        set_favorite_button_active(
            &controls.favorite_button,
            player.current.as_ref().is_some_and(|entry| entry.favorite),
        );
        controls
            .favorite_button
            .set_sensitive(player.current.is_some());
        controls
            .repeat_button
            .set_tooltip_text(Some(&crate::i18n::tr(repeat_label(player.repeat_mode))));

        controls
            .elapsed
            .set_text(&format_duration(player.position_seconds));
        if !self.state.seeking_player_controls.get() {
            let max = f64::from(player.duration_seconds.max(1));
            controls.progress.set_range(0.0, max);
            controls.progress.set_value(f64::from(
                player.position_seconds.min(player.duration_seconds),
            ));
        }
        controls
            .duration
            .set_text(&format_duration(player.duration_seconds));

        controls.mute_icon.set_icon_name(Some(if player.muted {
            "audio-volume-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }));
        controls.volume.set_value(player.volume);
        self.state.updating_player_controls.set(false);
    }
}

pub(super) fn build_bottom_player() -> PlayerControls {
    let root = gtk::Overlay::new();
    root.add_css_class("bottom-player");
    root.set_hexpand(true);
    root.set_vexpand(false);
    root.set_height_request(super::BOTTOM_PLAYER_HEIGHT);
    root.set_valign(gtk::Align::Center);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.set_hexpand(true);
    bar.set_valign(gtk::Align::Center);

    let NowPlayingControls {
        root: now_playing,
        cover,
        title,
        artist,
        album,
    } = build_now_playing_controls();
    bar.append(&now_playing);

    let TransportControls {
        root: transport,
        stop_button,
        previous_button,
        play_button,
        play_icon,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        elapsed,
        progress,
        duration,
    } = build_transport_controls();

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let PlayerActionControls {
        root: actions,
        queue_button,
        queue_icon,
        queue_icon_open,
        favorite_button,
        mute_button,
        mute_icon,
        volume,
    } = build_player_action_controls();
    bar.append(&actions);

    let transport_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    transport_slot
        .set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH + BOTTOM_PLAYER_TRANSPORT_OFFSET);
    transport_slot.set_halign(gtk::Align::Center);
    transport_slot.set_valign(gtk::Align::Center);
    transport_slot.set_margin_top(BOTTOM_PLAYER_TRANSPORT_MARGIN_TOP);
    transport_slot.append(&transport);
    let offset_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    offset_spacer.set_width_request(BOTTOM_PLAYER_TRANSPORT_OFFSET);
    transport_slot.append(&offset_spacer);

    root.set_child(Some(&bar));
    root.add_overlay(&transport_slot);

    PlayerControls {
        root,
        cover,
        cover_key: RefCell::new(None),
        title,
        artist,
        album,
        stop_button,
        previous_button,
        play_button,
        play_icon,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        queue_button,
        queue_icon,
        queue_icon_open,
        favorite_button,
        elapsed,
        progress,
        duration,
        mute_button,
        mute_icon,
        volume,
    }
}

fn build_now_playing_controls() -> NowPlayingControls {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    root.add_css_class("player-now-playing");
    root.set_valign(gtk::Align::Center);

    let cover = ArtworkTile::new(BOTTOM_PLAYER_COVER_SIZE, 42);
    cover.area.set_valign(gtk::Align::Center);
    root.append(&cover.area);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 1);
    identity.add_css_class("player-identity");
    identity.set_size_request(BOTTOM_PLAYER_IDENTITY_WIDTH, -1);
    identity.set_hexpand(false);
    identity.set_valign(gtk::Align::Center);
    let title = player_link("player-title");
    let artist = player_link("muted");
    let album = player_link("muted");
    identity.append(&title);
    identity.append(&artist);
    identity.append(&album);
    root.append(&identity);

    NowPlayingControls {
        root,
        cover,
        title,
        artist,
        album,
    }
}

fn build_transport_controls() -> TransportControls {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 5);
    root.add_css_class("player-transport");
    root.set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH);
    root.set_valign(gtk::Align::Center);

    let buttons = gtk::Fixed::new();
    buttons.add_css_class("player-button-row");
    buttons.set_halign(gtk::Align::Center);
    buttons.set_valign(gtk::Align::Center);
    buttons.set_size_request(
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        BOTTOM_PLAYER_BUTTON_ROW_HEIGHT,
    );

    let stop_button = stop_icon_button("Stop");
    let previous_button = skip_icon_button(false, "Previous");
    let (play_button, play_icon) = icon_button_with_image("media-playback-start-symbolic", "Play");
    let next_button = skip_icon_button(true, "Next");
    let (shuffle_button, _) = transport_symbol_button("media-playlist-shuffle-symbolic", "Shuffle");
    let repeat_button = repeat_icon_button("Repeat off");
    let (dj_button, _) = transport_symbol_button("media-optical-cd-audio-symbolic", "Auto DJ");

    configure_transport_side_button(&stop_button);
    configure_transport_side_button(&previous_button);
    configure_play_button(&play_button, &play_icon);
    configure_transport_side_button(&next_button);

    put_transport_button(&buttons, &stop_button, -3.0, BOTTOM_PLAYER_SIDE_BUTTON_SIZE);
    put_transport_button(
        &buttons,
        &shuffle_button,
        -2.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &previous_button,
        -1.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(&buttons, &play_button, 0.0, BOTTOM_PLAYER_PLAY_BUTTON_SIZE);
    put_transport_button(&buttons, &next_button, 1.0, BOTTOM_PLAYER_SIDE_BUTTON_SIZE);
    put_transport_button(
        &buttons,
        &repeat_button,
        2.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(&buttons, &dj_button, 3.0, BOTTOM_PLAYER_SIDE_BUTTON_SIZE);

    let progress_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    progress_row.add_css_class("player-progress-row");
    progress_row.set_halign(gtk::Align::Center);
    progress_row.set_valign(gtk::Align::Center);
    let elapsed = gtk::Label::new(Some("0:00"));
    elapsed.add_css_class("muted");
    elapsed.set_width_chars(4);
    elapsed.set_xalign(1.0);
    let progress = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    progress.add_css_class("player-progress");
    progress.set_draw_value(false);
    progress.set_width_request(BOTTOM_PLAYER_PROGRESS_WIDTH);
    let duration = gtk::Label::new(Some("0:00"));
    duration.add_css_class("muted");
    duration.set_width_chars(4);
    progress_row.append(&elapsed);
    progress_row.append(&progress);
    progress_row.append(&duration);

    root.append(&buttons);
    root.append(&progress_row);

    TransportControls {
        root,
        stop_button,
        previous_button,
        play_button,
        play_icon,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        elapsed,
        progress,
        duration,
    }
}

fn build_player_action_controls() -> PlayerActionControls {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    root.set_valign(gtk::Align::Center);
    let (queue_button, queue_icon, queue_icon_open) = queue_sidebar_button("Hide sidebar");
    root.append(&queue_button);
    root.append(&icon_button("audio-input-microphone-symbolic", "Lyrics"));
    let favorite_button = favorite_icon_button("Favorite");
    root.append(&favorite_button);
    let (mute_button, mute_icon) = icon_button_with_image("audio-volume-high-symbolic", "Mute");
    root.append(&mute_button);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    volume.add_css_class("volume-slider");
    volume.set_width_request(88);
    volume.set_value(1.0);
    volume.set_draw_value(false);
    root.append(&volume);

    PlayerActionControls {
        root,
        queue_button,
        queue_icon,
        queue_icon_open,
        favorite_button,
        mute_button,
        mute_icon,
        volume,
    }
}

fn configure_transport_side_button(button: &gtk::Button) {
    button.add_css_class("player-transport-button");
    button.set_size_request(
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
}

fn configure_play_button(button: &gtk::Button, icon: &gtk::Image) {
    button.add_css_class("player-transport-button");
    button.add_css_class("player-play-button");
    button.set_size_request(
        BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
        BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
    );
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_pixel_size(17);
}

fn transport_symbol_button(icon_name: &str, label: &str) -> (gtk::Button, gtk::Image) {
    let (button, icon) = icon_button_with_image(icon_name, label);
    configure_transport_side_button(&button);
    icon.set_pixel_size(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    (button, icon)
}

fn put_transport_button(buttons: &gtk::Fixed, button: &gtk::Button, slot: f64, size: i32) {
    let center_x =
        f64::from(BOTTOM_PLAYER_TRANSPORT_WIDTH) / 2.0 + BOTTOM_PLAYER_BUTTON_STEP * slot;
    let radius = f64::from(size) / 2.0;
    let y = f64::from(BOTTOM_PLAYER_BUTTON_ROW_HEIGHT - size) / 2.0 + BOTTOM_PLAYER_BUTTON_OFFSET_Y;
    buttons.put(button, center_x - radius, y);
}

pub(super) fn connect_player_controls(shell: &Rc<Shell>) {
    let controller = shell.controller.clone();
    shell
        .player_controls
        .stop_button
        .connect_clicked(move |_| controller.stop());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .previous_button
        .connect_clicked(move |_| controller.previous_track());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .play_button
        .connect_clicked(move |_| controller.play_pause());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .next_button
        .connect_clicked(move |_| controller.next_track());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .shuffle_button
        .connect_clicked(move |_| controller.toggle_shuffle());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .repeat_button
        .connect_clicked(move |_| controller.cycle_repeat());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .dj_button
        .connect_clicked(move |_| controller.toggle_auto_dj());

    let queue_shell = Rc::clone(shell);
    shell
        .player_controls
        .queue_button
        .connect_clicked(move |_| queue_shell.toggle_right_panel());

    let controller = shell.controller.clone();
    shell
        .player_controls
        .favorite_button
        .connect_clicked(move |_| controller.toggle_current_favorite());

    let title_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.title, move || {
        let Some(entry) = title_shell.state.player.borrow().current.clone() else {
            return;
        };
        title_shell.navigate(Route::Search {
            query: entry.title,
            kind: SearchKind::Tracks,
        });
    });

    let artist_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.artist, move || {
        let Some(entry) = artist_shell.state.player.borrow().current.clone() else {
            return;
        };
        if let Some(artist_id) = entry.artist_id {
            artist_shell.navigate(Route::ArtistDetail(artist_id));
        } else if !entry.artist.trim().is_empty() {
            artist_shell.navigate(Route::Search {
                query: entry.artist,
                kind: SearchKind::Artists,
            });
        }
    });

    let album_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.album, move || {
        let Some(entry) = album_shell.state.player.borrow().current.clone() else {
            return;
        };
        if let Some(album_id) = entry.album_id {
            album_shell.navigate(Route::AlbumDetail(album_id));
        } else if !entry.album.trim().is_empty() {
            album_shell.navigate(Route::Search {
                query: entry.album,
                kind: SearchKind::Albums,
            });
        }
    });

    let controller = shell.controller.clone();
    shell
        .player_controls
        .mute_button
        .connect_clicked(move |_| controller.toggle_mute());

    let seek_shell = Rc::clone(shell);
    shell
        .player_controls
        .progress
        .connect_change_value(move |scale, _scroll, value| {
            if seek_shell.state.updating_player_controls.get() {
                return glib::Propagation::Proceed;
            }
            let player = seek_shell.state.player.borrow();
            let duration_seconds = player.duration_seconds;
            if player.current.is_none() || duration_seconds == 0 {
                return glib::Propagation::Stop;
            }
            drop(player);

            seek_shell.state.seeking_player_controls.set(true);
            let generation = seek_shell.state.seek_generation.get().saturating_add(1);
            seek_shell.state.seek_generation.set(generation);
            let seconds = seekbar_target_seconds(value, duration_seconds);
            scale.set_value(f64::from(seconds));
            seek_shell
                .player_controls
                .elapsed
                .set_text(&format_duration(seconds));
            let seek_shell = Rc::clone(&seek_shell);
            glib::timeout_add_local_once(Duration::from_millis(350), move || {
                if seek_shell.state.seek_generation.get() == generation {
                    seek_shell.controller.seek(seconds);
                    seek_shell.state.seeking_player_controls.set(false);
                }
            });
            glib::Propagation::Stop
        });

    let volume_shell = Rc::clone(shell);
    shell
        .player_controls
        .volume
        .connect_value_changed(move |scale| {
            if volume_shell.state.updating_player_controls.get() {
                return;
            }
            volume_shell.controller.set_volume(scale.value());
        });
}
