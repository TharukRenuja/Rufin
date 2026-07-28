use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib;

use crate::layout::{
    configure_fill_width_clip, large_popup_content_height, large_popup_content_width,
    width_allocation_owner,
};
use crate::localization::{
    bind_drop_down_options, bind_widget_tooltip, bind_widget_tooltip_with, localized_label,
};
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use crate::shell::actions::{ADD_ICON, MORE_ICON, sort_order_icon};
use crate::shell::layout::WINDOW_CHROME_MARGIN_END;
use crate::shell::route::{MountedRoute, MountedRouteItemNavigation, MountedRouteResume};
use crate::{
    LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings, available_sort_fields,
};
use localization::{msgid, tr};

use super::collections::library_route_inset;
use super::library_fields::{
    layout_button_content, layout_icon, layout_title, next_layout, populate_library_field_rows,
    supported_layouts, sync_layout_buttons,
};
use super::route_layout::ROUTE_TOP_MARGIN;

const LIBRARY_CONFIG_DIALOG_WIDTH: i32 = 620;
const LIBRARY_CONFIG_DIALOG_HEIGHT: i32 = 560;
const LIBRARY_ROUTE_BOTTOM_MARGIN: i32 = 8;
const LIBRARY_TOOLBAR_END_MARGIN: i32 = 10;
const LIBRARY_TOOLBAR_CONTROL_SPACING: i32 = 12;
const LIBRARY_TOOLBAR_ICON_BUTTON_WIDTH: i32 = 34;
const LIBRARY_TOOLBAR_CLOSE_VISIBLE_SIZE: i32 = 24;
const LIBRARY_TOOLBAR_SORT_MIN_WIDTH: i32 = 112;
const LIBRARY_TOOLBAR_SORT_CHAR_WIDTH: i32 = 8;
const LIBRARY_TOOLBAR_SORT_HORIZONTAL_PADDING: i32 = 44;
const LIBRARY_TOOLBAR_COMPACT_COMMAND_WIDTH: i32 = 760;
const LIBRARY_TOOLBAR_WINDOW_CONTROLS_RESERVE: i32 =
    WINDOW_CHROME_MARGIN_END + LIBRARY_TOOLBAR_CLOSE_VISIBLE_SIZE + LIBRARY_TOOLBAR_CONTROL_SPACING;

pub(crate) struct LibraryPageShellOptions {
    pub(crate) key: LibraryListKey,
    pub(crate) empty: bool,
    pub(crate) empty_body: &'static str,
    pub(crate) search: gtk::SearchEntry,
    pub(crate) content: gtk::Widget,
}

#[derive(Clone)]
pub(crate) struct LibraryPageShell {
    widget: gtk::Widget,
    contents: gtk::Stack,
    toolbar: LibraryToolbarProjection,
}

impl LibraryPageShell {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn mounted_route(
        &self,
        resume: MountedRouteResume,
        item_navigation: MountedRouteItemNavigation,
    ) -> MountedRoute {
        MountedRoute::new(self.widget(), resume).with_item_navigation(item_navigation)
    }

    pub(crate) fn apply_library_list_settings(
        &self,
        key: LibraryListKey,
        settings: &LibraryListSettings,
    ) {
        self.toolbar.apply(key, settings);
    }

    pub(crate) fn set_empty(&self, empty: bool) {
        self.contents
            .set_visible_child_name(if empty { "empty" } else { "content" });
    }
}

#[derive(Clone)]
pub(crate) struct LibraryToolbarProjection {
    key: LibraryListKey,
    widget: gtk::Widget,
    sort_dropdown: gtk::DropDown,
    direction: gtk::Button,
    layout: gtk::Button,
    layout_mode: Rc<Cell<LibraryLayout>>,
    syncing: Rc<Cell<bool>>,
}

impl LibraryToolbarProjection {
    pub(crate) fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub(crate) fn set_layout_control_visible(&self, visible: bool) {
        self.layout.set_visible(visible);
    }

    pub(crate) fn apply(&self, key: LibraryListKey, settings: &LibraryListSettings) {
        if key != self.key {
            return;
        }
        self.syncing.set(true);
        self.sort_dropdown.set_selected(
            available_sort_fields(key)
                .iter()
                .position(|field| *field == settings.sort_key)
                .unwrap_or(0) as u32,
        );
        self.direction
            .set_icon_name(sort_order_icon(settings.descending));
        self.layout.set_icon_name(layout_icon(settings.layout));
        self.layout_mode.set(settings.layout);
        self.syncing.set(false);
    }
}

impl Shell {
    pub(crate) fn library_page_shell(
        self: &Rc<Self>,
        options: LibraryPageShellOptions,
    ) -> LibraryPageShell {
        let LibraryPageShellOptions {
            key,
            empty,
            empty_body,
            search,
            content,
        } = options;
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(LIBRARY_ROUTE_BOTTOM_MARGIN);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        let toolbar = self.library_toolbar_projection(key, search.clone());
        wrapper.append(&library_route_inset(toolbar.widget()));
        self.set_route_search(Some(search.clone()));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(
            &library_route_inset(self.route_empty_view(empty_body)),
            Some("empty"),
        );
        stack.add_named(&content, Some("content"));
        stack.set_visible_child_name(if empty { "empty" } else { "content" });
        wrapper.append(&stack);

        LibraryPageShell {
            widget: wrapper.upcast(),
            contents: stack,
            toolbar,
        }
    }

    pub(crate) fn set_route_search(&self, search: Option<gtk::SearchEntry>) {
        self.route_viewport.route_search.replace(search);
        self.route_viewport.route_search_focus.borrow_mut().take();
    }

    pub(crate) fn set_route_search_with_focus(
        &self,
        search: gtk::SearchEntry,
        focus: Rc<dyn Fn()>,
    ) {
        self.route_viewport.route_search.replace(Some(search));
        self.route_viewport.route_search_focus.replace(Some(focus));
    }

    pub(crate) fn focus_current_route_search(&self) {
        if !self.route_keyboard_available() {
            return;
        }
        let search = self.route_viewport.route_search.borrow().as_ref().cloned();
        if let Some(search) = search {
            let focus = self
                .route_viewport
                .route_search_focus
                .borrow()
                .as_ref()
                .cloned();
            if let Some(focus) = focus {
                focus();
            } else {
                search.grab_focus();
            }
        }
    }

    fn route_keyboard_available(&self) -> bool {
        !self.source.login_screen_active()
            && !self.fullscreen_player_visible()
            && !self.transient_route_input_active()
    }

    fn playback_keyboard_available(&self) -> bool {
        !self.source.login_screen_active() && !self.transient_route_input_active()
    }

    fn transient_route_input_active(&self) -> bool {
        self.preferences.active_dialog().is_some()
            || self.source.add_server.borrow().is_some()
            || self.lyrics.search_dialog.borrow().is_some()
    }

    pub(crate) fn connect_route_keyboard(self: &Rc<Self>) {
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let shell = Rc::clone(self);
        key.connect_key_pressed(move |_, key, _, state| {
            let current_focus = GtkWindowExt::focus(&shell.chrome.window);
            if key_has_no_shortcut_modifiers(state) {
                if key == gtk::gdk::Key::space
                    && shell.playback_keyboard_available()
                    && !focus_blocks_play_pause(current_focus.as_ref())
                {
                    shell.products.playback.transport.play_pause();
                    return glib::Propagation::Stop;
                }
                if shell.route_keyboard_available()
                    && !focus_blocks_page_navigation(current_focus.as_ref())
                    && let Some(direction) = page_navigation_direction(key)
                {
                    return shell.navigate_current_route_items(direction);
                }
            }
            let Some(search) = shell.route_viewport.route_search.borrow().as_ref().cloned() else {
                return glib::Propagation::Proceed;
            };
            let focus = shell
                .route_viewport
                .route_search_focus
                .borrow()
                .as_ref()
                .cloned();
            if !shell.settings.current.borrow().type_to_search_enabled
                || !shell.route_keyboard_available()
                || key_should_bypass_type_to_search(state)
                || focus_blocks_type_to_search(
                    GtkWindowExt::focus(&shell.chrome.window).as_ref(),
                    &search,
                )
            {
                return glib::Propagation::Proceed;
            }
            let Some(character) = key.to_unicode().filter(|character| !character.is_control())
            else {
                return glib::Propagation::Proceed;
            };
            if character.is_whitespace() && search.text().trim().is_empty() {
                return glib::Propagation::Proceed;
            }
            let mut position = search.position();
            if let Some((start, end)) = search.selection_bounds() {
                search.delete_text(start, end);
                position = start;
            }
            search.insert_text(&character.to_string(), &mut position);
            search.set_position(position);
            if let Some(focus) = focus {
                focus();
            } else {
                search.grab_focus();
            }
            glib::Propagation::Stop
        });
        self.chrome.window.add_controller(key);
    }
    pub(crate) fn library_toolbar_projection(
        self: &Rc<Self>,
        key: LibraryListKey,
        search: gtk::SearchEntry,
    ) -> LibraryToolbarProjection {
        let toolbar = gtk::Box::new(
            gtk::Orientation::Horizontal,
            LIBRARY_TOOLBAR_CONTROL_SPACING,
        );
        toolbar.add_css_class("track-toolbar");
        toolbar.set_hexpand(true);
        toolbar.set_halign(gtk::Align::Fill);
        toolbar.set_width_request(1);
        search.set_hexpand(true);
        search.set_width_request(1);
        toolbar.append(&search);
        let controls = gtk::Box::new(
            gtk::Orientation::Horizontal,
            LIBRARY_TOOLBAR_CONTROL_SPACING,
        );
        self.set_current_library_toolbar_controls(&controls);
        let command_button = match key {
            LibraryListKey::Playlists => {
                let create = gtk::Button::new();
                set_library_command_button_content(&create, false, ADD_ICON, "New Playlist");
                bind_widget_tooltip(&create, "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_playlist_dialog());
                controls.append(&create);
                Some(create)
            }
            LibraryListKey::SmartPlaylists => {
                let create = gtk::Button::new();
                set_library_command_button_content(&create, false, ADD_ICON, "New Playlist");
                bind_widget_tooltip(&create, "New Playlist");
                let shell = Rc::clone(self);
                create.connect_clicked(move |_| shell.new_smart_playlist_dialog());
                controls.append(&create);
                Some(create)
            }
            _ => None,
        };

        let settings = self.settings.current.borrow().library_list(key);
        let sort_messages = available_sort_fields(key)
            .iter()
            .map(|field| library_sort_title(key, *field))
            .collect::<Vec<_>>();
        let sort_options = gtk::StringList::new(&[]);
        let sort_dropdown = gtk::DropDown::new(Some(sort_options), None::<gtk::Expression>);
        bind_drop_down_options(&sort_dropdown, sort_messages, |labels| {
            toolbar_sort_width_for_labels(labels.iter().map(String::as_str))
        });
        sort_dropdown.set_hexpand(false);
        sort_dropdown.set_halign(gtk::Align::End);
        let syncing = Rc::new(Cell::new(false));
        sort_dropdown.set_selected(
            available_sort_fields(key)
                .iter()
                .position(|field| *field == settings.sort_key)
                .unwrap_or(0) as u32,
        );
        {
            let shell = Rc::clone(self);
            let syncing = Rc::clone(&syncing);
            sort_dropdown.connect_selected_notify(move |dropdown| {
                if syncing.get() {
                    return;
                }
                let sort_key = available_sort_fields(key)
                    .get(dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(LibraryField::Title);
                shell.update_library_list_settings(key, |settings| settings.sort_key = sort_key);
            });
        }
        controls.append(&sort_dropdown);

        let direction = gtk::Button::from_icon_name(sort_order_icon(settings.descending));
        configure_library_toolbar_icon_button(&direction, "Change sort order");
        {
            let shell = Rc::clone(self);
            let syncing = Rc::clone(&syncing);
            direction.connect_clicked(move |direction| {
                if syncing.get() {
                    return;
                }
                let mut descending = false;
                shell.update_library_list_settings(key, |settings| {
                    settings.descending = !settings.descending;
                    descending = settings.descending;
                });
                direction.set_icon_name(sort_order_icon(descending));
            });
        }
        controls.append(&direction);

        let layout = gtk::Button::from_icon_name(layout_icon(settings.layout));
        configure_library_toolbar_icon_button(&layout, "Layout");
        let layout_mode = Rc::new(Cell::new(settings.layout));
        let layout_mode_for_locale = Rc::clone(&layout_mode);
        bind_widget_tooltip_with(&layout, move || {
            format!(
                "{}: {}",
                tr("Layout"),
                tr(layout_title(layout_mode_for_locale.get()))
            )
        });
        {
            let shell = Rc::clone(self);
            let syncing = Rc::clone(&syncing);
            let layout_mode = Rc::clone(&layout_mode);
            layout.connect_clicked(move |_| {
                if syncing.get() {
                    return;
                }
                shell.update_library_list_settings(key, |settings| {
                    settings.layout = next_layout(key, settings.layout);
                    layout_mode.set(settings.layout);
                });
            });
        }
        controls.append(&layout);

        let configure = gtk::Button::from_icon_name(MORE_ICON);
        configure_library_toolbar_icon_button(&configure, "Customize display");
        {
            let shell = Rc::clone(self);
            configure.connect_clicked(move |_| {
                shell.present_library_config_dialog(key);
            });
        }
        controls.append(&configure);
        toolbar.append(&controls);
        let widget = if let Some(command_button) = command_button {
            let command_compact = Rc::new(Cell::new(false));
            apply_library_command_button_layout(&command_button, &command_compact, 1);
            let owner = width_allocation_owner(&toolbar, move |width| {
                apply_library_command_button_layout(&command_button, &command_compact, width);
            });
            owner.upcast()
        } else {
            toolbar.upcast()
        };
        LibraryToolbarProjection {
            key,
            widget,
            sort_dropdown,
            direction,
            layout,
            layout_mode,
            syncing,
        }
    }
    pub(crate) fn sync_library_toolbar_end_margin(&self) {
        let Some(controls) = self
            .route_viewport
            .current_library_toolbar_controls
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        else {
            return;
        };
        let margin = library_toolbar_end_margin(self.right_sidebar_visible());
        controls.set_margin_end(margin);
    }
    fn set_current_library_toolbar_controls(&self, controls: &gtk::Box) {
        self.route_viewport
            .current_library_toolbar_controls
            .replace(Some(controls.downgrade()));
        self.sync_library_toolbar_end_margin();
    }
    pub(crate) fn present_library_config_dialog(self: &Rc<Self>, key: LibraryListKey) {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(&tr("Customize display"), &tr(key.title()));
        header.set_title_widget(Some(&title));
        toolbar.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);

        let layout_group = adw::PreferencesGroup::builder()
            .title(tr("Layout"))
            .description(tr("Choose the current page layout."))
            .build();
        let reset = gtk::Button::with_label(&tr("Reset"));
        reset.add_css_class("destructive-action");
        reset.set_valign(gtk::Align::End);
        reset.set_margin_end(16);
        bind_widget_tooltip(&reset, msgid("Reset to defaults"));
        layout_group.set_header_suffix(Some(&reset));
        let layout_row = adw::ActionRow::builder().title(tr("View")).build();
        let layout_buttons = Rc::new(RefCell::new(
            Vec::<(LibraryLayout, gtk::ToggleButton)>::new(),
        ));
        let layout_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        layout_box.add_css_class("linked");
        layout_box.add_css_class("preference-selection-buttons");
        layout_box.set_valign(gtk::Align::Center);
        let mut first_button: Option<gtk::ToggleButton> = None;
        for layout in supported_layouts(key) {
            let button = gtk::ToggleButton::new();
            button.add_css_class("preference-selection-button");
            button.set_child(Some(&layout_button_content(layout)));
            button.set_tooltip_text(Some(&tr(layout_title(layout))));
            if let Some(first) = &first_button {
                button.set_group(Some(first));
            } else {
                first_button = Some(button.clone());
            }
            button.set_active(layout == self.settings.current.borrow().library_list(key).layout);
            layout_box.append(&button);
            layout_buttons.borrow_mut().push((layout, button));
        }
        layout_row.add_suffix(&layout_box);
        layout_group.add(&layout_row);
        content.append(&layout_group);

        let fields_group = adw::PreferencesGroup::builder().build();
        let rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
        content.append(&fields_group);

        for (layout, button) in layout_buttons.borrow().iter() {
            let shell = Rc::clone(self);
            let fields_group = fields_group.clone();
            let rows = Rc::clone(&rows);
            let layout_buttons = Rc::downgrade(&layout_buttons);
            let layout = *layout;
            button.connect_toggled(move |button| {
                if !button.is_active()
                    || shell.settings.current.borrow().library_list(key).layout == layout
                {
                    return;
                }
                shell.update_library_list_settings(key, |settings| {
                    settings.layout = layout;
                });
                if let Some(layout_buttons) = layout_buttons.upgrade() {
                    sync_layout_buttons(&layout_buttons, layout);
                }
                populate_library_field_rows(&shell, key, &fields_group, &rows);
            });
        }

        {
            let shell = Rc::clone(self);
            let fields_group = fields_group.clone();
            let rows = Rc::clone(&rows);
            let layout_buttons = Rc::clone(&layout_buttons);
            reset.connect_clicked(move |_| {
                let default_settings = LibraryListSettings::for_key(key);
                shell.update_library_list_settings(key, |settings| {
                    *settings = default_settings.clone();
                });
                sync_layout_buttons(&layout_buttons, default_settings.layout);
                populate_library_field_rows(&shell, key, &fields_group, &rows);
            });
        }

        populate_library_field_rows(self, key, &fields_group, &rows);

        let scroller = gtk::ScrolledWindow::new();
        configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&content));
        toolbar.set_content(Some(&scroller));

        let dialog = adw::Dialog::builder()
            .content_width(large_popup_content_width(LIBRARY_CONFIG_DIALOG_WIDTH))
            .content_height(large_popup_content_height(
                self.chrome.window.height(),
                LIBRARY_CONFIG_DIALOG_HEIGHT,
            ))
            .child(&toolbar)
            .build();
        present_light_dismiss_dialog(&dialog, &self.chrome.window);
    }
}

fn library_sort_title(key: LibraryListKey, field: LibraryField) -> &'static str {
    if key == LibraryListKey::PlaylistTracks && field == LibraryField::RowIndex {
        msgid("Playlist order")
    } else {
        field.title()
    }
}

pub(super) fn restore_single_click_activation_on_primary_press(
    target: &impl IsA<gtk::Widget>,
    restore: impl Fn() + 'static,
) {
    let pointer = gtk::GestureClick::new();
    pointer.set_button(gtk::gdk::BUTTON_PRIMARY);
    pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
    pointer.connect_pressed(move |_, _, _, _| restore());
    target.add_controller(pointer);
}

fn key_should_bypass_type_to_search(state: gtk::gdk::ModifierType) -> bool {
    state.intersects(
        gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::HYPER_MASK
            | gtk::gdk::ModifierType::META_MASK,
    )
}

fn key_has_no_shortcut_modifiers(state: gtk::gdk::ModifierType) -> bool {
    !state.intersects(
        gtk::gdk::ModifierType::SHIFT_MASK
            | gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::HYPER_MASK
            | gtk::gdk::ModifierType::META_MASK,
    )
}

fn page_navigation_direction(key: gtk::gdk::Key) -> Option<gtk::DirectionType> {
    match key {
        gtk::gdk::Key::Up => Some(gtk::DirectionType::Up),
        gtk::gdk::Key::Down => Some(gtk::DirectionType::Down),
        gtk::gdk::Key::Left => Some(gtk::DirectionType::Left),
        gtk::gdk::Key::Right => Some(gtk::DirectionType::Right),
        _ => None,
    }
}

fn focus_blocks_play_pause(focus: Option<&gtk::Widget>) -> bool {
    focus.is_some_and(|focus| focus_is_text_input(focus) || focus_is_in_dialog(focus))
}

fn focus_blocks_page_navigation(focus: Option<&gtk::Widget>) -> bool {
    focus.is_some_and(|focus| {
        focus_is_in_dialog(focus)
            || focus_is_text_input(focus)
            || focus.is::<gtk::Range>()
            || focus.ancestor(gtk::Range::static_type()).is_some()
            || focus.is::<gtk::DropDown>()
            || focus.ancestor(gtk::DropDown::static_type()).is_some()
    })
}

fn focus_is_in_dialog(focus: &gtk::Widget) -> bool {
    focus.is::<adw::Dialog>() || focus.ancestor(adw::Dialog::static_type()).is_some()
}

fn focus_is_text_input(focus: &gtk::Widget) -> bool {
    focus.is::<gtk::Editable>()
        || focus.is::<gtk::TextView>()
        || focus.ancestor(gtk::Editable::static_type()).is_some()
        || focus.ancestor(gtk::TextView::static_type()).is_some()
}

fn focus_blocks_type_to_search(focus: Option<&gtk::Widget>, search: &gtk::SearchEntry) -> bool {
    let Some(focus) = focus else {
        return false;
    };
    focus.is_ancestor(search) || focus_is_text_input(focus) || focus_is_in_dialog(focus)
}

fn library_toolbar_compact_for_width(width: i32) -> bool {
    width < LIBRARY_TOOLBAR_COMPACT_COMMAND_WIDTH
}
pub(crate) fn toolbar_sort_width_for_labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> i32 {
    labels
        .into_iter()
        .map(toolbar_sort_label_width)
        .max()
        .unwrap_or(LIBRARY_TOOLBAR_SORT_MIN_WIDTH)
}

fn toolbar_sort_label_width(label: &str) -> i32 {
    (label.chars().count() as i32 * LIBRARY_TOOLBAR_SORT_CHAR_WIDTH
        + LIBRARY_TOOLBAR_SORT_HORIZONTAL_PADDING)
        .max(LIBRARY_TOOLBAR_SORT_MIN_WIDTH)
}

pub(crate) fn library_toolbar_end_margin(right_sidebar_visible: bool) -> i32 {
    if right_sidebar_visible {
        LIBRARY_TOOLBAR_END_MARGIN
    } else {
        LIBRARY_TOOLBAR_WINDOW_CONTROLS_RESERVE
    }
}

fn apply_library_command_button_layout(
    command_button: &gtk::Button,
    command_compact: &Cell<bool>,
    width: i32,
) {
    let width = width.max(1);
    let compact = library_toolbar_compact_for_width(width);
    if command_compact.replace(compact) != compact {
        set_library_command_button_content(command_button, compact, ADD_ICON, "New Playlist");
    }
}

fn configure_library_toolbar_icon_button(button: &gtk::Button, tooltip: &str) {
    button.add_css_class("flat");
    button.add_css_class("icon-button");
    button.add_css_class("circular");
    button.add_css_class("library-toolbar-icon-button");
    button.set_width_request(LIBRARY_TOOLBAR_ICON_BUTTON_WIDTH);
    bind_widget_tooltip(button, tooltip);
}

fn set_library_command_button_content(
    button: &gtk::Button,
    compact: bool,
    icon_name: &str,
    label: &str,
) {
    button.add_css_class("flat");
    if compact {
        button.remove_css_class("pill-button");
        button.remove_css_class("pill");
        button.add_css_class("icon-button");
        button.add_css_class("circular");
        button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
        return;
    }

    button.remove_css_class("icon-button");
    button.remove_css_class("circular");
    button.add_css_class("pill-button");
    button.add_css_class("pill");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&localized_label(label));
    button.set_child(Some(&content));
}

pub(crate) fn non_propagating_width_scroller() -> gtk::ScrolledWindow {
    let clip = gtk::ScrolledWindow::new();
    clip.add_css_class("non-propagating-width-clip");
    configure_fill_width_clip(&clip, gtk::PolicyType::Never);
    // Unlike a route-level clip, this scroller must pass its allocated width
    // into the embedded child's height-for-width measurement.
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_propagate_natural_height(true);
    clip.set_hexpand(true);
    clip.set_halign(gtk::Align::Fill);
    clip
}
