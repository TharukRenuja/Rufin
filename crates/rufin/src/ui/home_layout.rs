use adw::prelude::*;

use crate::i18n::tr;

use super::icon_button;

const HOME_SHOWCASE_TEXT_MIN_WIDTH: i32 = 420;
const HOME_SHOWCASE_COMPACT_WIDTH: i32 = 640;
const HOME_SHOWCASE_FULL_COVER: i32 = 196;
const HOME_SHOWCASE_MIN_COVER: i32 = 150;
const HOME_SHOWCASE_TIGHT_WIDTH: i32 = 520;

pub(super) struct HomeSectionHeader {
    pub(super) root: gtk::Box,
    pub(super) previous: gtk::Button,
    pub(super) next: gtk::Button,
    pub(super) refresh: gtk::Button,
}

pub(super) fn home_section_header(title: &str) -> HomeSectionHeader {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header.set_width_request(1);

    let heading = gtk::Label::new(Some(&tr(title)));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    heading.set_hexpand(true);
    heading.set_width_chars(1);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.append(&heading);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    controls.set_halign(gtk::Align::End);
    controls.set_hexpand(false);

    let previous = icon_button("go-previous-symbolic", "Previous page");
    let next = icon_button("go-next-symbolic", "Next page");
    let refresh = icon_button("view-refresh-symbolic", "Refresh section");
    next.add_css_class("home-section-control-button");
    refresh.add_css_class("home-section-control-button");
    controls.append(&previous);
    controls.append(&next);
    controls.append(&refresh);
    header.append(&controls);

    HomeSectionHeader {
        root: header,
        previous,
        next,
        refresh,
    }
}

pub(in crate::ui) fn home_layout_width_signature(width: i32) -> i32 {
    let showcase: i32 = match home_showcase_mode(width) {
        HomeShowcaseMode::CoverOnly => 0,
        HomeShowcaseMode::Compact => 1,
        HomeShowcaseMode::Full => 2,
    };
    let cover = home_showcase_cover_size(width);
    showcase.saturating_mul(256).saturating_add(cover)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HomeShowcaseMode {
    CoverOnly,
    Compact,
    Full,
}

pub(super) fn home_showcase_mode(width: i32) -> HomeShowcaseMode {
    if width < HOME_SHOWCASE_TEXT_MIN_WIDTH {
        HomeShowcaseMode::CoverOnly
    } else if home_showcase_is_compact(width) {
        HomeShowcaseMode::Compact
    } else {
        HomeShowcaseMode::Full
    }
}

pub(super) fn home_showcase_cover_size(width: i32) -> i32 {
    if width < HOME_SHOWCASE_TEXT_MIN_WIDTH {
        width.clamp(96, HOME_SHOWCASE_MIN_COVER)
    } else if width < HOME_SHOWCASE_COMPACT_WIDTH {
        HOME_SHOWCASE_MIN_COVER
            + ((width - HOME_SHOWCASE_TEXT_MIN_WIDTH)
                * (HOME_SHOWCASE_FULL_COVER - HOME_SHOWCASE_MIN_COVER)
                / (HOME_SHOWCASE_COMPACT_WIDTH - HOME_SHOWCASE_TEXT_MIN_WIDTH))
    } else {
        HOME_SHOWCASE_FULL_COVER
    }
}

pub(super) fn home_showcase_is_compact(width: i32) -> bool {
    width < HOME_SHOWCASE_COMPACT_WIDTH
}

pub(super) fn home_showcase_spacing(width: i32) -> i32 {
    if width < HOME_SHOWCASE_TIGHT_WIDTH {
        12
    } else if home_showcase_is_compact(width) {
        18
    } else {
        24
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_compacts_width_bound_widgets() {
        assert_eq!(home_showcase_mode(419), HomeShowcaseMode::CoverOnly);
        assert_eq!(home_showcase_mode(420), HomeShowcaseMode::Compact);
        assert_eq!(home_showcase_mode(640), HomeShowcaseMode::Full);
        assert_eq!(home_showcase_cover_size(419), HOME_SHOWCASE_MIN_COVER);
        assert_eq!(home_showcase_cover_size(420), HOME_SHOWCASE_MIN_COVER);
        assert_eq!(home_showcase_cover_size(450), 156);
        assert_eq!(home_showcase_cover_size(520), 170);
        assert_eq!(home_showcase_cover_size(639), 195);
        assert_eq!(home_showcase_cover_size(640), HOME_SHOWCASE_FULL_COVER);
        assert_ne!(
            home_layout_width_signature(423),
            home_layout_width_signature(520)
        );
    }
}
