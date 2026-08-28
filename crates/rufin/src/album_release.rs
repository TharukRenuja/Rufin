//! Bounded Album release enrichment over exact Database candidates.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::Sender;
use library::{AlbumReleaseResult, ReadCancellation, SourceKey};
use playback::SourceSessionEpoch;
use tracing::{info, warn};
use ui::runtime::{CatalogChange, CatalogPublication, SourceEvent};

use crate::settings::SettingsFile;
use crate::source::WeakActiveSource;

const LOOKUP_LIMIT: usize = 8;

pub(crate) async fn run_selected_album_release_lookup(
    settings: SettingsFile,
    events: Sender<SourceEvent>,
    source_key: SourceKey,
    source_session_epoch: SourceSessionEpoch,
    selected: WeakActiveSource,
    cancelled: Arc<AtomicBool>,
) {
    let mut requested = 0;
    let mut found = 0;
    let mut missing = 0;
    let mut errors = 0;
    let Some(current) = selected.upgrade().and_then(|session| session.resolve()) else {
        return;
    };
    if current.source_key != source_key
        || current.source_session_epoch != source_session_epoch
        || !lookup_allowed(&settings, &cancelled)
    {
        return;
    }
    let candidates = match current
        .database
        .album_release_candidates(source_key, LOOKUP_LIMIT, &ReadCancellation::new())
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, %source_key, "could not read Album release candidates");
            return;
        }
    };
    for candidate in candidates {
        if !lookup_allowed(&settings, &cancelled) {
            break;
        }
        requested += 1;
        let identity = candidate.lookup_identity.clone();
        let lookup_identity = identity.clone();
        let lookup = tokio::task::spawn_blocking(move || {
            let (release_group, release) =
                lookup_identity.strip_prefix("release-group:").map_or_else(
                    || (None, lookup_identity.strip_prefix("release:")),
                    |id| (Some(id), None),
                );
            metadata_lookup::lookup_album_release(release_group, release)
        })
        .await;
        let result = match lookup {
            Ok(Ok(Some(metadata))) => {
                found += 1;
                AlbumReleaseResult::Found {
                    release_types: metadata.release_types,
                }
            }
            Ok(Ok(None)) => {
                missing += 1;
                AlbumReleaseResult::Missing
            }
            Ok(Err(error)) => {
                errors += 1;
                warn!(%error, album_key = %candidate.album_key, "Album release lookup failed");
                break;
            }
            Err(error) => {
                errors += 1;
                warn!(%error, album_key = %candidate.album_key, "Album release worker failed");
                break;
            }
        };
        match current
            .database
            .accept_album_release_result(source_key, candidate.album_key, identity.as_str(), result)
            .await
        {
            Ok(Some(_)) => {
                let _ = events.try_send(SourceEvent::CatalogPublished(CatalogPublication {
                    source_key,
                    source_session_epoch,
                    favorite: None,
                    change: CatalogChange::Album(candidate.album_key),
                }));
            }
            Ok(None) => {}
            Err(error) => {
                errors += 1;
                warn!(%error, "could not accept Album release result");
                break;
            }
        }
    }
    info!(%source_key, requested, found, missing, errors, "completed Album release lookup");
}

fn lookup_allowed(settings: &SettingsFile, cancelled: &AtomicBool) -> bool {
    !cancelled.load(Ordering::Acquire) && settings.load().ui.allows_external_metadata_lookup()
}
