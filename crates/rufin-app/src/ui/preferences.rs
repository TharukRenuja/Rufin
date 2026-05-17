use std::{cell::Cell, rc::Rc};

use adw::prelude::*;
use rufin_core::{
    DensityMode, DiscordDisplayType, DiscordLinkType, EQUALIZER_BAND_COUNT, EqualizerSettings,
    HomeBlockKind, PlaybackTransitionMode, ReplayGainMode, StreamQuality,
};
use rufin_playback::available_audio_outputs;

use crate::i18n::tr;

use super::Shell;

pub(super) fn present_preferences_dialog(shell: &Rc<Shell>) {
    let dialog = adw::PreferencesDialog::builder()
        .title(tr("Preferences"))
        .search_enabled(true)
        .content_width(560)
        .content_height(640)
        .build();

    let general_page = general_page(shell);
    let playback_page = playback_page(shell);
    let home_page = home_page(shell);
    let library_page = library_page(shell, &dialog);
    dialog.add(&general_page);
    dialog.add(&playback_page);
    dialog.add(&home_page);
    dialog.add(&library_page);
    dialog.present(Some(&shell.window));
}

fn general_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();

    let interface_group = adw::PreferencesGroup::builder()
        .title(tr("Interface"))
        .build();

    let density_titles = [tr("Auto"), tr("Normal"), tr("Compact")];
    let density_refs = density_titles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let density_options = gtk::StringList::new(&density_refs);
    let density_row = adw::ComboRow::builder()
        .title(tr("Left sidebar density"))
        .subtitle(tr("Choose when the left sidebar uses compact navigation."))
        .model(&density_options)
        .selected(density_index(shell.state.density_mode.get()))
        .build();
    let density_shell = Rc::clone(shell);
    density_row.connect_selected_notify(move |row| {
        density_shell.set_density_mode(density_from_index(row.selected()));
    });
    interface_group.add(&density_row);

    let settings = shell.state.settings.borrow().clone();
    let sidebar_row = adw::SwitchRow::builder()
        .title(tr("Show sidebar"))
        .subtitle(tr("Keep the queue sidebar visible in the main window."))
        .active(settings.right_panel_visible)
        .build();
    let sidebar_shell = Rc::clone(shell);
    sidebar_row.connect_active_notify(move |row| {
        sidebar_shell.set_right_panel_visible(row.is_active());
    });
    interface_group.add(&sidebar_row);

    let lyrics_panel_row = adw::SwitchRow::builder()
        .title(tr("Show Lyrics Panel"))
        .subtitle(tr("Keep the lyrics section visible below the queue."))
        .active(settings.lyrics_panel_visible)
        .build();
    let lyrics_panel_shell = Rc::clone(shell);
    lyrics_panel_row.connect_active_notify(move |row| {
        lyrics_panel_shell.set_lyrics_panel_visible(row.is_active());
    });
    interface_group.add(&lyrics_panel_row);

    page.add(&interface_group);

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

fn home_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Home"))
        .icon_name("go-home-symbolic")
        .build();

    let block_group = adw::PreferencesGroup::builder()
        .title(tr("Blocks"))
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

fn library_page(shell: &Rc<Shell>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Library"))
        .icon_name("network-server-symbolic")
        .build();

    let library = shell.state.library.borrow();
    let username = library
        .username
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("no account"));
    let server_name = library
        .server
        .as_ref()
        .map(|server| server.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| tr("No server"));
    let server_url = library
        .server
        .as_ref()
        .map(|server| server.base_url.clone())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| tr("No active server"));

    let server_group = adw::PreferencesGroup::builder()
        .title(tr("Music Server"))
        .build();
    let server_row = adw::ActionRow::builder()
        .title(server_name)
        .subtitle(format!(
            "{}\n{}: {}\n{}: {} {} / {} {}",
            server_url,
            tr("User"),
            username,
            tr("Cached"),
            library.cached_album_count,
            tr("albums"),
            library.cached_track_count,
            tr("tracks")
        ))
        .subtitle_lines(3)
        .build();
    server_group.add(&server_row);

    let status_row = adw::ActionRow::builder()
        .title(tr("Sync Status"))
        .subtitle(library.sync_status.clone())
        .build();
    server_group.add(&status_row);
    page.add(&server_group);
    drop(library);

    let actions_group = adw::PreferencesGroup::builder()
        .title(tr("Actions"))
        .build();

    let resync = button_row("Resync Library", "view-refresh-symbolic");
    let controller = shell.controller.clone();
    resync.connect_activated(move |_| controller.resync_active_server());
    actions_group.add(&resync);

    let clear_cache = button_row("Clear Cached Library", "edit-clear-symbolic");
    let clear_dialog = dialog.clone();
    let clear_shell = Rc::clone(shell);
    clear_cache.connect_activated(move |_| {
        clear_dialog.close();
        clear_shell.confirm_clear_cache();
    });
    actions_group.add(&clear_cache);

    let forget = button_row("Forget Server", "user-trash-symbolic");
    forget.add_css_class("destructive-action");
    let forget_dialog = dialog.clone();
    let forget_shell = Rc::clone(shell);
    forget.connect_activated(move |_| {
        forget_dialog.close();
        forget_shell.confirm_forget_server();
    });
    actions_group.add(&forget);

    page.add(&actions_group);
    page
}

fn button_row(title: &str, icon_name: &str) -> adw::ButtonRow {
    adw::ButtonRow::builder()
        .title(tr(title))
        .start_icon_name(icon_name)
        .end_icon_name("go-next-symbolic")
        .build()
}

fn density_index(density: DensityMode) -> u32 {
    match density {
        DensityMode::Auto => 0,
        DensityMode::Normal => 1,
        DensityMode::Compact => 2,
    }
}

fn density_from_index(index: u32) -> DensityMode {
    match index {
        1 => DensityMode::Normal,
        2 => DensityMode::Compact,
        _ => DensityMode::Auto,
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
