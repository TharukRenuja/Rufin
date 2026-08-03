//! One bounded read from a concrete source into an invisible Library candidate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use library::{CandidateFinish, CandidateHeader, Libraries, Library, PreparedSourceCandidate};
use sources::{Source, SourceInputIdentity, SourceReadProgress};

pub(super) async fn read_source(
    library: Libraries,
    identity: SourceInputIdentity,
    source: Arc<Source>,
    current: Option<Arc<Library>>,
    progress: Arc<dyn Fn(SourceReadProgress) + Send + Sync>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreparedSourceCandidate, String> {
    let header = CandidateHeader {
        source_id: identity.source_id,
        input_version: identity.version,
        input_digest: identity.digest,
    };
    let (batches, receiver) = async_channel::bounded(1);
    let candidate_library = library.clone();
    let writer = tokio::task::spawn_blocking(move || {
        let mut candidate = candidate_library
            .begin_source_candidate(header)
            .map_err(string_error)?;
        while let Ok(batch) = receiver.recv_blocking() {
            candidate.write(batch).map_err(string_error)?;
        }
        Ok::<_, String>(candidate)
    });

    let facts = Arc::clone(&source)
        .read_source_facts(batches, Arc::clone(&progress), Arc::clone(&cancelled))
        .await;
    let candidate = writer.await.map_err(string_error)??;
    let facts = facts.map_err(string_error)?;
    if cancelled.load(Ordering::Acquire) {
        return Err("source reading was cancelled".to_string());
    }

    tokio::task::spawn_blocking(move || {
        candidate
            .finish(
                CandidateFinish {
                    freshness: facts.freshness().cloned(),
                    home: facts.home().clone(),
                    accepted_at: unix_seconds(),
                },
                current.as_ref(),
            )
            .map_err(string_error)
    })
    .await
    .map_err(string_error)?
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

fn string_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}
