use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use ::library::FavoriteItemId;
use adw::prelude::*;
use artwork::CandidateSet;
use domain::{Route, format_duration};
use gtk::glib;
use playback::{PlaybackView, RepeatMode, TransportStatus};
use tracing::info;

use crate::i18n::{msgid, tr};

use super::playback_outputs::present_audio_output_popover;
use super::player_icons::{
    VolumeIcon, audio_output_icon_button, auto_dj_icon_button, lyrics_icon_button,
    play_icon_button, queue_sidebar_button, random_clover_icon_button, repeat_icon_button,
    set_repeat_button_icon, shuffle_icon_button, skip_icon_button, volume_icon_button,
    volume_icon_state,
};
use super::{
    ArtworkTile, MORE_ICON, Shell, THUMB_COVER_SIZE, add_dynamic_link_hover, add_label_click,
    add_widget_click, favorite_button_is_active, favorite_icon_button, icon_button,
    install_current_track_context_menu, present_current_track_context_menu, seekbar_target_seconds,
    set_active_class, set_favorite_button_active,
};

pub(super) const BOTTOM_PLAYER_HEIGHT: i32 = 96;
pub(in crate::ui) const BOTTOM_PLAYER_COVER_SIZE: i32 = 56;
const BOTTOM_PLAYER_EDGE_PADDING: i32 = 8;
const BOTTOM_PLAYER_HORIZONTAL_PADDING: i32 = 0;
const BOTTOM_PLAYER_NOW_PLAYING_SPACING: i32 = 8;
const BOTTOM_PLAYER_TRANSPORT_WIDTH: i32 = 300;
const BOTTOM_PLAYER_PROGRESS_WIDTH: i32 = 320;
const BOTTOM_PLAYER_PROGRESS_MIN_WIDTH: i32 = 140;
const BOTTOM_PLAYER_PROGRESS_MIN_NATURAL_WIDTH: i32 = 220;
const BOTTOM_PLAYER_TIME_LABEL_WIDTH: i32 = 32;
const BOTTOM_PLAYER_BUTTON_ROW_HEIGHT: i32 = 58;
const BOTTOM_PLAYER_SIDE_BUTTON_SIZE: i32 = 50;
const BOTTOM_PLAYER_PLAY_BUTTON_SIZE: i32 = 45;
const BOTTOM_PLAYER_BUTTON_OFFSET_Y: f64 = 3.0;
const BOTTOM_PLAYER_BUTTON_STEP: f64 = 38.0;
const BOTTOM_PLAYER_WAVEFORM_HEIGHT: i32 = 32;
const BOTTOM_PLAYER_ACTION_BUTTON_SIZE: i32 = 34;
const BOTTOM_PLAYER_ACTION_ICON_SIZE: i32 = 20;
const BOTTOM_PLAYER_LYRICS_ICON_SIZE: i32 = 24;
const BOTTOM_PLAYER_VOLUME_ICON_SIZE: i32 = 22;
const BOTTOM_PLAYER_TITLE_MENU_BUTTON_SIZE: i32 = 18;
const BOTTOM_PLAYER_TITLE_MENU_GAP: i32 = 0;
const BOTTOM_PLAYER_IDENTITY_HEIGHT: i32 = 58;
const BOTTOM_PLAYER_TITLE_ROW_HEIGHT: i32 = 20;
const BOTTOM_PLAYER_META_ROW_HEIGHT: i32 = 18;
const BOTTOM_PLAYER_META_CHAR_WIDTH: i32 = 9;
const BOTTOM_PLAYER_META_MIN_CHARS: i32 = 4;
const BOTTOM_PLAYER_ACTION_SPACING: i32 = 0;
const BOTTOM_PLAYER_PROGRESS_SPACING: i32 = 6;
const BOTTOM_PLAYER_VOLUME_SPACING: i32 = 1;
const BOTTOM_PLAYER_VOLUME_MIN_WIDTH: i32 = 48;
const BOTTOM_PLAYER_VOLUME_MAX_WIDTH: i32 = 160;
const BOTTOM_PLAYER_VOLUME_WIDTH_RATIO: f64 = 1.0 / 16.0;
const BOTTOM_PLAYER_RIGHT_EDGE_GAP: i32 = 8;
const BOTTOM_PLAYER_TRANSPORT_CLEARANCE: i32 = 8;
const BOTTOM_PLAYER_TINY_TRANSPORT_WIDTH: i32 = 126;
const BOTTOM_PLAYER_TINY_CONTROL_SPACING: i32 = 2;
const BOTTOM_PLAYER_TINY_CONTROLS_WIDTH: i32 = BOTTOM_PLAYER_TINY_TRANSPORT_WIDTH;
const BOTTOM_PLAYER_TINY_ROW_SPACING: i32 = 6;
const BOTTOM_PLAYER_COMPACT_MIN_WIDTH: i32 = 614;
const BOTTOM_PLAYER_TINY_WIDTH: i32 = BOTTOM_PLAYER_COMPACT_MIN_WIDTH;
const BOTTOM_PLAYER_FULL_PROGRESS_WIDTH: i32 = 864;
const BOTTOM_PLAYER_SHOW_FAVORITE_WIDTH: i32 = 636;
const BOTTOM_PLAYER_SHOW_LYRICS_WIDTH: i32 = 780;
const BOTTOM_PLAYER_SHOW_QUEUE_WIDTH: i32 = BOTTOM_PLAYER_SHOW_LYRICS_WIDTH;
const SEEK_PREVIEW_COMMIT_DELAY: Duration = Duration::from_millis(100);
const SEEK_PREVIEW_SETTLE_WINDOW: Duration = Duration::from_millis(1_000);
const SEEK_PREVIEW_TOLERANCE_MILLIS: u64 = 1_500;
const VOLUME_PERSIST_DELAY: Duration = Duration::from_millis(250);

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
    pub(super) title: gtk::Label,
    pub(super) menu_button: gtk::Button,
    pub(super) artist: gtk::Label,
    pub(super) album: gtk::Label,
    now_playing_wall: gtk::Box,
    now_playing: gtk::Box,
    identity: gtk::Box,
    title_row: gtk::Box,
    tiny_row: gtk::Box,
    tiny_controls: gtk::Box,
    tiny_layout: Cell<bool>,
    transport: gtk::Box,
    transport_slot: gtk::Box,
    transport_buttons: gtk::Fixed,
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
    progress_row: gtk::Box,
    pub(super) elapsed: gtk::Label,
    pub(super) progress_stack: gtk::Stack,
    pub(super) progress: gtk::Scale,
    pub(super) waveform: WaveformSeekBar,
    pub(super) waveform_key: RefCell<Option<String>>,
    pub(super) waveform_peak_count: Cell<usize>,
    pub(super) duration: gtk::Label,
    actions: gtk::Box,
    pub(super) mute_button: gtk::Button,
    pub(super) mute_icon: gtk::DrawingArea,
    mute_icon_state: Rc<Cell<VolumeIcon>>,
    pub(super) volume: gtk::Scale,
    pub(super) audio_output_button: gtk::Button,
}

struct NowPlayingControls {
    root: gtk::Box,
    cover: ArtworkTile,
    title: gtk::Label,
    identity: gtk::Box,
    title_row: gtk::Box,
    menu_button: gtk::Button,
    artist: gtk::Label,
    album: gtk::Label,
}

struct TransportControls {
    root: gtk::Box,
    buttons: gtk::Fixed,
    random_button: gtk::Button,
    previous_button: gtk::Button,
    play_button: gtk::Button,
    play_icon: gtk::DrawingArea,
    play_icon_playing: Rc<Cell<bool>>,
    next_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    dj_button: gtk::Button,
    progress_row: gtk::Box,
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
    mute_icon: gtk::DrawingArea,
    mute_icon_state: Rc<Cell<VolumeIcon>>,
    volume: gtk::Scale,
    audio_output_button: gtk::Button,
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
        let seek_label = crate::i18n::tr(msgid("Seek playback"));
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
    pub(in crate::ui) fn sync_bottom_player_favorite(&self) {
        let player = self.state.player.borrow();
        let current = player
            .as_ref()
            .and_then(|player| player.transport.current.as_ref());
        let favorite = current.is_some_and(|entry| {
            self.projected_track_favorite(&entry.track.id, entry.track.favorite)
        });
        set_favorite_button_active(&self.player_controls.favorite_button, favorite);
    }

    pub(in crate::ui) fn update_bottom_player(self: &Rc<Self>) {
        let player = self.state.player.borrow().clone();
        let current = player
            .as_ref()
            .and_then(|player| player.transport.current.as_ref());
        let source_id = player
            .as_ref()
            .map(|player| player.transport.source_id.clone());
        let state = player
            .as_ref()
            .map(|player| player.transport.state)
            .unwrap_or(TransportStatus::Stopped);
        let duration_seconds = player
            .as_ref()
            .map(|player| {
                (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32
            })
            .unwrap_or_default();
        let position_seconds = player
            .as_ref()
            .map(|player| {
                (player.transport.position_millis / 1_000).min(u64::from(u32::MAX)) as u32
            })
            .unwrap_or_default();
        let repeat_mode = player
            .as_ref()
            .map(|player| player.controls.repeat_mode)
            .unwrap_or(RepeatMode::Off);
        let controls = &self.player_controls;
        self.state.updating_player_controls.set(true);

        let cover_seed = current
            .map(|entry| entry.track.duration_seconds)
            .unwrap_or(42);
        controls.cover.set_seed(cover_seed);
        if let (Some(entry), Some(source_id)) = (current, source_id.as_ref()) {
            self.bind_playback_artwork_tile(
                &controls.cover,
                source_id,
                CandidateSet::track(&entry.track),
                cover_seed,
                BOTTOM_PLAYER_COVER_SIZE,
                THUMB_COVER_SIZE,
            );
        } else {
            self.clear_artwork_tile(&controls.cover);
        }

        controls.play_icon_playing.set(matches!(
            state,
            TransportStatus::Playing | TransportStatus::Buffering
        ));
        controls.play_icon.queue_draw();
        controls
            .play_button
            .set_tooltip_text(Some(&playback_state_label(state)));

        let title = current
            .map(|entry| entry.track.title.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| crate::i18n::tr("Nothing playing"));
        let artist = current
            .map(|entry| entry.track.artist.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| crate::i18n::tr("Queue a track to begin"));
        let album = current
            .map(|entry| entry.track.album.as_str())
            .unwrap_or("");
        controls.title.set_text(&title);
        controls.artist.set_text(&artist);
        controls.album.set_text(album);
        controls.title.set_sensitive(current.is_some());
        controls.menu_button.set_sensitive(current.is_some());
        controls
            .artist
            .set_sensitive(current.is_some_and(|entry| !entry.track.artist.is_empty()));
        controls
            .album
            .set_sensitive(current.is_some_and(|entry| !entry.track.album.is_empty()));

        let shuffle_enabled = player
            .as_ref()
            .is_some_and(|player| player.controls.shuffle_enabled);
        let auto_dj_enabled = player
            .as_ref()
            .is_some_and(|player| player.controls.auto_dj_enabled);
        set_active_class(&controls.shuffle_button, shuffle_enabled);
        set_active_class(&controls.repeat_button, repeat_mode != RepeatMode::Off);
        set_repeat_button_icon(&controls.repeat_button, repeat_mode);
        set_active_class(&controls.dj_button, auto_dj_enabled);
        controls
            .dj_button
            .set_tooltip_text(Some(&if auto_dj_enabled {
                tr("Auto DJ on")
            } else {
                tr("Auto DJ")
            }));
        controls.favorite_button.set_sensitive(current.is_some());
        controls
            .repeat_button
            .set_tooltip_text(Some(&repeat_label(repeat_mode)));

        let preview_seconds = self.state.seek_preview_seconds.get();
        let displayed_seconds = preview_seconds.unwrap_or(position_seconds);
        controls
            .elapsed
            .set_text(&format_duration(displayed_seconds));
        let max = f64::from(duration_seconds.max(1));
        controls.progress.set_range(0.0, max);
        controls
            .progress
            .set_value(f64::from(displayed_seconds.min(duration_seconds)));
        let position_fraction = if duration_seconds == 0 {
            0.0
        } else {
            f64::from(displayed_seconds.min(duration_seconds)) / f64::from(duration_seconds)
        };
        controls.waveform.set_position_fraction(position_fraction);
        let waveform_enabled = self.state.settings.borrow().seekbar_waveform_enabled;
        let waveform = self.state.waveform.borrow();
        let waveform_key = waveform_enabled.then(|| waveform.key.clone()).flatten();
        let waveform_peaks = waveform_enabled
            .then(|| waveform.peaks.as_deref().map(Vec::as_slice))
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
            .set_text(&format_duration(duration_seconds));

        let (muted, volume) = player
            .as_ref()
            .map(|player| (player.controls.muted, player.controls.volume))
            .unwrap_or((false, 1.0));
        let volume_icon = volume_icon_state(muted, volume);
        if controls.mute_icon_state.get() != volume_icon {
            controls.mute_icon_state.set(volume_icon);
            controls.mute_icon.queue_draw();
        }
        controls.volume.set_value(volume);
        self.state.updating_player_controls.set(false);
    }

    pub(in crate::ui) fn maybe_clear_player_seek_preview(
        &self,
        player: &PlaybackView,
        track_changed: bool,
    ) {
        let Some(target_seconds) = self.state.seek_preview_seconds.get() else {
            return;
        };
        if track_changed
            || player.transport.current.is_none()
            || seek_preview_matches_position(target_seconds, player.transport.position_millis)
        {
            self.clear_player_seek_preview();
        }
    }

    fn clear_player_seek_preview(&self) {
        self.state.seek_preview_seconds.set(None);
    }
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
        menu_button,
        artist,
        album,
        identity,
        title_row,
    } = build_now_playing_controls();

    let TransportControls {
        root: transport,
        buttons: transport_buttons,
        random_button,
        previous_button,
        play_button,
        play_icon,
        play_icon_playing,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        progress_row,
        elapsed,
        progress_stack,
        progress,
        waveform,
        duration,
    } = build_transport_controls();

    let now_playing_wall = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    now_playing_wall.add_css_class("player-now-playing-wall");
    now_playing_wall.set_hexpand(true);
    now_playing_wall.set_vexpand(false);
    now_playing_wall.set_valign(gtk::Align::Center);
    now_playing_wall.set_width_request(1);
    now_playing_wall.set_overflow(gtk::Overflow::Hidden);
    now_playing_wall.append(&now_playing);

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
        mute_icon_state,
        volume,
        audio_output_button,
    } = build_player_action_controls();

    let transport_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    transport_slot.set_width_request(BOTTOM_PLAYER_TRANSPORT_WIDTH);
    transport_slot.set_halign(gtk::Align::Center);
    transport_slot.set_valign(gtk::Align::Center);
    transport_slot.append(&transport);

    let tiny_controls = gtk::Box::new(
        gtk::Orientation::Horizontal,
        BOTTOM_PLAYER_TINY_CONTROL_SPACING,
    );
    tiny_controls.set_halign(gtk::Align::End);
    tiny_controls.set_valign(gtk::Align::Center);
    tiny_controls.set_width_request(BOTTOM_PLAYER_TINY_CONTROLS_WIDTH);

    let tiny_row = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_TINY_ROW_SPACING);
    tiny_row.set_hexpand(true);
    tiny_row.set_halign(gtk::Align::Fill);
    tiny_row.set_valign(gtk::Align::Center);
    tiny_row.set_width_request(1);
    tiny_row.append(&tiny_controls);

    root.set_start_widget(Some(&now_playing_wall));
    root.set_center_widget(Some(&transport_slot));
    root.set_end_widget(Some(&actions));

    PlayerControls {
        root,
        cover,
        title,
        menu_button,
        artist,
        album,
        now_playing_wall,
        now_playing,
        identity,
        title_row,
        tiny_row,
        tiny_controls,
        tiny_layout: Cell::new(false),
        transport,
        transport_slot,
        transport_buttons,
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
        progress_row,
        elapsed,
        progress_stack,
        progress,
        waveform,
        waveform_key: RefCell::new(None),
        waveform_peak_count: Cell::new(0),
        duration,
        actions,
        mute_button,
        mute_icon,
        mute_icon_state,
        volume,
        audio_output_button,
    }
}

fn build_now_playing_controls() -> NowPlayingControls {
    let root = gtk::Box::new(
        gtk::Orientation::Horizontal,
        BOTTOM_PLAYER_NOW_PLAYING_SPACING,
    );
    root.add_css_class("player-now-playing");
    root.set_hexpand(true);
    root.set_width_request(1);
    root.set_halign(gtk::Align::Fill);
    root.set_valign(gtk::Align::Center);
    root.set_margin_start(BOTTOM_PLAYER_HORIZONTAL_PADDING);

    let cover = ArtworkTile::new(BOTTOM_PLAYER_COVER_SIZE, 42);
    cover.area.set_valign(gtk::Align::Center);
    cover.area.set_cursor_from_name(Some("pointer"));
    let cover_label = tr("Open fullscreen player");
    cover.area.set_tooltip_text(Some(&cover_label));
    cover
        .area
        .update_property(&[gtk::accessible::Property::Label(&cover_label)]);
    root.append(&cover.area);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 1);
    identity.add_css_class("player-identity");
    identity.set_hexpand(true);
    identity.set_width_request(1);
    identity.set_halign(gtk::Align::Fill);
    identity.set_valign(gtk::Align::Center);
    let title = player_link("player-title");
    let menu_button = icon_button(MORE_ICON, "More actions");
    menu_button.add_css_class("player-title-menu-button");
    menu_button.set_size_request(
        BOTTOM_PLAYER_TITLE_MENU_BUTTON_SIZE,
        BOTTOM_PLAYER_TITLE_MENU_BUTTON_SIZE,
    );
    menu_button.set_valign(gtk::Align::Center);
    menu_button.set_sensitive(false);
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_TITLE_MENU_GAP);
    title_row.add_css_class("player-title-row");
    title_row.set_halign(gtk::Align::Start);
    title_row.set_valign(gtk::Align::Center);
    title_row.set_height_request(BOTTOM_PLAYER_TITLE_ROW_HEIGHT);
    let (title_chars, meta_chars) = bottom_player_metadata_chars(BOTTOM_PLAYER_COMPACT_MIN_WIDTH);
    title.set_height_request(BOTTOM_PLAYER_TITLE_ROW_HEIGHT);
    title.set_max_width_chars(title_chars);
    title_row.append(&title);
    title_row.append(&menu_button);
    let artist = player_link("player-primary");
    let album = player_link("player-primary");
    artist.set_height_request(BOTTOM_PLAYER_META_ROW_HEIGHT);
    artist.set_max_width_chars(meta_chars);
    album.set_height_request(BOTTOM_PLAYER_META_ROW_HEIGHT);
    album.set_max_width_chars(meta_chars);
    artist.set_hexpand(true);
    artist.set_width_request(1);
    album.set_hexpand(true);
    album.set_width_request(1);
    identity.append(&title_row);
    identity.append(&artist);
    identity.append(&album);
    let identity_slot = bottom_player_identity_slot(&identity);
    root.append(&identity_slot);

    NowPlayingControls {
        root,
        cover,
        title,
        identity,
        title_row,
        menu_button,
        artist,
        album,
    }
}

fn bottom_player_identity_slot(identity: &gtk::Box) -> gtk::ScrolledWindow {
    let slot = gtk::ScrolledWindow::new();
    slot.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    slot.set_overflow(gtk::Overflow::Hidden);
    slot.set_width_request(1);
    slot.set_height_request(BOTTOM_PLAYER_IDENTITY_HEIGHT);
    slot.set_min_content_width(0);
    slot.set_max_content_width(1);
    slot.set_min_content_height(BOTTOM_PLAYER_IDENTITY_HEIGHT);
    slot.set_max_content_height(BOTTOM_PLAYER_IDENTITY_HEIGHT);
    slot.set_propagate_natural_width(false);
    slot.set_propagate_natural_height(false);
    slot.set_hexpand(true);
    slot.set_halign(gtk::Align::Fill);
    slot.set_valign(gtk::Align::Center);
    slot.set_child(Some(identity));
    slot
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

    put_transport_button(
        &buttons,
        &dj_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        -3.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &shuffle_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        -2.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &previous_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        -1.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &play_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        0.0,
        BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &next_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        1.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &repeat_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        2.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );
    put_transport_button(
        &buttons,
        &random_button,
        BOTTOM_PLAYER_TRANSPORT_WIDTH,
        3.0,
        BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
    );

    let progress_row = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_PROGRESS_SPACING);
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
        buttons,
        random_button,
        previous_button,
        play_button,
        play_icon,
        play_icon_playing,
        next_button,
        shuffle_button,
        repeat_button,
        dj_button,
        progress_row,
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
    queue_icon.set_content_width(BOTTOM_PLAYER_ACTION_ICON_SIZE);
    queue_icon.set_content_height(BOTTOM_PLAYER_ACTION_ICON_SIZE);
    let (lyrics_button, lyrics_icon, lyrics_icon_open) = lyrics_icon_button("Hide lyrics");
    lyrics_icon.set_content_width(BOTTOM_PLAYER_LYRICS_ICON_SIZE);
    lyrics_icon.set_content_height(BOTTOM_PLAYER_LYRICS_ICON_SIZE);
    lyrics_icon.set_margin_top(1);
    configure_player_action_button(&lyrics_button);
    root.append(&lyrics_button);
    configure_player_action_button(&queue_button);
    root.append(&queue_button);
    let favorite_button = favorite_icon_button("Favorite");
    favorite_button.add_css_class("player-favorite-button");
    set_button_image_pixel_size(&favorite_button, BOTTOM_PLAYER_ACTION_ICON_SIZE);
    configure_player_action_button(&favorite_button);
    root.append(&favorite_button);

    let volume_group = gtk::Box::new(gtk::Orientation::Horizontal, BOTTOM_PLAYER_VOLUME_SPACING);
    volume_group.set_valign(gtk::Align::Center);
    let audio_output_button = audio_output_icon_button("Audio output");
    audio_output_button.add_css_class("player-audio-output-button");
    configure_player_action_button(&audio_output_button);
    volume_group.append(&audio_output_button);
    let (mute_button, mute_icon, mute_icon_state) = volume_icon_button("Mute");
    mute_icon.set_content_width(BOTTOM_PLAYER_VOLUME_ICON_SIZE);
    mute_icon.set_content_height(BOTTOM_PLAYER_VOLUME_ICON_SIZE);
    mute_button.add_css_class("player-mute-button");
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
        mute_icon_state,
        volume,
        audio_output_button,
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

fn set_button_image_pixel_size(button: &gtk::Button, size: i32) {
    if let Some(image) = button
        .child()
        .and_then(|child| child.downcast::<gtk::Image>().ok())
    {
        image.set_pixel_size(size);
    }
}

fn bottom_player_volume_width(player_width: i32) -> i32 {
    let right_side_width = (player_width - BOTTOM_PLAYER_TRANSPORT_WIDTH) / 2;
    let visible_action_count = match bottom_player_actions(player_width) {
        BottomPlayerActions::Volume => 0,
        BottomPlayerActions::Favorite => 1,
        BottomPlayerActions::Lyrics => 2,
        BottomPlayerActions::Queue => 3,
    };
    let fixed_action_count = visible_action_count + 2;
    let action_width_without_volume = BOTTOM_PLAYER_ACTION_BUTTON_SIZE * fixed_action_count
        + BOTTOM_PLAYER_ACTION_SPACING * visible_action_count
        + BOTTOM_PLAYER_VOLUME_SPACING * 2
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

fn bottom_player_content_width(player_width: i32) -> i32 {
    (player_width - BOTTOM_PLAYER_EDGE_PADDING * 2).max(1)
}

fn bottom_player_progress_row_width(player_width: i32) -> i32 {
    let progress_width =
        bottom_player_progress_width(player_width).max(BOTTOM_PLAYER_PROGRESS_MIN_NATURAL_WIDTH);
    progress_width + BOTTOM_PLAYER_TIME_LABEL_WIDTH * 2 + BOTTOM_PLAYER_PROGRESS_SPACING * 2
}

fn bottom_player_transport_budget(player_width: i32) -> i32 {
    if bottom_player_tiny(player_width) {
        return BOTTOM_PLAYER_TINY_CONTROLS_WIDTH;
    }

    BOTTOM_PLAYER_TRANSPORT_WIDTH.max(bottom_player_progress_row_width(player_width))
}

fn bottom_player_now_playing_budget(player_width: i32) -> i32 {
    let content_width = bottom_player_content_width(player_width);
    if bottom_player_tiny(player_width) {
        return content_width
            - BOTTOM_PLAYER_TINY_CONTROLS_WIDTH
            - BOTTOM_PLAYER_TINY_ROW_SPACING
            - BOTTOM_PLAYER_TRANSPORT_CLEARANCE;
    }

    (content_width - bottom_player_transport_budget(player_width)) / 2
}

fn bottom_player_metadata_chars(player_width: i32) -> (i32, i32) {
    let side_width = bottom_player_now_playing_budget(player_width);
    let text_width = side_width - BOTTOM_PLAYER_COVER_SIZE - BOTTOM_PLAYER_NOW_PLAYING_SPACING;
    let title_width =
        text_width - BOTTOM_PLAYER_TITLE_MENU_BUTTON_SIZE - BOTTOM_PLAYER_TITLE_MENU_GAP;
    let title_chars =
        (title_width / BOTTOM_PLAYER_META_CHAR_WIDTH).max(BOTTOM_PLAYER_META_MIN_CHARS);
    let meta_chars = (text_width / BOTTOM_PLAYER_META_CHAR_WIDTH).max(BOTTOM_PLAYER_META_MIN_CHARS);

    (title_chars, meta_chars)
}

fn apply_bottom_player_metadata_widths(player: &PlayerControls, player_width: i32) {
    let (title_chars, meta_chars) = bottom_player_metadata_chars(player_width);
    player.title.set_max_width_chars(title_chars);
    player.artist.set_max_width_chars(meta_chars);
    player.album.set_max_width_chars(meta_chars);
}

fn bottom_player_widget_widths(widget: &impl glib::object::IsA<gtk::Widget>) -> (i32, i32, i32) {
    let widget = widget.as_ref();
    let (minimum, natural, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    (widget.width(), minimum, natural)
}

fn log_bottom_player_layout_probe(stage: &'static str, player_width: i32, player: &PlayerControls) {
    if std::env::var_os("RUFIN_DEBUG_LAYOUT").is_none() {
        return;
    }

    let (title_chars, meta_chars) = bottom_player_metadata_chars(player_width);
    info!(
        stage,
        player_width,
        content_width = bottom_player_content_width(player_width),
        tiny = bottom_player_tiny(player_width),
        transport_budget = bottom_player_transport_budget(player_width),
        now_playing_budget = bottom_player_now_playing_budget(player_width),
        progress_width = bottom_player_progress_width(player_width),
        progress_row_budget = bottom_player_progress_row_width(player_width),
        title_chars,
        meta_chars,
        title_len = player.title.text().chars().count(),
        artist_len = player.artist.text().chars().count(),
        album_len = player.album.text().chars().count(),
        title_hexpand = player.title.hexpands(),
        title_row_halign = ?player.title_row.halign(),
        root = ?bottom_player_widget_widths(&player.root),
        now_playing_wall = ?bottom_player_widget_widths(&player.now_playing_wall),
        now_playing = ?bottom_player_widget_widths(&player.now_playing),
        identity = ?bottom_player_widget_widths(&player.identity),
        title_row = ?bottom_player_widget_widths(&player.title_row),
        title = ?bottom_player_widget_widths(&player.title),
        menu = ?bottom_player_widget_widths(&player.menu_button),
        artist = ?bottom_player_widget_widths(&player.artist),
        album = ?bottom_player_widget_widths(&player.album),
        transport_slot = ?bottom_player_widget_widths(&player.transport_slot),
        transport = ?bottom_player_widget_widths(&player.transport),
        actions = ?bottom_player_widget_widths(&player.actions),
        queue = ?bottom_player_widget_widths(&player.queue_button),
        lyrics = ?bottom_player_widget_widths(&player.lyrics_button),
        favorite = ?bottom_player_widget_widths(&player.favorite_button),
        mute = ?bottom_player_widget_widths(&player.mute_button),
        tiny_row = ?bottom_player_widget_widths(&player.tiny_row),
        tiny_controls = ?bottom_player_widget_widths(&player.tiny_controls),
        progress_row = ?bottom_player_widget_widths(&player.progress_row),
        progress_stack = ?bottom_player_widget_widths(&player.progress_stack),
        volume = ?bottom_player_widget_widths(&player.volume),
        "bottom player layout probe"
    );
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

fn bottom_player_tiny(player_width: i32) -> bool {
    player_width < BOTTOM_PLAYER_TINY_WIDTH
}

fn put_transport_button(
    buttons: &gtk::Fixed,
    button: &gtk::Button,
    width: i32,
    slot: f64,
    size: i32,
) {
    let (x, y) = transport_button_position(width, slot, size);
    buttons.put(button, x, y);
}

fn move_transport_button(
    buttons: &gtk::Fixed,
    button: &gtk::Button,
    width: i32,
    slot: f64,
    size: i32,
) {
    let (x, y) = transport_button_position(width, slot, size);
    buttons.move_(button, x, y);
}

fn transport_button_position(width: i32, slot: f64, size: i32) -> (f64, f64) {
    let center_x = f64::from(width) / 2.0 + BOTTOM_PLAYER_BUTTON_STEP * slot;
    let radius = f64::from(size) / 2.0;
    let y = f64::from(BOTTOM_PLAYER_BUTTON_ROW_HEIGHT - size) / 2.0 + BOTTOM_PLAYER_BUTTON_OFFSET_Y;
    (center_x - radius, y)
}

fn player_link(css_class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("player-link");
    label.add_css_class(css_class);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_single_line_mode(true);
    label.set_lines(1);
    label.set_width_chars(1);
    label.set_halign(gtk::Align::Fill);
    label.set_valign(gtk::Align::Center);
    label.set_hexpand(false);
    label.set_yalign(0.5);
    label.set_cursor_from_name(Some("pointer"));
    add_dynamic_link_hover(label.upcast_ref(), &label);
    label
}

fn playback_state_label(state: TransportStatus) -> String {
    match state {
        TransportStatus::Stopped | TransportStatus::Failed => tr("Play"),
        TransportStatus::Paused => tr("Resume"),
        TransportStatus::Resolving | TransportStatus::Buffering | TransportStatus::Playing => {
            tr("Pause")
        }
    }
}

fn repeat_label(repeat_mode: RepeatMode) -> String {
    match repeat_mode {
        RepeatMode::Off => tr("Repeat off"),
        RepeatMode::One => tr("Repeat one"),
        RepeatMode::All => tr("Repeat all"),
    }
}

fn preview_player_seek(shell: &Rc<Shell>, seconds: u32) {
    shell.state.seek_preview_seconds.set(Some(seconds));
    shell.player_controls.progress.set_value(f64::from(seconds));
    shell
        .player_controls
        .elapsed
        .set_text(&format_duration(seconds));
    let duration_seconds = shell
        .state
        .player
        .borrow()
        .as_ref()
        .map(|player| (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32)
        .unwrap_or_default();
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
    let duration_seconds = {
        let player = shell.state.player.borrow();
        let Some(player) = player.as_ref() else {
            return;
        };
        let duration_seconds =
            (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32;
        if player.transport.current.is_none() || duration_seconds == 0 {
            return;
        }
        duration_seconds
    };
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
    let menu_shell = Rc::clone(shell);
    shell
        .player_controls
        .menu_button
        .connect_clicked(move |button| present_current_track_context_menu(button, &menu_shell));
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

    let feedback_shell = Rc::clone(shell);
    shell
        .player_controls
        .shuffle_button
        .connect_clicked(move |_| {
            let Some(enabled) = feedback_shell
                .state
                .player
                .borrow()
                .as_ref()
                .map(|player| !player.controls.shuffle_enabled)
            else {
                return;
            };
            feedback_shell.controller.toggle_shuffle();
            feedback_shell.show_control_feedback_toast(if enabled {
                tr("Shuffle on")
            } else {
                tr("Shuffle off")
            });
        });

    let feedback_shell = Rc::clone(shell);
    shell
        .player_controls
        .repeat_button
        .connect_clicked(move |_| {
            let Some(repeat_mode) = feedback_shell
                .state
                .player
                .borrow()
                .as_ref()
                .map(|player| player.controls.repeat_mode)
            else {
                return;
            };
            let title = match repeat_mode {
                RepeatMode::Off => tr("Repeat all"),
                RepeatMode::All => tr("Repeat one"),
                RepeatMode::One => tr("Repeat off"),
            };
            feedback_shell.controller.cycle_repeat();
            feedback_shell.show_control_feedback_toast(title);
        });

    let feedback_shell = Rc::clone(shell);
    shell.player_controls.dj_button.connect_clicked(move |_| {
        let Some(enabled) = feedback_shell
            .state
            .player
            .borrow()
            .as_ref()
            .map(|player| !player.controls.auto_dj_enabled)
        else {
            return;
        };
        feedback_shell.controller.toggle_auto_dj();
        feedback_shell.show_control_feedback_toast(if enabled {
            tr("Auto DJ on")
        } else {
            tr("Auto DJ off")
        });
    });

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

    let favorite_shell = Rc::clone(shell);
    shell
        .player_controls
        .favorite_button
        .connect_clicked(move |button| {
            let Some(entry) = favorite_shell
                .state
                .player
                .borrow()
                .as_ref()
                .and_then(|player| player.transport.current.clone())
            else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                FavoriteItemId::Track(entry.track.id.clone()),
                favorite,
                Some(button),
            );
        });

    let title_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.title, move || {
        let Some(entry) = title_shell
            .state
            .player
            .borrow()
            .as_ref()
            .and_then(|player| player.transport.current.clone())
        else {
            return;
        };
        title_shell.navigate(Route::AlbumDetail(entry.track.album_id.clone()));
    });

    let artist_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.artist, move || {
        let Some(entry) = artist_shell
            .state
            .player
            .borrow()
            .as_ref()
            .and_then(|player| player.transport.current.clone())
        else {
            return;
        };
        if let Some(artist_id) = entry.track.artist_id.clone() {
            artist_shell.navigate(Route::ArtistDetail(artist_id));
        }
    });

    let album_shell = Rc::clone(shell);
    add_label_click(&shell.player_controls.album, move || {
        let Some(entry) = album_shell
            .state
            .player
            .borrow()
            .as_ref()
            .and_then(|player| player.transport.current.clone())
        else {
            return;
        };
        album_shell.navigate(Route::AlbumDetail(entry.track.album_id.clone()));
    });

    let mute_shell = Rc::clone(shell);
    shell.player_controls.mute_button.connect_clicked(move |_| {
        let Some(muted) = mute_shell
            .state
            .player
            .borrow()
            .as_ref()
            .map(|player| !player.controls.muted)
        else {
            return;
        };
        mute_shell.apply_user_muted(muted);
    });
    let output_shell = Rc::clone(shell);
    shell
        .player_controls
        .audio_output_button
        .connect_clicked(move |button| {
            present_audio_output_popover(button, &output_shell, gtk::PositionType::Top, None);
        });
    let seek_shell = Rc::clone(shell);
    shell
        .player_controls
        .progress
        .connect_change_value(move |scale, scroll, value| {
            if seek_shell.state.updating_player_controls.get() {
                return glib::Propagation::Proceed;
            }
            let duration_seconds = {
                let player = seek_shell.state.player.borrow();
                let Some(player) = player.as_ref() else {
                    return glib::Propagation::Stop;
                };
                let duration_seconds =
                    (player.transport.duration_millis / 1_000).min(u64::from(u32::MAX)) as u32;
                if player.transport.current.is_none() || duration_seconds == 0 {
                    return glib::Propagation::Stop;
                }
                duration_seconds
            };

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
            volume_shell.apply_user_volume(scale.value());
        });
}

impl Shell {
    pub(super) fn apply_user_volume(self: &Rc<Self>, volume: f64) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.state.settings.borrow_mut().playback.volume = volume;
        self.controller.set_volume(volume);
        if let Some(source) = self.state.volume_persist_source.borrow_mut().take() {
            source.remove();
        }
        let shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(VOLUME_PERSIST_DELAY, move || {
            *shell.state.volume_persist_source.borrow_mut() = None;
            shell.controller.persist_volume(volume);
        });
        *self.state.volume_persist_source.borrow_mut() = Some(source);
    }

    pub(super) fn apply_user_muted(&self, muted: bool) {
        self.state.settings.borrow_mut().playback.muted = muted;
        self.controller.set_muted(muted);
    }
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
            let root_width = self.player_controls.root.width();
            let player_width = if root_width > 1 {
                root_width + BOTTOM_PLAYER_EDGE_PADDING * 2
            } else {
                player_width
            };
            let progress_width = bottom_player_progress_width(player_width);
            apply_bottom_player_metadata_widths(&self.player_controls, player_width);
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
            self.apply_bottom_player_tiny(bottom_player_tiny(player_width));
            self.apply_bottom_player_actions(bottom_player_actions(player_width));
            log_bottom_player_layout_probe(
                "apply_bottom_player_width",
                player_width,
                &self.player_controls,
            );
        }
    }

    fn apply_bottom_player_tiny(&self, tiny: bool) {
        let player = &self.player_controls;
        let transport_width = if tiny {
            BOTTOM_PLAYER_TINY_TRANSPORT_WIDTH
        } else {
            BOTTOM_PLAYER_TRANSPORT_WIDTH
        };
        player.transport.set_width_request(transport_width);
        player.transport_slot.set_width_request(transport_width);
        player
            .transport_buttons
            .set_size_request(transport_width, BOTTOM_PLAYER_BUTTON_ROW_HEIGHT);
        player.tiny_row.set_width_request(1);
        player.now_playing_wall.set_width_request(1);
        player.transport_slot.set_halign(if tiny {
            gtk::Align::End
        } else {
            gtk::Align::Center
        });
        move_transport_button(
            &player.transport_buttons,
            &player.previous_button,
            transport_width,
            -1.0,
            BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
        );
        move_transport_button(
            &player.transport_buttons,
            &player.play_button,
            transport_width,
            0.0,
            BOTTOM_PLAYER_PLAY_BUTTON_SIZE,
        );
        move_transport_button(
            &player.transport_buttons,
            &player.next_button,
            transport_width,
            1.0,
            BOTTOM_PLAYER_SIDE_BUTTON_SIZE,
        );
        player.dj_button.set_visible(!tiny);
        player.shuffle_button.set_visible(!tiny);
        player.repeat_button.set_visible(!tiny);
        player.random_button.set_visible(!tiny);
        player.progress_row.set_visible(!tiny);
        player.actions.set_visible(!tiny);
        if player.tiny_layout.replace(tiny) == tiny {
            return;
        }
        if tiny {
            player.root.set_start_widget(None::<&gtk::Widget>);
            player.root.set_center_widget(None::<&gtk::Widget>);
            player.root.set_end_widget(None::<&gtk::Widget>);
            player.tiny_row.prepend(&player.now_playing_wall);
            player.tiny_controls.prepend(&player.transport_slot);
            player.root.set_start_widget(Some(&player.tiny_row));
        } else {
            player.root.set_start_widget(None::<&gtk::Widget>);
            player.tiny_row.remove(&player.now_playing_wall);
            player.tiny_controls.remove(&player.transport_slot);
            player.root.set_start_widget(Some(&player.now_playing_wall));
            player.root.set_center_widget(Some(&player.transport_slot));
            player.root.set_end_widget(Some(&player.actions));
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
    fn player_metadata_budget_tracks_layout_width() {
        let (tiny_title, tiny_meta) = super::bottom_player_metadata_chars(450);
        assert!(tiny_title > 20);
        assert!(tiny_meta >= tiny_title);

        let (compact_title, compact_meta) = super::bottom_player_metadata_chars(614);
        assert!(compact_meta >= compact_title);

        let (narrow_title, narrow_meta) = super::bottom_player_metadata_chars(643);
        assert!(narrow_title >= 9);
        assert!(narrow_title > compact_title);
        assert!(narrow_meta >= narrow_title);

        let (normal_title, normal_meta) = super::bottom_player_metadata_chars(788);
        assert!(normal_title > narrow_title);
        assert!(normal_meta >= normal_title);

        let (wide_title, wide_meta) = super::bottom_player_metadata_chars(960);
        assert!(wide_title > normal_title);
        assert!(wide_meta >= wide_title);
    }

    #[test]
    fn player_restores_actions_by_priority() {
        assert_eq!(
            super::bottom_player_actions(614),
            super::BottomPlayerActions::Volume
        );
        assert_eq!(
            super::bottom_player_actions(635),
            super::BottomPlayerActions::Volume
        );
        assert_eq!(
            super::bottom_player_actions(636),
            super::BottomPlayerActions::Favorite
        );
        assert_eq!(
            super::bottom_player_actions(700),
            super::BottomPlayerActions::Favorite
        );
        assert_eq!(
            super::bottom_player_actions(800),
            super::BottomPlayerActions::Queue
        );
        assert_eq!(
            super::bottom_player_actions(900),
            super::BottomPlayerActions::Queue
        );
    }

    #[test]
    fn player_enters_tiny_mode_until_compact_width() {
        assert!(super::bottom_player_tiny(450));
        assert!(super::bottom_player_tiny(613));
        assert!(!super::bottom_player_tiny(614));
    }

    #[test]
    fn player_volume_zero_uses_muted_icon() {
        assert_eq!(
            super::volume_icon_state(false, 0.0),
            super::VolumeIcon::Muted
        );
        assert_eq!(
            super::volume_icon_state(false, 0.01),
            super::VolumeIcon::High
        );
        assert_eq!(
            super::volume_icon_state(true, 1.0),
            super::VolumeIcon::Muted
        );
    }
}
