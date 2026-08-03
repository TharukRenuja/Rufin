//! Bounded release-type lookup for one selected-library snapshot.
//!
//! Source owns when this work starts and whether it is still current. Library
//! owns candidates and exact acceptance, while Album Lookup performs the
//! external request.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::Sender;
use library::{AlbumReleaseResult, SourceId};
use playback::SourceSessionEpoch;
use tracing::{info, warn};
use ui::runtime::{SelectedLibraryUpdate, SourceEvent};

use crate::settings::SettingsFile;
use crate::source::WeakActiveSource;

const LOOKUP_LIMIT: usize = 500;

pub(crate) fn run_selected_album_release_lookup(
    settings: SettingsFile,
    events: Sender<SourceEvent>,
    source_id: SourceId,
    source_session_epoch: SourceSessionEpoch,
    selected: WeakActiveSource,
    cancelled: Arc<AtomicBool>,
) {
    if !lookup_allowed(&settings, &cancelled) {
        return;
    }
    let Some(current) = selected.upgrade().and_then(|selected| selected.resolve()) else {
        return;
    };
    let library_id = current.library.library_id();
    let candidates = match current.library.take_album_release_lookups(LOOKUP_LIMIT) {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, %source_id, "could not read album release lookup candidates");
            return;
        }
    };
    drop(current);
    let requested = candidates.len();
    let mut found = 0_usize;
    let mut missing = 0_usize;
    let mut errors = 0_usize;
    for candidate in candidates {
        if !lookup_allowed(&settings, &cancelled) {
            break;
        }
        let (release_group_id, release_id) = match &candidate.identity {
            library::AlbumReleaseIdentity::ReleaseGroup(id) => (Some(id.as_str()), None),
            library::AlbumReleaseIdentity::Release(id) => (None, Some(id.as_str())),
        };
        let result = match metadata_lookup::lookup_album_release(release_group_id, release_id) {
            Ok(Some(metadata)) => {
                found += 1;
                AlbumReleaseResult::Found {
                    release_types: metadata.release_types,
                    is_compilation: metadata.is_compilation,
                }
            }
            Ok(None) => {
                missing += 1;
                AlbumReleaseResult::Missing
            }
            Err(error) => {
                errors += 1;
                warn!(
                    %error,
                    album_id = %candidate.album_id,
                    "failed to look up album release"
                );
                continue;
            }
        };
        if !lookup_allowed(&settings, &cancelled) {
            break;
        }
        let Some(current) = selected.upgrade().and_then(|selected| selected.resolve()) else {
            break;
        };
        if current.library.library_id() != library_id {
            break;
        }
        match current
            .library
            .accept_album_release_result(candidate, result)
        {
            Ok(Some(change)) if lookup_allowed(&settings, &cancelled) => {
                let _ = events.try_send(SourceEvent::LibraryUpdate(SelectedLibraryUpdate {
                    source_id: source_id.clone(),
                    source_session_epoch,
                    change,
                    home: None,
                }));
            }
            Ok(_) => {}
            Err(error) => {
                errors += 1;
                warn!(%error, "could not accept album release metadata");
            }
        }
    }
    info!(
        %source_id,
        requested,
        found,
        missing,
        errors,
        cancelled = !lookup_allowed(&settings, &cancelled),
        "completed album release lookup"
    );
}

fn lookup_allowed(settings: &SettingsFile, cancelled: &AtomicBool) -> bool {
    !cancelled.load(Ordering::Acquire) && settings.load().ui.allows_external_metadata_lookup()
}
