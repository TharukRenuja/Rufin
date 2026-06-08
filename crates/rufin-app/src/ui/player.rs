use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use rufin_core::{RepeatMode, Route, SearchKind, format_duration};
use rufin_playback::PlaybackState;

use crate::controller::PlaybackSnapshot;

use super::player_icons::{
    auto_dj_icon_button, lyrics_icon_button, play_icon_button, queue_sidebar_button,
    random_clover_icon_button, repeat_icon_button, set_repeat_button_icon, shuffle_icon_button,
    skip_icon_button,
};
use super::{
    ArtworkTile, Shell, THUMB_COVER_SIZE, add_dynamic_link_hover, add_label_click,
    add_widget_click, favorite_icon_button, icon_button_with_image,
    install_current_track_context_menu, seekbar_target_seconds, set_active_class,
    set_favorite_button_active,
};

pub(super) const BOTTOM_PLAYER_HEIGHT: i32 = 96;
pub(in crate::ui) const BOTTOM_PLAYER_COVER_SIZE: i32 = 72;
const BOTTOM_PLAYER_HORIZONTAL_PADDING: i32 = 6;
const BOTTOM_PLAYER_NOW_PLAYING_SPACING: i32 = 8;
const BOTTOM_PLAYER_TRANSPORT_WIDTH: i32 = 300;
const BOTTOM_PLAYER_PROGRESS_WIDTH: i32 = 320;
const BOTTOM_PLAYER_PROGRESS_MIN_WIDTH: i32 = 140;
const BOTTOM_PLAYER_BUTTON_ROW_HEIGHT: i32 = 58;
const BOTTOM_PLAYER_SIDE_BUTTON_SIZE: i32 = 50;
const BOTTOM_PLAYER_PLAY_BUTTON_SIZE: i32 = 45;
const BOTTOM_PLAYER_BUTTON_OFFSET_Y: f64 = 3.0;
const BOTTOM_PLAYER_BUTTON_STEP: f64 = 38.0;
const BOTTOM_PLAYER_WAVEFORM_HEIGHT: i32 = 32;
const BOTTOM_PLAYER_ACTION_BUTTON_SIZE: i32 = 34;
const BOTTOM_PLAYER_ACTION_SPACING: i32 = 3;
const BOTTOM_PLAYER_VOLUME_SPACING: i32 = 1;
const BOTTOM_PLAYER_VOLUME_MIN_WIDTH: i32 = 48;
const BOTTOM_PLAYER_VOLUME_MAX_WIDTH: i32 = 160;
const BOTTOM_PLAYER_VOLUME_WIDTH_RATIO: f64 = 1.0 / 16.0;
const BOTTOM_PLAYER_RIGHT_EDGE_GAP: i32 = 8;
const BOTTOM_PLAYER_TRANSPORT_CLEARANCE: i32 = 8;
const BOTTOM_PLAYER_COMPACT_MIN_WIDTH: i32 = 614;
const BOTTOM_PLAYER_FULL_PROGRESS_WIDTH: i32 = 864;
const BOTTOM_PLAYER_SHOW_FAVORITE_WIDTH: i32 = BOTTOM_PLAYER_COMPACT_MIN_WIDTH;
const BOTTOM_PLAYER_SHOW_LYRICS_WIDTH: i32 = 780;
const BOTTOM_PLAYER_SHOW_QUEUE_WIDTH: i32 = 864;
const SEEK_PREVIEW_COMMIT_DELAY: Duration = Duration::from_millis(100);
const SEEK_PREVIEW_SETTLE_WINDOW: Duration = Duration::from_millis(1_000);
const SEEK_PREVIEW_TOLERANCE_MILLIS: u64 = 1_500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BottomPlayerActions {
    Volume,
    Favorite,
    Lyrics,
    Queue,
}

pub(super) struct PlayerControls {
    pub(super) root: gtk::CenterBox,
    pub(super) cover: ArtworkTile,
    pub(super) cover_key: RefCell<Option<String>>,
    pub(super) title: gtk::Label,
    pub(super) artist: gtk::Label,
    pub(super) album: gtk::Label,
    pub(super) random_button: gtk::Button,
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
    pub(super) progress_stack: gtk::Stack,
    pub(super) progress: gtk::Scale,
    pub(super) waveform: WaveformSeekBar,
    pub(super) waveform_key: RefCell<Option<String>>,
    pub(super) waveform_peak_count: Cell<usize>,
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
    random_button: gtk::Button,
    previous_button: gtk::Button,
    play_button: gtk::Button,
    play_icon: gtk::DrawingArea,
    play_icon_playing: Rc<Cell<bool>>,
    next_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    dj_button: gtk::Button,
    elapsed: gtk::Label,
    progress_stack: gtk::Stack,
    progress: gtk::Scale,
    waveform: WaveformSeekBar,
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

#[derive(Clone)]
pub(super) struct WaveformSeekBar {
    area: gtk::DrawingArea,
    peaks: Rc<RefCell<Vec<(f64, f64)>>>,
    position: Rc<Cell<f64>>,
}

impl WaveformSeekBar {
    fn new() -> Self {
        let area = gtk::DrawingArea::new();
        area.add_css_class("player-waveform");
        area.set_content_width(BOTTOM_PLAYER_PROGRESS_WIDTH);
        area.set_content_height(BOTTOM_PLAYER_WAVEFORM_HEIGHT);
        area.set_width_request(BOTTOM_PLAYER_PROGRESS_WIDTH);
        area.set_focusable(true);
        area.set_valign(gtk::Align::Center);
        let seek_label = crate::i18n::tr("Seek");
        area.set_tooltip_text(Some(&seek_label));
        area.update_property(&[
            gtk::accessible::Property::Label(&seek_label),
            gtk::accessible::Property::ValueMin(0.0),
            gtk::accessible::Property::ValueMax(1.0),
            gtk::accessible::Property::ValueNow(0.0),
        ]);

        let peaks = Rc::new(RefCell::new(Vec::new()));
        let position = Rc::new(Cell::new(0.0));
        let hover_position = Rc::new(Cell::new(None));
        let draw_peaks = Rc::clone(&peaks);
        let draw_position = Rc::clone(&position);
        let draw_hover_position = Rc::clone(&hover_position);
        area.set_draw_func(move |area, context, width, height| {
            draw_waveform_seekbar(
                area,
                context,
                width,
                height,
                &draw_peaks.borrow(),
                draw_position.get(),
                draw_hover_position.get(),
            );
        });

        let motion = gtk::EventControllerMotion::new();
        let motion_area = area.clone();
        let motion_hover = Rc::clone(&hover_position);
        motion.connect_motion(move |_, x, _| {
            motion_hover.set(Some(waveform_fraction_for_x(&motion_area, x)));
            motion_area.queue_draw();
        });
        let leave_area = area.clone();
        let leave_hover = Rc::clone(&hover_position);
        motion.connect_leave(move |_| {
            leave_hover.set(None);
            leave_area.queue_draw();
        });
        area.add_controller(motion);

        Self {
            area,
            peaks,
            position,
        }
    }

    fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    fn set_peaks(&self, peaks: Option<&[(f64, f64)]>) {
        let next = peaks.map(normalize_waveform_peaks).unwrap_or_default();
        *self.peaks.borrow_mut() = next;
        self.area.queue_draw();
    }

    fn set_position_fraction(&self, position: f64) {
        let position = position.clamp(0.0, 1.0);
        self.position.set(position);
        self.area
            .update_property(&[gtk::accessible::Property::ValueNow(position)]);
        self.area.queue_draw();
    }

    fn connect_seek<F, C>(&self, seek: F, commit: C)
    where
        F: Fn(f64) + 'static,
        C: Fn() + 'static,
    {
        let seek = Rc::new(seek);
        let commit = Rc::new(commit);

        let click = gtk::GestureClick::new();
        click.set_button(1);
        let click_area = self.area.clone();
        let click_seek = Rc::clone(&seek);
        click.connect_pressed(move |_, _, x, _| {
            click_area.grab_focus();
            click_seek(waveform_fraction_for_x(&click_area, x));
        });
        let click_commit = Rc::clone(&commit);
        click.connect_released(move |_, _, _, _| {
            click_commit();
        });
        self.area.add_controller(click);

        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        let drag_area = self.area.clone();
        let drag_seek = Rc::clone(&seek);
        drag.connect_drag_begin(move |gesture, x, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            drag_area.grab_focus();
            drag_seek(waveform_fraction_for_x(&drag_area, x));
        });
        let drag_area = self.area.clone();
        let drag_seek = Rc::clone(&seek);
        drag.connect_drag_update(move |gesture, x_offset, _| {
            let Some((start_x, _)) = gesture.start_point() else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            drag_seek(waveform_fraction_for_x(&drag_area, start_x + x_offset));
        });
        let drag_commit = Rc::clone(&commit);
        drag.connect_drag_end(move |_, _, _| {
            drag_commit();
        });
        self.area.add_controller(drag);

        let key = gtk::EventControllerKey::new();
        let key_position = Rc::clone(&self.position);
        let key_seek = Rc::clone(&seek);
        let key_commit = Rc::clone(&commit);
        key.connect_key_pressed(move |_, key, _, _| {
            let delta = match key {
                gtk::gdk::Key::Left => -0.02,
                gtk::gdk::Key::Right => 0.02,
                _ => return glib::Propagation::Proceed,
            };
            key_seek((key_position.get() + delta).clamp(0.0, 1.0));
            key_commit();
            glib::Propagation::Stop
        });
        self.area.add_controller(key);
    }
}

fn waveform_fraction_for_x(area: &gtk::DrawingArea, x: f64) -> f64 {
    let width = f64::from(area.width()).max(1.0);
    let fraction = (x / width).clamp(0.0, 1.0);
    if area.direction() == gtk::TextDirection::Rtl {
        1.0 - fraction
    } else {
        fraction
    }
}

fn normalize_waveform_peaks(peaks: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let max = peaks
        .iter()
        .flat_map(|(left, right)| [*left, *right])
        .filter(|value| value.is_finite())
        .fold(0.0_f64, f64::max);
    if max <= f64::EPSILON {
        return Vec::new();
    }
    peaks
        .iter()
        .filter(|(left, right)| left.is_finite() && right.is_finite())
        .map(|(left, right)| ((left / max).clamp(0.0, 1.0), (right / max).clamp(0.0, 1.0)))
        .collect()
}

fn draw_waveform_seekbar(
    area: &gtk::DrawingArea,
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    peaks: &[(f64, f64)],
    position: f64,
    hover_position: Option<f64>,
) {
    let width = f64::from(width.max(0));
    let height = f64::from(height.max(0));
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let is_rtl = area.direction() == gtk::TextDirection::Rtl;
    let position = visual_waveform_fraction(position, is_rtl) * width;
    let hover_position =
        hover_position.map(|value| visual_waveform_fraction(value, is_rtl) * width);
    let color = area.color();
    let center_y = height / 2.0;
    let bar_width = 2.0;
    let gap = 2.0;
    let block = bar_width + gap;
    let bar_count = ((width + gap) / block).floor().max(1.0) as usize;

    if peaks.is_empty() {
        set_waveform_source(context, &color, 0.32);
        context.rectangle(0.0, center_y - 1.0, width, 2.0);
        let _ = context.fill();
        return;
    }

    for index in 0..bar_count {
        let start = index * peaks.len() / bar_count;
        let end = ((index + 1) * peaks.len() / bar_count)
            .max(start + 1)
            .min(peaks.len());
        let samples = &peaks[start..end];
        let amplitude = samples
            .iter()
            .map(|(left, right)| (left + right) / 2.0)
            .sum::<f64>()
            / samples.len() as f64;
        let amplitude = amplitude.powf(0.72);
        let bar_height = (amplitude * height * 0.86).clamp(2.0, height);
        let x = index as f64 * block;
        let center_x = x + bar_width / 2.0;
        let opacity = waveform_bar_opacity(center_x, position, hover_position, is_rtl);
        set_waveform_source(context, &color, opacity);
        context.rectangle(x, center_y - bar_height / 2.0, bar_width, bar_height);
        let _ = context.fill();
    }
}

fn visual_waveform_fraction(value: f64, is_rtl: bool) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if is_rtl { 1.0 - value } else { value }
}

fn waveform_bar_opacity(x: f64, position: f64, hover_position: Option<f64>, is_rtl: bool) -> f64 {
    if let Some(hover_position) = hover_position {
        let start = position.min(hover_position);
        let end = position.max(hover_position);
        if (start..=end).contains(&x) {
            return 0.58;
        }
    }
    if (!is_rtl && x <= position) || (is_rtl && x >= position) {
        1.0
    } else {
        0.28
    }
}

fn set_waveform_source(context: &gtk::cairo::Context, color: &gtk::gdk::RGBA, opacity: f64) {
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()) * opacity,
    );
}

impl Shell {
    pub(in crate::ui) fn update_bottom_player(self: &Rc<Self>) {
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
            if let Some(key) = self.current_playback_cover_cache_key(image_ref, THUMB_COVER_SIZE) {
                let cover_key_changed =
                    controls.cover_key.borrow().as_deref() != Some(key.as_str());
                if cover_key_changed {
                    let has_decoded_cover =
                        self.decoded_cover_has_min_size(&key, BOTTOM_PLAYER_COVER_SIZE);
                    let has_cached_cover_file =
                        self.controller.cached_cover_path_for_key(&key).is_some();
                    if player_cover_replacement_is_ready(has_decoded_cover, has_cached_cover_file) {
                        controls.cover.advance_generation();
                    } else {
                        controls.cover.clear_image();
                    }
                    self.request_cover_for_tile(
                        &controls.cover,
                        key.clone(),
                        image_ref.clone(),
                        BOTTOM_PLAYER_COVER_SIZE,
                        THUMB_COVER_SIZE,
                    );
                    *controls.cover_key.borrow_mut() = Some(key);
                }
            } else {
                controls.cover.clear_image();
                *controls.cover_key.borrow_mut() = None;
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

        let preview_seconds = self.state.seek_preview_seconds.get();
        let displayed_seconds = preview_seconds.unwrap_or(player.position_seconds);
        controls
            .elapsed
            .set_text(&format_duration(displayed_seconds));
        let max = f64::from(player.duration_seconds.max(1));
        controls.progress.set_range(0.0, max);
        controls
            .progress
            .set_value(f64::from(displayed_seconds.min(player.duration_seconds)));
        let position_fraction = if player.duration_seconds == 0 {
            0.0
        } else {
            f64::from(displayed_seconds.min(player.duration_seconds))
                / f64::from(player.duration_seconds)
        };
        controls.waveform.set_position_fraction(position_fraction);
        let waveform_enabled = self.state.settings.borrow().seekbar_waveform_enabled;
        let waveform_key = waveform_enabled
            .then(|| player.waveform_cache_key.clone())
            .flatten();
        let waveform_peaks = waveform_enabled
            .then(|| player.waveform_peaks.as_deref().map(Vec::as_slice))
            .flatten();
        let waveform_peak_count = waveform_peaks.map_or(0, <[_]>::len);
        let waveform_key_changed = controls.waveform_key.borrow().as_ref() != waveform_key.as_ref();
        if waveform_key_changed || controls.waveform_peak_count.get() != waveform_peak_count {
            controls.waveform.set_peaks(waveform_peaks);
            *controls.waveform_key.borrow_mut() = waveform_key;
            controls.waveform_peak_count.set(waveform_peak_count);
        }
        if waveform_enabled {
            controls
                .progress_stack
                .set_visible_child(controls.waveform.widget());
        } else {
            controls
                .progress_stack
                .set_visible_child(&controls.progress);
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

    pub(in crate::ui) fn maybe_clear_player_seek_preview(
        &self,
        player: &PlaybackSnapshot,
        track_changed: bool,
    ) {
        let Some(target_seconds) = self.state.seek_preview_seconds.get() else {
            return;
        };
        if track_changed
            || player.current.is_none()
            || seek_preview_matches_position(target_seconds, player.position_millis)
        {
            self.clear_player_seek_preview();
        }
    }

    fn clear_player_seek_preview(&self) {
        self.state.seek_preview_seconds.set(None);
    }
}

fn player_cover_replacement_is_ready(has_decoded_cover: bool, has_cached_cover_file: bool) -> bool {
    has_decoded_cover || has_cached_cover_file
}

fn seek_preview_matches_position(target_seconds: u32, position_millis: u64) -> bool {
    let target_millis = u64::from(target_seconds) * 1_000;
    let lower = target_millis.saturating_sub(SEEK_PREVIEW_TOLERANCE_MILLIS);
    let upper = target_millis.saturating_add(SEEK_PREVIEW_TOLERANCE_MILLIS);
    (lower..=upper).contains(&position_millis)
}

pub(super) fn build_bottom_player() -> PlayerControls {
    let root = gtk::CenterBox::new();
    root.add_css_class("bottom-player");
    root.set_orientation(gtk::Orientation::Horizontal);
    root.set_shrink_center_last(true);
    root.set_hexpand(true);
    root.set_vexpand(false);
    root.set_height_request(BOTTOM_PLAYER_HEIGHT);
    root.set_valign(gtk::Align::Center);

    let NowPlayingControls {
        root: now_playing,
        cover,
        title,
        artist,
        album,
    } = build_now_playing_controls();

    let TransportControls {
        root: transport,
        random_button,
        previous_button,
        play_button,
        play_icon,
        play_icon_playing,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        elapsed,
        progress_stack,
        progress,
        waveform,
        duration,
    } = build_transport_controls();

    let now_playing_wall = gtk::ScrolledWindow::new();
    now_playing_wall.add_css_class("player-now-playing-wall");
    now_playing_wall.set_hexpand(true);
    now_playing_wall.set_vexpand(false);
    now_playing_wall.set_valign(gtk::Align::Center);
    configure_player_wall(&now_playing_wall);
    now_playing_wall.set_child(Some(&now_playing));

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

    let transport_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    transport_slot.set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH);
    transport_slot.set_halign(gtk::Align::Center);
    transport_slot.set_valign(gtk::Align::Center);
    transport_slot.append(&transport);

    root.set_start_widget(Some(&now_playing_wall));
    root.set_center_widget(Some(&transport_slot));
    root.set_end_widget(Some(&actions));

    PlayerControls {
        root,
        cover,
        cover_key: RefCell::new(None),
        title,
        artist,
        album,
        random_button,
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
        progress_stack,
        progress,
        waveform,
        waveform_key: RefCell::new(None),
        waveform_peak_count: Cell::new(0),
        duration,
        mute_button,
        mute_icon,
        volume,
    }
}

fn build_now_playing_controls() -> NowPlayingControls {
    let root = gtk::Box::new(
        gtk::Orientation::Horizontal,
        BOTTOM_PLAYER_NOW_PLAYING_SPACING,
    );
    root.add_css_class("player-now-playing");
    root.set_halign(gtk::Align::Start);
    root.set_valign(gtk::Align::Center);
    root.set_margin_start(BOTTOM_PLAYER_HORIZONTAL_PADDING);

    let cover = ArtworkTile::new(BOTTOM_PLAYER_COVER_SIZE, 42);
    cover.area.set_valign(gtk::Align::Center);
    cover.area.set_cursor_from_name(Some("pointer"));
    let cover_label = crate::i18n::tr("Open fullscreen player");
    cover.area.set_tooltip_text(Some(&cover_label));
    cover
        .area
        .update_property(&[gtk::accessible::Property::Label(&cover_label)]);
    root.append(&cover.area);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 1);
    identity.add_css_class("player-identity");
    identity.set_hexpand(true);
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

    let dj_button = auto_dj_icon_button("Auto DJ");
    let previous_button = skip_icon_button(false, "Previous");
    let (play_button, play_icon, play_icon_playing) = play_icon_button("Play");
    let next_button = skip_icon_button(true, "Next");
    let shuffle_button = shuffle_icon_button("Shuffle");
    let repeat_button = repeat_icon_button("Repeat off");
    let random_button = random_clover_icon_button("Play random");

    configure_transport_side_button(&dj_button);
    configure_transport_side_button(&shuffle_button);
    configure_transport_side_button(&previous_button);
    configure_play_button(&play_button);
    configure_transport_side_button(&next_button);
    configure_transport_side_button(&repeat_button);
    configure_transport_side_button(&random_button);

    put_transport_button(&buttons, &dj_button, -3.0, BOTTOM_PLAYER_SIDE_BUTTON_SIZE);
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
    put_transport_button(
        &buttons,
        &random_button,
        3.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );

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
    let waveform = WaveformSeekBar::new();
    let progress_stack = gtk::Stack::new();
    progress_stack.set_size_request(BOTTOM_PLAYER_PROGRESS_WIDTH, BOTTOM_PLAYER_WAVEFORM_HEIGHT);
    progress_stack.set_hhomogeneous(false);
    progress_stack.set_vhomogeneous(true);
    progress_stack.add_named(&progress, Some("scale"));
    progress_stack.add_named(waveform.widget(), Some("waveform"));
    progress_stack.set_visible_child(&progress);
    let duration = gtk::Label::new(Some("0:00"));
    duration.add_css_class("muted");
    duration.set_width_chars(4);
    progress_row.append(&elapsed);
    progress_row.append(&progress_stack);
    progress_row.append(&duration);

    root.append(&buttons);
    root.append(&progress_row);

    TransportControls {
        root,
        random_button,
        previous_button,
        play_button,
        play_icon,
        play_icon_playing,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        elapsed,
        progress_stack,
        progress,
        waveform,
        duration,
    }
}

fn build_player_action_controls() -> PlayerActionControls {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_ACTION_SPACING);
    root.set_halign(gtk::Align::End);
    root.set_valign(gtk::Align::Center);
    let (queue_button, queue_icon, queue_icon_open) = queue_sidebar_button("Hide sidebar");
    let (lyrics_button, lyrics_icon, lyrics_icon_open) = lyrics_icon_button("Hide lyrics");
    configure_player_action_button(&lyrics_button);
    root.append(&lyrics_button);
    configure_player_action_button(&queue_button);
    root.append(&queue_button);
    let favorite_button = favorite_icon_button("Favorite");
    configure_player_action_button(&favorite_button);
    root.append(&favorite_button);

    let volume_group = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_VOLUME_SPACING);
    volume_group.set_valign(gtk::Align::Center);
    let (mute_button, mute_icon) = icon_button_with_image("audio-volume-high-symbolic", "Mute");
    configure_player_action_button(&mute_button);
    volume_group.append(&mute_button);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    volume.add_css_class("volume-slider");
    volume.set_valign(gtk::Align::Center);
    volume.set_width_request(BOTTOM_PLAYER_VOLUME_MIN_WIDTH);
    volume.set_value(1.0);
    volume.set_draw_value(false);
    volume_group.append(&volume);
    root.append(&volume_group);

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

fn configure_player_action_button(button: &gtk::Button) {
    button.set_valign(gtk::Align::Center);
}

fn bottom_player_volume_width(player_width: i32) -> i32 {
    let right_side_width = (player_width - BOTTOM_PLAYER_TRANSPORT_WIDTH) / 2;
    let visible_action_count = match bottom_player_actions(player_width) {
        BottomPlayerActions::Volume => 0,
        BottomPlayerActions::Favorite => 1,
        BottomPlayerActions::Lyrics => 2,
        BottomPlayerActions::Queue => 3,
    };
    let action_width_without_volume = BOTTOM_PLAYER_ACTION_BUTTON_SIZE * (visible_action_count + 1)
        + BOTTOM_PLAYER_ACTION_SPACING * visible_action_count
        + BOTTOM_PLAYER_VOLUME_SPACING
        + BOTTOM_PLAYER_RIGHT_EDGE_GAP
        + BOTTOM_PLAYER_TRANSPORT_CLEARANCE;
    let available_width = right_side_width - action_width_without_volume;
    let proportional_width =
        (f64::from(player_width) * BOTTOM_PLAYER_VOLUME_WIDTH_RATIO).round() as i32;

    proportional_width.min(available_width).clamp(
        BOTTOM_PLAYER_VOLUME_MIN_WIDTH,
        BOTTOM_PLAYER_VOLUME_MAX_WIDTH,
    )
}

fn bottom_player_progress_width(player_width: i32) -> i32 {
    if player_width >= BOTTOM_PLAYER_FULL_PROGRESS_WIDTH {
        return BOTTOM_PLAYER_PROGRESS_WIDTH;
    }

    let span = BOTTOM_PLAYER_FULL_PROGRESS_WIDTH - BOTTOM_PLAYER_COMPACT_MIN_WIDTH;
    let width_span = BOTTOM_PLAYER_PROGRESS_WIDTH - BOTTOM_PLAYER_PROGRESS_MIN_WIDTH;
    let progress = (player_width - BOTTOM_PLAYER_COMPACT_MIN_WIDTH).clamp(0, span);
    BOTTOM_PLAYER_PROGRESS_MIN_WIDTH + width_span * progress / span
}

fn bottom_player_actions(player_width: i32) -> BottomPlayerActions {
    if player_width >= BOTTOM_PLAYER_SHOW_QUEUE_WIDTH {
        BottomPlayerActions::Queue
    } else if player_width >= BOTTOM_PLAYER_SHOW_LYRICS_WIDTH {
        BottomPlayerActions::Lyrics
    } else if player_width >= BOTTOM_PLAYER_SHOW_FAVORITE_WIDTH {
        BottomPlayerActions::Favorite
    } else {
        BottomPlayerActions::Volume
    }
}

struct PlayerWallSpec {
    horizontal_policy: gtk::PolicyType,
    vertical_policy: gtk::PolicyType,
    overflow: gtk::Overflow,
    min_content_width: i32,
    propagate_natural_width: bool,
    propagate_natural_height: bool,
}

fn player_wall_spec() -> PlayerWallSpec {
    PlayerWallSpec {
        horizontal_policy: gtk::PolicyType::External,
        vertical_policy: gtk::PolicyType::Never,
        overflow: gtk::Overflow::Hidden,
        min_content_width: 0,
        propagate_natural_width: false,
        propagate_natural_height: true,
    }
}

fn configure_player_wall(wall: &gtk::ScrolledWindow) {
    let spec = player_wall_spec();
    // this wall is intentionally boring: now playing can use all space
    // allocated to the left side, but it cannot draw into the center controls.
    wall.set_policy(spec.horizontal_policy, spec.vertical_policy);
    wall.set_overflow(spec.overflow);
    wall.set_min_content_width(spec.min_content_width);
    wall.set_propagate_natural_width(spec.propagate_natural_width);
    wall.set_propagate_natural_height(spec.propagate_natural_height);
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

fn preview_player_seek(shell: &Rc<Shell>, seconds: u32) {
    shell.state.seek_preview_seconds.set(Some(seconds));
    shell.player_controls.progress.set_value(f64::from(seconds));
    shell
        .player_controls
        .elapsed
        .set_text(&format_duration(seconds));
    let duration_seconds = shell.state.player.borrow().duration_seconds;
    let position = if duration_seconds == 0 {
        0.0
    } else {
        f64::from(seconds.min(duration_seconds)) / f64::from(duration_seconds)
    };
    shell
        .player_controls
        .waveform
        .set_position_fraction(position);
}

fn preview_player_seek_fraction(shell: &Rc<Shell>, position: f64) {
    let player = shell.state.player.borrow();
    let duration_seconds = player.duration_seconds;
    if player.current.is_none() || duration_seconds == 0 {
        return;
    }
    drop(player);
    let seconds = seekbar_target_seconds(position * f64::from(duration_seconds), duration_seconds);
    preview_player_seek(shell, seconds);
}

fn queue_player_seek_preview_commit(shell: &Rc<Shell>) {
    let generation = shell.state.seek_generation.get().saturating_add(1);
    shell.state.seek_generation.set(generation);

    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(SEEK_PREVIEW_COMMIT_DELAY, move || {
        if shell.state.seek_generation.get() == generation {
            commit_player_seek_preview(&shell, generation);
        }
    });
}

fn commit_player_seek_preview_now(shell: &Rc<Shell>) {
    let generation = shell.state.seek_generation.get().saturating_add(1);
    shell.state.seek_generation.set(generation);
    commit_player_seek_preview(shell, generation);
}

fn commit_player_seek_preview(shell: &Rc<Shell>, generation: u64) {
    let Some(seconds) = shell.state.seek_preview_seconds.get() else {
        return;
    };
    shell.state.seek_preview_seconds.set(None);
    shell.controller.seek(seconds);

    let shell = Rc::clone(shell);
    glib::timeout_add_local_once(SEEK_PREVIEW_SETTLE_WINDOW, move || {
        if shell.state.seek_generation.get() == generation {
            shell.clear_player_seek_preview();
        }
    });
}

pub(super) fn connect_player_controls(shell: &Rc<Shell>) {
    connect_bottom_player_volume_resize(shell);
    install_current_track_context_menu(&shell.player_controls.cover.area, shell);
    let fullscreen_shell = Rc::clone(shell);
    add_widget_click(shell.player_controls.cover.area.upcast_ref(), move || {
        fullscreen_shell.toggle_fullscreen_player();
    });

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

    let random_shell = Rc::clone(shell);
    shell
        .player_controls
        .random_button
        .connect_clicked(move |_| super::random_play::present_random_play_dialog(&random_shell));

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
        .connect_change_value(move |scale, scroll, value| {
            if seek_shell.state.updating_player_controls.get() {
                return glib::Propagation::Proceed;
            }
            let player = seek_shell.state.player.borrow();
            let duration_seconds = player.duration_seconds;
            if player.current.is_none() || duration_seconds == 0 {
                return glib::Propagation::Stop;
            }
            drop(player);

            let seconds = seekbar_target_seconds(value, duration_seconds);
            preview_player_seek(&seek_shell, seconds);
            scale.set_value(f64::from(seconds));
            if scroll != gtk::ScrollType::Jump {
                commit_player_seek_preview_now(&seek_shell);
            } else {
                queue_player_seek_preview_commit(&seek_shell);
            }
            glib::Propagation::Stop
        });

    let seek_shell = Rc::clone(shell);
    let seek_click = gtk::GestureClick::new();
    seek_click.connect_released(move |_, _, _, _| {
        commit_player_seek_preview_now(&seek_shell);
    });
    shell.player_controls.progress.add_controller(seek_click);

    let seek_shell = Rc::clone(shell);
    let commit_shell = Rc::clone(shell);
    shell.player_controls.waveform.connect_seek(
        move |position| preview_player_seek_fraction(&seek_shell, position),
        move || commit_player_seek_preview_now(&commit_shell),
    );

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

fn connect_bottom_player_volume_resize(shell: &Rc<Shell>) {
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |window, _| {
            resize_shell.apply_bottom_player_width(window.width());
        });

    let window = shell.window.clone();
    let resize_shell = Rc::clone(shell);
    window.connect_realize(move |window| {
        let Some(surface) = window.surface() else {
            return;
        };
        let surface_resize_shell = Rc::clone(&resize_shell);
        surface.connect_width_notify(move |surface| {
            surface_resize_shell.apply_bottom_player_width(surface.width());
        });
        resize_shell.apply_bottom_player_width(surface.width());
    });

    let resize_shell = Rc::clone(shell);
    shell.player_controls.root.add_tick_callback(move |_, _| {
        let width = resize_shell.window.width();
        if width > 0 {
            resize_shell.apply_bottom_player_width(width);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

impl Shell {
    pub(in crate::ui) fn apply_bottom_player_width(&self, player_width: i32) {
        if player_width > 0 {
            let progress_width = bottom_player_progress_width(player_width);
            self.player_controls
                .progress
                .set_width_request(progress_width);
            self.player_controls
                .progress_stack
                .set_size_request(progress_width, BOTTOM_PLAYER_WAVEFORM_HEIGHT);
            self.player_controls
                .waveform
                .widget()
                .set_width_request(progress_width);
            self.player_controls
                .waveform
                .widget()
                .set_content_width(progress_width);
            self.player_controls
                .volume
                .set_width_request(bottom_player_volume_width(player_width));
            self.apply_bottom_player_actions(bottom_player_actions(player_width));
        }
    }

    fn apply_bottom_player_actions(&self, actions: BottomPlayerActions) {
        let player = &self.player_controls;
        player.favorite_button.set_visible(matches!(
            actions,
            BottomPlayerActions::Favorite
                | BottomPlayerActions::Lyrics
                | BottomPlayerActions::Queue
        ));
        player.lyrics_button.set_visible(matches!(
            actions,
            BottomPlayerActions::Lyrics | BottomPlayerActions::Queue
        ));
        player
            .queue_button
            .set_visible(matches!(actions, BottomPlayerActions::Queue));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn player_keep_visible() {
        assert!(super::player_cover_replacement_is_ready(true, false));
        assert!(super::player_cover_replacement_is_ready(false, true));
        assert!(super::player_cover_replacement_is_ready(true, true));
        assert!(!super::player_cover_replacement_is_ready(false, false));
    }

    #[test]
    fn player_scale_narrows() {
        assert_eq!(super::bottom_player_volume_width(2560), 160);
        assert_eq!(super::bottom_player_volume_width(1920), 120);
        assert_eq!(super::bottom_player_volume_width(960), 60);
        assert_eq!(super::bottom_player_volume_width(820), 51);
    }

    #[test]
    fn player_progress_only_narrows_at_compact_widths() {
        assert_eq!(
            super::bottom_player_progress_width(1024),
            super::BOTTOM_PLAYER_PROGRESS_WIDTH
        );
        assert_eq!(
            super::bottom_player_progress_width(864),
            super::BOTTOM_PLAYER_PROGRESS_WIDTH
        );
        assert_eq!(
            super::bottom_player_progress_width(614),
            super::BOTTOM_PLAYER_PROGRESS_MIN_WIDTH
        );
    }

    #[test]
    fn player_restores_actions_by_priority() {
        assert_eq!(
            super::bottom_player_actions(614),
            super::BottomPlayerActions::Favorite
        );
        assert_eq!(
            super::bottom_player_actions(700),
            super::BottomPlayerActions::Favorite
        );
        assert_eq!(
            super::bottom_player_actions(800),
            super::BottomPlayerActions::Lyrics
        );
        assert_eq!(
            super::bottom_player_actions(900),
            super::BottomPlayerActions::Queue
        );
    }

    #[test]
    fn player_now_width() {
        let spec = super::player_wall_spec();

        assert_eq!(spec.horizontal_policy, gtk::PolicyType::External);
        assert_eq!(spec.vertical_policy, gtk::PolicyType::Never);
        assert_eq!(spec.overflow, gtk::Overflow::Hidden);
        assert_eq!(spec.min_content_width, 0);
        assert!(!spec.propagate_natural_width);
        assert!(spec.propagate_natural_height);
    }
}
