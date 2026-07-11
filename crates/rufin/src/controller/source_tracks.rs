use domain::{AppSettings, Track};
use library::SavedSource;

use crate::cover_art_policy;

use super::{
    StoreHandle,
    root::{scrub_selected_track_image_refs, track_album_refs_with_settings},
};

pub(in crate::controller) fn prepare_cached_tracks(
    store: &StoreHandle,
    saved: &SavedSource,
    settings: &AppSettings,
    tracks: &mut [Track],
) -> Result<(), String> {
    scrub_selected_track_image_refs(saved, settings, tracks);
    cover_art_policy::bind_tracks(tracks, settings);
    track_album_refs_with_settings(store, saved, settings, tracks, &[])
}

pub(in crate::controller) fn prepare_source_tracks(
    store: &StoreHandle,
    saved: &SavedSource,
    settings: &AppSettings,
    tracks: &mut [Track],
) -> Result<(), String> {
    prepare_cached_tracks(store, saved, settings, tracks)
}
