use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use rufin_core::{
    AudioscrobblerScrobbleSettings, DiscordDisplayType, DiscordLinkType, EQUALIZER_BAND_COUNT,
    EqualizerSettings, HomeBlockKind, LeftSidebarMode, MAX_NARROW_LAYOUT_THRESHOLD,
    MIN_NARROW_LAYOUT_THRESHOLD, PlaybackTransitionMode, ReplayGainMode, RightSidebarMode,
    SidebarRouteItem, SidebarRouteItemSettings, StreamQuality,
};
use rufin_playback::available_audio_outputs;

use crate::{
    external_scrobbling::{self, AudioscrobblerSession},
    i18n::tr,
};

use super::{
    Shell,
    layout::{large_popup_content_height, large_popup_content_width},
};

const PREFERENCES_DIALOG_WIDTH: i32 = 700;
const PREFERENCES_DIALOG_HEIGHT: i32 = 640;
const SURFACE_SCROLL_FACTOR: f64 = 2.5;
const LASTFM_API_CREATE_URL: &str = "https://www.last.fm/api/account/create";
const LISTENBRAINZ_TOKEN_URL: &str = "https://listenbrainz.org/settings/";

#[path = "preferences/library.rs"]
mod library;

pub(super) fn present_preferences_dialog(shell: &Rc<Shell>) {
    present_preferences_dialog_with_page(shell, PreferencesInitialPage::General);
}

pub(super) fn present_library_preferences_dialog(shell: &Rc<Shell>) {
    present_preferences_dialog_with_page(shell, PreferencesInitialPage::Library);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreferencesInitialPage {
    General,
    Library,
}

fn present_preferences_dialog_with_page(shell: &Rc<Shell>, initial_page: PreferencesInitialPage) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Preferences"), "")));
    toolbar.add_top_bar(&header);

    let stack = adw::ViewStack::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    let switcher = adw::ViewSwitcher::builder()
        .policy(adw::ViewSwitcherPolicy::Wide)
        .stack(&stack)
        .build();
    let switcher_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switcher_bar.add_css_class("preferences-tab-bar");
    switcher_bar.set_halign(gtk::Align::Center);
    switcher_bar.append(&switcher);
    toolbar.add_top_bar(&switcher_bar);
    toolbar.set_content(Some(&stack));

    let dialog = adw::Dialog::builder()
        .title(tr("Preferences"))
        .content_width(large_popup_content_width(PREFERENCES_DIALOG_WIDTH))
        .content_height(large_popup_content_height(
            shell.window.height(),
            PREFERENCES_DIALOG_HEIGHT,
        ))
        .child(&toolbar)
        .build();
    dialog.add_css_class("preferences");

    let general_page = general_page(shell);
    let layout_page = layout_page(shell);
    let scrobbling_page = scrobbling_page(shell);
    let playback_page = playback_page(shell);
    let library_page = library::library_page(shell, &dialog);
    stack.add_titled_with_icon(
        &general_page,
        Some("general"),
        &tr("General"),
        "preferences-system-symbolic",
    );
    stack.add_titled_with_icon(
        &layout_page,
        Some("layout"),
        &tr("Layout"),
        "preferences-desktop-display-symbolic",
    );
    stack.add_titled_with_icon(
        &scrobbling_page,
        Some("scrobbling"),
        &tr("Scrobbling"),
        "emblem-shared-symbolic",
    );
    stack.add_titled_with_icon(
        &playback_page,
        Some("playback"),
        &tr("Playback"),
        "media-playback-start-symbolic",
    );
    stack.add_titled_with_icon(
        &library_page,
        Some("library"),
        &tr("Library"),
        "audio-x-generic-symbolic",
    );
    if matches!(initial_page, PreferencesInitialPage::Library) {
        stack.set_visible_child_name("library");
    }

    dialog.present(Some(&shell.window));
}

fn general_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();

    let settings = shell.state.settings.borrow().clone();

    let privacy_group = adw::PreferencesGroup::builder()
        .title(tr("Privacy"))
        .build();
    let private_row = adw::SwitchRow::builder()
        .title(tr("Private mode"))
        .subtitle(tr(
            "Stop playback reporting, external lyrics, external metadata, notifications, and presence.",
        ))
        .active(settings.private_mode)
        .build();
    let private_shell = Rc::clone(shell);
    private_row.connect_active_notify(move |row| {
        private_shell.set_private_mode(row.is_active());
    });
    privacy_group.add(&private_row);

    let notifications_row = adw::SwitchRow::builder()
        .title(tr("Now playing notifications"))
        .subtitle(tr("Show a desktop notification when the track changes."))
        .active(settings.notifications_enabled)
        .build();
    let notifications_shell = Rc::clone(shell);
    notifications_row.connect_active_notify(move |row| {
        notifications_shell.set_notifications_enabled(row.is_active());
    });
    privacy_group.add(&notifications_row);
    page.add(&privacy_group);

    let metadata_group = adw::PreferencesGroup::builder()
        .title(tr("Metadata"))
        .description(tr(
            "External metadata uses public MusicBrainz and Cover Art Archive lookups.",
        ))
        .build();
    let external_metadata_row = adw::SwitchRow::builder()
        .title(tr("External cover lookup"))
        .subtitle(tr("Use remote album art when server artwork is missing."))
        .active(settings.external_metadata_enabled)
        .build();
    let metadata_shell = Rc::clone(shell);
    external_metadata_row.connect_active_notify(move |row| {
        metadata_shell.set_external_metadata_enabled(row.is_active());
    });
    metadata_group.add(&external_metadata_row);
    page.add(&metadata_group);

    let lyrics_group = adw::PreferencesGroup::builder().title(tr("Lyrics")).build();
    let external_row = adw::SwitchRow::builder()
        .title(tr("External lyric lookup"))
        .subtitle(tr(
            "Use remote lyric providers when server lyrics are unavailable.",
        ))
        .active(settings.external_lyrics_enabled)
        .build();

    let prefer_server_row = adw::SwitchRow::builder()
        .title(tr("Prefer server lyrics"))
        .subtitle(tr("Search server lyrics before external providers."))
        .active(settings.prefer_server_lyrics)
        .sensitive(settings.external_lyrics_enabled)
        .build();
    let prefer_server_shell = Rc::clone(shell);
    prefer_server_row.connect_active_notify(move |row| {
        prefer_server_shell.set_prefer_server_lyrics(row.is_active());
    });

    let external_shell = Rc::clone(shell);
    let prefer_server_row_for_external = prefer_server_row.clone();
    external_row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        prefer_server_row_for_external.set_sensitive(enabled);
        external_shell.set_external_lyrics_enabled(enabled);
    });
    lyrics_group.add(&external_row);
    lyrics_group.add(&prefer_server_row);

    let ask_save_row = adw::SwitchRow::builder()
        .title(tr("Ask where to save to lyrics"))
        .subtitle(tr(
            "If not set, lyrics are exported to the folder you set, or your ~/Music folder.",
        ))
        .active(settings.ask_lyrics_save_path)
        .build();
    let ask_save_shell = Rc::clone(shell);
    ask_save_row.connect_active_notify(move |row| {
        ask_save_shell.set_ask_lyrics_save_path(row.is_active());
    });
    lyrics_group.add(&ask_save_row);

    let export_subtitle = settings
        .lyrics_export_folder
        .clone()
        .unwrap_or_else(|| tr("Use ~/Music"));
    let export_folder_row = adw::ActionRow::builder()
        .title(tr("Lyrics export folder"))
        .subtitle(export_subtitle)
        .build();
    let export_button = gtk::Button::with_label(&tr("Choose"));
    export_button.set_valign(gtk::Align::Center);
    export_folder_row.add_suffix(&export_button);
    export_folder_row.set_activatable_widget(Some(&export_button));
    let export_shell = Rc::clone(shell);
    let export_row = export_folder_row.clone();
    export_button.connect_clicked(move |_| {
        let shell = Rc::clone(&export_shell);
        let row = export_row.clone();
        gtk::glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Select Lyrics Export Folder"))
                .build();
            let Ok(folder) = dialog.select_folder_future(Some(&shell.window)).await else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            let text = path.display().to_string();
            row.set_subtitle(&text);
            shell.set_lyrics_export_folder(Some(text));
        });
    });
    lyrics_group.add(&export_folder_row);
    page.add(&lyrics_group);

    let discord_group = adw::PreferencesGroup::builder()
        .title(tr("Discord"))
        .description(tr(
            "Rich presence uses Discord IPC. Last.fm and MusicBrainz covers are public metadata lookups.",
        ))
        .build();
    let presence_row = adw::SwitchRow::builder()
        .title(tr("Rich presence"))
        .subtitle(tr("Show the current track in Discord."))
        .active(settings.discord_presence_enabled)
        .build();
    let presence_shell = Rc::clone(shell);
    presence_row.connect_active_notify(move |row| {
        presence_shell.set_discord_presence_enabled(row.is_active());
    });
    discord_group.add(&presence_row);

    let display_titles = [tr("Application name"), tr("Song title"), tr("Artist name")];
    let display_refs = display_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let display_options = gtk::StringList::new(&display_refs);
    let display_row = adw::ComboRow::builder()
        .title(tr("Status display"))
        .subtitle(tr("Choose which line Discord emphasizes."))
        .model(&display_options)
        .selected(discord_display_index(settings.discord_display_type))
        .build();
    let display_shell = Rc::clone(shell);
    display_row.connect_selected_notify(move |row| {
        display_shell.set_discord_display_type(discord_display_from_index(row.selected()));
    });
    discord_group.add(&display_row);

    let link_titles = [
        tr("None"),
        tr("Last.fm"),
        tr("MusicBrainz"),
        tr("MusicBrainz and Last.fm"),
    ];
    let link_refs = link_titles.iter().map(String::as_str).collect::<Vec<_>>();
    let link_options = gtk::StringList::new(&link_refs);
    let link_row = adw::ComboRow::builder()
        .title(tr("Activity links and MusicBrainz covers"))
        .subtitle(tr(
            "Add external links and enable MusicBrainz cover fallback.",
        ))
        .model(&link_options)
        .selected(discord_link_index(settings.discord_link_type))
        .build();
    let link_shell = Rc::clone(shell);
    link_row.connect_selected_notify(move |row| {
        link_shell.set_discord_link_type(discord_link_from_index(row.selected()));
    });
    discord_group.add(&link_row);

    let paused_row = adw::SwitchRow::builder()
        .title(tr("Show paused status"))
        .subtitle(tr("Keep rich presence visible while playback is paused."))
        .active(settings.discord_show_paused)
        .build();
    let paused_shell = Rc::clone(shell);
    paused_row.connect_active_notify(move |row| {
        paused_shell.set_discord_show_paused(row.is_active());
    });
    discord_group.add(&paused_row);

    let listening_row = adw::SwitchRow::builder()
        .title(tr("Use listening activity"))
        .subtitle(tr("Set the Discord activity type to Listening."))
        .active(settings.discord_show_as_listening)
        .build();
    let listening_shell = Rc::clone(shell);
    listening_row.connect_active_notify(move |row| {
        listening_shell.set_discord_show_as_listening(row.is_active());
    });
    discord_group.add(&listening_row);

    let state_icon_row = adw::SwitchRow::builder()
        .title(tr("Show playback icon"))
        .subtitle(tr(
            "Show playing or paused icons when the Discord app assets exist.",
        ))
        .active(settings.discord_show_state_icon)
        .build();
    let state_icon_shell = Rc::clone(shell);
    state_icon_row.connect_active_notify(move |row| {
        state_icon_shell.set_discord_show_state_icon(row.is_active());
    });
    discord_group.add(&state_icon_row);

    let lastfm_row = adw::PasswordEntryRow::builder()
        .title(tr("Last.fm API key"))
        .show_apply_button(true)
        .build();
    lastfm_row.set_text(&settings.lastfm_api_key);
    let lastfm_shell = Rc::clone(shell);
    lastfm_row.connect_apply(move |row| {
        lastfm_shell.set_lastfm_api_key(row.text().to_string());
    });
    discord_group.add(&lastfm_row);

    page.add(&discord_group);

    page
}

fn interface_group(shell: &Rc<Shell>) -> adw::PreferencesGroup {
    let settings = shell.state.settings.borrow().clone();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Interface"))
        .build();

    let default_left_row = left_sidebar_row(
        &tr("Default left sidebar"),
        settings.layout.default_profile.left_sidebar,
    );
    let default_left_shell = Rc::clone(shell);
    default_left_row.connect_selected_notify(move |row| {
        let mode = left_sidebar_mode_from_index(row.selected());
        default_left_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.default_profile.left_sidebar == mode {
                return false;
            }
            settings.layout.default_profile.left_sidebar = mode;
            true
        });
        default_left_shell.update_layout();
    });
    group.add(&default_left_row);

    let default_right_row = right_sidebar_row(
        &tr("Default right sidebar"),
        settings.layout.default_profile.right_sidebar,
    );
    let default_right_shell = Rc::clone(shell);
    default_right_row.connect_selected_notify(move |row| {
        let mode = right_sidebar_mode_from_index(row.selected());
        default_right_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.default_profile.right_sidebar == mode {
                return false;
            }
            settings.layout.default_profile.right_sidebar = mode;
            if mode.is_visible() {
                settings.layout.default_profile.last_visible_right_sidebar = mode;
            }
            settings.layout.sanitize();
            true
        });
        default_right_shell.update_layout();
    });
    group.add(&default_right_row);

    let lyrics_panel_row = adw::SwitchRow::builder()
        .title(tr("Show Lyrics Panel"))
        .subtitle(tr("Keep the lyrics section visible below the queue."))
        .active(settings.lyrics_panel_visible)
        .build();
    let lyrics_panel_shell = Rc::clone(shell);
    lyrics_panel_row.connect_active_notify(move |row| {
        lyrics_panel_shell.set_lyrics_panel_visible(row.is_active());
    });
    group.add(&lyrics_panel_row);

    let narrow_row = adw::SwitchRow::builder()
        .title(tr("Use different layout below threshold"))
        .active(settings.layout.narrow_enabled)
        .build();
    group.add(&narrow_row);

    let threshold_adjustment = gtk::Adjustment::new(
        f64::from(settings.layout.narrow_threshold),
        f64::from(MIN_NARROW_LAYOUT_THRESHOLD),
        f64::from(MAX_NARROW_LAYOUT_THRESHOLD),
        10.0,
        100.0,
        0.0,
    );
    let threshold_row = adw::SpinRow::builder()
        .title(tr("Narrow layout threshold"))
        .adjustment(&threshold_adjustment)
        .digits(0)
        .numeric(true)
        .sensitive(settings.layout.narrow_enabled)
        .build();
    let threshold_shell = Rc::clone(shell);
    threshold_row.connect_value_notify(move |row| {
        let threshold = row.value().round() as i32;
        threshold_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_threshold == threshold {
                return false;
            }
            settings.layout.narrow_threshold = threshold;
            settings.layout.sanitize();
            true
        });
        threshold_shell.update_layout();
    });
    group.add(&threshold_row);

    let narrow_left_row = left_sidebar_row(
        &tr("Narrow left sidebar"),
        settings.layout.narrow_profile.left_sidebar,
    );
    narrow_left_row.set_sensitive(settings.layout.narrow_enabled);
    let narrow_left_shell = Rc::clone(shell);
    narrow_left_row.connect_selected_notify(move |row| {
        let mode = left_sidebar_mode_from_index(row.selected());
        narrow_left_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_profile.left_sidebar == mode {
                return false;
            }
            settings.layout.narrow_profile.left_sidebar = mode;
            true
        });
        narrow_left_shell.update_layout();
    });
    group.add(&narrow_left_row);

    let narrow_right_row = right_sidebar_row(
        &tr("Narrow right sidebar"),
        settings.layout.narrow_profile.right_sidebar,
    );
    narrow_right_row.set_sensitive(settings.layout.narrow_enabled);
    let narrow_right_shell = Rc::clone(shell);
    narrow_right_row.connect_selected_notify(move |row| {
        let mode = right_sidebar_mode_from_index(row.selected());
        narrow_right_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_profile.right_sidebar == mode {
                return false;
            }
            settings.layout.narrow_profile.right_sidebar = mode;
            if mode.is_visible() {
                settings.layout.narrow_profile.last_visible_right_sidebar = mode;
            }
            settings.layout.sanitize();
            true
        });
        narrow_right_shell.update_layout();
    });
    group.add(&narrow_right_row);

    let threshold_row_for_toggle = threshold_row.clone();
    let narrow_left_row_for_toggle = narrow_left_row.clone();
    let narrow_right_row_for_toggle = narrow_right_row.clone();
    let narrow_shell = Rc::clone(shell);
    narrow_row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        threshold_row_for_toggle.set_sensitive(enabled);
        narrow_left_row_for_toggle.set_sensitive(enabled);
        narrow_right_row_for_toggle.set_sensitive(enabled);
        narrow_shell.update_app_settings("layout setting", |settings| {
            if settings.layout.narrow_enabled == enabled {
                return false;
            }
            settings.layout.narrow_enabled = enabled;
            true
        });
        narrow_shell.update_layout();
    });

    group
}

fn sidebar_items_group(shell: &Rc<Shell>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Sidebar Items"))
        .build();
    let rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));
    populate_sidebar_item_rows(shell, &group, &rows);
    group
}

fn populate_sidebar_item_rows(
    shell: &Rc<Shell>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }

    let items = shell.state.settings.borrow().sidebar.route_items.clone();
    for entry in items {
        let row = sidebar_item_row(shell, group, rows, entry);
        group.add(&row);
        rows.borrow_mut().push(row);
    }

    let server_row = adw::ActionRow::builder()
        .title(tr("Server selector"))
        .subtitle(if shell.state.settings.borrow().sidebar.server_visible {
            tr("Visible")
        } else {
            tr("Hidden")
        })
        .build();
    let server_switch = gtk::Switch::new();
    server_switch.set_active(shell.state.settings.borrow().sidebar.server_visible);
    server_switch.set_valign(gtk::Align::Center);
    server_row.add_suffix(&server_switch);
    server_row.set_activatable_widget(Some(&server_switch));
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        server_switch.connect_active_notify(move |switch| {
            let visible = switch.is_active();
            shell.update_app_settings("sidebar setting", |settings| {
                if settings.sidebar.server_visible == visible {
                    return false;
                }
                settings.sidebar.server_visible = visible;
                true
            });
            shell.rebuild_sidebar_navigation();
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }
    group.add(&server_row);
    rows.borrow_mut().push(server_row);
}

fn sidebar_item_row(
    shell: &Rc<Shell>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    entry: SidebarRouteItemSettings,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(tr(sidebar_route_item_title(entry.item)))
        .subtitle(if entry.visible {
            tr("Visible")
        } else {
            tr("Hidden")
        })
        .build();

    let drag = gtk::Image::from_icon_name("list-drag-handle-symbolic");
    drag.add_css_class("dim-label");
    drag.set_tooltip_text(Some(&tr("Drag to reorder")));
    row.add_prefix(&drag);

    let visible = gtk::Switch::new();
    visible.set_active(entry.visible);
    visible.set_valign(gtk::Align::Center);
    row.add_suffix(&visible);
    row.set_activatable_widget(Some(&visible));

    let up = gtk::Button::from_icon_name("go-up-symbolic");
    up.add_css_class("flat");
    up.set_tooltip_text(Some(&tr("Move up")));
    up.set_valign(gtk::Align::Center);
    row.add_suffix(&up);

    let down = gtk::Button::from_icon_name("go-down-symbolic");
    down.add_css_class("flat");
    down.set_tooltip_text(Some(&tr("Move down")));
    down.set_valign(gtk::Align::Center);
    row.add_suffix(&down);

    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        visible.connect_active_notify(move |switch| {
            let item = entry.item;
            let is_visible = switch.is_active();
            shell.update_app_settings("sidebar setting", |settings| {
                if let Some(stored) = settings
                    .sidebar
                    .route_items
                    .iter_mut()
                    .find(|stored| stored.item == item)
                {
                    if stored.visible == is_visible {
                        return false;
                    }
                    stored.visible = is_visible;
                }
                settings.sidebar.sanitize();
                true
            });
            shell.rebuild_sidebar_navigation();
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        up.connect_clicked(move |_| {
            move_sidebar_item(&shell, entry.item, -1);
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }
    {
        let shell = Rc::clone(shell);
        let group = group.clone();
        let rows = Rc::clone(rows);
        down.connect_clicked(move |_| {
            move_sidebar_item(&shell, entry.item, 1);
            populate_sidebar_item_rows(&shell, &group, &rows);
        });
    }

    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    let item_id = sidebar_route_item_drag_id(entry.item).to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&item_id.to_value()))
    });
    drag.add_controller(source);

    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let shell = Rc::clone(shell);
    let group = group.clone();
    let rows = Rc::clone(rows);
    let row_for_drop = row.clone();
    drop_target.connect_drop(move |_, value, _, y| {
        let Ok(source_id) = value.get::<String>() else {
            return false;
        };
        let Some(source_item) = sidebar_route_item_from_drag_id(&source_id) else {
            return false;
        };
        if source_item == entry.item {
            return false;
        }
        let after = y > f64::from(row_for_drop.height()) / 2.0;
        let changed = shell
            .update_app_settings("sidebar setting", |settings| {
                let changed = reorder_sidebar_item_settings(
                    &mut settings.sidebar.route_items,
                    source_item,
                    entry.item,
                    after,
                );
                if changed {
                    settings.sidebar.sanitize();
                }
                changed
            })
            .is_some();
        if changed {
            shell.rebuild_sidebar_navigation();
            populate_sidebar_item_rows(&shell, &group, &rows);
        }
        changed
    });
    row.add_controller(drop_target);

    row
}

fn move_sidebar_item(shell: &Rc<Shell>, item: SidebarRouteItem, delta: isize) {
    shell.update_app_settings("sidebar setting", |settings| {
        let Some(index) = settings
            .sidebar
            .route_items
            .iter()
            .position(|entry| entry.item == item)
        else {
            return false;
        };
        let new_index = if delta < 0 {
            index.saturating_sub(1)
        } else {
            (index + 1).min(settings.sidebar.route_items.len().saturating_sub(1))
        };
        if index == new_index {
            return false;
        }
        settings.sidebar.route_items.swap(index, new_index);
        settings.sidebar.sanitize();
        true
    });
    shell.rebuild_sidebar_navigation();
}

fn reorder_sidebar_item_settings(
    items: &mut Vec<SidebarRouteItemSettings>,
    source: SidebarRouteItem,
    target: SidebarRouteItem,
    after: bool,
) -> bool {
    if source == target {
        return false;
    }
    let before = items.clone();
    let Some(source_index) = items.iter().position(|entry| entry.item == source) else {
        return false;
    };
    let entry = items.remove(source_index);
    let Some(mut target_index) = items.iter().position(|entry| entry.item == target) else {
        items.insert(source_index.min(items.len()), entry);
        return false;
    };
    if after {
        target_index += 1;
    }
    items.insert(target_index.min(items.len()), entry);
    *items != before
}

fn sidebar_route_item_drag_id(item: SidebarRouteItem) -> &'static str {
    match item {
        SidebarRouteItem::Home => "Home",
        SidebarRouteItem::Favorites => "Favorites",
        SidebarRouteItem::Albums => "Albums",
        SidebarRouteItem::Tracks => "Tracks",
        SidebarRouteItem::Artists => "Artists",
        SidebarRouteItem::AlbumArtists => "AlbumArtists",
        SidebarRouteItem::Genres => "Genres",
        SidebarRouteItem::Folders => "Folders",
        SidebarRouteItem::Playlists => "Playlists",
    }
}

fn sidebar_route_item_from_drag_id(id: &str) -> Option<SidebarRouteItem> {
    SidebarRouteItem::all()
        .into_iter()
        .find(|item| sidebar_route_item_drag_id(*item) == id)
}

fn sidebar_route_item_title(item: SidebarRouteItem) -> &'static str {
    match item {
        SidebarRouteItem::Home => "Home",
        SidebarRouteItem::Favorites => "Favorites",
        SidebarRouteItem::Albums => "Albums",
        SidebarRouteItem::Tracks => "Tracks",
        SidebarRouteItem::Artists => "Artists",
        SidebarRouteItem::AlbumArtists => "Album Artists",
        SidebarRouteItem::Genres => "Genres",
        SidebarRouteItem::Folders => "Folders",
        SidebarRouteItem::Playlists => "Playlists",
    }
}

fn scrobbling_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Scrobbling"))
        .icon_name("emblem-shared-symbolic")
        .build();
    let app_settings = shell.state.settings.borrow().clone();
    let settings = app_settings.scrobbling.clone();
    let lastfm_api_key_text = if app_settings.lastfm_api_key.trim().is_empty() {
        settings.lastfm.api_key.clone()
    } else {
        app_settings.lastfm_api_key.clone()
    };

    let lastfm_group = adw::PreferencesGroup::builder()
        .title(tr("Last.fm"))
        .build();
    let lastfm_enabled = adw::SwitchRow::builder()
        .title(tr("Last.fm scrobbling"))
        .active(settings.lastfm.enabled)
        .build();
    let lastfm_enabled_shell = Rc::clone(shell);
    lastfm_enabled.connect_active_notify(move |row| {
        lastfm_enabled_shell.update_scrobbling_settings("Last.fm scrobbling setting", |settings| {
            if settings.lastfm.enabled == row.is_active() {
                return false;
            }
            settings.lastfm.enabled = row.is_active();
            true
        });
    });
    lastfm_group.add(&lastfm_enabled);

    let lastfm_api_help = adw::ActionRow::builder()
        .title(tr("API keys"))
        .subtitle(inline_link_markup(
            &tr("If you do not have API keys, create them"),
            LASTFM_API_CREATE_URL,
            &tr("here"),
            &tr(". You only need ot fill email and an application name parts"),
        ))
        .use_markup(true)
        .build();
    lastfm_group.add(&lastfm_api_help);

    let lastfm_api_key = adw::PasswordEntryRow::builder()
        .title(tr("API key"))
        .text(&lastfm_api_key_text)
        .show_apply_button(true)
        .build();
    let lastfm_api_shell = Rc::clone(shell);
    lastfm_api_key.connect_apply(move |row| {
        let api_key = row.text().trim().to_string();
        lastfm_api_shell.update_app_settings("Last.fm API key setting", |settings| {
            if settings.lastfm_api_key == api_key && settings.scrobbling.lastfm.api_key == api_key {
                return false;
            }
            settings.lastfm_api_key = api_key.clone();
            settings.scrobbling.lastfm.api_key = api_key;
            settings.scrobbling.lastfm.session_key.clear();
            settings.scrobbling.lastfm.username.clear();
            true
        });
    });
    lastfm_group.add(&lastfm_api_key);

    let lastfm_api_secret = adw::PasswordEntryRow::builder()
        .title(tr("Shared secret"))
        .text(&settings.lastfm.api_secret)
        .show_apply_button(true)
        .build();
    let lastfm_secret_shell = Rc::clone(shell);
    lastfm_api_secret.connect_apply(move |row| {
        let api_secret = row.text().trim().to_string();
        lastfm_secret_shell.update_scrobbling_settings(
            "Last.fm shared secret setting",
            |settings| {
                if settings.lastfm.api_secret == api_secret {
                    return false;
                }
                settings.lastfm.api_secret = api_secret;
                settings.lastfm.session_key.clear();
                settings.lastfm.username.clear();
                true
            },
        );
    });
    lastfm_group.add(&lastfm_api_secret);

    let lastfm_connection = adw::ActionRow::builder()
        .title(tr("Connection"))
        .subtitle(audioscrobbler_connection_subtitle(&settings.lastfm))
        .build();
    let lastfm_connect_label = if settings.lastfm.session_key.is_empty() {
        tr("Connect")
    } else {
        tr("Reconnect")
    };
    let lastfm_connect = gtk::Button::with_label(&lastfm_connect_label);
    lastfm_connect.set_valign(gtk::Align::Center);
    lastfm_connection.add_suffix(&lastfm_connect);
    lastfm_connection.set_activatable_widget(Some(&lastfm_connect));
    let lastfm_connect_shell = Rc::clone(shell);
    let lastfm_api_key_row = lastfm_api_key.clone();
    let lastfm_secret_row = lastfm_api_secret.clone();
    let lastfm_connection_row = lastfm_connection.clone();
    lastfm_connect.connect_clicked(move |button| {
        let api_key = lastfm_api_key_row.text().trim().to_string();
        let api_secret = lastfm_secret_row.text().trim().to_string();
        if api_key.is_empty() || api_secret.is_empty() {
            lastfm_connection_row.set_subtitle(&tr("Enter API credentials first"));
            return;
        }
        button.set_sensitive(false);
        lastfm_connection_row.set_subtitle(&tr("Opening Last.fm authorization..."));
        let shell = Rc::clone(&lastfm_connect_shell);
        let row = lastfm_connection_row.clone();
        let button = button.clone();
        gtk::glib::spawn_future_local(async move {
            match connect_lastfm_session(&shell, api_key, api_secret).await {
                Ok(session) => {
                    row.set_subtitle(&audioscrobbler_connected_subtitle(&session.username));
                    button.set_label(&tr("Reconnect"));
                }
                Err(error) => {
                    row.set_subtitle(&error);
                }
            }
            button.set_sensitive(true);
        });
    });
    lastfm_group.add(&lastfm_connection);

    let lastfm_now_playing = adw::SwitchRow::builder()
        .title(tr("Now playing updates"))
        .active(settings.lastfm.now_playing_enabled)
        .build();
    let lastfm_now_playing_shell = Rc::clone(shell);
    lastfm_now_playing.connect_active_notify(move |row| {
        lastfm_now_playing_shell.update_scrobbling_settings(
            "Last.fm now playing setting",
            |settings| {
                if settings.lastfm.now_playing_enabled == row.is_active() {
                    return false;
                }
                settings.lastfm.now_playing_enabled = row.is_active();
                true
            },
        );
    });
    lastfm_group.add(&lastfm_now_playing);
    page.add(&lastfm_group);

    let librefm_group = adw::PreferencesGroup::builder()
        .title(tr("Libre.fm"))
        .description(tr(
            "If the page doesn't load, then Libre.fm blocks your IP range/VPN",
        ))
        .build();
    let librefm_enabled = adw::SwitchRow::builder()
        .title(tr("Libre.fm scrobbling"))
        .active(settings.librefm.enabled)
        .build();
    let librefm_enabled_shell = Rc::clone(shell);
    librefm_enabled.connect_active_notify(move |row| {
        librefm_enabled_shell.update_scrobbling_settings(
            "Libre.fm scrobbling setting",
            |settings| {
                if settings.librefm.enabled == row.is_active() {
                    return false;
                }
                settings.librefm.enabled = row.is_active();
                true
            },
        );
    });
    librefm_group.add(&librefm_enabled);

    let librefm_connection = adw::ActionRow::builder()
        .title(tr("Connection"))
        .subtitle(audioscrobbler_connection_subtitle(&settings.librefm))
        .build();
    let librefm_connect_label = if settings.librefm.session_key.is_empty() {
        tr("Connect")
    } else {
        tr("Reconnect")
    };
    let librefm_connect = gtk::Button::with_label(&librefm_connect_label);
    librefm_connect.set_valign(gtk::Align::Center);
    librefm_connection.add_suffix(&librefm_connect);
    librefm_connection.set_activatable_widget(Some(&librefm_connect));
    let librefm_connect_shell = Rc::clone(shell);
    let librefm_connection_row = librefm_connection.clone();
    librefm_connect.connect_clicked(move |button| {
        button.set_sensitive(false);
        librefm_connection_row.set_subtitle(&tr("Opening Libre.fm authorization..."));
        let shell = Rc::clone(&librefm_connect_shell);
        let row = librefm_connection_row.clone();
        let button = button.clone();
        gtk::glib::spawn_future_local(async move {
            match connect_librefm_session(&shell).await {
                Ok(session) => {
                    row.set_subtitle(&audioscrobbler_connected_subtitle(&session.username));
                    button.set_label(&tr("Reconnect"));
                }
                Err(error) => {
                    row.set_subtitle(&error);
                }
            }
            button.set_sensitive(true);
        });
    });
    librefm_group.add(&librefm_connection);

    let librefm_now_playing = adw::SwitchRow::builder()
        .title(tr("Now playing updates"))
        .active(settings.librefm.now_playing_enabled)
        .build();
    let librefm_now_playing_shell = Rc::clone(shell);
    librefm_now_playing.connect_active_notify(move |row| {
        librefm_now_playing_shell.update_scrobbling_settings(
            "Libre.fm now playing setting",
            |settings| {
                if settings.librefm.now_playing_enabled == row.is_active() {
                    return false;
                }
                settings.librefm.now_playing_enabled = row.is_active();
                true
            },
        );
    });
    librefm_group.add(&librefm_now_playing);
    page.add(&librefm_group);

    let listenbrainz_group = adw::PreferencesGroup::builder()
        .title(tr("ListenBrainz"))
        .build();
    let listenbrainz_enabled = adw::SwitchRow::builder()
        .title(tr("ListenBrainz scrobbling"))
        .active(settings.listenbrainz.enabled)
        .build();
    let listenbrainz_enabled_shell = Rc::clone(shell);
    listenbrainz_enabled.connect_active_notify(move |row| {
        listenbrainz_enabled_shell.update_scrobbling_settings(
            "ListenBrainz scrobbling setting",
            |settings| {
                if settings.listenbrainz.enabled == row.is_active() {
                    return false;
                }
                settings.listenbrainz.enabled = row.is_active();
                true
            },
        );
    });
    listenbrainz_group.add(&listenbrainz_enabled);

    let listenbrainz_token_help = adw::ActionRow::builder()
        .title(tr("Get token"))
        .subtitle(inline_link_markup(
            &tr("Find your ListenBrainz user token"),
            LISTENBRAINZ_TOKEN_URL,
            &tr("here"),
            ".",
        ))
        .use_markup(true)
        .build();
    listenbrainz_group.add(&listenbrainz_token_help);

    let listenbrainz_token = adw::PasswordEntryRow::builder()
        .title(tr("User token"))
        .text(&settings.listenbrainz.user_token)
        .show_apply_button(true)
        .build();
    let listenbrainz_token_shell = Rc::clone(shell);
    listenbrainz_token.connect_apply(move |row| {
        let token = row.text().trim().to_string();
        listenbrainz_token_shell.update_scrobbling_settings(
            "ListenBrainz token setting",
            |settings| {
                if settings.listenbrainz.user_token == token {
                    return false;
                }
                settings.listenbrainz.user_token = token;
                true
            },
        );
    });
    listenbrainz_group.add(&listenbrainz_token);

    let listenbrainz_now_playing = adw::SwitchRow::builder()
        .title(tr("Now playing updates"))
        .active(settings.listenbrainz.now_playing_enabled)
        .build();
    let listenbrainz_now_playing_shell = Rc::clone(shell);
    listenbrainz_now_playing.connect_active_notify(move |row| {
        listenbrainz_now_playing_shell.update_scrobbling_settings(
            "ListenBrainz now playing setting",
            |settings| {
                if settings.listenbrainz.now_playing_enabled == row.is_active() {
                    return false;
                }
                settings.listenbrainz.now_playing_enabled = row.is_active();
                true
            },
        );
    });
    listenbrainz_group.add(&listenbrainz_now_playing);
    page.add(&listenbrainz_group);

    page
}

fn audioscrobbler_connection_subtitle(settings: &AudioscrobblerScrobbleSettings) -> String {
    if settings.session_key.trim().is_empty() {
        tr("Not connected")
    } else {
        audioscrobbler_connected_subtitle(&settings.username)
    }
}

fn audioscrobbler_connected_subtitle(username: &str) -> String {
    let username = username.trim();
    if username.is_empty() {
        tr("Connected")
    } else {
        format!("{} {username}", tr("Connected as"))
    }
}

fn inline_link_markup(before: &str, url: &str, label: &str, after: &str) -> String {
    let before = gtk::glib::markup_escape_text(before);
    let url = gtk::glib::markup_escape_text(url);
    let label = gtk::glib::markup_escape_text(label);
    let after = gtk::glib::markup_escape_text(after);
    format!("{before} <a href=\"{url}\">{label}</a>{after}")
}

async fn connect_lastfm_session(
    shell: &Rc<Shell>,
    api_key: String,
    api_secret: String,
) -> Result<AudioscrobblerSession, String> {
    let token_api_key = api_key.clone();
    let token_api_secret = api_secret.clone();
    let token = gtk::gio::spawn_blocking(move || {
        external_scrobbling::request_lastfm_auth_token(&token_api_key, &token_api_secret)
    })
    .await
    .map_err(|_| "Last.fm authorization task failed.".to_string())??;
    let url = external_scrobbling::lastfm_auth_url(&api_key, &token);
    let launcher = gtk::UriLauncher::new(&url);
    launcher
        .launch_future(Some(&shell.window))
        .await
        .map_err(|error| format!("Failed to open Last.fm authorization: {error}"))?;

    for _ in 0..30 {
        gtk::glib::timeout_future_seconds(2).await;
        let session_api_key = api_key.clone();
        let session_api_secret = api_secret.clone();
        let session_token = token.clone();
        let maybe_session = gtk::gio::spawn_blocking(move || {
            external_scrobbling::request_lastfm_session(
                &session_api_key,
                &session_api_secret,
                &session_token,
            )
        })
        .await
        .map_err(|_| "Last.fm session task failed.".to_string())??;
        if let Some(session) = maybe_session {
            shell.update_app_settings("Last.fm connection setting", |settings| {
                settings.lastfm_api_key = api_key.clone();
                settings.scrobbling.lastfm.api_key = api_key.clone();
                settings.scrobbling.lastfm.api_secret = api_secret.clone();
                settings.scrobbling.lastfm.username = session.username.clone();
                settings.scrobbling.lastfm.session_key = session.session_key.clone();
                true
            });
            shell.update_discord_presence(&shell.state.player.borrow());
            return Ok(session);
        }
    }

    Err(tr("Timed out waiting for Last.fm authorization."))
}

async fn connect_librefm_session(shell: &Rc<Shell>) -> Result<AudioscrobblerSession, String> {
    let token = gtk::gio::spawn_blocking(external_scrobbling::request_librefm_auth_token)
        .await
        .map_err(|_| "Libre.fm authorization task failed.".to_string())??;
    let url = external_scrobbling::librefm_auth_url(&token);
    let launcher = gtk::UriLauncher::new(&url);
    launcher
        .launch_future(Some(&shell.window))
        .await
        .map_err(|error| format!("Failed to open Libre.fm authorization: {error}"))?;

    for _ in 0..30 {
        gtk::glib::timeout_future_seconds(2).await;
        let session_token = token.clone();
        let maybe_session = gtk::gio::spawn_blocking(move || {
            external_scrobbling::request_librefm_session(&session_token)
        })
        .await
        .map_err(|_| "Libre.fm session task failed.".to_string())??;
        if let Some(session) = maybe_session {
            shell.update_scrobbling_settings("Libre.fm connection setting", |settings| {
                settings.librefm.username = session.username.clone();
                settings.librefm.api_key = "rufin".to_string();
                settings.librefm.api_secret = "rufin".to_string();
                settings.librefm.session_key = session.session_key.clone();
                true
            });
            return Ok(session);
        }
    }

    Err(tr("Timed out waiting for Libre.fm authorization"))
}

fn playback_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Playback"))
        .icon_name("audio-x-generic-symbolic")
        .build();

    let settings = shell.state.settings.borrow().playback.clone();

    let transition_group = adw::PreferencesGroup::builder()
        .title(tr("Transitions"))
        .build();
    let transition_titles = [tr("Gapless"), tr("Crossfade")];
    let transition_refs = transition_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let transition_options = gtk::StringList::new(&transition_refs);
    let transition_row = adw::ComboRow::builder()
        .title(tr("Transition mode"))
        .model(&transition_options)
        .selected(transition_index(settings.transition_mode))
        .build();
    let transition_shell = Rc::clone(shell);
    transition_row.connect_selected_notify(move |row| {
        transition_shell.update_playback_settings(|settings| {
            settings.transition_mode = transition_from_index(row.selected());
        });
    });
    transition_group.add(&transition_row);

    let crossfade_row = adw::ActionRow::builder()
        .title(tr("Crossfade duration"))
        .subtitle(tr("Seconds"))
        .build();
    let crossfade = gtk::SpinButton::with_range(1.0, 12.0, 1.0);
    crossfade.set_value(f64::from(settings.crossfade_seconds));
    crossfade.set_valign(gtk::Align::Center);
    let crossfade_shell = Rc::clone(shell);
    crossfade.connect_value_changed(move |spin| {
        crossfade_shell.update_playback_settings(|settings| {
            settings.crossfade_seconds = spin.value().round() as u8;
        });
    });
    crossfade_row.add_suffix(&crossfade);
    crossfade_row.set_activatable_widget(Some(&crossfade));
    transition_group.add(&crossfade_row);
    page.add(&transition_group);

    let gain_group = adw::PreferencesGroup::builder()
        .title(tr("Leveling"))
        .build();
    let replay_gain_titles = [tr("Off"), tr("Track"), tr("Album")];
    let replay_gain_refs = replay_gain_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let replay_gain_options = gtk::StringList::new(&replay_gain_refs);
    let replay_gain_row = adw::ComboRow::builder()
        .title(tr("ReplayGain"))
        .model(&replay_gain_options)
        .selected(replay_gain_index(settings.replay_gain))
        .build();
    let replay_gain_shell = Rc::clone(shell);
    replay_gain_row.connect_selected_notify(move |row| {
        replay_gain_shell.update_playback_settings(|settings| {
            settings.replay_gain = replay_gain_from_index(row.selected());
        });
    });
    gain_group.add(&replay_gain_row);
    page.add(&gain_group);

    let streaming_group = adw::PreferencesGroup::builder()
        .title(tr("Streaming"))
        .build();
    let quality_titles = [
        tr("Original"),
        tr("320 kbps"),
        tr("256 kbps"),
        tr("192 kbps"),
        tr("128 kbps"),
    ];
    let quality_refs = quality_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let quality_options = gtk::StringList::new(&quality_refs);
    let quality_row = adw::ComboRow::builder()
        .title(tr("Stream quality"))
        .model(&quality_options)
        .selected(stream_quality_index(settings.stream_quality))
        .build();
    let quality_shell = Rc::clone(shell);
    quality_row.connect_selected_notify(move |row| {
        quality_shell.update_playback_settings(|settings| {
            settings.stream_quality = stream_quality_from_index(row.selected());
        });
    });
    streaming_group.add(&quality_row);
    page.add(&streaming_group);

    let output_group = adw::PreferencesGroup::builder().title(tr("Output")).build();
    let outputs = playback_output_options();
    let output_titles = outputs
        .iter()
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>();
    let output_options = gtk::StringList::new(&output_titles);
    let output_row = adw::ComboRow::builder()
        .title(tr("Audio output"))
        .model(&output_options)
        .selected(audio_output_index(
            &outputs,
            settings.audio_output.as_deref(),
        ))
        .build();
    let output_shell = Rc::clone(shell);
    output_row.connect_selected_notify(move |row| {
        let selected = outputs
            .get(row.selected() as usize)
            .and_then(|(id, _)| id.clone());
        output_shell.update_playback_settings(|settings| {
            settings.audio_output = selected;
        });
    });
    output_group.add(&output_row);
    page.add(&output_group);

    let equalizer_group = adw::PreferencesGroup::builder()
        .title(tr("Equalizer"))
        .build();
    let resetting_equalizer = Rc::new(Cell::new(false));
    let equalizer_row = adw::SwitchRow::builder()
        .title(tr("Enable equalizer"))
        .active(settings.equalizer.enabled)
        .build();
    let equalizer_shell = Rc::clone(shell);
    let switch_reset_guard = Rc::clone(&resetting_equalizer);
    equalizer_row.connect_active_notify(move |row| {
        if switch_reset_guard.get() {
            return;
        }
        equalizer_shell.update_playback_settings(|settings| {
            settings.equalizer.enabled = row.is_active();
        });
    });
    equalizer_group.add(&equalizer_row);

    let band_scales = Rc::new(std::cell::RefCell::new(Vec::with_capacity(
        EQUALIZER_BAND_COUNT,
    )));
    for index in 0..EQUALIZER_BAND_COUNT {
        let row = adw::ActionRow::builder()
            .title(equalizer_band_title(index))
            .build();
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -12.0, 12.0, 0.5);
        scale.set_value(settings.equalizer.bands.get(index).copied().unwrap_or(0.0));
        scale.set_draw_value(true);
        scale.set_digits(1);
        scale.set_width_request(220);
        scale.set_valign(gtk::Align::Center);
        install_equalizer_vertical_scroll_passthrough(&scale);
        let band_shell = Rc::clone(shell);
        let scale_reset_guard = Rc::clone(&resetting_equalizer);
        scale.connect_value_changed(move |scale| {
            if scale_reset_guard.get() {
                return;
            }
            band_shell.update_playback_settings(|settings| {
                if settings.equalizer.bands.len() != EQUALIZER_BAND_COUNT {
                    settings.equalizer.sanitize();
                }
                if let Some(gain) = settings.equalizer.bands.get_mut(index) {
                    *gain = scale.value();
                }
            });
        });
        row.add_suffix(&scale);
        row.set_activatable_widget(Some(&scale));
        equalizer_group.add(&row);
        band_scales.borrow_mut().push(scale);
    }

    let reset_row = adw::ActionRow::builder()
        .title(tr("Reset equalizer"))
        .subtitle(tr("Restore neutral bands and disable equalizer."))
        .build();
    let reset_button = gtk::Button::with_label(&tr("Reset"));
    reset_button.set_valign(gtk::Align::Center);
    reset_button.add_css_class("destructive-action");
    let reset_shell = Rc::clone(shell);
    let reset_switch = equalizer_row.clone();
    let reset_scales = Rc::clone(&band_scales);
    let reset_guard = Rc::clone(&resetting_equalizer);
    reset_button.connect_clicked(move |_| {
        reset_guard.set(true);
        reset_switch.set_active(false);
        for scale in reset_scales.borrow().iter() {
            scale.set_value(0.0);
        }
        reset_guard.set(false);
        reset_shell.update_playback_settings(|settings| {
            settings.equalizer = EqualizerSettings::default();
        });
    });
    reset_row.add_suffix(&reset_button);
    reset_row.set_activatable_widget(Some(&reset_button));
    equalizer_group.add(&reset_row);
    page.add(&equalizer_group);

    page
}

fn install_equalizer_vertical_scroll_passthrough(scale: &gtk::Scale) {
    let controller = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let scale_weak = scale.downgrade();
    controller.connect_scroll(move |controller, _, dy| {
        if dy == 0.0 {
            return gtk::glib::Propagation::Proceed;
        }

        let Some(scale) = scale_weak.upgrade() else {
            return gtk::glib::Propagation::Stop;
        };
        let scale_widget = scale.upcast::<gtk::Widget>();
        scroll_nearest_parent_vertically(&scale_widget, dy, controller.unit());
        gtk::glib::Propagation::Stop
    });
    scale.add_controller(controller);
}

fn scroll_nearest_parent_vertically(widget: &gtk::Widget, dy: f64, unit: gtk::gdk::ScrollUnit) {
    let Some(scroller) = nearest_parent_scrolled_window(widget) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let page_size = adjustment.page_size();
    let multiplier = match unit {
        gtk::gdk::ScrollUnit::Surface => SURFACE_SCROLL_FACTOR,
        _ => page_size.powf(2.0 / 3.0),
    };
    let max_value = (adjustment.upper() - page_size).max(adjustment.lower());
    let value = (adjustment.value() + dy * multiplier).clamp(adjustment.lower(), max_value);
    adjustment.set_value(value);
}

fn nearest_parent_scrolled_window(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut parent = widget.parent();
    while let Some(widget) = parent {
        if let Ok(scroller) = widget.clone().downcast::<gtk::ScrolledWindow>() {
            return Some(scroller);
        }
        parent = widget.parent();
    }
    None
}

fn layout_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Layout"))
        .icon_name("preferences-desktop-display-symbolic")
        .build();

    page.add(&interface_group(shell));
    page.add(&sidebar_items_group(shell));

    let block_group = adw::PreferencesGroup::builder()
        .title(tr("Home Blocks"))
        .description(tr("Choose which Home blocks are visible and their order."))
        .build();
    let rows = Rc::new(std::cell::RefCell::new(Vec::new()));
    populate_home_block_rows(shell, &block_group, &rows);
    page.add(&block_group);

    page
}

fn transition_index(mode: PlaybackTransitionMode) -> u32 {
    match mode {
        PlaybackTransitionMode::Gapless => 0,
        PlaybackTransitionMode::Crossfade => 1,
    }
}

fn transition_from_index(index: u32) -> PlaybackTransitionMode {
    match index {
        1 => PlaybackTransitionMode::Crossfade,
        _ => PlaybackTransitionMode::Gapless,
    }
}

fn replay_gain_index(mode: ReplayGainMode) -> u32 {
    match mode {
        ReplayGainMode::Off => 0,
        ReplayGainMode::Track => 1,
        ReplayGainMode::Album => 2,
    }
}

fn replay_gain_from_index(index: u32) -> ReplayGainMode {
    match index {
        1 => ReplayGainMode::Track,
        2 => ReplayGainMode::Album,
        _ => ReplayGainMode::Off,
    }
}

fn stream_quality_index(quality: StreamQuality) -> u32 {
    match quality {
        StreamQuality::Original => 0,
        StreamQuality::MaxBitrateKbps(320) => 1,
        StreamQuality::MaxBitrateKbps(256) => 2,
        StreamQuality::MaxBitrateKbps(192) => 3,
        StreamQuality::MaxBitrateKbps(128) => 4,
        StreamQuality::MaxBitrateKbps(_) => 0,
    }
}

fn stream_quality_from_index(index: u32) -> StreamQuality {
    match index {
        1 => StreamQuality::MaxBitrateKbps(320),
        2 => StreamQuality::MaxBitrateKbps(256),
        3 => StreamQuality::MaxBitrateKbps(192),
        4 => StreamQuality::MaxBitrateKbps(128),
        _ => StreamQuality::Original,
    }
}

fn playback_output_options() -> Vec<(Option<String>, String)> {
    let mut outputs = vec![(None, tr("System default"))];
    outputs.extend(
        available_audio_outputs()
            .into_iter()
            .filter(|output| output.id != "autoaudiosink")
            .map(|output| (Some(output.id), output.name)),
    );
    outputs
}

fn audio_output_index(outputs: &[(Option<String>, String)], selected: Option<&str>) -> u32 {
    outputs
        .iter()
        .position(|(id, _)| id.as_deref() == selected)
        .unwrap_or_default() as u32
}

fn equalizer_band_title(index: usize) -> String {
    const BANDS: [&str; EQUALIZER_BAND_COUNT] = [
        "31 Hz", "62 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz",
        "16 kHz",
    ];
    BANDS.get(index).copied().unwrap_or("Band").to_string()
}

fn populate_home_block_rows(
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

fn home_block_row_order(visible_blocks: &[HomeBlockKind]) -> Vec<HomeBlockKind> {
    let mut blocks = visible_blocks.to_vec();
    for block in HomeBlockKind::all() {
        if !blocks.contains(&block) {
            blocks.push(block);
        }
    }
    blocks
}

fn insert_home_block_in_order(
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

fn home_block_subtitle(block: HomeBlockKind, active: bool, visible_index: Option<usize>) -> String {
    if let Some(index) = visible_index {
        return format!("{} {}", tr("Position"), index + 1);
    }
    match block.section_kind() {
        Some(_) => tr("Hidden server section"),
        None if active => tr("Visible"),
        None => tr("Hidden"),
    }
}

fn reorder_home_blocks(
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

fn home_block_drag_id(block: HomeBlockKind) -> &'static str {
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

fn home_block_from_drag_id(id: &str) -> Option<HomeBlockKind> {
    HomeBlockKind::all()
        .into_iter()
        .find(|block| home_block_drag_id(*block) == id)
}

fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build()
}

fn left_sidebar_row(title: &str, mode: LeftSidebarMode) -> adw::ComboRow {
    let titles = [tr("Full"), tr("Compact")];
    let refs = titles.iter().map(String::as_str).collect::<Vec<_>>();
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&refs))
        .selected(left_sidebar_mode_index(mode))
        .build()
}

fn left_sidebar_mode_index(mode: LeftSidebarMode) -> u32 {
    match mode {
        LeftSidebarMode::Full => 0,
        LeftSidebarMode::Compact => 1,
    }
}

fn left_sidebar_mode_from_index(index: u32) -> LeftSidebarMode {
    match index {
        1 => LeftSidebarMode::Compact,
        _ => LeftSidebarMode::Full,
    }
}

fn right_sidebar_row(title: &str, mode: RightSidebarMode) -> adw::ComboRow {
    let titles = [
        tr("Hidden"),
        tr("Compact"),
        tr("Default"),
        tr("Comfortable"),
        tr("Spacious"),
    ];
    let refs = titles.iter().map(String::as_str).collect::<Vec<_>>();
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&refs))
        .selected(right_sidebar_mode_index(mode))
        .build()
}

fn right_sidebar_mode_index(mode: RightSidebarMode) -> u32 {
    match mode {
        RightSidebarMode::Hidden => 0,
        RightSidebarMode::Compact => 1,
        RightSidebarMode::Default => 2,
        RightSidebarMode::Comfortable => 3,
        RightSidebarMode::Spacious => 4,
    }
}

fn right_sidebar_mode_from_index(index: u32) -> RightSidebarMode {
    match index {
        1 => RightSidebarMode::Compact,
        2 => RightSidebarMode::Default,
        3 => RightSidebarMode::Comfortable,
        4 => RightSidebarMode::Spacious,
        _ => RightSidebarMode::Hidden,
    }
}

fn discord_display_index(display_type: DiscordDisplayType) -> u32 {
    match display_type {
        DiscordDisplayType::Application => 0,
        DiscordDisplayType::Song => 1,
        DiscordDisplayType::Artist => 2,
    }
}

fn discord_display_from_index(index: u32) -> DiscordDisplayType {
    match index {
        1 => DiscordDisplayType::Song,
        2 => DiscordDisplayType::Artist,
        _ => DiscordDisplayType::Application,
    }
}

fn discord_link_index(link_type: DiscordLinkType) -> u32 {
    match link_type {
        DiscordLinkType::None => 0,
        DiscordLinkType::LastFm => 1,
        DiscordLinkType::MusicBrainz => 2,
        DiscordLinkType::MusicBrainzLastFm => 3,
    }
}

fn discord_link_from_index(index: u32) -> DiscordLinkType {
    match index {
        1 => DiscordLinkType::LastFm,
        2 => DiscordLinkType::MusicBrainz,
        3 => DiscordLinkType::MusicBrainzLastFm,
        _ => DiscordLinkType::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_drag_reorder_inserts_before_and_after_target_rows() {
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
    fn home_block_drag_reorder_only_moves_visible_blocks() {
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
}
