use super::{
    layout::{reorder_home_blocks, visibility_position_subtitle},
    reorder_sidebar_item_settings, sidebar_route_item_subtitle,
};
use crate::{SidebarRouteItem, SidebarRouteItemSettings};
use library::HomeBlockKind;
use localization::tr;

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
