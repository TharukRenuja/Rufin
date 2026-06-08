use adw::prelude::*;
use rufin_core::{LayoutProfile, LayoutSettings, LeftSidebarMode, RightSidebarMode};

use super::Shell;

pub(super) const COMPACT_RAIL_WIDTH: i32 = 64;
pub(super) const NORMAL_SIDEBAR_WIDTH: i32 = 176;
pub(super) const RIGHT_SIDEBAR_COMPACT_WIDTH: i32 = 250;
pub(super) const RIGHT_SIDEBAR_DEFAULT_WIDTH: i32 = 300;
pub(super) const RIGHT_SIDEBAR_COMFORTABLE_WIDTH: i32 = 400;
pub(super) const RIGHT_SIDEBAR_SPACIOUS_WIDTH: i32 = 500;
pub(super) const MIN_USEFUL_MAIN_WIDTH: i32 = 550;
pub(super) const MIN_APP_WINDOW_WIDTH: i32 = COMPACT_RAIL_WIDTH + MIN_USEFUL_MAIN_WIDTH;
pub(super) const HOME_ALBUM_GAP: i32 = 14;
pub(super) const DETAIL_ROUTE_SCROLL_GUTTER: i32 = 24;
const HOME_ALBUM_MIN_SIZE: i32 = 150;
const HOME_ALBUM_TARGET_SIZE: i32 = 180;
const HOME_ALBUM_MAX_SIZE: i32 = 210;
const HOME_ALBUM_MIN_COLUMNS: usize = 1;
const HOME_ALBUM_MAX_COLUMNS: usize = 12;
pub(super) const PRIMARY_ROUTE_MARGIN_START: i32 = 0;
pub(super) const PRIMARY_ROUTE_MARGIN_END: i32 = 28;
const HOME_ALBUM_HORIZONTAL_MARGINS: i32 = PRIMARY_ROUTE_MARGIN_START + PRIMARY_ROUTE_MARGIN_END;
const CARD_LABEL_LINE_HEIGHT: i32 = 20;
pub(super) const HOME_ALBUM_CARD_LABEL_GAP: i32 = 4;
pub(super) const HOME_ALBUM_TITLE_LINES: i32 = 1;
pub(super) const HOME_ALBUM_ARTIST_LINES: i32 = 1;
pub(super) const HOME_ALBUM_YEAR_LINES: i32 = 1;

pub(super) const MIN_RESTORED_WINDOW_HEIGHT: i32 = 360;
const LARGE_POPUP_HEIGHT_PERCENT: i32 = 85;
const LARGE_POPUP_WIDTH_NUMERATOR: i32 = 11;
const LARGE_POPUP_WIDTH_DENOMINATOR: i32 = 10;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActiveLayoutProfile {
    Default,
    Narrow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct ResolvedLayout {
    pub(super) profile: ActiveLayoutProfile,
    pub(super) left_sidebar: LeftSidebarMode,
    pub(super) right_sidebar: RightSidebarMode,
    pub(super) right_sidebar_width: i32,
    pub(super) main_width: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct SidebarWidths {
    pub(super) full: i32,
    pub(super) compact: i32,
}

impl Default for SidebarWidths {
    fn default() -> Self {
        Self {
            full: NORMAL_SIDEBAR_WIDTH,
            compact: COMPACT_RAIL_WIDTH,
        }
    }
}

fn sidebar_width(mode: LeftSidebarMode, widths: SidebarWidths) -> i32 {
    match mode {
        LeftSidebarMode::Full => widths.full,
        LeftSidebarMode::Compact => widths.compact,
    }
}

pub(super) fn right_sidebar_width(mode: RightSidebarMode) -> i32 {
    match mode {
        RightSidebarMode::Hidden => 0,
        RightSidebarMode::Compact => RIGHT_SIDEBAR_COMPACT_WIDTH,
        RightSidebarMode::Default => RIGHT_SIDEBAR_DEFAULT_WIDTH,
        RightSidebarMode::Comfortable => RIGHT_SIDEBAR_COMFORTABLE_WIDTH,
        RightSidebarMode::Spacious => RIGHT_SIDEBAR_SPACIOUS_WIDTH,
    }
}

pub(super) fn resolve_layout(settings: &LayoutSettings, window_width: i32) -> ResolvedLayout {
    resolve_layout_with_sidebar_widths(settings, window_width, SidebarWidths::default())
}

pub(in crate::ui) fn resolve_layout_with_sidebar_widths(
    settings: &LayoutSettings,
    window_width: i32,
    sidebar_widths: SidebarWidths,
) -> ResolvedLayout {
    let window_width = window_width.max(MIN_APP_WINDOW_WIDTH);
    let (profile, configured) =
        if settings.narrow_enabled && window_width < settings.narrow_threshold {
            (ActiveLayoutProfile::Narrow, &settings.narrow_profile)
        } else {
            (ActiveLayoutProfile::Default, &settings.default_profile)
        };
    resolve_layout_for_profile(profile, configured, window_width, sidebar_widths)
}

fn resolve_layout_for_profile(
    profile: ActiveLayoutProfile,
    configured: &LayoutProfile,
    window_width: i32,
    sidebar_widths: SidebarWidths,
) -> ResolvedLayout {
    let mut left_sidebar = configured.left_sidebar;
    let mut right_sidebar = resolved_right_sidebar_for_width(
        configured.right_sidebar,
        window_width - sidebar_width(left_sidebar, sidebar_widths),
    );
    let mut resolved_right_sidebar_width = right_sidebar_width(right_sidebar);
    let mut main_width =
        window_width - sidebar_width(left_sidebar, sidebar_widths) - resolved_right_sidebar_width;

    if main_width < MIN_USEFUL_MAIN_WIDTH && left_sidebar == LeftSidebarMode::Full {
        left_sidebar = LeftSidebarMode::Compact;
        right_sidebar = resolved_right_sidebar_for_width(
            right_sidebar,
            window_width - sidebar_width(left_sidebar, sidebar_widths),
        );
        resolved_right_sidebar_width = right_sidebar_width(right_sidebar);
        main_width = window_width
            - sidebar_width(left_sidebar, sidebar_widths)
            - resolved_right_sidebar_width;
    }

    ResolvedLayout {
        profile,
        left_sidebar,
        right_sidebar,
        right_sidebar_width: resolved_right_sidebar_width,
        main_width: main_width.max(1),
    }
}

fn resolved_right_sidebar_for_width(
    configured: RightSidebarMode,
    available_after_left_sidebar: i32,
) -> RightSidebarMode {
    let mut mode = configured;
    while mode.is_visible()
        && available_after_left_sidebar - right_sidebar_width(mode) < MIN_USEFUL_MAIN_WIDTH
    {
        mode = smaller_right_sidebar_mode(mode);
    }
    mode
}

fn smaller_right_sidebar_mode(mode: RightSidebarMode) -> RightSidebarMode {
    match mode {
        RightSidebarMode::Spacious => RightSidebarMode::Comfortable,
        RightSidebarMode::Comfortable => RightSidebarMode::Default,
        RightSidebarMode::Default => RightSidebarMode::Compact,
        RightSidebarMode::Compact | RightSidebarMode::Hidden => RightSidebarMode::Hidden,
    }
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
    home_album_content_width_for(route_content_width(shell))
}

pub(super) fn route_content_width(shell: &Shell) -> i32 {
    if shell.state.startup_route_render_pending.get() {
        return shell.state.main_content_width.get().max(1);
    }

    route_content_width_for(
        shell.route_host.width(),
        shell.state.main_content_width.get(),
    )
}

pub(super) fn detail_route_inner_width(shell: &Shell, horizontal_inset: i32) -> i32 {
    route_content_width(shell)
        .saturating_sub(horizontal_inset)
        .saturating_sub(DETAIL_ROUTE_SCROLL_GUTTER)
        .max(1)
}

pub(super) fn detail_showcase_cover_size(width: i32) -> i32 {
    if width < 520 {
        width.clamp(72, 168)
    } else if width < 760 {
        220
    } else {
        250
    }
}

pub(super) fn detail_showcase_spacing(width: i32) -> i32 {
    if width < 520 {
        12
    } else if width < 760 {
        14
    } else {
        16
    }
}

fn route_content_width_for(route_width: i32, resolved_width: i32) -> i32 {
    match (route_width > 1, resolved_width > 1) {
        (true, true) => route_width.min(resolved_width),
        (true, false) => route_width,
        (false, true) => resolved_width,
        (false, false) => 1,
    }
}

fn home_album_content_width_for(width: i32) -> i32 {
    (width.max(1) - HOME_ALBUM_HORIZONTAL_MARGINS).max(1)
}

pub(super) fn home_album_card_size(width: i32, page_size: usize) -> i32 {
    home_album_raw_card_size(width, page_size).clamp(1, HOME_ALBUM_MAX_SIZE)
}

fn home_album_raw_card_size(width: i32, page_size: usize) -> i32 {
    let page_size = page_size.max(1) as i32;
    let gaps = HOME_ALBUM_GAP * (page_size - 1);
    ((width - gaps).max(page_size)) / page_size
}

pub(super) fn large_popup_content_height(app_height: i32, fallback_height: i32) -> i32 {
    if app_height <= 0 {
        return fallback_height;
    }
    (app_height * LARGE_POPUP_HEIGHT_PERCENT + 50) / 100
}

pub(super) fn large_popup_content_width(base_width: i32) -> i32 {
    (base_width * LARGE_POPUP_WIDTH_NUMERATOR + LARGE_POPUP_WIDTH_DENOMINATOR / 2)
        / LARGE_POPUP_WIDTH_DENOMINATOR
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
    use rufin_core::{LayoutSettings, RightSidebarMode};

    use super::*;

    #[test]
    fn album_page_width() {
        let three_cards_width = HOME_ALBUM_TARGET_SIZE * 3 + HOME_ALBUM_GAP * 2;
        assert_eq!(home_album_page_size(three_cards_width, None), 3);
        assert_eq!(home_album_page_size(three_cards_width + 1, None), 3);

        let four_cards_width = HOME_ALBUM_TARGET_SIZE * 4 + HOME_ALBUM_GAP * 3;
        assert_eq!(home_album_page_size(four_cards_width, None), 4);
        assert_eq!(home_album_page_size(1, None), 1);
        assert_eq!(home_album_page_size(10_000, None), HOME_ALBUM_MAX_COLUMNS);
    }

    #[test]
    fn layout_allow_panes() {
        let tight_width = HOME_ALBUM_MIN_SIZE + HOME_ALBUM_GAP - 1;

        assert_eq!(home_album_page_size(tight_width, None), 1);
        assert_eq!(
            home_album_content_width_for(120),
            120 - HOME_ALBUM_HORIZONTAL_MARGINS
        );
    }

    #[test]
    fn layout_change_bounds() {
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
    fn layout_add_layouts() {
        let ten_target_cards_width = HOME_ALBUM_TARGET_SIZE * 10 + HOME_ALBUM_GAP * 9;

        assert_eq!(home_album_page_size(ten_target_cards_width, None), 10);
        assert_eq!(home_album_page_size(ten_target_cards_width, Some(7)), 9);
    }

    #[test]
    fn layout_stay_page() {
        assert_eq!(clamp_home_album_page_start(0, 3, 0), 0);
        assert_eq!(clamp_home_album_page_start(3, 3, 10), 3);
        assert_eq!(clamp_home_album_page_start(9, 3, 10), 9);
        assert_eq!(clamp_home_album_page_start(12, 3, 10), 9);
    }

    #[test]
    fn layout_home_bounded() {
        assert_eq!(home_album_card_size(10_000, 2), HOME_ALBUM_MAX_SIZE);
        assert_eq!(home_album_card_size(1, 8), 1);
    }

    #[test]
    fn album_alloc_width() {
        assert_eq!(
            home_album_content_width_for(900),
            900 - HOME_ALBUM_HORIZONTAL_MARGINS
        );
        assert_eq!(
            home_album_content_width_for(650),
            650 - HOME_ALBUM_HORIZONTAL_MARGINS
        );
    }

    #[test]
    fn layout_cap_width() {
        assert_eq!(route_content_width_for(900, 500), 500);
        assert_eq!(route_content_width_for(900, 1), 900);
        assert_eq!(route_content_width_for(1, 500), 500);
    }

    #[test]
    fn detail_cover_fits_narrow_width() {
        assert_eq!(detail_showcase_cover_size(120), 120);
        assert_eq!(detail_showcase_cover_size(40), 72);
        assert_eq!(detail_showcase_cover_size(519), 168);
        assert_eq!(detail_showcase_cover_size(520), 220);
    }

    #[test]
    fn layout_home_text() {
        assert_eq!(
            home_album_card_height(180),
            180 + HOME_ALBUM_CARD_LABEL_GAP * 3 + card_label_height(3)
        );
    }

    #[test]
    fn layout_use_default() {
        let settings = LayoutSettings::default();
        let resolved = resolve_layout(&settings, 1_500);

        assert_eq!(resolved.profile, ActiveLayoutProfile::Default);
        assert_eq!(resolved.left_sidebar, LeftSidebarMode::Full);
        assert_eq!(resolved.right_sidebar, RightSidebarMode::Comfortable);
        assert_eq!(
            resolved.right_sidebar_width,
            RIGHT_SIDEBAR_COMFORTABLE_WIDTH
        );
    }

    #[test]
    fn layout_use_narrow() {
        let settings = LayoutSettings::default();
        let resolved = resolve_layout(&settings, 950);

        assert_eq!(resolved.profile, ActiveLayoutProfile::Narrow);
        assert_eq!(resolved.left_sidebar, LeftSidebarMode::Compact);
        assert_eq!(resolved.right_sidebar, RightSidebarMode::Default);
        assert_eq!(resolved.right_sidebar_width, RIGHT_SIDEBAR_DEFAULT_WIDTH);
    }

    #[test]
    fn layout_degrades_sidebar() {
        let mut settings = LayoutSettings {
            narrow_enabled: false,
            ..Default::default()
        };
        settings.default_profile.right_sidebar = RightSidebarMode::Spacious;

        let resolved = resolve_layout(&settings, NORMAL_SIDEBAR_WIDTH + 800);

        assert_eq!(resolved.left_sidebar, LeftSidebarMode::Full);
        assert_eq!(resolved.right_sidebar, RightSidebarMode::Compact);
        assert!(resolved.main_width >= MIN_USEFUL_MAIN_WIDTH);
    }

    #[test]
    fn layout_compacts_fallback() {
        let mut settings = LayoutSettings::default();
        settings.default_profile.right_sidebar = RightSidebarMode::Hidden;

        let resolved = resolve_layout(&settings, NORMAL_SIDEBAR_WIDTH + MIN_USEFUL_MAIN_WIDTH - 10);

        assert_eq!(resolved.left_sidebar, LeftSidebarMode::Compact);
        assert_eq!(resolved.right_sidebar, RightSidebarMode::Hidden);
    }

    #[test]
    fn layout_keeps_main_floor_at_window_minimum() {
        let settings = LayoutSettings::default();
        let resolved = resolve_layout(&settings, 1);

        assert_eq!(resolved.left_sidebar, LeftSidebarMode::Compact);
        assert_eq!(resolved.right_sidebar, RightSidebarMode::Hidden);
        assert_eq!(resolved.main_width, MIN_USEFUL_MAIN_WIDTH);
    }

    #[test]
    fn layout_scale_width() {
        assert_eq!(large_popup_content_height(1_000, 640), 850);
        assert_eq!(large_popup_content_height(0, 640), 640);
        assert_eq!(large_popup_content_width(560), 616);
        assert_eq!(large_popup_content_width(620), 682);
    }
}
