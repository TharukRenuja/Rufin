use domain::{AppSettings, GeneratedTrackStrategy, Track};
use library::SavedServer;

use crate::cover_art_policy;

use super::{
    AppController,
    root::{scrub_selected_track_image_refs, track_album_refs_with_settings},
};

pub(in crate::controller) fn prepare_provider_tracks(
    controller: &AppController,
    saved: &SavedServer,
    settings: &AppSettings,
    tracks: &mut [Track],
) -> Result<(), String> {
    scrub_selected_track_image_refs(saved, settings, tracks);
    cover_art_policy::bind_tracks(tracks, settings);
    track_album_refs_with_settings(&controller.store, saved, settings, tracks, &[])?;
    if !tracks.is_empty() {
        controller
            .store
            .with_store(|store| store.upsert_tracks(&saved.server.id, tracks, 0))?;
    }
    Ok(())
}

pub(in crate::controller) fn generated_track_strategy_for_saved(
    saved: &SavedServer,
) -> GeneratedTrackStrategy {
    if saved.server.provider == "jellyfin" && saved.use_jellyfin_instant_mix {
        GeneratedTrackStrategy::MixOnly
    } else {
        GeneratedTrackStrategy::ProviderDefault
    }
}
