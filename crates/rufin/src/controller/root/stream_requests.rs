use super::*;

use library::SourceObject;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SLOW_STREAM_RESOLVE_STAGE_MS: u128 = 250;

pub(in crate::controller) fn resolve_stream_request(
    store: &StoreHandle,
    runtime: &Runtime,
    active_source: &ActiveSourceSlot,
    source_id: &SourceId,
    request: &StreamRequest,
) -> Result<StreamDescriptor, String> {
    let active = crate::source_setup::selected_active_source(active_source, source_id)?;
    resolve_stream_for_active(store, runtime, &active, source_id, request)
}

fn resolve_stream_for_active(
    store: &StoreHandle,
    runtime: &Runtime,
    active: &crate::source_setup::ActiveSource,
    source_id: &SourceId,
    request: &StreamRequest,
) -> Result<StreamDescriptor, String> {
    let started = Instant::now();
    let track_id = &request.track_id;
    let PlaybackStreamLookup {
        cue_source,
        local_path,
    } = playback_stream_lookup(store, active, source_id, track_id)?;
    if let Some(source) = cue_source.as_ref()
        && let Some(stream) = cue_track_stream_from_source(source)?
    {
        return Ok(stream);
    }
    if let Some(local_path) = local_path {
        let url = reqwest::Url::from_file_path(&local_path).map_err(|()| {
            format!(
                "Could not turn local track path into a file URI: {}",
                local_path.display()
            )
        })?;
        debug!(
            source_id = %source_id,
            source_kind = %active.identity.kind,
            track_id = %track_id.as_str(),
            path = %local_path.display(),
            "resolved track to local playback file"
        );
        return Ok(StreamDescriptor::new(url.to_string()));
    }
    let stream = runtime
        .block_on(active.streams.resolve_stream(request))
        .map_err(|error| error.to_string())?;
    debug!(
        source_id = %source_id,
        source_kind = %active.identity.kind,
        track_id = %track_id.as_str(),
        elapsed_ms = started.elapsed().as_millis(),
        "resolved source playback descriptor"
    );
    Ok(stream)
}

fn log_slow_stream_stage(stage: &str, elapsed: Duration, source_id: &SourceId, track_id: &TrackId) {
    let elapsed_ms = elapsed.as_millis();
    if elapsed_ms > SLOW_STREAM_RESOLVE_STAGE_MS {
        info!(
            stage,
            elapsed_ms,
            source_id = %source_id,
            track_id = %track_id.as_str(),
            "slow playback stream resolve stage"
        );
    }
}

struct PlaybackStreamLookup {
    cue_source: Option<SourceObject>,
    local_path: Option<PathBuf>,
}

fn playback_stream_lookup(
    store: &StoreHandle,
    active: &crate::source_setup::ActiveSource,
    source_id: &SourceId,
    track_id: &TrackId,
) -> Result<PlaybackStreamLookup, String> {
    let stage_started = Instant::now();
    let lookup = store
        .with_store_fast(|store| {
            let Some(_saved) = store.stored_source(source_id)? else {
                return Ok(None);
            };
            let cue_source = store
                .load_track_source_object(source_id, track_id)?
                .filter(|source| source.source_object_kind == "cue_track");
            let local_path = (active.playback_file)(store, source_id, track_id)?;
            Ok(Some(PlaybackStreamLookup {
                cue_source,
                local_path,
            }))
        })?
        .ok_or_else(|| "No matching saved server is saved.".to_string())?;
    log_slow_stream_stage(
        "cached-playback-source",
        stage_started.elapsed(),
        source_id,
        track_id,
    );
    Ok(lookup)
}

fn cue_track_stream_from_source(source: &SourceObject) -> Result<Option<StreamDescriptor>, String> {
    let Some(path) = source.source_path.as_deref().map(PathBuf::from) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let start_millis = source
        .segment_start_ms
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default();
    let Some(end_millis) = source
        .segment_end_ms
        .and_then(|value| u64::try_from(value).ok())
    else {
        return Ok(None);
    };
    let url = reqwest::Url::from_file_path(&path).map_err(|()| {
        format!(
            "Could not turn cue source path into a file URI: {}",
            path.display()
        )
    })?;
    Ok(Some(
        StreamDescriptor::new(url.to_string()).with_source_window(start_millis, end_millis),
    ))
}
