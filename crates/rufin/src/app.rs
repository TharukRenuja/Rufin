//! Constructs Rufin's product owners and connects their concrete event lanes.

use std::sync::Arc;

use ::scrobbling::Scrobbler;
use async_channel::unbounded;
use playback::PlaybackHandles;
use secrets::SwitchableSecretStore;
use tracing::warn;
use ui::runtime::{ProductHandles, ProductReceivers, RuntimeInputs};

use crate::paths;
use crate::playback::PlaybackOwner;
use crate::release_update::ReleaseUpdateOwner;
use crate::scrobbling::ScrobblingOwner;
use crate::settings::{
    SettingsFile, SettingsUiPort, platform_secret_store, startup_scrobbling_settings,
};
use crate::source::{SourceBootstrap, SourceOutputs, SourceOwner};
use crate::waveform::WaveformOwner;

pub(crate) fn runtime_inputs() -> Result<RuntimeInputs, String> {
    paths::prepare()?;
    crate::schema30_migration::install_if_needed(
        &paths::settings_file(),
        &paths::released_store_file(),
        &paths::store_file(),
    )?;
    let runtime = tokio::runtime::Handle::current();
    let settings = SettingsFile::open(paths::settings_file())?;
    let stored = settings.load();
    let secrets = Arc::new(SwitchableSecretStore::new(platform_secret_store(&stored)));
    let (library, repair) =
        library::Library::open_with_repair(paths::store_file()).map_err(string_error)?;
    if let Some(repair) = repair {
        warn!(
            preserved_store = %repair.preserved_store.display(),
            recovered_rows = repair.recovered_rows,
            skipped_rows = repair.skipped_rows,
            unreadable_families = ?repair.unreadable_families,
            "repaired the Rufin Store; source facts will be rebuilt"
        );
    }
    let scrobbler = Arc::new(Scrobbler::new(
        library.clone(),
        startup_scrobbling_settings(&settings, &secrets),
        stored.ui.private_mode,
    )?);

    let (source_events, source_receiver) = unbounded();
    let (discovery_events, discovery_receiver) = unbounded();
    let (waveform_events, waveform_receiver) = unbounded();
    let (lyrics_events, lyrics_receiver) = unbounded();
    let (release_update_events, release_update_receiver) = unbounded();
    let artwork =
        artwork::Artwork::new(paths::artwork_dir(), runtime.clone()).map_err(string_error)?;
    let discord = Arc::new(desktop_integration::Discord::new());
    let release_updates =
        ReleaseUpdateOwner::new(settings.clone(), runtime.clone(), release_update_events);
    let release_notes = release_updates.bundled_notes();

    let SourceBootstrap {
        owner: source,
        configured,
        operation,
    } = SourceOwner::open_dormant(
        artwork.clone(),
        library.clone(),
        settings.clone(),
        Arc::clone(&secrets),
        Arc::clone(&scrobbler),
        runtime.clone(),
        SourceOutputs {
            events: source_events.clone(),
            discovery: discovery_events,
        },
    );
    let waveform = WaveformOwner::new(
        runtime.clone(),
        waveform_events,
        paths::playback_dir(),
        stored.ui.seekbar_waveform_enabled,
    );
    let lyrics = lyrics::LyricsService::new(
        library.clone(),
        runtime.clone(),
        stored.ui.lyrics.clone(),
        stored.ui.private_mode,
        lyrics_events,
    );
    let playback = PlaybackOwner::new(
        library.clone(),
        settings.clone(),
        runtime.clone(),
        source_events,
        source.acceptance_sender(),
        Arc::clone(&waveform),
        Arc::clone(&lyrics),
        Arc::clone(&discord),
        Arc::clone(&scrobbler),
    );
    let scrobbling = ScrobblingOwner::new(
        settings.clone(),
        Arc::clone(&secrets),
        runtime.clone(),
        scrobbler,
        Arc::clone(&playback),
    );

    source.attach_playback(&playback);

    let settings_playback = Arc::clone(&playback);
    let settings_lyrics = Arc::clone(&lyrics);
    let settings_source = Arc::clone(&source);
    let settings_scrobbling = Arc::clone(&scrobbling);
    let settings_handle = SettingsUiPort::new(settings, move |previous, current| {
        if previous.ui.rich_presence != current.ui.rich_presence
            || previous.ui.private_mode != current.ui.private_mode
            || previous.ui.lastfm_api_key != current.ui.lastfm_api_key
        {
            settings_playback.update_discord_settings();
        }
        if previous.ui.seekbar_waveform_enabled != current.ui.seekbar_waveform_enabled {
            settings_playback.waveform_setting_changed(current.ui.seekbar_waveform_enabled);
        }
        if previous.ui.playback != current.ui.playback {
            settings_playback.playback_settings_changed(current.ui.playback.clone());
        }
        if previous.ui.auto_dj_refill_threshold != current.ui.auto_dj_refill_threshold {
            settings_playback.auto_dj_threshold_changed(
                current.ui.auto_dj_enabled,
                current.ui.auto_dj_refill_threshold,
            );
        }
        if previous.ui.private_mode != current.ui.private_mode {
            settings_scrobbling.private_mode_changed(current.ui.private_mode);
        }
        if previous.ui.lyrics != current.ui.lyrics
            || previous.ui.private_mode != current.ui.private_mode
        {
            settings_lyrics.settings_changed(current.ui.lyrics.clone(), current.ui.private_mode);
        }
        if previous.ui.allows_external_album_lookup() != current.ui.allows_external_album_lookup() {
            settings_source
                .album_release_settings_changed(current.ui.allows_external_album_lookup());
        }
    });

    source.start()?;
    let source_handle: ui::runtime::SourceHandle = source.clone();
    let smart_playlists: ui::runtime::SmartPlaylistHandle = source;
    let transport: playback::TransportHandle = playback.clone();
    let queue: playback::QueueHandle = playback.clone();
    let radio: playback::RadioHandle = playback;

    Ok(RuntimeInputs {
        products: ProductHandles {
            source: source_handle,
            smart_playlists,
            playback: PlaybackHandles {
                transport,
                queue,
                radio,
            },
            artwork,
            lyrics: lyrics.handle(),
            release_updates,
            scrobbling,
        },
        settings: settings_handle,
        receivers: ProductReceivers {
            source: source_receiver,
            source_discovery: discovery_receiver,
            waveform: waveform_receiver,
            lyrics: lyrics_receiver,
            release_updates: release_update_receiver,
        },
        configured_sources: configured,
        source_operation: operation,
        release_notes,
    })
}

fn string_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}
