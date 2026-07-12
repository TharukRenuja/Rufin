use std::collections::HashSet;

use library::{AlbumId, ArtistId, ImageRef, SourceId, Track, TrackId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sequence::{
    OccurrenceId, Provenance, RepeatMode, RestoredSequence, Sequence, SequenceEntry, SequenceError,
};

pub const CHECKPOINT_FORMAT_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointHeader {
    pub source_id: SourceId,
    pub revision: u64,
    pub selected_occurrence: Option<OccurrenceId>,
    pub progress_millis: u64,
    pub repeat_mode: RepeatMode,
    pub shuffle_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointRecord {
    pub header: CheckpointHeader,
    pub payload: String,
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("playback checkpoint JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("playback checkpoint format version {0} is unsupported")]
    UnsupportedVersion(u16),
    #[error("playback checkpoint sequence is invalid: {0}")]
    Sequence(#[from] SequenceError),
    #[error("legacy queue occurrence {0} has no album identity")]
    MissingLegacyAlbum(OccurrenceId),
    #[error("legacy queue track lookup failed: {0}")]
    LegacyTrackLookup(String),
}

#[derive(Deserialize, Serialize)]
struct PayloadV1 {
    version: u16,
    entries: Vec<SequenceEntry>,
    traversal: Vec<OccurrenceId>,
}

pub fn encode_checkpoint(sequence: &Sequence) -> Result<CheckpointRecord, CheckpointError> {
    let header = CheckpointHeader {
        source_id: sequence.source_id().clone(),
        revision: sequence.revision(),
        selected_occurrence: sequence.selected().map(|entry| entry.occurrence.clone()),
        progress_millis: sequence.progress_millis(),
        repeat_mode: sequence.repeat_mode(),
        shuffle_enabled: sequence.shuffle_enabled(),
    };
    let traversal = if sequence.shuffle_enabled() {
        sequence.traversal().into_iter().cloned().collect()
    } else {
        Vec::new()
    };
    let payload = serde_json::to_string(&PayloadV1 {
        version: CHECKPOINT_FORMAT_VERSION,
        entries: sequence.entries().to_vec(),
        traversal,
    })?;
    Ok(CheckpointRecord { header, payload })
}

pub fn decode_checkpoint(record: &CheckpointRecord) -> Result<Sequence, CheckpointError> {
    let payload: PayloadV1 = serde_json::from_str(&record.payload)?;
    if payload.version != CHECKPOINT_FORMAT_VERSION {
        return Err(CheckpointError::UnsupportedVersion(payload.version));
    }
    Sequence::restore(RestoredSequence {
        source_id: record.header.source_id.clone(),
        entries: payload.entries,
        selected: record.header.selected_occurrence.clone(),
        repeat_mode: record.header.repeat_mode,
        shuffle_enabled: record.header.shuffle_enabled,
        traversal: payload.traversal,
        revision: record.header.revision,
        progress_millis: record.header.progress_millis,
    })
    .map_err(Into::into)
}

pub fn decode_legacy_queue_snapshot(value: &str) -> Result<Sequence, CheckpointError> {
    decode_legacy_queue_snapshot_with_tracks(value, |_| Ok(None))
}

pub fn decode_legacy_queue_snapshot_with_tracks(
    value: &str,
    mut resolve_track: impl FnMut(&TrackId) -> Result<Option<Track>, String>,
) -> Result<Sequence, CheckpointError> {
    let snapshot: LegacyQueueSnapshot = serde_json::from_str(value)?;
    let selected = snapshot
        .current_index
        .and_then(|index| snapshot.entries.get(index))
        .map(|entry| entry.id.clone());
    let canonical = snapshot
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    let traversal = if snapshot.shuffle.enabled {
        legacy_traversal(&snapshot.shuffle_order, &canonical)?
    } else {
        canonical
    };
    let entries = snapshot
        .entries
        .into_iter()
        .map(|entry| {
            let resolved = if entry.album_id.is_none() {
                resolve_track(&entry.track_id).map_err(CheckpointError::LegacyTrackLookup)?
            } else {
                None
            };
            entry.into_sequence_entry(resolved)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Sequence::restore(RestoredSequence {
        source_id: snapshot.source_id,
        entries,
        selected,
        repeat_mode: snapshot.repeat_mode,
        shuffle_enabled: snapshot.shuffle.enabled,
        traversal,
        revision: 1,
        progress_millis: u64::from(snapshot.progress_seconds) * 1_000,
    })
    .map_err(Into::into)
}

fn legacy_traversal(
    order: &[usize],
    canonical: &[OccurrenceId],
) -> Result<Vec<OccurrenceId>, SequenceError> {
    if order.len() != canonical.len() {
        return Err(SequenceError::InvalidTraversal);
    }
    let mut seen = HashSet::with_capacity(order.len());
    order
        .iter()
        .map(|index| {
            if !seen.insert(*index) {
                return Err(SequenceError::InvalidTraversal);
            }
            canonical
                .get(*index)
                .cloned()
                .ok_or(SequenceError::InvalidTraversal)
        })
        .collect()
}

#[derive(Deserialize)]
struct LegacyQueueSnapshot {
    #[serde(alias = "server_id")]
    source_id: SourceId,
    entries: Vec<LegacyQueueEntry>,
    current_index: Option<usize>,
    repeat_mode: RepeatMode,
    shuffle: LegacyShuffleState,
    #[serde(default)]
    shuffle_order: Vec<usize>,
    #[serde(default)]
    progress_seconds: u32,
}

#[derive(Deserialize)]
struct LegacyShuffleState {
    enabled: bool,
}

#[derive(Deserialize)]
struct LegacyQueueEntry {
    id: OccurrenceId,
    track_id: TrackId,
    album_id: Option<AlbumId>,
    title: String,
    artist: String,
    artist_id: Option<ArtistId>,
    album: String,
    #[serde(default)]
    year: u16,
    duration_seconds: u32,
    #[serde(default)]
    favorite: bool,
    image_ref: Option<ImageRef>,
    local_path: Option<String>,
    source_format: Option<String>,
    origin: Option<LegacyOrigin>,
}

#[derive(Deserialize)]
enum LegacyOrigin {
    Source { shuffle_key: String },
    Manual {},
    Random {},
    AutoDj {},
    RestoredUnknown {},
}

impl LegacyQueueEntry {
    fn into_sequence_entry(
        self,
        resolved: Option<Track>,
    ) -> Result<SequenceEntry, CheckpointError> {
        let LegacyQueueEntry {
            id,
            track_id,
            album_id,
            title,
            artist,
            artist_id,
            album,
            year,
            duration_seconds,
            favorite,
            image_ref,
            local_path,
            source_format,
            origin,
        } = self;
        let provenance = match origin {
            Some(LegacyOrigin::Source { shuffle_key }) => {
                source_context(&shuffle_key).unwrap_or(Provenance::Legacy)
            }
            Some(LegacyOrigin::Manual { .. }) => Provenance::Manual,
            Some(LegacyOrigin::Random { .. }) => Provenance::Random,
            Some(LegacyOrigin::AutoDj { .. }) => Provenance::AutoDj,
            Some(LegacyOrigin::RestoredUnknown { .. }) | None => Provenance::Legacy,
        };
        let track = if let Some(track) = resolved {
            track
        } else {
            let album_id =
                album_id.ok_or_else(|| CheckpointError::MissingLegacyAlbum(id.clone()))?;
            Track {
                id: track_id,
                album_id,
                title,
                artist,
                artist_id,
                artist_credits: Vec::new(),
                album_artist_credits: Vec::new(),
                album,
                year,
                release_date: None,
                date_added: None,
                last_played: None,
                play_count: None,
                user_rating: None,
                duration_seconds,
                favorite,
                disc_number: 0,
                track_number: 0,
                image_ref,
                album_artwork: None,
                genres: Vec::new(),
                musicbrainz_recording_id: None,
                musicbrainz_release_track_id: None,
                local_path,
                source_format,
                comment: None,
                skip_count: None,
                bpm: None,
                moods: Vec::new(),
            }
        };
        Ok(SequenceEntry {
            occurrence: id,
            track,
            provenance,
        })
    }
}

fn source_context(shuffle_key: &str) -> Option<Provenance> {
    let value = shuffle_key.strip_prefix("source-shuffle|source=")?;
    let (before_track, _) = value.rsplit_once("|track=")?;
    let (context_id, source_rank) = before_track.rsplit_once("|source-index=")?;
    Some(Provenance::Context {
        context_id: context_id.to_string(),
        source_rank: source_rank.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, SourceId, Track, TrackId};

    use super::*;
    use crate::sequence::{Batch, BatchItem, Placement};

    #[test]
    fn v1_round_trip_preserves_structure_and_scalar_state() {
        let mut sequence = Sequence::new(SourceId::fake(4));
        sequence
            .apply_batch(
                Batch::new(vec![item(1), item(2)]),
                Placement::Replace { anchor_index: 1 },
            )
            .expect("replace sequence");
        assert!(sequence.set_shuffle_seed(true, 7));
        sequence.set_repeat_mode(RepeatMode::All);
        sequence.set_progress_millis(91_250);

        let restored =
            decode_checkpoint(&encode_checkpoint(&sequence).expect("encode")).expect("decode");

        assert_eq!(restored.source_id(), sequence.source_id());
        assert_eq!(restored.entries(), sequence.entries());
        assert_eq!(restored.selected(), sequence.selected());
        assert_eq!(restored.repeat_mode(), RepeatMode::All);
        assert_eq!(restored.progress_millis(), 91_250);
        assert_eq!(restored.traversal(), sequence.traversal());
    }

    #[test]
    fn legacy_queue_json_decodes_alias_origins_shuffle_and_seconds() {
        let value = serde_json::json!({
            "server_id": "source-7",
            "entries": [
                legacy_entry(
                    "queue-1",
                    "track-1",
                    Some(serde_json::json!({
                        "Source": {
                            "shuffle_key": "source-shuffle|source={\"descriptor\":\"album\"}|source-index=8|track=track-1"
                        }
                    })),
                ),
                legacy_entry("queue-2", "track-2", None),
            ],
            "current_index": 1,
            "repeat_mode": "All",
            "shuffle": { "enabled": true, "seed": 83 },
            "shuffle_order": [1, 0],
            "progress_seconds": 73,
        });

        let restored = decode_legacy_queue_snapshot(&value.to_string()).expect("legacy decode");

        assert_eq!(restored.source_id(), &SourceId::fake(7));
        assert_eq!(
            restored.selected().map(|entry| entry.occurrence.as_str()),
            Some("queue-2")
        );
        assert_eq!(restored.progress_millis(), 73_000);
        assert_eq!(
            restored
                .traversal()
                .into_iter()
                .map(OccurrenceId::as_str)
                .collect::<Vec<_>>(),
            vec!["queue-2", "queue-1"]
        );
        assert_eq!(
            restored.entries()[0].provenance,
            Provenance::Context {
                context_id: "{\"descriptor\":\"album\"}".to_string(),
                source_rank: 8,
            }
        );
        assert_eq!(restored.entries()[1].provenance, Provenance::Legacy);
    }

    fn item(number: u32) -> BatchItem {
        BatchItem::new(track(number), Provenance::Manual)
    }

    fn track(number: u32) -> Track {
        Track {
            id: TrackId::fake(number),
            album_id: AlbumId::fake(1),
            title: format!("Track {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: number as u16,
            image_ref: None,
            album_artwork: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }

    fn legacy_entry(
        id: &str,
        track_id: &str,
        origin: Option<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "track_id": track_id,
            "album_id": "album-1",
            "title": id,
            "artist": "Artist",
            "artist_id": null,
            "album": "Album",
            "year": 2025,
            "duration_seconds": 180,
            "favorite": false,
            "image_ref": null,
            "local_path": null,
            "source_format": null,
            "origin": origin,
        })
    }
}
