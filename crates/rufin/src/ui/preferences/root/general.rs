use super::*;
use crate::ui::{
    build_equalizer_preset_row, connect_equalizer_scale_commit, equalizer_band_title,
    equalizer_default_preset_bands, equalizer_preset_bands, equalizer_preset_name_at,
    equalizer_preset_position, equalizer_selected_preset, install_equalizer_scroll,
};

pub(in crate::ui) fn scrobbling_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Scrobbling"))
        .icon_name(SCROBBLING_ICON_NAME)
        .build();
    let app_settings = shell.controller.load_settings_with_scrobbling_secrets();
    *shell.state.settings.borrow_mut() = app_settings.clone();
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
            &tr(". You only need to fill email and an application name parts"),
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
        if lastfm_api_shell
            .update_app_settings_with_scrobbling_secrets("Last.fm API key setting", |settings| {
                if settings.lastfm_api_key == api_key
                    && settings.scrobbling.lastfm.api_key == api_key
                {
                    return false;
                }
                settings.lastfm_api_key = api_key.clone();
                settings.scrobbling.lastfm.api_key = api_key;
                settings.scrobbling.lastfm.session_key.clear();
                settings.scrobbling.lastfm.username.clear();
                true
            })
            .is_some()
        {
            lastfm_api_shell.retry_external_cover_lookups("Last.fm API key setting");
        }
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
pub(in crate::ui) fn audioscrobbler_connection_subtitle(
    settings: &AudioscrobblerScrobbleSettings,
) -> String {
    if settings.session_key.trim().is_empty() {
        tr("Not connected")
    } else {
        audioscrobbler_connected_subtitle(&settings.username)
    }
}
pub(in crate::ui) fn audioscrobbler_connected_subtitle(username: &str) -> String {
    let username = username.trim();
    if username.is_empty() {
        tr("Connected")
    } else {
        tr_with("Connected as {username}", &[("username", username)])
    }
}
pub(in crate::ui) fn inline_link_markup(
    before: &str,
    url: &str,
    label: &str,
    after: &str,
) -> String {
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
            shell.update_app_settings_with_scrobbling_secrets(
                "Last.fm connection setting",
                |settings| {
                    settings.lastfm_api_key = api_key.clone();
                    settings.scrobbling.lastfm.api_key = api_key.clone();
                    settings.scrobbling.lastfm.api_secret = api_secret.clone();
                    settings.scrobbling.lastfm.username = session.username.clone();
                    settings.scrobbling.lastfm.session_key = session.session_key.clone();
                    true
                },
            );
            shell.retry_external_cover_lookups("Last.fm connection setting");
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
pub(in crate::ui) fn playback_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Playback"))
        .icon_name("route-tracks-symbolic")
        .build();

    let app_settings = shell.state.settings.borrow().clone();
    let settings = app_settings.playback.clone();

    let transition_group = adw::PreferencesGroup::builder()
        .title(tr("Queue and transitions"))
        .build();
    let transition_titles = [tr("Default"), tr("Gapless"), tr("Crossfade")];
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
    let crossfade = gtk::SpinButton::with_range(
        f64::from(MIN_CROSSFADE_SECONDS),
        f64::from(MAX_CROSSFADE_SECONDS),
        1.0,
    );
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

    let skip_same_album_crossfade_row = adw::SwitchRow::builder()
        .title(tr("Skip same-album crossfade"))
        .subtitle(tr("Keep album transitions gapless when possible"))
        .active(settings.skip_same_album_crossfade)
        .build();
    let skip_same_album_crossfade_shell = Rc::clone(shell);
    skip_same_album_crossfade_row.connect_active_notify(move |row| {
        skip_same_album_crossfade_shell.update_playback_settings(|settings| {
            settings.skip_same_album_crossfade = row.is_active();
        });
    });
    transition_group.add(&skip_same_album_crossfade_row);

    let audio_fade_row = adw::SwitchRow::builder()
        .title(tr("Audio fade on play/pause"))
        .subtitle(tr("Fade audio when playback is paused or resumed"))
        .active(settings.audio_fade_on_status_change)
        .build();
    let audio_fade_shell = Rc::clone(shell);
    audio_fade_row.connect_active_notify(move |row| {
        audio_fade_shell.update_playback_settings(|settings| {
            settings.audio_fade_on_status_change = row.is_active();
        });
    });
    transition_group.add(&audio_fade_row);

    let refill_row = adw::ActionRow::builder()
        .title(tr("Auto DJ refill threshold"))
        .subtitle(tr("Add tracks when fewer than this many remain"))
        .build();
    let refill = gtk::SpinButton::with_range(
        f64::from(MIN_AUTO_DJ_REFILL_THRESHOLD),
        f64::from(MAX_AUTO_DJ_REFILL_THRESHOLD),
        1.0,
    );
    refill.set_value(f64::from(app_settings.auto_dj_refill_threshold));
    refill.set_valign(gtk::Align::Center);
    let refill_shell = Rc::clone(shell);
    refill.connect_value_changed(move |spin| {
        let threshold = spin.value().round() as u8;
        let changed = refill_shell
            .update_app_settings("Auto DJ setting", |settings| {
                if settings.auto_dj_refill_threshold == threshold {
                    return false;
                }
                settings.auto_dj_refill_threshold = threshold;
                true
            })
            .is_some();
        if changed {
            refill_shell.controller.refill_auto_dj_queue();
        }
    });
    refill_row.add_suffix(&refill);
    refill_row.set_activatable_widget(Some(&refill));
    transition_group.add(&refill_row);

    page.add(&transition_group);

    let audio_group = adw::PreferencesGroup::builder().title(tr("Audio")).build();
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
    audio_group.add(&replay_gain_row);

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
    audio_group.add(&quality_row);

    let waveform_row = adw::SwitchRow::builder()
        .title(tr("Waveform seekbar"))
        .subtitle(tr("Generate and cache waveforms for the current track"))
        .active(app_settings.seekbar_waveform_enabled)
        .build();
    let waveform_shell = Rc::clone(shell);
    waveform_row.connect_active_notify(move |row| {
        waveform_shell.set_seekbar_waveform_enabled(row.is_active());
    });
    audio_group.add(&waveform_row);

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
    audio_group.add(&output_row);
    page.add(&audio_group);

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

    let selected_preset =
        equalizer_preset_position(&equalizer_selected_preset(&settings.equalizer));
    let selected_preset = Rc::new(Cell::new(selected_preset));
    let preset_row = build_equalizer_preset_row("Preset", selected_preset.get());
    let preset_shell = Rc::clone(shell);
    let preset_switch = equalizer_row.clone();
    let preset_reset_guard = Rc::clone(&resetting_equalizer);
    equalizer_group.add(&preset_row);

    let band_scales = Rc::new(std::cell::RefCell::new(Vec::with_capacity(
        EQUALIZER_BAND_COUNT,
    )));
    let pending_equalizer_update = Rc::new(RefCell::new(None::<gtk::glib::SourceId>));
    let equalizer_drag_active = Rc::new(Cell::new(false));
    let equalizer_commit: Rc<dyn Fn()> = {
        let band_shell = Rc::clone(shell);
        let update_preset = preset_row.clone();
        let update_selected_preset = Rc::clone(&selected_preset);
        let update_guard = Rc::clone(&resetting_equalizer);
        let update_scales = Rc::clone(&band_scales);
        Rc::new(move || {
            let bands = update_scales
                .borrow()
                .iter()
                .map(gtk::Scale::value)
                .collect::<Vec<_>>();
            band_shell.update_playback_settings(|settings| {
                if settings.equalizer.bands.len() != EQUALIZER_BAND_COUNT {
                    settings.equalizer.sanitize();
                }
                settings.equalizer.bands = bands.clone();
                settings.equalizer.selected_preset = "Custom".to_string();
            });
            let preset =
                equalizer_selected_preset(&band_shell.state.settings.borrow().playback.equalizer);
            update_guard.set(true);
            update_selected_preset.set(equalizer_preset_position(&preset));
            update_preset.set_selected(equalizer_preset_position(&preset));
            update_guard.set(false);
        })
    };
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
        install_equalizer_scroll(&scale);
        connect_equalizer_scale_commit(
            &scale,
            Rc::clone(&resetting_equalizer),
            Rc::clone(&pending_equalizer_update),
            Rc::clone(&equalizer_drag_active),
            Rc::clone(&equalizer_commit),
        );
        row.add_suffix(&scale);
        row.set_activatable_widget(Some(&scale));
        equalizer_group.add(&row);
        band_scales.borrow_mut().push(scale);
    }

    let preset_scales = Rc::clone(&band_scales);
    let preset_selected_preset = Rc::clone(&selected_preset);
    preset_row.connect_selected_notify(move |row| {
        if preset_reset_guard.get() {
            return;
        }
        let Some(preset) = equalizer_preset_name_at(row.selected()) else {
            return;
        };
        let bands = equalizer_preset_bands(&preset);
        preset_reset_guard.set(true);
        preset_switch.set_active(true);
        preset_selected_preset.set(equalizer_preset_position(&preset));
        for (scale, gain) in preset_scales.borrow().iter().zip(bands.iter()) {
            scale.set_value(*gain);
        }
        preset_reset_guard.set(false);
        preset_shell.update_playback_settings(|settings| {
            settings.equalizer.enabled = true;
            settings.equalizer.selected_preset = preset.clone();
            settings.equalizer.bands = bands;
            settings.equalizer.sanitize();
        });
    });

    let reset_row = adw::ActionRow::builder()
        .title(tr("Reset equalizer"))
        .subtitle(tr("Restore selected preset to default bands."))
        .build();
    let reset_button = gtk::Button::with_label(&tr("Reset"));
    reset_button.set_valign(gtk::Align::Center);
    reset_button.add_css_class("destructive-action");
    let reset_shell = Rc::clone(shell);
    let reset_preset = preset_row.clone();
    let reset_selected_preset = Rc::clone(&selected_preset);
    let reset_scales = Rc::clone(&band_scales);
    let reset_guard = Rc::clone(&resetting_equalizer);
    reset_button.connect_clicked(move |_| {
        let preset = equalizer_preset_name_at(reset_selected_preset.get()).unwrap_or_else(|| {
            equalizer_selected_preset(&reset_shell.state.settings.borrow().playback.equalizer)
        });
        let bands = equalizer_default_preset_bands(&preset);
        reset_guard.set(true);
        reset_selected_preset.set(equalizer_preset_position(&preset));
        reset_preset.set_selected(equalizer_preset_position(&preset));
        for (scale, gain) in reset_scales.borrow().iter().zip(bands.iter()) {
            scale.set_value(*gain);
        }
        reset_guard.set(false);
        reset_shell.update_playback_settings(|settings| {
            settings.equalizer.selected_preset = preset;
            settings.equalizer.bands = bands;
            settings.equalizer.sanitize();
        });
    });
    reset_row.add_suffix(&reset_button);
    reset_row.set_activatable_widget(Some(&reset_button));
    equalizer_group.add(&reset_row);
    page.add(&equalizer_group);

    page
}
pub(in crate::ui) fn layout_page(shell: &Rc<Shell>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(tr("Layout"))
        .icon_name("preferences-desktop-display-symbolic")
        .build();

    page.add(&interface_group(shell));
    page.add(&sidebar_items_group(shell));

    let block_group = adw::PreferencesGroup::builder()
        .title(tr("Home Blocks"))
        .build();
    let rows = Rc::new(std::cell::RefCell::new(Vec::new()));
    populate_home_block_rows(shell, &block_group, &rows);
    page.add(&block_group);

    page
}
pub(in crate::ui) fn transition_index(mode: PlaybackTransitionMode) -> u32 {
    match mode {
        PlaybackTransitionMode::Default => 0,
        PlaybackTransitionMode::Gapless => 1,
        PlaybackTransitionMode::Crossfade => 2,
    }
}
pub(in crate::ui) fn transition_from_index(index: u32) -> PlaybackTransitionMode {
    match index {
        1 => PlaybackTransitionMode::Gapless,
        2 => PlaybackTransitionMode::Crossfade,
        _ => PlaybackTransitionMode::Default,
    }
}
pub(in crate::ui) fn replay_gain_index(mode: ReplayGainMode) -> u32 {
    match mode {
        ReplayGainMode::Off => 0,
        ReplayGainMode::Track => 1,
        ReplayGainMode::Album => 2,
    }
}
pub(in crate::ui) fn replay_gain_from_index(index: u32) -> ReplayGainMode {
    match index {
        1 => ReplayGainMode::Track,
        2 => ReplayGainMode::Album,
        _ => ReplayGainMode::Off,
    }
}
pub(in crate::ui) fn stream_quality_index(quality: StreamQuality) -> u32 {
    match quality {
        StreamQuality::Original => 0,
        StreamQuality::MaxBitrateKbps(320) => 1,
        StreamQuality::MaxBitrateKbps(256) => 2,
        StreamQuality::MaxBitrateKbps(192) => 3,
        StreamQuality::MaxBitrateKbps(128) => 4,
        StreamQuality::MaxBitrateKbps(_) => 0,
    }
}
pub(in crate::ui) fn stream_quality_from_index(index: u32) -> StreamQuality {
    match index {
        1 => StreamQuality::MaxBitrateKbps(320),
        2 => StreamQuality::MaxBitrateKbps(256),
        3 => StreamQuality::MaxBitrateKbps(192),
        4 => StreamQuality::MaxBitrateKbps(128),
        _ => StreamQuality::Original,
    }
}
pub(in crate::ui) fn playback_output_options() -> Vec<(Option<String>, String)> {
    let mut outputs = vec![(None, tr("System default"))];
    outputs.extend(
        available_audio_outputs()
            .into_iter()
            .filter(|output| output.id != "autoaudiosink")
            .map(|output| (Some(output.id), output.name)),
    );
    outputs
}
pub(in crate::ui) fn audio_output_index(
    outputs: &[(Option<String>, String)],
    selected: Option<&str>,
) -> u32 {
    outputs
        .iter()
        .position(|(id, _)| id.as_deref() == selected)
        .unwrap_or_default() as u32
}
