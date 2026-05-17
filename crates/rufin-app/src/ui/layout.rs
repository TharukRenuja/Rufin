use adw::prelude::*;
use rufin_core::{AppSettings, EffectiveDensity};

use super::Shell;

pub(super) const COMPACT_RAIL_WIDTH: i32 = 64;
pub(super) const NORMAL_SIDEBAR_WIDTH: i32 = 176;
pub(super) const HOME_ALBUM_GAP: i32 = 14;
const HOME_ALBUM_MIN_SIZE: i32 = 150;
const HOME_ALBUM_TARGET_SIZE: i32 = 180;
const HOME_ALBUM_MAX_SIZE: i32 = 210;
const HOME_ALBUM_MIN_COLUMNS: usize = 2;
const HOME_ALBUM_MAX_COLUMNS: usize = 12;
pub(super) const PRIMARY_ROUTE_MARGIN_START: i32 = 0;
pub(super) const PRIMARY_ROUTE_MARGIN_END: i32 = 28;
const HOME_ALBUM_HORIZONTAL_MARGINS: i32 = PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END;
const CARD_LABEL_LINE_HEIGHT: i32 = 20;
pub(super) const HOME_ALBUM_CARD_LABEL_GAP: i32 = 4;
pub(super) const HOME_ALBUM_TITLE_LINES: i32 = 1;
pub(super) const HOME_ALBUM_ARTIST_LINES: i32 = 1;
pub(super) const HOME_ALBUM_YEAR_LINES: i32 = 1;

const MAIN_PANEL_UNITS: i32 = 7;
const TOTAL_PANEL_UNITS: i32 = 10;
const RIGHT_PANEL_MIN_PERCENT: i32 = 10;
const RIGHT_PANEL_MAX_PERCENT: i32 = 50;
const COMPACT_RIGHT_PANEL_MAX_PERCENT: i32 = 38;
const COMPACT_PRIMARY_MIN_WIDTH: i32 = 560;
const MIN_RESTORED_WINDOW_WIDTH: i32 = 480;
pub(super) const MIN_RESTORED_WINDOW_HEIGHT: i32 = 360;
const MAX_RESTORED_WINDOW_WIDTH: i32 = 1400;
pub(super) const MAX_RESTORED_WINDOW_HEIGHT: i32 = 900;

pub(super) fn home_album_page_size(width: i32, current_page_size: Option<usize>) -> usize {
    let width = width.max(1);
    let mut page_size = current_page_size
        .unwrap_or_else(|| {
            let item_width = HOME_ALBUM_TARGET_SIZE + HOME_ALBUM_GAP;
            ((width + HOME_ALBUM_GAP) / item_width)
                .clamp(HOME_ALBUM_MIN_COLUMNS as i32, HOME_ALBUM_MAX_COLUMNS as i32)
                as usize
        })
        .clamp(HOME_ALBUM_MIN_COLUMNS, HOME_ALBUM_MAX_COLUMNS);

    while page_size > HOME_ALBUM_MIN_COLUMNS
        && home_album_raw_card_size(width, page_size) < HOME_ALBUM_MIN_SIZE
    {
        page_size -= 1;
    }
    while page_size < HOME_ALBUM_MAX_COLUMNS
        && home_album_raw_card_size(width, page_size) > HOME_ALBUM_MAX_SIZE
    {
        page_size += 1;
    }

    page_size
}

pub(super) fn clamp_content_split_position_for_density(
    split_width: i32,
    position: i32,
    density: EffectiveDensity,
) -> i32 {
    if split_width <= 1 {
        return position;
    }
    let max_right_percent = match density {
        EffectiveDensity::Normal => RIGHT_PANEL_MAX_PERCENT,
        EffectiveDensity::Compact => COMPACT_RIGHT_PANEL_MAX_PERCENT,
    };
    let primary_min_width = match density {
        EffectiveDensity::Normal => 0,
        EffectiveDensity::Compact => COMPACT_PRIMARY_MIN_WIDTH,
    };
    let min_right_width = split_width * RIGHT_PANEL_MIN_PERCENT / 100;
    let max_position = split_width - min_right_width;
    let max_right_width = split_width * max_right_percent / 100;
    let min_position = (split_width - max_right_width)
        .max(primary_min_width.min(max_position))
        .min(max_position);
    position.clamp(min_position, max_position)
}

fn right_panel_position_ratio(split_width: i32, position: i32) -> f64 {
    if split_width <= 0 {
        return 0.0;
    }
    let right_width = split_width - position.clamp(0, split_width);
    f64::from(right_width) / f64::from(split_width)
}

fn content_split_position_from_right_panel_ratio_for_density(
    split_width: i32,
    ratio: f64,
    density: EffectiveDensity,
) -> i32 {
    let right_width = (f64::from(split_width) * ratio.clamp(0.0, 1.0)).round() as i32;
    clamp_content_split_position_for_density(split_width, split_width - right_width, density)
}

pub(super) fn content_split_initial_position_for_density(
    split_width: i32,
    saved_ratio: Option<f64>,
    density: EffectiveDensity,
) -> i32 {
    saved_ratio
        .filter(|ratio| ratio.is_finite())
        .map(|ratio| {
            content_split_position_from_right_panel_ratio_for_density(split_width, ratio, density)
        })
        .unwrap_or_else(|| {
            clamp_content_split_position_for_density(
                split_width,
                default_content_split_position(split_width),
                density,
            )
        })
}

pub(super) fn content_split_target_position_for_density(
    split_width: i32,
    previous_width: i32,
    stored_position: i32,
    current_position: i32,
    saved_ratio: Option<f64>,
    density: EffectiveDensity,
) -> i32 {
    let target_position = if previous_width <= 1 {
        content_split_initial_position_for_density(split_width, saved_ratio, density)
    } else if previous_width != split_width && stored_position > 1 {
        stored_position * split_width / previous_width
    } else if current_position > 1 {
        current_position
    } else {
        content_split_initial_position_for_density(split_width, saved_ratio, density)
    };
    clamp_content_split_position_for_density(split_width, target_position, density)
}

fn default_content_split_position(split_width: i32) -> i32 {
    split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS
}

pub(super) fn right_panel_saved_ratio(
    settings: &AppSettings,
    density: EffectiveDensity,
) -> Option<f64> {
    match density {
        EffectiveDensity::Normal => settings.right_panel_ratio,
        EffectiveDensity::Compact => settings.compact_right_panel_ratio,
    }
}

pub(super) fn update_right_panel_split_settings(
    settings: &mut AppSettings,
    split_width: i32,
    position: i32,
    density: EffectiveDensity,
) -> bool {
    if split_width <= 1 || position <= 0 || position >= split_width {
        return false;
    }

    let position = clamp_content_split_position_for_density(split_width, position, density);
    let ratio = right_panel_position_ratio(split_width, position);
    match density {
        EffectiveDensity::Normal => {
            if settings.right_panel_position == Some(position)
                && settings.right_panel_ratio == Some(ratio)
            {
                return false;
            }
            settings.right_panel_position = Some(position);
            settings.right_panel_ratio = Some(ratio);
        }
        EffectiveDensity::Compact => {
            if settings.compact_right_panel_position == Some(position)
                && settings.compact_right_panel_ratio == Some(ratio)
            {
                return false;
            }
            settings.compact_right_panel_position = Some(position);
            settings.compact_right_panel_ratio = Some(ratio);
        }
    }
    true
}

pub(super) fn clamp_home_album_page_start(
    page_start: usize,
    page_size: usize,
    album_count: usize,
) -> usize {
    if album_count == 0 {
        return 0;
    }
    let page_size = page_size.max(1);
    let last_page_start = ((album_count - 1) / page_size) * page_size;
    page_start.min(last_page_start)
}

pub(super) fn home_album_content_width(shell: &Shell) -> i32 {
    home_album_content_width_for(
        shell.route_host.width(),
        shell.content_split.width(),
        shell.content_split.position(),
        shell.state.right_panel_visible.get(),
    )
}

fn home_album_content_width_for(
    route_width: i32,
    split_width: i32,
    split_position: i32,
    right_panel_visible: bool,
) -> i32 {
    let mut route_width = if !right_panel_visible && split_width > 1 {
        split_width
    } else {
        route_width
    };
    if right_panel_visible && split_position > 1 {
        route_width = if route_width > 1 {
            route_width.min(split_position)
        } else {
            split_position
        };
    }
    if route_width <= 1 && split_width > 1 {
        route_width = split_width * MAIN_PANEL_UNITS / TOTAL_PANEL_UNITS;
    }
    (route_width - HOME_ALBUM_HORIZONTAL_MARGINS).max(HOME_ALBUM_MIN_SIZE)
}

pub(super) fn home_album_card_size(width: i32, page_size: usize) -> i32 {
    home_album_raw_card_size(width, page_size).clamp(1, HOME_ALBUM_MAX_SIZE)
}

fn home_album_raw_card_size(width: i32, page_size: usize) -> i32 {
    let page_size = page_size.max(1) as i32;
    let gaps = HOME_ALBUM_GAP * (page_size - 1);
    ((width - gaps).max(page_size)) / page_size
}

pub(super) fn restored_window_size(width: Option<i32>, height: Option<i32>) -> Option<(i32, i32)> {
    let (width, height) = (width?, height?);
    if width < MIN_RESTORED_WINDOW_WIDTH || height < MIN_RESTORED_WINDOW_HEIGHT {
        return None;
    }
    Some((
        width.clamp(MIN_RESTORED_WINDOW_WIDTH, MAX_RESTORED_WINDOW_WIDTH),
        height.clamp(MIN_RESTORED_WINDOW_HEIGHT, MAX_RESTORED_WINDOW_HEIGHT),
    ))
}

fn card_label_width_chars(size: i32) -> i32 {
    (size / 8).clamp(8, 28)
}

fn constrain_card_label(label: &gtk::Label, size: i32) {
    label.set_width_request(size);
    label.set_size_request(size, -1);
    label.set_width_chars(1);
    label.set_max_width_chars(card_label_width_chars(size));
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(false);
}

pub(super) fn clipped_card_label(label: &gtk::Label, size: i32) -> gtk::Widget {
    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("card-label-clip");
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_width_request(size);
    clip.set_size_request(size, -1);
    clip.set_min_content_width(size);
    clip.set_max_content_width(size);
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(true);
    clip.set_hexpand(false);
    clip.set_child(Some(label));
    clip.upcast()
}

pub(super) fn clipped_card_label_with_lines(
    label: &gtk::Label,
    size: i32,
    lines: i32,
) -> gtk::Widget {
    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("card-label-clip");
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_width_request(size);
    clip.set_size_request(size, card_label_height(lines));
    clip.set_min_content_width(size);
    clip.set_max_content_width(size);
    clip.set_min_content_height(card_label_height(lines));
    clip.set_max_content_height(card_label_height(lines));
    clip.set_propagate_natural_width(false);
    clip.set_propagate_natural_height(false);
    clip.set_hexpand(false);
    clip.set_child(Some(label));
    clip.upcast()
}

fn card_label_height(lines: i32) -> i32 {
    CARD_LABEL_LINE_HEIGHT * lines.max(1)
}

pub(super) fn home_album_card_height(size: i32) -> i32 {
    size + HOME_ALBUM_CARD_LABEL_GAP * 3
        + card_label_height(HOME_ALBUM_TITLE_LINES)
        + card_label_height(HOME_ALBUM_ARTIST_LINES)
        + card_label_height(HOME_ALBUM_YEAR_LINES)
}

pub(super) fn constrain_single_line_card_label(label: &gtk::Label, size: i32) {
    constrain_card_label(label, size);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
}

#[cfg(test)]
mod tests {
    use rufin_core::AppSettings;

    use super::*;

    #[test]
    fn home_album_page_size_uses_stable_content_width() {
        let three_cards_width = HOME_ALBUM_TARGET_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, None), 3);
        assert_eq!(home_album_page_size(three_cards_width + 1, None), 3);

        let four_cards_width = HOME_ALBUM_TARGET_SIZE * 4 + HOME_ALBUM_GAP * 3;
        assert_eq!(home_album_page_size(four_cards_width, None), 4);
        assert_eq!(home_album_page_size(1, None), 2);
        assert_eq!(home_album_page_size(10_000, None), HOME_ALBUM_MAX_COLUMNS);
    }

    #[test]
    fn home_album_page_size_changes_without_bouncing_near_size_bounds() {
        let three_cards_width = HOME_ALBUM_MIN_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_width - 1, Some(3)), 3);
        assert_eq!(
            home_album_page_size((HOME_ALBUM_MIN_SIZE - 20) * 3 + HOME_ALBUM_GAP * 2, Some(3)),
            2
        );

        let three_cards_max_width = HOME_ALBUM_MAX_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_max_width, Some(3)), 3);
        assert_eq!(home_album_page_size(three_cards_max_width + 3, Some(3)), 4);
    }

    #[test]
    fn home_album_page_size_adds_columns_on_wide_layouts() {
        let ten_target_cards_width = HOME_ALBUM_TARGET_SIZE * 10 + HOME_ALBUM_GAP * 9;

        assert_eq!(home_album_page_size(ten_target_cards_width, None), 10);
        assert_eq!(home_album_page_size(ten_target_cards_width, Some(7)), 9);
    }

    #[test]
    fn home_album_page_start_stays_on_full_pages() {
        assert_eq!(clamp_home_album_page_start(0, 3, 0), 0);
        assert_eq!(clamp_home_album_page_start(3, 3, 10), 3);
        assert_eq!(clamp_home_album_page_start(9, 3, 10), 9);
        assert_eq!(clamp_home_album_page_start(12, 3, 10), 9);
    }

    #[test]
    fn home_album_card_size_remains_bounded() {
        assert_eq!(home_album_card_size(10_000, 2), HOME_ALBUM_MAX_SIZE);
        assert_eq!(home_album_card_size(1, 8), 1);
    }

    #[test]
    fn home_album_width_uses_full_split_width_when_right_panel_is_hidden() {
        let stale_route_width = 640;
        let split_width = 1_000;
        assert_eq!(
            home_album_content_width_for(stale_route_width, split_width, 650, false),
            split_width - HOME_ALBUM_HORIZONTAL_MARGINS
        );
        assert_eq!(
            home_album_content_width_for(900, split_width, 650, true),
            650 - HOME_ALBUM_HORIZONTAL_MARGINS
        );
    }

    #[test]
    fn home_album_card_height_reserves_three_text_rows() {
        assert_eq!(
            home_album_card_height(180),
            180 + HOME_ALBUM_CARD_LABEL_GAP * 3 + card_label_height(3)
        );
    }

    #[test]
    fn content_split_position_limits_right_panel() {
        let density = rufin_core::EffectiveDensity::Normal;
        assert_eq!(
            clamp_content_split_position_for_density(1_000, 100, density),
            500
        );
        assert_eq!(
            clamp_content_split_position_for_density(1_000, 950, density),
            900
        );
        assert_eq!(
            clamp_content_split_position_for_density(1_000, 625, density),
            625
        );
        assert_eq!(default_content_split_position(1_000), 700);
        assert_eq!(
            content_split_initial_position_for_density(1_000, None, density),
            700
        );
        assert_eq!(
            content_split_initial_position_for_density(1_000, Some(0.25), density),
            750
        );
        assert_eq!(
            content_split_position_from_right_panel_ratio_for_density(1_000, 0.25, density),
            750
        );
        assert_eq!(right_panel_position_ratio(1_000, 750), 0.25);
        assert_eq!(
            content_split_target_position_for_density(1_000, 0, 0, 600, None, density),
            700
        );
        assert_eq!(
            content_split_target_position_for_density(1_400, 1_000, 500, 700, None, density),
            700
        );
        let mut settings = AppSettings::default();
        assert!(update_right_panel_split_settings(
            &mut settings,
            1_000,
            650,
            rufin_core::EffectiveDensity::Normal,
        ));
        assert_eq!(settings.right_panel_position, Some(650));
        assert_eq!(settings.right_panel_ratio, Some(0.35));
        assert!(update_right_panel_split_settings(
            &mut settings,
            1_000,
            760,
            rufin_core::EffectiveDensity::Compact,
        ));
        assert_eq!(settings.compact_right_panel_position, Some(760));
        assert_eq!(settings.compact_right_panel_ratio, Some(0.24));
        assert_eq!(
            right_panel_saved_ratio(&settings, rufin_core::EffectiveDensity::Normal),
            Some(0.35)
        );
        assert_eq!(
            right_panel_saved_ratio(&settings, rufin_core::EffectiveDensity::Compact),
            Some(0.24)
        );
    }

    #[test]
    fn compact_content_split_preserves_primary_width() {
        assert_eq!(
            clamp_content_split_position_for_density(
                1_000,
                100,
                rufin_core::EffectiveDensity::Compact
            ),
            620
        );
        assert_eq!(
            clamp_content_split_position_for_density(
                1_000,
                950,
                rufin_core::EffectiveDensity::Compact
            ),
            900
        );
        assert_eq!(
            content_split_initial_position_for_density(
                1_000,
                Some(0.5),
                rufin_core::EffectiveDensity::Compact
            ),
            620
        );

        let mut settings = AppSettings::default();
        assert!(update_right_panel_split_settings(
            &mut settings,
            1_000,
            500,
            rufin_core::EffectiveDensity::Compact,
        ));
        assert_eq!(settings.compact_right_panel_position, Some(620));
        assert_eq!(settings.compact_right_panel_ratio, Some(0.38));
    }

    #[test]
    fn restored_window_size_ignores_tiny_and_clamps_huge_geometry() {
        assert_eq!(restored_window_size(None, Some(700)), None);
        assert_eq!(restored_window_size(Some(400), Some(700)), None);
        assert_eq!(
            restored_window_size(Some(1061), Some(2251)),
            Some((1061, 900))
        );
        assert_eq!(
            restored_window_size(Some(1800), Some(1200)),
            Some((1400, 900))
        );
    }
}
