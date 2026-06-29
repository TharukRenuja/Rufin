use super::*;

pub(in crate::ui) fn populate_home_block_rows(
    shell: &Rc<Shell>,
    group: &adw::PreferencesGroup,
    rows: &Rc<std::cell::RefCell<Vec<adw::ActionRow>>>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }

    let visible_blocks = shell.state.settings.borrow().home_blocks.clone();
    let ordered_blocks = home_block_row_order(&visible_blocks);
    for block in ordered_blocks {
        let active = visible_blocks.contains(&block);
        let visible_index = visible_blocks
            .iter()
            .position(|candidate| *candidate == block);
        let row = adw::ActionRow::builder()
            .title(tr(block.title()))
            .subtitle(home_block_subtitle(block, active, visible_index))
            .build();

        let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        drag.add_css_class("dim-label");
        drag.set_tooltip_text(Some(&tr("Drag to reorder")));
        row.add_prefix(&drag);

        let up = gtk::Button::from_icon_name("go-up-symbolic");
        up.add_css_class("flat");
        up.set_tooltip_text(Some(&tr("Move up")));
        up.set_valign(gtk::Align::Center);
        up.set_sensitive(visible_index.is_some_and(|index| index > 0));
        let shell_for_up = Rc::clone(shell);
        let group_for_up = group.clone();
        let rows_for_up = Rc::clone(rows);
        up.connect_clicked(move |_| {
            let mut blocks = shell_for_up.state.settings.borrow().home_blocks.clone();
            if let Some(index) = blocks.iter().position(|candidate| *candidate == block)
                && index > 0
            {
                blocks.swap(index - 1, index);
                shell_for_up.set_home_blocks(blocks);
                populate_home_block_rows(&shell_for_up, &group_for_up, &rows_for_up);
            }
        });
        row.add_suffix(&up);

        let down = gtk::Button::from_icon_name("go-down-symbolic");
        down.add_css_class("flat");
        down.set_tooltip_text(Some(&tr("Move down")));
        down.set_valign(gtk::Align::Center);
        down.set_sensitive(visible_index.is_some_and(|index| index + 1 < visible_blocks.len()));
        let shell_for_down = Rc::clone(shell);
        let group_for_down = group.clone();
        let rows_for_down = Rc::clone(rows);
        down.connect_clicked(move |_| {
            let mut blocks = shell_for_down.state.settings.borrow().home_blocks.clone();
            if let Some(index) = blocks.iter().position(|candidate| *candidate == block)
                && index + 1 < blocks.len()
            {
                blocks.swap(index, index + 1);
                shell_for_down.set_home_blocks(blocks);
                populate_home_block_rows(&shell_for_down, &group_for_down, &rows_for_down);
            }
        });
        row.add_suffix(&down);

        let toggle = gtk::Switch::builder()
            .active(active)
            .valign(gtk::Align::Center)
            .sensitive(!active || visible_blocks.len() > 1)
            .build();
        let shell_for_toggle = Rc::clone(shell);
        let group_for_toggle = group.clone();
        let rows_for_toggle = Rc::clone(rows);
        toggle.connect_active_notify(move |toggle| {
            let mut blocks = shell_for_toggle.state.settings.borrow().home_blocks.clone();
            let currently_active = blocks.contains(&block);
            let requested = toggle.is_active();
            if requested == currently_active {
                return;
            }
            if requested {
                let order = home_block_row_order(&blocks);
                insert_home_block_in_order(&mut blocks, block, &order);
            } else if blocks.len() > 1 {
                blocks.retain(|candidate| *candidate != block);
            }
            shell_for_toggle.set_home_blocks(blocks);
            populate_home_block_rows(&shell_for_toggle, &group_for_toggle, &rows_for_toggle);
        });
        row.add_suffix(&toggle);
        row.set_activatable_widget(Some(&toggle));

        let source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::MOVE)
            .build();
        let block_id = home_block_drag_id(block).to_string();
        source.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(&block_id.to_value()))
        });
        drag.add_controller(source);

        let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
        let shell_for_drop = Rc::clone(shell);
        let group_for_drop = group.clone();
        let rows_for_drop = Rc::clone(rows);
        let row_for_drop = row.clone();
        drop_target.connect_drop(move |_, value, _, y| {
            let Ok(source_id) = value.get::<String>() else {
                return false;
            };
            let Some(source_block) = home_block_from_drag_id(&source_id) else {
                return false;
            };
            if source_block == block {
                return false;
            }
            let after = y > f64::from(row_for_drop.height()) / 2.0;
            let mut blocks = shell_for_drop.state.settings.borrow().home_blocks.clone();
            if !reorder_home_blocks(&mut blocks, source_block, block, after) {
                return false;
            }
            shell_for_drop.set_home_blocks(blocks);
            populate_home_block_rows(&shell_for_drop, &group_for_drop, &rows_for_drop);
            true
        });
        row.add_controller(drop_target);

        group.add(&row);
        rows.borrow_mut().push(row);
    }
}
pub(in crate::ui) fn home_block_row_order(visible_blocks: &[HomeBlockKind]) -> Vec<HomeBlockKind> {
    let mut blocks = visible_blocks.to_vec();
    for block in HomeBlockKind::all() {
        if !blocks.contains(&block) {
            blocks.push(block);
        }
    }
    blocks
}
pub(in crate::ui) fn insert_home_block_in_order(
    blocks: &mut Vec<HomeBlockKind>,
    block: HomeBlockKind,
    order: &[HomeBlockKind],
) {
    if blocks.contains(&block) {
        return;
    }
    let target_order = order
        .iter()
        .position(|candidate| *candidate == block)
        .unwrap_or(usize::MAX);
    let insert_at = blocks
        .iter()
        .position(|candidate| {
            order
                .iter()
                .position(|ordered| ordered == candidate)
                .unwrap_or(usize::MAX)
                > target_order
        })
        .unwrap_or(blocks.len());
    blocks.insert(insert_at, block);
}
pub(in crate::ui) fn home_block_subtitle(
    block: HomeBlockKind,
    active: bool,
    visible_index: Option<usize>,
) -> String {
    if let Some(index) = visible_index {
        return format!("{} {}", tr("Position"), index + 1);
    }
    match block.section_kind() {
        Some(_) => tr("Hidden server section"),
        None if active => tr("Visible"),
        None => tr("Hidden"),
    }
}
pub(in crate::ui) fn reorder_home_blocks(
    blocks: &mut Vec<HomeBlockKind>,
    source: HomeBlockKind,
    target: HomeBlockKind,
    after: bool,
) -> bool {
    if source == target {
        return false;
    }
    let before = blocks.clone();
    let Some(source_index) = blocks.iter().position(|block| *block == source) else {
        return false;
    };
    let block = blocks.remove(source_index);
    let Some(mut target_index) = blocks.iter().position(|block| *block == target) else {
        blocks.insert(source_index.min(blocks.len()), block);
        return false;
    };
    if after {
        target_index += 1;
    }
    blocks.insert(target_index.min(blocks.len()), block);
    *blocks != before
}
pub(in crate::ui) fn home_block_drag_id(block: HomeBlockKind) -> &'static str {
    match block {
        HomeBlockKind::Showcase => "Showcase",
        HomeBlockKind::Explore => "Explore",
        HomeBlockKind::MostPlayed => "MostPlayed",
        HomeBlockKind::NewlyAdded => "NewlyAdded",
        HomeBlockKind::RecentlyPlayed => "RecentlyPlayed",
        HomeBlockKind::RecentlyReleased => "RecentlyReleased",
        HomeBlockKind::Genres => "Genres",
    }
}
pub(in crate::ui) fn home_block_from_drag_id(id: &str) -> Option<HomeBlockKind> {
    HomeBlockKind::all()
        .into_iter()
        .find(|block| home_block_drag_id(*block) == id)
}
pub(in crate::ui) fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build()
}
pub(in crate::ui) fn left_sidebar_row<F>(
    title: &str,
    mode: LeftSidebarMode,
    on_selected: F,
) -> adw::ActionRow
where
    F: Fn(u32) + 'static,
{
    selection_row(
        title,
        &[tr("Full"), tr("Compact"), tr("Hidden")],
        left_sidebar_mode_index(mode),
        on_selected,
    )
}
pub(in crate::ui) fn left_sidebar_mode_index(mode: LeftSidebarMode) -> u32 {
    match mode {
        LeftSidebarMode::Full => 0,
        LeftSidebarMode::Compact => 1,
        LeftSidebarMode::Hidden => 2,
    }
}
pub(in crate::ui) fn left_sidebar_mode_from_index(index: u32) -> LeftSidebarMode {
    match index {
        1 => LeftSidebarMode::Compact,
        2 => LeftSidebarMode::Hidden,
        _ => LeftSidebarMode::Full,
    }
}
pub(in crate::ui) fn right_sidebar_row<F>(
    title: &str,
    mode: RightSidebarMode,
    on_selected: F,
) -> adw::ActionRow
where
    F: Fn(u32) + 'static,
{
    selection_row(
        title,
        &[
            tr("Hidden"),
            tr("Compact"),
            tr("Default"),
            tr("Comfortable"),
            tr("Spacious"),
        ],
        right_sidebar_mode_index(mode),
        on_selected,
    )
}
pub(in crate::ui) fn right_sidebar_mode_index(mode: RightSidebarMode) -> u32 {
    match mode {
        RightSidebarMode::Hidden => 0,
        RightSidebarMode::Compact => 1,
        RightSidebarMode::Default => 2,
        RightSidebarMode::Comfortable => 3,
        RightSidebarMode::Spacious => 4,
    }
}
pub(in crate::ui) fn right_sidebar_mode_from_index(index: u32) -> RightSidebarMode {
    match index {
        1 => RightSidebarMode::Compact,
        2 => RightSidebarMode::Default,
        3 => RightSidebarMode::Comfortable,
        4 => RightSidebarMode::Spacious,
        _ => RightSidebarMode::Hidden,
    }
}
pub(in crate::ui) fn discord_display_index(display_type: DiscordDisplayType) -> u32 {
    match display_type {
        DiscordDisplayType::Application => 0,
        DiscordDisplayType::Song => 1,
        DiscordDisplayType::Artist => 2,
    }
}
pub(in crate::ui) fn discord_display_from_index(index: u32) -> DiscordDisplayType {
    match index {
        1 => DiscordDisplayType::Song,
        2 => DiscordDisplayType::Artist,
        _ => DiscordDisplayType::Application,
    }
}
pub(in crate::ui) fn discord_link_index(link_type: DiscordLinkType) -> u32 {
    match link_type {
        DiscordLinkType::None => 0,
        DiscordLinkType::LastFm => 1,
        DiscordLinkType::MusicBrainz => 2,
        DiscordLinkType::MusicBrainzLastFm => 3,
    }
}
pub(in crate::ui) fn discord_link_from_index(index: u32) -> DiscordLinkType {
    match index {
        1 => DiscordLinkType::LastFm,
        2 => DiscordLinkType::MusicBrainz,
        3 => DiscordLinkType::MusicBrainzLastFm,
        _ => DiscordLinkType::None,
    }
}
