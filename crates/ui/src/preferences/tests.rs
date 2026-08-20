use super::{
    general::{
        accent_preference_from_index, accent_preference_index, theme_preference_from_index,
        theme_preference_index,
    },
    layout::{reorder_home_blocks, visibility_position_subtitle},
    quality_accessible_title, quality_button_title, reorder_sidebar_item_settings,
    sidebar_route_item_subtitle,
};
use crate::{AccentPreference, SidebarRouteItem, SidebarRouteItemSettings, ThemePreference};
use library::{HomeBlockKind, StreamQuality};
use localization::tr;

#[test]
fn theme_preference_selector_round_trips_every_choice() {
    for preference in [
        ThemePreference::System,
        ThemePreference::Light,
        ThemePreference::Dark,
    ] {
        assert_eq!(
            theme_preference_from_index(theme_preference_index(preference)),
            preference
        );
    }
}

#[test]
fn accent_preference_selector_round_trips_every_choice() {
    for preference in AccentPreference::ALL {
        assert_eq!(
            accent_preference_from_index(accent_preference_index(preference)),
            preference
        );
    }
}

#[test]
fn quality_buttons_show_the_bitrate_without_repeating_the_unit() {
    let quality = StreamQuality::MaxBitrateKbps(320);

    assert_eq!(quality_button_title(quality), "320");
    assert_eq!(quality_accessible_title(quality), tr("320 kbps"));
    assert_eq!(
        quality_button_title(StreamQuality::Original),
        tr("Original")
    );
}

#[test]
fn reorder_list_subtitles_show_visibility_before_position() {
    assert_eq!(
        visibility_position_subtitle(true, 0),
        format!("{} · {} 1", tr("Visible"), tr("Position"))
    );
    assert_eq!(
        visibility_position_subtitle(false, 3),
        format!("{} · {} 4", tr("Hidden"), tr("Position"))
    );

    let hidden_sidebar_item = SidebarRouteItemSettings {
        item: SidebarRouteItem::Albums,
        visible: false,
    };
    assert_eq!(
        sidebar_route_item_subtitle(&hidden_sidebar_item, 2),
        format!("{} · {} 3", tr("Hidden"), tr("Position"))
    );
}

#[test]
fn reorder_sidebar_target() {
    let mut items = sidebar_settings(&[
        SidebarRouteItem::Home,
        SidebarRouteItem::Favorites,
        SidebarRouteItem::Albums,
        SidebarRouteItem::Tracks,
    ]);

    assert!(reorder_sidebar_item_settings(
        &mut items,
        SidebarRouteItem::Tracks,
        SidebarRouteItem::Favorites,
        false,
    ));
    assert_eq!(
        sidebar_item_order(&items),
        vec![
            SidebarRouteItem::Home,
            SidebarRouteItem::Tracks,
            SidebarRouteItem::Favorites,
            SidebarRouteItem::Albums,
        ]
    );

    assert!(reorder_sidebar_item_settings(
        &mut items,
        SidebarRouteItem::Home,
        SidebarRouteItem::Albums,
        true,
    ));
    assert_eq!(
        sidebar_item_order(&items),
        vec![
            SidebarRouteItem::Tracks,
            SidebarRouteItem::Favorites,
            SidebarRouteItem::Albums,
            SidebarRouteItem::Home,
        ]
    );

    assert!(!reorder_sidebar_item_settings(
        &mut items,
        SidebarRouteItem::Favorites,
        SidebarRouteItem::Tracks,
        true,
    ));
}
#[test]
fn reorder_move_blocks() {
    let mut blocks = vec![
        HomeBlockKind::Showcase,
        HomeBlockKind::Explore,
        HomeBlockKind::Genres,
    ];

    assert!(reorder_home_blocks(
        &mut blocks,
        HomeBlockKind::Genres,
        HomeBlockKind::Showcase,
        false,
    ));
    assert_eq!(
        blocks,
        vec![
            HomeBlockKind::Genres,
            HomeBlockKind::Showcase,
            HomeBlockKind::Explore,
        ]
    );

    let before = blocks.clone();
    assert!(!reorder_home_blocks(
        &mut blocks,
        HomeBlockKind::MostPlayed,
        HomeBlockKind::Showcase,
        false,
    ));
    assert_eq!(blocks, before);

    assert!(!reorder_home_blocks(
        &mut blocks,
        HomeBlockKind::Explore,
        HomeBlockKind::RecentlyPlayed,
        true,
    ));
    assert_eq!(blocks, before);
}
fn sidebar_settings(items: &[SidebarRouteItem]) -> Vec<SidebarRouteItemSettings> {
    items
        .iter()
        .copied()
        .map(|item| SidebarRouteItemSettings {
            item,
            visible: true,
        })
        .collect()
}
fn sidebar_item_order(items: &[SidebarRouteItemSettings]) -> Vec<SidebarRouteItem> {
    items.iter().map(|entry| entry.item).collect()
}
