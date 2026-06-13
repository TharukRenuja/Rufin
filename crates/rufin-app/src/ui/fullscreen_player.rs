use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use rufin_core::{EQUALIZER_BAND_COUNT, EqualizerSettings, QueueEntry};
use rufin_playback::PlaybackState;

use crate::i18n::tr;
use crate::lyrics::LyricsPane;
use crate::ui::{
    connect_equalizer_scale_commit, cover_fetch_size_for_display, equalizer_band_label_parts,
    equalizer_band_title, equalizer_default_preset_bands, equalizer_preset_bands,
    equalizer_preset_names, equalizer_selected_preset,
};

use super::{
    ArtworkTile, CoverDecodePriority, GRID_COVER_SIZE, Shell, THUMB_COVER_SIZE,
    cover_artwork_id_for_key, cover_request_id_for_key, icon_button, player::BOTTOM_PLAYER_HEIGHT,
    player_icons::lyrics_icon_area,
};

const FULLSCREEN_PLAYER_TRANSITION_MS: u32 = 320;
const FULLSCREEN_PLAYER_TRANSITION_US: i64 = FULLSCREEN_PLAYER_TRANSITION_MS as i64 * 1_000;
const FULLSCREEN_PLAYER_DEFERRED_UPDATE_MS: u64 = 16;
const FULLSCREEN_PLAYER_DEFERRED_COVER_MS: u64 = 80;
const FULLSCREEN_PLAYER_DEFAULT_COVER_SIZE: i32 = 320;
const FULLSCREEN_PLAYER_MIN_COVER_SIZE: i32 = 140;
const FULLSCREEN_PLAYER_MAX_COVER_SIZE: i32 = 320;
const FULLSCREEN_PLAYER_HORIZONTAL_MARGIN: i32 = 64;
const FULLSCREEN_PLAYER_VERTICAL_RESERVED: i32 = 360;
const FULLSCREEN_ICON_SIZE: i32 = 18;
const FULLSCREEN_VISUALIZER_BANDS: usize = 320;
const FULLSCREEN_VISUALIZER_MIN_RATIO: f64 = 20.0 / 24_000.0;
const FULLSCREEN_VISUALIZER_MAX_RATIO: f64 = 22_050.0 / 24_000.0;
const FULLSCREEN_VISUALIZER_EMA_WEIGHT: f64 = 0.72;
const FULLSCREEN_VISUALIZER_MIN_COLUMNS: usize = 64;
const FULLSCREEN_VISUALIZER_MAX_COLUMNS: usize = 128;
const FULLSCREEN_VISUALIZER_TOP_GAP: f64 = 50.0;
const FULLSCREEN_EQUALIZER_SCALE_HEIGHT: i32 = 196;

pub(super) struct FullscreenPlayerParts {
    pub(super) root: gtk::Box,
    pub(super) animation_tick: RefCell<Option<gtk::TickCallbackId>>,
    pub(super) close_button: gtk::Button,
    pub(super) cover: ArtworkTile,
    pub(super) cover_key: RefCell<Option<String>>,
    pub(super) title: gtk::Label,
    pub(super) artist: gtk::Label,
    pub(super) album: gtk::Label,
    pub(super) meta: gtk::Label,
    pub(super) stack: adw::ViewStack,
    pub(super) lyrics_pane: LyricsPane,
    pub(super) queue_panel: gtk::Box,
    pub(super) visualizer_area: gtk::DrawingArea,
    pub(super) visualizer_levels: Rc<RefCell<Vec<f64>>>,
    pub(super) visualizer_targets: Rc<RefCell<Vec<f64>>>,
    pub(super) visualizer_tick: RefCell<Option<gtk::TickCallbackId>>,
    pub(super) visualizer_active: Cell<bool>,
    pub(super) equalizer_enabled: gtk::Switch,
    pub(super) equalizer_scales: Vec<gtk::Scale>,
    pub(super) equalizer_reset_button: gtk::Button,
    pub(super) equalizer_preset_button: gtk::MenuButton,
    pub(super) equalizer_preset_popover: gtk::Popover,
    pub(super) equalizer_preset_buttons: Vec<(gtk::Button, String)>,
    pub(super) equalizer_syncing: Rc<Cell<bool>>,
}

struct EqualizerPanel {
    root: gtk::ScrolledWindow,
    enabled: gtk::Switch,
    scales: Vec<gtk::Scale>,
    reset_button: gtk::Button,
    preset_button: gtk::MenuButton,
    preset_popover: gtk::Popover,
    preset_buttons: Vec<(gtk::Button, String)>,
    syncing: Rc<Cell<bool>>,
}

pub(super) fn build_fullscreen_player() -> FullscreenPlayerParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("fullscreen-player");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_visible(false);
    root.set_can_target(false);
    root.set_sensitive(false);
    root.set_opacity(0.0);

    let top_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top_bar.add_css_class("fullscreen-player-top-bar");
    top_bar.set_valign(gtk::Align::Center);

    let close_button = icon_button("go-down-symbolic", "Close fullscreen player");
    close_button.add_css_class("fullscreen-player-close-button");
    top_bar.append(&close_button);

    let top_bar_handle = gtk::WindowHandle::new();
    top_bar_handle.set_child(Some(&top_bar));
    root.append(&top_bar_handle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("fullscreen-player-body");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    hero.add_css_class("fullscreen-player-hero");
    hero.set_halign(gtk::Align::Center);
    hero.set_valign(gtk::Align::Center);
    hero.set_hexpand(true);

    let cover = ArtworkTile::new(FULLSCREEN_PLAYER_DEFAULT_COVER_SIZE, 42);
    cover.area.add_css_class("fullscreen-player-cover");
    cover.area.set_halign(gtk::Align::End);
    hero.append(&cover.area);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 5);
    details.add_css_class("fullscreen-player-details");
    details.set_halign(gtk::Align::Start);
    details.set_valign(gtk::Align::Center);

    let title = fullscreen_player_label("fullscreen-player-title");
    let artist = fullscreen_player_label("fullscreen-player-artist");
    let album = fullscreen_player_label("fullscreen-player-album");
    let meta = fullscreen_player_label("fullscreen-player-meta");
    meta.add_css_class("muted");
    for label in [&title, &artist, &album, &meta] {
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        label.set_justify(gtk::Justification::Left);
    }
    details.append(&title);
    details.append(&artist);
    details.append(&album);
    details.append(&meta);
    hero.append(&details);
    body.append(&hero);

    let stack = adw::ViewStack::builder()
        .hhomogeneous(false)
        .vhomogeneous(false)
        .hexpand(true)
        .vexpand(true)
        .build();

    let queue_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    queue_panel.add_css_class("fullscreen-player-pane");
    queue_panel.add_css_class("fullscreen-player-queue-panel");
    queue_panel.set_hexpand(true);
    queue_panel.set_vexpand(true);
    stack.add_titled(&queue_panel, Some("queue"), &tr("Queue"));

    let lyrics_pane = LyricsPane::new(&tr("Lyrics"));
    lyrics_pane.set_title("");
    lyrics_pane.widget().add_css_class("fullscreen-player-pane");
    stack.add_titled(lyrics_pane.widget(), Some("lyrics"), &tr("Lyrics"));

    let visualizer_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    visualizer_panel.add_css_class("fullscreen-player-pane");
    visualizer_panel.add_css_class("fullscreen-player-visualizer");
    visualizer_panel.set_hexpand(true);
    visualizer_panel.set_vexpand(true);
    let visualizer_levels = Rc::new(RefCell::new(Vec::new()));
    let visualizer_targets = Rc::new(RefCell::new(Vec::new()));
    let visualizer_area = build_fullscreen_visualizer_area(Rc::clone(&visualizer_levels));
    visualizer_panel.append(&visualizer_area);
    stack.add_titled(&visualizer_panel, Some("visualizer"), &tr("Visualizer"));

    let equalizer = build_fullscreen_equalizer_panel();
    stack.add_titled(&equalizer.root, Some("equalizer"), &tr("Equalizer"));
    stack.set_visible_child_name("lyrics");

    let switcher_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switcher_bar.add_css_class("fullscreen-player-tab-bar");
    switcher_bar.set_halign(gtk::Align::Center);
    switcher_bar.append(&fullscreen_player_switcher(&stack));
    body.append(&switcher_bar);
    body.append(&stack);
    root.append(&body);

    FullscreenPlayerParts {
        root,
        animation_tick: RefCell::new(None),
        close_button,
        cover,
        cover_key: RefCell::new(None),
        title,
        artist,
        album,
        meta,
        stack,
        lyrics_pane,
        queue_panel,
        visualizer_area,
        visualizer_levels,
        visualizer_targets,
        visualizer_tick: RefCell::new(None),
        visualizer_active: Cell::new(false),
        equalizer_enabled: equalizer.enabled,
        equalizer_scales: equalizer.scales,
        equalizer_reset_button: equalizer.reset_button,
        equalizer_preset_button: equalizer.preset_button,
        equalizer_preset_popover: equalizer.preset_popover,
        equalizer_preset_buttons: equalizer.preset_buttons,
        equalizer_syncing: equalizer.syncing,
    }
}

fn fullscreen_player_switcher(stack: &adw::ViewStack) -> gtk::Box {
    let switcher = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switcher.add_css_class("linked");
    switcher.set_halign(gtk::Align::Center);

    let queue = fullscreen_player_tab_button(
        gtk::Image::from_icon_name("view-list-ordered-symbolic").upcast(),
        &tr("Queue"),
    );
    let lyrics = fullscreen_player_tab_button(
        lyrics_icon_area(Rc::new(Cell::new(true))).upcast(),
        &tr("Lyrics"),
    );
    let visualizer =
        fullscreen_player_tab_button(fullscreen_visualizer_icon().upcast(), &tr("Visualizer"));
    let equalizer =
        fullscreen_player_tab_button(fullscreen_equalizer_icon().upcast(), &tr("Equalizer"));
    lyrics.set_active(true);

    let queue_stack = stack.clone();
    queue.connect_clicked(move |_| {
        queue_stack.set_visible_child_name("queue");
    });
    let lyrics_stack = stack.clone();
    lyrics.connect_clicked(move |_| {
        lyrics_stack.set_visible_child_name("lyrics");
    });
    let visualizer_stack = stack.clone();
    visualizer.connect_clicked(move |_| {
        visualizer_stack.set_visible_child_name("visualizer");
    });
    let equalizer_stack = stack.clone();
    equalizer.connect_clicked(move |_| {
        equalizer_stack.set_visible_child_name("equalizer");
    });

    let visualizer_for_notify = visualizer.clone();
    let lyrics_for_notify = lyrics.clone();
    let queue_for_notify = queue.clone();
    let equalizer_for_notify = equalizer.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        let page = stack.visible_child_name();
        visualizer_for_notify.set_active(page.as_deref() == Some("visualizer"));
        lyrics_for_notify.set_active(page.as_deref() == Some("lyrics"));
        queue_for_notify.set_active(page.as_deref() == Some("queue"));
        equalizer_for_notify.set_active(page.as_deref() == Some("equalizer"));
    });

    switcher.append(&queue);
    switcher.append(&lyrics);
    switcher.append(&visualizer);
    switcher.append(&equalizer);
    switcher
}

fn fullscreen_player_tab_button(icon: gtk::Widget, label: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    content.append(&icon);
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    button.set_tooltip_text(Some(label));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn fullscreen_visualizer_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(FULLSCREEN_ICON_SIZE);
    area.set_content_height(FULLSCREEN_ICON_SIZE);
    area.set_draw_func(|area, context, width, height| {
        let color = area.color();
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            0.9,
        );
        context.set_line_width(1.6);
        let center = f64::from(height) * 0.5;
        let bars = [0.38, 0.74, 0.48, 0.92, 0.56];
        let step = f64::from(width) / (bars.len() + 1) as f64;
        for (index, level) in bars.iter().enumerate() {
            let x = step * (index + 1) as f64;
            let half = f64::from(height) * level * 0.36;
            context.move_to(x, center - half);
            context.line_to(x, center + half);
            let _ = context.stroke();
        }
    });
    area
}

fn build_fullscreen_visualizer_area(levels: Rc<RefCell<Vec<f64>>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("fullscreen-player-visualizer-area");
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_halign(gtk::Align::Fill);
    area.set_valign(gtk::Align::Fill);
    area.set_content_height(230);
    area.set_draw_func(move |_, context, width, height| {
        let levels = levels.borrow();
        if levels.is_empty() {
            draw_visualizer_idle(context, width, height);
        } else {
            draw_visualizer_wave(context, width, height, &levels);
        }
    });
    area
}

fn draw_visualizer_idle(context: &gtk::cairo::Context, width: i32, height: i32) {
    draw_visualizer_bars(
        context,
        width,
        height,
        &idle_visualizer_levels(),
        0.46,
        false,
    );
}

fn draw_visualizer_wave(context: &gtk::cairo::Context, width: i32, height: i32, levels: &[f64]) {
    if levels.len() < 2 {
        draw_visualizer_idle(context, width, height);
        return;
    }
    draw_visualizer_bars(context, width, height, levels, 1.0, true);
}

fn draw_visualizer_bars(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    levels: &[f64],
    alpha: f64,
    normalize: bool,
) {
    let width = f64::from(width.max(1));
    let height = f64::from(height.max(1));
    let gap = 2.0;
    let left = width * 0.008;
    let available_width = (width * 0.984).max(1.0);
    let columns = (width / 8.0).round() as usize;
    let columns = columns.clamp(
        FULLSCREEN_VISUALIZER_MIN_COLUMNS,
        FULLSCREEN_VISUALIZER_MAX_COLUMNS,
    );
    let cell =
        ((available_width - gap * columns.saturating_sub(1) as f64) / columns as f64).max(2.0);
    let grid_height = (height - FULLSCREEN_VISUALIZER_TOP_GAP).max(height * 0.64);
    let rows = ((grid_height + gap) / (cell + gap)).floor() as usize;
    let rows = rows.clamp(8, 32);
    let bottom = height;
    let bars = visualizer_bar_levels(levels, columns, normalize);
    if bars.is_empty() {
        return;
    }

    for (column, level) in bars.iter().copied().enumerate() {
        let scaled = level * rows as f64;
        let x = left + column as f64 * (cell + gap);
        let full_cells = scaled.floor().clamp(1.0, rows as f64) as usize;
        for row in 0..full_cells {
            let color_t = if full_cells > 1 {
                row as f64 / (full_cells - 1) as f64
            } else {
                0.0
            };
            let (red, green, blue) = visualizer_bar_color(color_t);
            context.set_source_rgba(red, green, blue, alpha * (0.72 + color_t * 0.24));
            let y = bottom - cell - row as f64 * (cell + gap);
            context.rectangle(x, y, cell, cell);
            let _ = context.fill();
        }

        let cap_row = full_cells;
        let cap_alpha = scaled - scaled.floor();
        if cap_row > 0 && cap_row < rows && cap_alpha >= 0.14 {
            let (red, green, blue) = visualizer_bar_color(1.0);
            context.set_source_rgba(red, green, blue, alpha * cap_alpha * 0.76);
            let y = bottom - cell - cap_row as f64 * (cell + gap);
            context.rectangle(x, y, cell, cell);
            let _ = context.fill();
        }
    }
}

fn visualizer_bar_color(row_t: f64) -> (f64, f64, f64) {
    let row_t = row_t.clamp(0.0, 1.0);
    if row_t < 0.62 {
        let mix = row_t / 0.62;
        (
            lerp(0.86, 1.0, mix),
            lerp(0.10, 0.36, mix),
            lerp(0.04, 0.03, mix),
        )
    } else {
        let mix = (row_t - 0.62) / 0.38;
        (
            lerp(1.0, 1.0, mix),
            lerp(0.36, 0.66, mix),
            lerp(0.03, 0.08, mix),
        )
    }
}

fn lerp(start: f64, end: f64, mix: f64) -> f64 {
    start + (end - start) * mix
}

fn idle_visualizer_levels() -> Vec<f64> {
    (0..FULLSCREEN_VISUALIZER_BANDS)
        .map(|index| {
            let t = index as f64 / FULLSCREEN_VISUALIZER_BANDS as f64;
            0.040
                + (t * std::f64::consts::TAU * 0.85).sin() * 0.014
                + (t * std::f64::consts::TAU * 1.75 + 1.2).sin() * 0.008
        })
        .collect()
}

fn visualizer_bar_levels(levels: &[f64], columns: usize, normalize: bool) -> Vec<f64> {
    if columns == 0 || levels.is_empty() {
        return Vec::new();
    }
    let bars = (0..columns)
        .map(|column| {
            let start = column * levels.len() / columns;
            let end = ((column + 1) * levels.len() / columns).max(start + 1);
            let mut total = 0.0;
            let mut peak = 0.0_f64;
            let mut count = 0;
            for level in &levels[start..end.min(levels.len())] {
                let level = level.clamp(0.0, 1.0);
                total += level;
                peak = peak.max(level);
                count += 1;
            }
            let average = if count == 0 {
                0.0
            } else {
                total / count as f64
            };
            let position = if columns > 1 {
                column as f64 / (columns - 1) as f64
            } else {
                0.0
            };
            let low_taper = (position / 0.18).clamp(0.0, 1.0);
            let gain = 0.82 + low_taper * low_taper * (3.0 - 2.0 * low_taper) * 0.18;
            ((average * 0.38 + peak * 0.62) * gain * 1.28)
                .clamp(0.0, 1.0)
                .powf(0.62)
        })
        .collect::<Vec<_>>();
    if normalize {
        normalize_visualizer_bars(bars)
    } else {
        bars.into_iter().map(|level| level * 0.42).collect()
    }
}

fn normalize_visualizer_bars(mut bars: Vec<f64>) -> Vec<f64> {
    let peak = bars.iter().copied().fold(0.0_f64, f64::max);
    if peak < 0.08 {
        return bars;
    }
    let scale = (0.96 / peak).clamp(1.0, 2.35);
    for level in &mut bars {
        *level = (*level * scale).clamp(0.0, 1.0);
    }
    bars
}

fn visualizer_display_levels(levels: &[f64]) -> Vec<f64> {
    let source = levels
        .iter()
        .copied()
        .map(|level| level.clamp(0.0, 1.0))
        .collect::<Vec<_>>();
    if source.is_empty() {
        return vec![0.0; FULLSCREEN_VISUALIZER_BANDS];
    }
    if source.len() == 1 {
        return vec![source[0]; FULLSCREEN_VISUALIZER_BANDS];
    }
    if source.len() < 4 {
        let source_max = source.len().saturating_sub(1) as f64;
        let display_max = FULLSCREEN_VISUALIZER_BANDS.saturating_sub(1) as f64;
        return (0..FULLSCREEN_VISUALIZER_BANDS)
            .map(|index| {
                let position = index as f64 / display_max * source_max;
                let lower = position.floor() as usize;
                let upper = position.ceil().min(source_max) as usize;
                let mix = position - lower as f64;
                source[lower] + (source[upper] - source[lower]) * mix
            })
            .collect();
    }

    let source_last = source.len().saturating_sub(1) as f64;
    let source_min = (source_last * FULLSCREEN_VISUALIZER_MIN_RATIO)
        .round()
        .clamp(1.0, source_last);
    let source_max = (source_last * FULLSCREEN_VISUALIZER_MAX_RATIO)
        .round()
        .clamp(source_min + 1.0, source_last);
    let log_span = (source_max / source_min).ln();

    (0..FULLSCREEN_VISUALIZER_BANDS)
        .map(|index| {
            let center_t = (index as f64 + 0.5) / FULLSCREEN_VISUALIZER_BANDS as f64;
            let position = source_min * (log_span * center_t).exp();
            let lower = position.floor().min(source_last) as usize;
            let upper = position.ceil().min(source_last) as usize;
            let mix = position - lower as f64;
            source[lower] + (source[upper] - source[lower]) * mix
        })
        .collect()
}

fn fullscreen_equalizer_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(FULLSCREEN_ICON_SIZE);
    area.set_content_height(FULLSCREEN_ICON_SIZE);
    area.set_draw_func(|area, context, width, height| {
        let color = area.color();
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            0.92,
        );
        let width = f64::from(width);
        let height = f64::from(height);
        let tracks = [
            (width * 0.24, height * 0.38),
            (width * 0.5, height * 0.58),
            (width * 0.76, height * 0.38),
        ];
        context.set_line_width(2.2);
        for (x, _) in tracks {
            context.move_to(x, 2.6);
            context.line_to(x, height - 2.6);
            let _ = context.stroke();
        }

        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            0.92,
        );
        for (x, y) in tracks {
            context.rectangle(x - 2.4, y - 1.7, 4.8, 3.4);
            let _ = context.fill();
        }
    });
    area
}

fn build_fullscreen_equalizer_panel() -> EqualizerPanel {
    let root = gtk::ScrolledWindow::new();
    root.add_css_class("fullscreen-player-pane");
    root.add_css_class("fullscreen-player-equalizer-scroller");
    root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_propagate_natural_height(false);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.add_css_class("fullscreen-player-equalizer");
    content.set_hexpand(true);
    content.set_vexpand(true);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.set_halign(gtk::Align::Center);
    header.set_valign(gtk::Align::Center);

    let enabled = gtk::Switch::new();
    enabled.set_valign(gtk::Align::Center);
    let enabled_label = gtk::Label::new(Some(&tr("Enable equalizer")));
    enabled_label.set_valign(gtk::Align::Center);
    header.append(&enabled_label);
    header.append(&enabled);

    let preset_button = gtk::MenuButton::new();
    preset_button.set_label(&tr("Preset"));
    preset_button.set_valign(gtk::Align::Center);
    let preset_popover = gtk::Popover::new();
    let preset_menu = gtk::Box::new(gtk::Orientation::Vertical, 4);
    preset_menu.add_css_class("fullscreen-player-equalizer-preset-menu");
    let mut preset_buttons = Vec::new();
    for name in equalizer_preset_names() {
        let button = gtk::Button::with_label(&tr(name));
        button.set_halign(gtk::Align::Fill);
        button.set_valign(gtk::Align::Center);
        button.add_css_class("flat");
        preset_menu.append(&button);
        preset_buttons.push((button, name.to_string()));
    }
    preset_popover.set_child(Some(&preset_menu));
    preset_button.set_popover(Some(&preset_popover));
    header.append(&preset_button);

    let reset_button = gtk::Button::with_label(&tr("Reset"));
    reset_button.add_css_class("destructive-action");
    header.append(&reset_button);
    content.append(&header);

    let band_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    band_row.set_halign(gtk::Align::Center);
    band_row.set_valign(gtk::Align::Center);
    band_row.set_vexpand(true);
    band_row.set_size_request(-1, 240);
    band_row.append(&fullscreen_equalizer_level_labels());

    let bands = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bands.add_css_class("fullscreen-player-equalizer-bands");
    bands.set_halign(gtk::Align::Center);
    bands.set_valign(gtk::Align::Center);
    let mut scales = Vec::with_capacity(EQUALIZER_BAND_COUNT);
    for index in 0..EQUALIZER_BAND_COUNT {
        let band = gtk::Box::new(gtk::Orientation::Vertical, 6);
        band.set_halign(gtk::Align::Center);
        band.set_valign(gtk::Align::Center);
        let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, -12.0, 12.0, 0.5);
        scale.set_inverted(true);
        scale.set_value(0.0);
        scale.set_draw_value(false);
        scale.set_size_request(36, FULLSCREEN_EQUALIZER_SCALE_HEIGHT);
        scale.set_valign(gtk::Align::Center);
        scale.set_tooltip_text(Some(&equalizer_band_title(index)));
        band.append(&scale);
        band.append(&fullscreen_equalizer_band_label(index));
        bands.append(&band);
        scales.push(scale);
    }
    band_row.append(&bands);
    band_row.append(&fullscreen_equalizer_level_labels());
    content.append(&band_row);
    root.set_child(Some(&content));

    EqualizerPanel {
        root,
        enabled,
        scales,
        reset_button,
        preset_button,
        preset_popover,
        preset_buttons,
        syncing: Rc::new(Cell::new(false)),
    }
}

fn fullscreen_equalizer_level_labels() -> gtk::Widget {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("fullscreen-player-equalizer-levels");
    column.set_height_request(FULLSCREEN_EQUALIZER_SCALE_HEIGHT);
    column.set_valign(gtk::Align::Center);
    for (index, value) in ["12 dB", "6 dB", "0 dB", "-6 dB", "-12 dB"]
        .iter()
        .enumerate()
    {
        if index > 0 {
            let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            spacer.set_vexpand(true);
            column.append(&spacer);
        }
        let label = gtk::Label::new(Some(value));
        label.add_css_class("muted");
        label.add_css_class("fullscreen-player-equalizer-level-label");
        label.set_xalign(1.0);
        column.append(&label);
    }
    column.upcast()
}

fn fullscreen_equalizer_band_label(index: usize) -> gtk::Widget {
    let (value, unit) = equalizer_band_label_parts(index);
    let label = gtk::Box::new(gtk::Orientation::Vertical, 0);
    label.add_css_class("fullscreen-player-equalizer-band-label");
    label.set_halign(gtk::Align::Center);
    for text in [value, unit] {
        let row = gtk::Label::new(Some(&text));
        row.add_css_class("muted");
        row.set_xalign(0.5);
        row.set_width_chars(1);
        row.set_max_width_chars(4);
        row.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.append(&row);
    }
    label.upcast()
}

pub(super) fn connect_fullscreen_player_controls(shell: &Rc<Shell>) {
    let close_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .close_button
        .connect_clicked(move |_| close_shell.close_fullscreen_player());

    let key_shell = Rc::clone(shell);
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape && key_shell.state.fullscreen_player_visible.get() {
            key_shell.close_fullscreen_player();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shell.window.add_controller(key);

    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            resize_shell.refresh_fullscreen_player_layout();
        });
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("height"), move |_, _| {
            resize_shell.refresh_fullscreen_player_layout();
        });
    let resize_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .root
        .connect_notify_local(Some("width"), move |_, _| {
            resize_shell.refresh_fullscreen_player_layout();
        });
    let resize_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .root
        .connect_notify_local(Some("height"), move |_, _| {
            resize_shell.refresh_fullscreen_player_layout();
        });

    let queue_tab_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .stack
        .connect_visible_child_name_notify(move |stack| {
            if !queue_tab_shell.state.fullscreen_player_visible.get() {
                return;
            }
            match stack.visible_child_name().as_deref() {
                Some("queue") => queue_tab_shell.schedule_queue_panel_render(),
                Some("lyrics") => queue_tab_shell.refresh_fullscreen_lyrics_position(),
                _ => {}
            }
            queue_tab_shell.sync_fullscreen_visualizer_state();
            if stack.visible_child_name().as_deref() == Some("visualizer") {
                let visualizer_shell = Rc::clone(&queue_tab_shell);
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    visualizer_shell.sync_fullscreen_visualizer_state();
                });
            }
        });

    let equalizer_shell = Rc::clone(shell);
    let equalizer_guard = Rc::clone(&shell.fullscreen_player.equalizer_syncing);
    shell
        .fullscreen_player
        .equalizer_enabled
        .connect_state_set(move |row, enabled| {
            if equalizer_guard.get() {
                return glib::Propagation::Proceed;
            }
            row.set_state(enabled);
            equalizer_shell.update_playback_settings(|settings| {
                settings.equalizer.enabled = enabled;
                if settings.equalizer.bands.len() != EQUALIZER_BAND_COUNT {
                    settings.equalizer.sanitize();
                }
            });
            glib::Propagation::Stop
        });

    let pending_equalizer_update = Rc::new(RefCell::new(None::<glib::SourceId>));
    let equalizer_pointer_active = Rc::new(Cell::new(false));
    let equalizer_commit: Rc<dyn Fn()> = {
        let shell = Rc::clone(shell);
        Rc::new(move || shell.update_fullscreen_equalizer_from_scales())
    };
    for scale in &shell.fullscreen_player.equalizer_scales {
        connect_equalizer_scale_commit(
            scale,
            Rc::clone(&shell.fullscreen_player.equalizer_syncing),
            Rc::clone(&pending_equalizer_update),
            Rc::clone(&equalizer_pointer_active),
            Rc::clone(&equalizer_commit),
        );
    }

    let reset_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .equalizer_reset_button
        .connect_clicked(move |_| {
            reset_shell.reset_fullscreen_equalizer_preset();
        });

    for (button, name) in &shell.fullscreen_player.equalizer_preset_buttons {
        let preset_shell = Rc::clone(shell);
        let preset_name = name.clone();
        button.connect_clicked(move |_| {
            let bands = {
                let settings = preset_shell.state.settings.borrow();
                equalizer_preset_bands(&settings.playback.equalizer, &preset_name)
            };
            preset_shell
                .fullscreen_player
                .equalizer_preset_button
                .set_label(&tr(&preset_name));
            preset_shell
                .fullscreen_player
                .equalizer_preset_popover
                .popdown();
            preset_shell.apply_fullscreen_equalizer_bands(true, preset_name.clone(), bands);
        });
    }
}

impl Shell {
    pub(super) fn open_fullscreen_player(self: &Rc<Self>) {
        if self.state.player.borrow().current.is_none() {
            return;
        }
        self.state.fullscreen_player_visible.set(true);
        self.animate_fullscreen_player(true);
        let player = self.state.player.borrow().clone();
        self.update_fullscreen_player_text(&player);
        self.update_fullscreen_player_cover(&player);
        let update_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(FULLSCREEN_PLAYER_DEFERRED_UPDATE_MS),
            move || {
                if update_shell.state.fullscreen_player_visible.get() {
                    let player = update_shell.state.player.borrow().clone();
                    update_shell.update_fullscreen_player_text(&player);
                }
            },
        );
        let cover_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(FULLSCREEN_PLAYER_DEFERRED_COVER_MS),
            move || {
                if cover_shell.state.fullscreen_player_visible.get() {
                    let player = cover_shell.state.player.borrow().clone();
                    cover_shell.update_fullscreen_player_cover(&player);
                }
            },
        );
        if self.fullscreen_player.stack.visible_child_name().as_deref() == Some("queue") {
            let queue_shell = Rc::clone(self);
            glib::timeout_add_local_once(
                Duration::from_millis(u64::from(FULLSCREEN_PLAYER_TRANSITION_MS)),
                move || {
                    if queue_shell.state.fullscreen_player_visible.get()
                        && queue_shell
                            .fullscreen_player
                            .stack
                            .visible_child_name()
                            .as_deref()
                            == Some("queue")
                    {
                        queue_shell.render_queue_panel();
                    }
                },
            );
        }
        if self.fullscreen_player.stack.visible_child_name().as_deref() == Some("lyrics") {
            let lyrics_shell = Rc::clone(self);
            glib::idle_add_local_once(move || {
                lyrics_shell.refresh_fullscreen_lyrics_position();
            });
        }
        self.sync_fullscreen_visualizer_state();
        let _focused = self.fullscreen_player.close_button.grab_focus();
    }

    pub(super) fn close_fullscreen_player(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.replace(false) {
            return;
        }
        self.animate_fullscreen_player(false);
        self.sync_fullscreen_visualizer_state();
    }

    pub(super) fn toggle_fullscreen_player(self: &Rc<Self>) {
        if self.state.fullscreen_player_visible.get() {
            self.close_fullscreen_player();
        } else {
            self.open_fullscreen_player();
        }
    }

    pub(super) fn update_fullscreen_player(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.get() {
            return;
        }
        let player = self.state.player.borrow().clone();
        self.update_fullscreen_player_text(&player);
        self.update_fullscreen_player_cover(&player);
        self.sync_fullscreen_equalizer_controls(&self.state.settings.borrow().playback.equalizer);
        self.sync_fullscreen_visualizer_state();
    }

    fn refresh_fullscreen_player_layout(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.get() {
            return;
        }
        self.update_fullscreen_player();
        self.schedule_queue_panel_render();
        let refresh_shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            if refresh_shell.state.fullscreen_player_visible.get() {
                refresh_shell.update_fullscreen_player();
                refresh_shell.schedule_queue_panel_render();
            }
        });
    }

    pub(super) fn sync_fullscreen_equalizer_controls(&self, equalizer: &EqualizerSettings) {
        self.fullscreen_player.equalizer_syncing.set(true);
        self.fullscreen_player
            .equalizer_enabled
            .set_active(equalizer.enabled);
        for (index, scale) in self.fullscreen_player.equalizer_scales.iter().enumerate() {
            let value = equalizer.bands.get(index).copied().unwrap_or(0.0);
            scale.set_value(value);
        }
        self.sync_fullscreen_equalizer_preset_label(equalizer);
        self.fullscreen_player.equalizer_syncing.set(false);
    }

    fn sync_fullscreen_equalizer_preset_label(&self, equalizer: &EqualizerSettings) {
        let label = tr(&equalizer_selected_preset(equalizer));
        self.fullscreen_player
            .equalizer_preset_button
            .set_label(&label);
    }

    fn apply_fullscreen_equalizer_bands(
        self: &Rc<Self>,
        enabled: bool,
        preset: String,
        bands: Vec<f64>,
    ) {
        let mut equalizer = self.state.settings.borrow().playback.equalizer.clone();
        equalizer.enabled = enabled;
        equalizer.selected_preset = preset;
        equalizer.bands = bands;
        equalizer.sanitize();
        self.sync_fullscreen_equalizer_controls(&equalizer);
        self.update_playback_settings(|settings| {
            settings.equalizer.enabled = equalizer.enabled;
            settings.equalizer.selected_preset = equalizer.selected_preset;
            settings.equalizer.bands = equalizer.bands;
            settings.equalizer.sanitize();
        });
    }

    fn reset_fullscreen_equalizer_preset(self: &Rc<Self>) {
        let (preset, enabled) = {
            let settings = self.state.settings.borrow();
            (
                equalizer_selected_preset(&settings.playback.equalizer),
                settings.playback.equalizer.enabled,
            )
        };
        let bands = equalizer_default_preset_bands(&preset);
        let mut equalizer = self.state.settings.borrow().playback.equalizer.clone();
        equalizer.enabled = enabled;
        equalizer.selected_preset = preset.clone();
        equalizer.bands = bands.clone();
        equalizer.preset_overrides.remove(&preset);
        equalizer.sanitize();
        self.sync_fullscreen_equalizer_controls(&equalizer);
        self.update_playback_settings(|settings| {
            settings.equalizer.enabled = enabled;
            settings.equalizer.selected_preset = preset;
            settings.equalizer.bands = bands;
            settings
                .equalizer
                .preset_overrides
                .remove(&settings.equalizer.selected_preset);
            settings.equalizer.sanitize();
        });
    }

    fn update_fullscreen_equalizer_from_scales(self: &Rc<Self>) {
        let bands = self
            .fullscreen_player
            .equalizer_scales
            .iter()
            .map(gtk::Scale::value)
            .collect::<Vec<_>>();
        self.update_playback_settings(|settings| {
            if settings.equalizer.bands.len() != EQUALIZER_BAND_COUNT {
                settings.equalizer.sanitize();
            }
            settings.equalizer.bands = bands.clone();
            let preset = equalizer_selected_preset(&settings.equalizer);
            settings.equalizer.selected_preset = preset.clone();
            settings.equalizer.preset_overrides.insert(preset, bands);
        });
    }

    pub(in crate::ui) fn apply_fullscreen_visualizer_levels(self: &Rc<Self>, levels: Vec<f64>) {
        if levels.is_empty() {
            self.clear_fullscreen_visualizer();
            return;
        }
        if !self.fullscreen_player.visualizer_active.get() {
            return;
        }
        *self.fullscreen_player.visualizer_targets.borrow_mut() =
            visualizer_display_levels(&levels);
        self.start_fullscreen_visualizer_tick();
    }

    fn sync_fullscreen_visualizer_state(self: &Rc<Self>) {
        let active = self.state.fullscreen_player_visible.get()
            && self.fullscreen_player.stack.visible_child_name().as_deref() == Some("visualizer")
            && matches!(
                self.state.player.borrow().state,
                PlaybackState::Playing | PlaybackState::Buffering
            );
        let changed = self.fullscreen_player.visualizer_active.replace(active) != active;
        if active {
            self.controller.set_visualizer_enabled(true);
            self.start_fullscreen_visualizer_tick();
            return;
        }
        if changed {
            self.controller.set_visualizer_enabled(false);
            self.stop_fullscreen_visualizer_tick();
            self.clear_fullscreen_visualizer();
        }
    }

    fn start_fullscreen_visualizer_tick(self: &Rc<Self>) {
        if self.fullscreen_player.visualizer_tick.borrow().is_some() {
            return;
        }
        let levels = Rc::clone(&self.fullscreen_player.visualizer_levels);
        let targets = Rc::clone(&self.fullscreen_player.visualizer_targets);
        let tick = self
            .fullscreen_player
            .visualizer_area
            .add_tick_callback(move |area, _| {
                let mut current = levels.borrow_mut();
                let target = targets.borrow();
                let len = target
                    .len()
                    .max(current.len())
                    .max(FULLSCREEN_VISUALIZER_BANDS);
                current.resize(len, 0.0);
                for index in 0..len {
                    let next = target.get(index).copied().unwrap_or(0.0);
                    let value = current[index];
                    current[index] = next * FULLSCREEN_VISUALIZER_EMA_WEIGHT
                        + value * (1.0 - FULLSCREEN_VISUALIZER_EMA_WEIGHT);
                }
                area.queue_draw();
                glib::ControlFlow::Continue
            });
        *self.fullscreen_player.visualizer_tick.borrow_mut() = Some(tick);
    }

    fn stop_fullscreen_visualizer_tick(&self) {
        if let Some(tick) = self.fullscreen_player.visualizer_tick.borrow_mut().take() {
            tick.remove();
        }
    }

    fn clear_fullscreen_visualizer(&self) {
        self.fullscreen_player
            .visualizer_levels
            .borrow_mut()
            .clear();
        self.fullscreen_player
            .visualizer_targets
            .borrow_mut()
            .clear();
        self.fullscreen_player.visualizer_area.queue_draw();
    }

    fn refresh_fullscreen_lyrics_position(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.get()
            || self.fullscreen_player.stack.visible_child_name().as_deref() != Some("lyrics")
        {
            return;
        }
        self.refocus_fullscreen_lyrics_position();
        let idle_shell = Rc::clone(self);
        glib::idle_add_local_once(move || {
            idle_shell.refocus_fullscreen_lyrics_position();
        });
        let settle_shell = Rc::clone(self);
        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            settle_shell.refocus_fullscreen_lyrics_position();
        });
    }

    fn refocus_fullscreen_lyrics_position(&self) {
        if !self.state.fullscreen_player_visible.get()
            || self.fullscreen_player.stack.visible_child_name().as_deref() != Some("lyrics")
        {
            return;
        }
        let lyrics = self.state.lyrics.borrow();
        self.fullscreen_player
            .lyrics_pane
            .refocus_highlight(lyrics.as_ref(), self.current_position_millis());
    }

    fn update_fullscreen_player_cover(
        self: &Rc<Self>,
        player: &crate::controller::PlaybackSnapshot,
    ) {
        let cover_size = self.fullscreen_player_cover_size();
        self.fullscreen_player.cover.set_square_size(cover_size);
        let cover_seed = player
            .current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or(42);
        self.fullscreen_player.cover.set_seed(cover_seed);

        if let Some(image_ref) = player
            .current
            .as_ref()
            .and_then(|entry| entry.image_ref.as_ref())
        {
            let fetch_size = cover_fetch_size_for_display(cover_size);
            if let Some(key) = self.current_playback_cover_cache_key(image_ref, fetch_size) {
                let request_key = cover_request_id_for_key(&key, cover_size);
                let pixbuf = self
                    .cloned_decoded_cover(&key, cover_size)
                    .map(|cover| {
                        self.touch_decoded_cover(&key, CoverDecodePriority::Visible);
                        cover.pixbuf
                    })
                    .or_else(|| {
                        self.fullscreen_cover_preview(image_ref, fetch_size).map(
                            |(preview_key, pixbuf)| {
                                self.touch_decoded_cover(
                                    &preview_key,
                                    CoverDecodePriority::Visible,
                                );
                                pixbuf
                            },
                        )
                    });
                let outcome = self.fullscreen_player.cover.bind_selected_cover(
                    cover_seed,
                    cover_artwork_id_for_key(&key, image_ref),
                    request_key.clone(),
                    pixbuf,
                );
                if outcome.request_needed {
                    self.request_bound_cover_for_tile(
                        &self.fullscreen_player.cover,
                        key,
                        image_ref.clone(),
                        outcome.generation,
                        cover_size,
                        fetch_size,
                    );
                }
                *self.fullscreen_player.cover_key.borrow_mut() = Some(request_key);
            } else {
                self.clear_fullscreen_player_cover();
            }
        } else {
            self.clear_fullscreen_player_cover();
        }
    }

    fn fullscreen_cover_preview(
        &self,
        image_ref: &rufin_core::ImageRef,
        fetch_size: u32,
    ) -> Option<(String, gdk_pixbuf::Pixbuf)> {
        for size in fullscreen_cover_preview_sizes(fetch_size) {
            let Some(key) = self.current_playback_cover_cache_key(image_ref, size) else {
                continue;
            };
            if let Some(cover) = self.cloned_decoded_cover(&key, 1) {
                return Some((key, cover.pixbuf));
            }
        }
        None
    }

    fn update_fullscreen_player_text(&self, player: &crate::controller::PlaybackSnapshot) {
        let title = player
            .current
            .as_ref()
            .map(|entry| entry.title.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Nothing playing"));
        let artist = player
            .current
            .as_ref()
            .map(|entry| entry.artist.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Queue a track to begin"));
        let album = player
            .current
            .as_ref()
            .map(|entry| entry.album.as_str())
            .unwrap_or("");
        self.fullscreen_player.title.set_text(&title);
        self.fullscreen_player.artist.set_text(&artist);
        self.fullscreen_player.album.set_text(album);
        self.fullscreen_player
            .title
            .set_sensitive(player.current.is_some());
        self.fullscreen_player.artist.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.artist.is_empty()),
        );
        self.fullscreen_player.album.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.album.is_empty()),
        );
        self.fullscreen_player
            .meta
            .set_text(&self.fullscreen_player_meta_text(player));
        self.fullscreen_player
            .meta
            .set_visible(player.current.is_some());
    }

    fn fullscreen_player_meta_text(&self, player: &crate::controller::PlaybackSnapshot) -> String {
        let source_label = player
            .current
            .as_ref()
            .and_then(|entry| self.current_track_source_label(entry));
        fullscreen_player_meta_text(player.current.as_ref(), source_label.as_deref())
    }

    fn current_track_source_label(&self, entry: &QueueEntry) -> Option<String> {
        if let Some(source) = entry
            .source_format
            .as_deref()
            .and_then(audio_source_label_from_format)
        {
            return Some(source);
        }
        if let Some(source) = entry
            .local_path
            .as_deref()
            .and_then(audio_source_label_from_path)
        {
            return Some(source);
        }
        if let Some(source) = self
            .controller
            .cached_track_source_format(&entry.track_id)
            .as_deref()
            .and_then(audio_source_label_from_format)
        {
            return Some(source);
        }
        if let Some(source) = self
            .controller
            .cached_track_local_path(&entry.track_id)
            .as_deref()
            .and_then(audio_source_label_from_path)
        {
            return Some(source);
        }

        let library = self.state.library.borrow();
        library
            .tracks
            .iter()
            .chain(library.favorites.iter())
            .chain(library.search.tracks.iter())
            .chain(
                library
                    .home_sections
                    .iter()
                    .flat_map(|section| section.tracks.iter()),
            )
            .find(|track| track.id == entry.track_id)
            .and_then(|track| {
                track
                    .source_format
                    .as_deref()
                    .and_then(audio_source_label_from_format)
                    .or_else(|| {
                        track
                            .local_path
                            .as_deref()
                            .and_then(audio_source_label_from_path)
                    })
            })
    }

    fn fullscreen_player_cover_size(&self) -> i32 {
        let width = self.window.width().max(1);
        let fallback_height = (self.window.height() - BOTTOM_PLAYER_HEIGHT).max(1);
        let height = fallback_height.max(1);
        fullscreen_artwork_size_for(width, height)
    }

    fn clear_fullscreen_player_cover(&self) {
        self.fullscreen_player.cover.clear_image();
        *self.fullscreen_player.cover_key.borrow_mut() = None;
    }

    fn animate_fullscreen_player(self: &Rc<Self>, opening: bool) {
        if let Some(tick) = self.fullscreen_player.animation_tick.borrow_mut().take() {
            tick.remove();
        }

        let root = self.fullscreen_player.root.clone();
        let height = self.fullscreen_player_hidden_offset();
        let started_at = Rc::new(Cell::new(None));

        root.set_visible(true);
        root.set_opacity(1.0);
        root.set_can_target(opening);
        root.set_sensitive(opening);
        root.set_margin_top(if opening { height } else { 0 });

        let tick_shell = Rc::clone(self);
        let tick_started_at = Rc::clone(&started_at);
        let tick = root.add_tick_callback(move |root, clock| {
            let now = clock.frame_time();
            let start = tick_started_at.get().unwrap_or_else(|| {
                tick_started_at.set(Some(now));
                now
            });
            let elapsed = now.saturating_sub(start);
            let progress =
                (elapsed as f64 / FULLSCREEN_PLAYER_TRANSITION_US as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            let offset = if opening {
                (1.0 - eased) * f64::from(height)
            } else {
                eased * f64::from(height)
            };
            root.set_margin_top(offset.round() as i32);

            if progress >= 1.0 {
                root.set_can_target(opening);
                root.set_sensitive(opening);
                if opening {
                    root.set_margin_top(0);
                    root.set_opacity(1.0);
                } else {
                    root.set_margin_top(0);
                    root.set_opacity(0.0);
                }
                root.set_visible(true);
                tick_shell
                    .fullscreen_player
                    .animation_tick
                    .borrow_mut()
                    .take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.fullscreen_player.animation_tick.borrow_mut() = Some(tick);
    }

    pub(in crate::ui) fn fullscreen_player_hidden_offset(&self) -> i32 {
        self.fullscreen_player
            .root
            .height()
            .max(self.app_content_stack.height())
            .max((self.window.height() - BOTTOM_PLAYER_HEIGHT).max(1))
    }
}

fn fullscreen_player_label(css_class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class(css_class);
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(1);
    label.set_max_width_chars(48);
    label.set_halign(gtk::Align::Center);
    label
}

fn fullscreen_player_meta_text(entry: Option<&QueueEntry>, source_label: Option<&str>) -> String {
    let Some(entry) = entry else {
        return String::new();
    };
    fullscreen_player_meta_parts(entry.year, source_label)
}

fn fullscreen_player_meta_parts(year: u16, source_label: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(source) = source_label
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        parts.push(source.to_string());
    }
    if year > 0 {
        parts.push(year.to_string());
    }
    parts.join(" - ")
}

fn audio_source_label_from_path(path: &str) -> Option<String> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let extension = Path::new(path).extension()?.to_str()?.trim();
    audio_source_label_from_format(extension)
}

fn audio_source_label_from_format(value: &str) -> Option<String> {
    let value = value
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_start_matches('.');
    if value.is_empty() {
        return None;
    }
    let normalized = match value.to_ascii_lowercase().as_str() {
        "mpeg" | "mpga" => "MP3".to_string(),
        other => other.to_ascii_uppercase(),
    };
    Some(normalized)
}

fn fullscreen_cover_preview_sizes(fetch_size: u32) -> Vec<u32> {
    let mut sizes = vec![GRID_COVER_SIZE, THUMB_COVER_SIZE];
    sizes.retain(|size| *size < fetch_size);
    sizes
}

pub(super) fn fullscreen_artwork_size_for(width: i32, height: i32) -> i32 {
    let width_limit = (width - FULLSCREEN_PLAYER_HORIZONTAL_MARGIN).max(1);
    let height_limit = (height - FULLSCREEN_PLAYER_VERTICAL_RESERVED).max(1);
    width_limit.min(height_limit).clamp(
        FULLSCREEN_PLAYER_MIN_COVER_SIZE,
        FULLSCREEN_PLAYER_MAX_COVER_SIZE,
    )
}

#[cfg(test)]
mod tests {
    use super::super::equalizer::equalizer_presets;
    use super::fullscreen_artwork_size_for;
    use rufin_core::EQUALIZER_BAND_COUNT;

    #[test]
    fn fullscreen_stay_windows() {
        assert_eq!(fullscreen_artwork_size_for(480, 360), 140);
    }

    #[test]
    fn fullscreen_cap_windows() {
        assert_eq!(fullscreen_artwork_size_for(1440, 900), 320);
    }

    #[test]
    fn fullscreen_use_width() {
        assert_eq!(fullscreen_artwork_size_for(900, 560), 200);
    }

    #[test]
    fn fullscreen_use_duration() {
        assert_eq!(
            super::fullscreen_player_meta_parts(2013, Some("FLAC")),
            "FLAC - 2013"
        );
    }

    #[test]
    fn fullscreen_use_extension() {
        assert_eq!(
            super::audio_source_label_from_path("/music/album/track.mpc").as_deref(),
            Some("MPC")
        );
    }

    #[test]
    fn fullscreen_ignore_query() {
        assert_eq!(
            super::audio_source_label_from_path("/music/album/track.flac?token=redacted")
                .as_deref(),
            Some("FLAC")
        );
    }

    #[test]
    fn fullscreen_normalize_type() {
        assert_eq!(
            super::audio_source_label_from_format("audio/mpeg").as_deref(),
            Some("MP3")
        );
    }

    #[test]
    fn fullscreen_equalizer_presets_cover_all_bands() {
        for (_, bands) in equalizer_presets() {
            assert_eq!(bands.len(), EQUALIZER_BAND_COUNT);
            assert!(bands.iter().all(|gain| (-12.0..=12.0).contains(gain)));
        }
    }
}
