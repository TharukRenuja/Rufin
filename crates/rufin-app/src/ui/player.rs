use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use rufin_core::{RepeatMode, Route, SearchKind, format_duration};
use rufin_playback::PlaybackState;

use super::{
    ArtworkTile, CoverBinding, Shell, THUMB_COVER_SIZE, add_dynamic_link_hover, add_label_click,
    favorite_icon_button, icon_button_with_image, seekbar_target_seconds, set_active_class,
    set_favorite_button_active,
};

pub(super) const BOTTOM_PLAYER_HEIGHT: i32 = 96;
const BOTTOM_PLAYER_COVER_SIZE: i32 = 72;
const BOTTOM_PLAYER_IDENTITY_WIDTH: i32 = 190;
const BOTTOM_PLAYER_IDENTITY_MAX_CHARS: i32 = 24;
const BOTTOM_PLAYER_TRANSPORT_WIDTH: i32 = 420;
const BOTTOM_PLAYER_PROGRESS_WIDTH: i32 = 320;
const BOTTOM_PLAYER_BUTTON_ROW_HEIGHT: i32 = 58;
const BOTTOM_PLAYER_SIDE_BUTTON_SIZE: i32 = 50;
const BOTTOM_PLAYER_PLAY_BUTTON_SIZE: i32 = 45;
const BOTTOM_PLAYER_BUTTON_OFFSET_Y: f64 = 3.0;
const BOTTOM_PLAYER_BUTTON_STEP: f64 = 52.0;
const BOTTOM_PLAYER_TRANSPORT_ICON_SIZE: i32 = 23;

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
    pub(super) play_icon: gtk::DrawingArea,
    pub(super) play_icon_playing: Rc<Cell<bool>>,
    pub(super) next_button: gtk::Button,
    pub(super) shuffle_button: gtk::Button,
    pub(super) repeat_button: gtk::Button,
    pub(super) dj_button: gtk::Button,
    pub(super) queue_button: gtk::Button,
    pub(super) queue_icon: gtk::DrawingArea,
    pub(super) queue_icon_open: Rc<Cell<bool>>,
    pub(super) lyrics_button: gtk::Button,
    pub(super) lyrics_icon: gtk::DrawingArea,
    pub(super) lyrics_icon_open: Rc<Cell<bool>>,
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
    play_icon: gtk::DrawingArea,
    play_icon_playing: Rc<Cell<bool>>,
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
    lyrics_button: gtk::Button,
    lyrics_icon: gtk::DrawingArea,
    lyrics_icon_open: Rc<Cell<bool>>,
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

        controls.play_icon_playing.set(matches!(
            player.state,
            PlaybackState::Playing | PlaybackState::Buffering
        ));
        controls.play_icon.queue_draw();
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
    root.set_height_request(BOTTOM_PLAYER_HEIGHT);
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
        play_icon_playing,
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
        lyrics_button,
        lyrics_icon,
        lyrics_icon_open,
        favorite_button,
        mute_button,
        mute_icon,
        volume,
    } = build_player_action_controls();
    bar.append(&actions);

    let transport_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    transport_slot.set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH);
    transport_slot.set_halign(gtk::Align::Center);
    transport_slot.set_valign(gtk::Align::Center);
    transport_slot.append(&transport);

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
        play_icon_playing,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        queue_button,
        queue_icon,
        queue_icon_open,
        lyrics_button,
        lyrics_icon,
        lyrics_icon_open,
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
    let (play_button, play_icon, play_icon_playing) = play_icon_button("Play");
    let next_button = skip_icon_button(true, "Next");
    let shuffle_button = shuffle_icon_button("Shuffle");
    let repeat_button = repeat_icon_button("Repeat off");
    let dj_button = auto_dj_icon_button("Auto DJ");

    configure_transport_side_button(&stop_button);
    configure_transport_side_button(&shuffle_button);
    configure_transport_side_button(&previous_button);
    configure_play_button(&play_button);
    configure_transport_side_button(&next_button);
    configure_transport_side_button(&repeat_button);
    configure_transport_side_button(&dj_button);

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
        play_icon_playing,
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
    let (lyrics_button, lyrics_icon, lyrics_icon_open) = lyrics_icon_button("Hide lyrics");
    root.append(&lyrics_button);
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
        lyrics_button,
        lyrics_icon,
        lyrics_icon_open,
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

fn configure_play_button(button: &gtk::Button) {
    button.add_css_class("player-transport-button");
    button.add_css_class("player-play-button");
    button.set_size_request(
        BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
        BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
    );
}

fn put_transport_button(buttons: &gtk::Fixed, button: &gtk::Button, slot: f64, size: i32) {
    let center_x =
        f64::from(BOTTOM_PLAYER_TRANSPORT_WIDTH) / 2.0 + BOTTOM_PLAYER_BUTTON_STEP * slot;
    let radius = f64::from(size) / 2.0;
    let y = f64::from(BOTTOM_PLAYER_BUTTON_ROW_HEIGHT - size) / 2.0 + BOTTOM_PLAYER_BUTTON_OFFSET_Y;
    buttons.put(button, center_x - radius, y);
}

fn player_link(css_class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("player-link");
    label.add_css_class(css_class);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(1);
    label.set_max_width_chars(BOTTOM_PLAYER_IDENTITY_MAX_CHARS);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(false);
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), &label);
    label
}

fn playback_state_label(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Stopped => "Play",
        PlaybackState::Paused => "Resume",
        PlaybackState::Buffering => "Pause",
        PlaybackState::Playing => "Pause",
    }
}

fn repeat_label(repeat_mode: RepeatMode) -> &'static str {
    match repeat_mode {
        RepeatMode::Off => "Repeat off",
        RepeatMode::One => "Repeat one",
        RepeatMode::All => "Repeat all",
    }
}

fn set_repeat_button_icon(button: &gtk::Button, repeat_mode: RepeatMode) {
    button.set_child(Some(&repeat_icon_area(repeat_mode)));
}

fn set_icon_source(area: &gtk::DrawingArea, context: &gtk::cairo::Context) {
    let color = area.color();
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

fn drawing_icon_button(label: &str, icon: gtk::DrawingArea) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_tooltip_text(Some(&crate::i18n::tr(label)));
    button.set_child(Some(&icon));
    button
}

fn stop_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let size = f64::from(width.min(height)) * 0.48;
        context.rectangle(
            (f64::from(width) - size) / 2.0,
            (f64::from(height) - size) / 2.0,
            size,
            size,
        );
        let _ = context.fill();
    });
    drawing_icon_button(label, icon)
}

fn skip_icon_button(forward: bool, label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let width = f64::from(width);
        let height = f64::from(height);
        let center_y = height / 2.0;
        let top = center_y - height * 0.28;
        let bottom = center_y + height * 0.28;
        let bar_width = (width * 0.12).clamp(2.1, 2.8);
        if forward {
            context.move_to(width * 0.30, top);
            context.line_to(width * 0.30, bottom);
            context.line_to(width * 0.70, center_y);
            context.close_path();
            let _ = context.fill();
            context.rectangle(width * 0.76, top, bar_width, bottom - top);
            let _ = context.fill();
        } else {
            context.rectangle(width * 0.20, top, bar_width, bottom - top);
            let _ = context.fill();
            context.move_to(width * 0.70, top);
            context.line_to(width * 0.70, bottom);
            context.line_to(width * 0.30, center_y);
            context.close_path();
            let _ = context.fill();
        }
    });
    drawing_icon_button(label, icon)
}

fn play_icon_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let playing = Rc::new(Cell::new(false));
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    let icon_playing = Rc::clone(&playing);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        let width = f64::from(width);
        let height = f64::from(height);
        if icon_playing.get() {
            let bar_width = width * 0.15;
            let bar_height = height * 0.48;
            let y = (height - bar_height) / 2.0;
            context.rectangle(width * 0.34, y, bar_width, bar_height);
            context.rectangle(width * 0.56, y, bar_width, bar_height);
            let _ = context.fill();
        } else {
            context.move_to(width * 0.38, height * 0.28);
            context.line_to(width * 0.38, height * 0.72);
            context.line_to(width * 0.72, height * 0.50);
            context.close_path();
            let _ = context.fill();
        }
    });
    let button = drawing_icon_button(label, icon.clone());
    (button, icon, playing)
}

fn shuffle_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.8);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let left = width * 0.22;
        let right = width * 0.72;
        let arrow = width * 0.13;
        let top_y = height * 0.34;
        let bottom_y = height * 0.66;

        context.move_to(left, top_y);
        context.curve_to(width * 0.38, top_y, width * 0.43, bottom_y, right, bottom_y);
        context.line_to(right - arrow, bottom_y - arrow * 0.75);
        context.move_to(right, bottom_y);
        context.line_to(right - arrow, bottom_y + arrow * 0.75);

        context.move_to(left, bottom_y);
        context.curve_to(width * 0.38, bottom_y, width * 0.43, top_y, right, top_y);
        context.line_to(right - arrow, top_y - arrow * 0.75);
        context.move_to(right, top_y);
        context.line_to(right - arrow, top_y + arrow * 0.75);
        let _ = context.stroke();
    });
    drawing_icon_button(label, icon)
}

fn repeat_icon_button(label: &str) -> gtk::Button {
    let button = drawing_icon_button(label, repeat_icon_area(RepeatMode::Off));
    button.add_css_class("player-repeat-button");
    button
}

fn repeat_icon_area(repeat_mode: RepeatMode) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.9);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let center_x = width / 2.0;
        let center_y = height / 2.0;
        let radius = width.min(height) * 0.29;
        context.arc(
            center_x,
            center_y,
            radius,
            -0.15,
            std::f64::consts::PI * 1.72,
        );
        let _ = context.stroke();

        let arrow_x = center_x + radius * (-0.15_f64).cos();
        let arrow_y = center_y + radius * (-0.15_f64).sin();
        let arrow = width.min(height) * 0.13;
        context.move_to(arrow_x, arrow_y);
        context.line_to(arrow_x - arrow * 0.98, arrow_y - arrow * 0.35);
        context.move_to(arrow_x, arrow_y);
        context.line_to(arrow_x - arrow * 0.38, arrow_y + arrow);
        let _ = context.stroke();

        if repeat_mode == RepeatMode::One {
            context.set_line_width(1.35);
            let one_x = width / 2.0;
            let one_top = height * 0.40;
            let one_bottom = height * 0.66;
            context.move_to(one_x, one_top);
            context.line_to(one_x, one_bottom);
            context.move_to(one_x - 1.5, one_top + 1.0);
            context.line_to(one_x, one_top);
            let _ = context.stroke();
        }
    });
    icon
}

fn lyrics_icon_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let open = Rc::new(Cell::new(true));
    let icon = lyrics_icon_area(Rc::clone(&open));
    let button = drawing_icon_button(label, icon.clone());
    (button, icon, open)
}

fn lyrics_icon_area(open: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.7);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let left = width * 0.25;
        let right = width * 0.75;
        let top = height * 0.25;
        let bottom = height * 0.66;
        let radius = width * 0.09;

        context.move_to(left + radius, top);
        context.line_to(right - radius, top);
        context.curve_to(right, top, right, top, right, top + radius);
        context.line_to(right, bottom - radius);
        context.curve_to(right, bottom, right, bottom, right - radius, bottom);
        context.line_to(width * 0.45, bottom);
        context.line_to(width * 0.32, height * 0.79);
        context.line_to(width * 0.34, bottom);
        context.line_to(left + radius, bottom);
        context.curve_to(left, bottom, left, bottom, left, bottom - radius);
        context.line_to(left, top + radius);
        context.curve_to(left, top, left, top, left + radius, top);
        let _ = context.stroke();

        if open.get() {
            context.move_to(width * 0.36, height * 0.42);
            context.line_to(width * 0.64, height * 0.42);
            context.move_to(width * 0.36, height * 0.54);
            context.line_to(width * 0.58, height * 0.54);
            let _ = context.stroke();
        }
    });
    icon
}

fn auto_dj_icon_button(label: &str) -> gtk::Button {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_content_height(BOTTOM_PLAYER_TRANSPORT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_draw_func(move |area, context, width, height| {
        set_icon_source(area, context);
        context.set_line_width(1.8);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_line_join(gtk::cairo::LineJoin::Round);

        let width = f64::from(width);
        let height = f64::from(height);
        let center_x = width / 2.0;
        let center_y = height * 0.53;
        let radius = width.min(height) * 0.29;

        context.arc(
            center_x,
            center_y,
            radius,
            std::f64::consts::PI,
            std::f64::consts::TAU,
        );
        let _ = context.stroke();

        context.rectangle(width * 0.22, height * 0.50, width * 0.13, height * 0.24);
        context.rectangle(width * 0.65, height * 0.50, width * 0.13, height * 0.24);
        let _ = context.stroke();

        context.set_line_width(1.45);
        context.move_to(width * 0.75, height * 0.21);
        context.line_to(width * 0.75, height * 0.36);
        context.move_to(width * 0.68, height * 0.285);
        context.line_to(width * 0.82, height * 0.285);
        let _ = context.stroke();
    });
    drawing_icon_button(label, icon)
}

fn queue_sidebar_button(label: &str) -> (gtk::Button, gtk::DrawingArea, Rc<Cell<bool>>) {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    let label = crate::i18n::tr(label);
    button.set_tooltip_text(Some(&label));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);

    let open = Rc::new(Cell::new(true));
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(16);
    icon.set_content_height(16);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);

    let icon_open = Rc::clone(&open);
    icon.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        let set_source = |alpha: f64| {
            context.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()) * alpha,
            );
        };

        let width = f64::from(width);
        let height = f64::from(height);
        let x = (width - 14.0) / 2.0;
        let y = (height - 12.0) / 2.0;
        let icon_width = 14.0;
        let icon_height = 12.0;
        let separator_x = x + icon_width - 4.5;
        let center_y = y + icon_height / 2.0;

        if icon_open.get() {
            set_source(0.32);
            context.rectangle(separator_x, y, icon_width - (separator_x - x), icon_height);
            let _ = context.fill();
        }

        set_source(1.0);
        context.set_line_width(1.4);
        context.rectangle(x + 0.7, y + 0.7, icon_width - 1.4, icon_height - 1.4);
        let _ = context.stroke();

        context.move_to(separator_x, y + 1.2);
        context.line_to(separator_x, y + icon_height - 1.2);
        let _ = context.stroke();

        if !icon_open.get() {
            context.set_line_width(1.5);
            context.move_to(separator_x + 2.6, center_y - 3.0);
            context.line_to(separator_x + 1.0, center_y);
            context.line_to(separator_x + 2.6, center_y + 3.0);
            let _ = context.stroke();
        }
    });
    button.set_child(Some(&icon));
    (button, icon, open)
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

    let lyrics_shell = Rc::clone(shell);
    shell
        .player_controls
        .lyrics_button
        .connect_clicked(move |_| lyrics_shell.toggle_lyrics_panel());

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
