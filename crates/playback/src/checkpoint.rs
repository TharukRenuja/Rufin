use library::{
    ArtistCredit, LoadedLibrary, PlaybackCheckpoint, PlaybackFallbackTrack, PlaybackOccurrence,
    PlaybackOccurrenceId, PlaybackProvenance, PlaybackQueueSnapshot, PlaybackState, Track,
    TrackData, TrackId, TrackRelations,
};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::{
    OccurrenceId, Provenance, RepeatMode, RestoredSequence, Sequence, SequenceEntry, SequenceError,
};

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("playback checkpoint sequence is invalid: {0}")]
    Sequence(#[from] SequenceError),
    #[error("playback checkpoint has no fallback for Track {0}")]
    MissingFallback(TrackId),
    #[error("playback checkpoint could not read the loaded library: {0}")]
    Loaded(String),
}

/// Captures one compact durable queue value.
///
/// Every occurrence stores identity and provenance. One fallback value per
/// distinct Track keeps restoration playable while canonical source facts are
/// rebuilding.
pub fn build_checkpoint(sequence: &Sequence) -> PlaybackCheckpoint {
    let mut seen = HashSet::new();
    let fallback_tracks = sequence
        .entries()
        .iter()
        .filter(|entry| seen.insert(entry.track.id.clone()))
        .map(|entry| fallback_track(&entry.track))
        .collect();
    let occurrences = sequence
        .entries()
        .iter()
        .map(|entry| PlaybackOccurrence {
            id: PlaybackOccurrenceId::new(entry.occurrence.as_str()),
            track_id: entry.track.id.clone(),
            provenance: playback_provenance(&entry.provenance),
        })
        .collect();
    let traversal = if sequence.shuffle_enabled() {
        sequence
            .traversal()
            .into_iter()
            .map(|occurrence| PlaybackOccurrenceId::new(occurrence.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    PlaybackCheckpoint {
        source_id: sequence.source_id().clone(),
        revision: sequence.revision(),
        queue: PlaybackQueueSnapshot {
            occurrences,
            fallback_tracks,
            traversal,
        },
        state: PlaybackState {
            selected: sequence
                .selected()
                .map(|entry| PlaybackOccurrenceId::new(entry.occurrence.as_str())),
            progress_millis: sequence.progress_millis(),
        },
    }
}

pub fn restore_checkpoint(
    checkpoint: &PlaybackCheckpoint,
    loaded: Option<&LoadedLibrary>,
    repeat_mode: RepeatMode,
    shuffle_enabled: bool,
    shuffle_seed: u64,
) -> Result<Sequence, CheckpointError> {
    let mut fallbacks = checkpoint
        .queue
        .fallback_tracks
        .iter()
        .map(|fallback| (fallback.id.clone(), track_from_fallback(fallback)))
        .collect::<HashMap<_, _>>();
    if let Some(loaded) = loaded.filter(|loaded| loaded.source_id() == &checkpoint.source_id) {
        for track_id in checkpoint
            .queue
            .occurrences
            .iter()
            .map(|occurrence| &occurrence.track_id)
            .collect::<HashSet<_>>()
        {
            if let Some(track) = loaded
                .track(track_id)
                .map_err(|error| CheckpointError::Loaded(error.to_string()))?
            {
                fallbacks.insert(track_id.clone(), track);
            }
        }
    }
    let entries = checkpoint
        .queue
        .occurrences
        .iter()
        .map(|occurrence| {
            let track = fallbacks
                .get(&occurrence.track_id)
                .cloned()
                .ok_or_else(|| CheckpointError::MissingFallback(occurrence.track_id.clone()))?;
            Ok(SequenceEntry {
                occurrence: OccurrenceId::new(occurrence.id.as_str()),
                track,
                provenance: sequence_provenance(&occurrence.provenance),
            })
        })
        .collect::<Result<Vec<_>, CheckpointError>>()?;
    let stored_traversal = checkpoint
        .queue
        .traversal
        .iter()
        .map(|occurrence| OccurrenceId::new(occurrence.as_str()))
        .collect::<Vec<_>>();
    let restore_stored_traversal = shuffle_enabled && !stored_traversal.is_empty();
    let mut sequence = Sequence::restore(RestoredSequence {
        source_id: checkpoint.source_id.clone(),
        entries,
        selected: checkpoint
            .state
            .selected
            .as_ref()
            .map(|occurrence| OccurrenceId::new(occurrence.as_str())),
        repeat_mode,
        shuffle_enabled: restore_stored_traversal,
        traversal: stored_traversal,
        revision: checkpoint.revision,
        progress_millis: checkpoint.state.progress_millis,
    })?;
    if shuffle_enabled && !restore_stored_traversal {
        let revision = sequence.revision;
        sequence.set_shuffle_seed(true, shuffle_seed);
        sequence.revision = revision;
    }
    Ok(sequence)
}

fn fallback_track(track: &Track) -> PlaybackFallbackTrack {
    PlaybackFallbackTrack {
        id: track.id.clone(),
        album_id: track.album_id.clone(),
        primary_artist_id: track.primary_artist_id().cloned(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        year: track.year,
        duration_seconds: track.duration_seconds,
        favorite: track.favorite,
        track_number: track.track_number,
        disc_number: track.disc_number,
        image_ref: track.image_ref.clone(),
        local_artwork: track.local_artwork.clone(),
        musicbrainz_recording_id: track.musicbrainz_recording_id.clone(),
        source_format: track.source_format.clone(),
        source_path: track.source_path.clone(),
        cue: track.cue.clone(),
    }
}

fn track_from_fallback(fallback: &PlaybackFallbackTrack) -> Track {
    let artists = fallback
        .primary_artist_id
        .clone()
        .map(|id| {
            vec![ArtistCredit {
                id,
                name: fallback.artist.clone(),
                musicbrainz_artist_id: None,
            }]
        })
        .unwrap_or_default();
    Track::new(TrackData {
        id: fallback.id.clone(),
        album_id: fallback.album_id.clone(),
        title: fallback.title.clone(),
        artist: fallback.artist.clone(),
        album: fallback.album.clone(),
        album_artwork: None,
        year: fallback.year,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: fallback.duration_seconds,
        favorite: fallback.favorite,
        disc_number: fallback.disc_number,
        track_number: fallback.track_number,
        image_ref: fallback.image_ref.clone(),
        local_artwork: fallback.local_artwork.clone(),
        musicbrainz_recording_id: fallback.musicbrainz_recording_id.clone(),
        musicbrainz_release_track_id: None,
        source_path: fallback.source_path.clone(),
        cue: fallback.cue.clone(),
        source_format: fallback.source_format.clone(),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations {
            artists,
            ..TrackRelations::default()
        },
    })
}

fn playback_provenance(provenance: &Provenance) -> PlaybackProvenance {
    match provenance {
        Provenance::Context {
            context_id,
            source_rank,
        } => PlaybackProvenance::Context {
            context_id: context_id.clone(),
            source_rank: *source_rank,
        },
        Provenance::Manual => PlaybackProvenance::Manual,
        Provenance::Random => PlaybackProvenance::Random,
        Provenance::Radio => PlaybackProvenance::Radio,
        Provenance::AutoDj => PlaybackProvenance::AutoDj,
        Provenance::Legacy => PlaybackProvenance::Legacy,
    }
}

fn sequence_provenance(provenance: &PlaybackProvenance) -> Provenance {
    match provenance {
        PlaybackProvenance::Context {
            context_id,
            source_rank,
        } => Provenance::Context {
            context_id: context_id.clone(),
            source_rank: *source_rank,
        },
        PlaybackProvenance::Manual => Provenance::Manual,
        PlaybackProvenance::Random => Provenance::Random,
        PlaybackProvenance::Radio => Provenance::Radio,
        PlaybackProvenance::AutoDj => Provenance::AutoDj,
        PlaybackProvenance::Legacy => Provenance::Legacy,
    }
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, CueSegment, SourceId, TrackId};

    use super::*;
    use crate::{Batch, BatchItem, Placement};

    #[test]
    fn compact_checkpoint_round_trip_preserves_duplicate_handles_and_cue_playback() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        let track = Track::new(TrackData {
            id: TrackId::fake(1),
            album_id: Some(AlbumId::fake(1)),
            title: "Cue Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_artwork: None,
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 2,
            track_number: 7,
            image_ref: None,
            local_artwork: None,
            musicbrainz_recording_id: Some("recording".to_string()),
            musicbrainz_release_track_id: None,
            source_path: Some("/music/disc.flac".to_string()),
            cue: Some(CueSegment {
                cue_path: "/music/disc.cue".to_string(),
                start_millis: 10_000,
                end_millis: 20_000,
            }),
            source_format: Some("FLAC".to_string()),
            comment: None,
            skip_count: None,
            bpm: None,
            relations: TrackRelations::default(),
        });
        sequence
            .apply_batch(
                Batch::new(vec![
                    BatchItem::new(track.clone(), Provenance::Manual),
                    BatchItem::new(track.clone(), Provenance::Radio),
                ]),
                Placement::Replace { anchor_index: 1 },
            )
            .expect("apply duplicate Track");
        sequence.set_progress_millis(12_345);
        assert!(sequence.set_shuffle_seed(true, 17));
        let checkpoint = build_checkpoint(&sequence);
        let stored_traversal = checkpoint.queue.traversal.clone();

        assert_eq!(checkpoint.queue.occurrences.len(), 2);
        assert_eq!(checkpoint.queue.fallback_tracks.len(), 1);
        let restored = restore_checkpoint(&checkpoint, None, RepeatMode::All, true, 999)
            .expect("restore checkpoint");
        assert_eq!(restored.selected_index(), Some(1));
        assert_eq!(restored.repeat_mode(), RepeatMode::All);
        assert!(restored.shuffle_enabled());
        assert_eq!(restored.progress_millis(), 12_345);
        assert_eq!(
            restored
                .traversal()
                .into_iter()
                .map(|occurrence| PlaybackOccurrenceId::new(occurrence.as_str()))
                .collect::<Vec<_>>(),
            stored_traversal
        );
        assert!(Track::ptr_eq(
            &restored.entries()[0].track,
            &restored.entries()[1].track
        ));
        assert_eq!(
            restored.entries()[0]
                .track
                .cue
                .as_ref()
                .map(|cue| (cue.start_millis, cue.end_millis)),
            Some((10_000, 20_000))
        );
        assert_eq!(
            restored.entries()[0]
                .track
                .musicbrainz_recording_id
                .as_deref(),
            Some("recording")
        );
    }

    #[test]
    fn app_preferences_control_restore_when_no_traversal_was_stored() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(
                Batch::new(vec![item(1), item(2), item(3), item(4)]),
                Placement::Replace { anchor_index: 2 },
            )
            .expect("build queue");
        let checkpoint = build_checkpoint(&sequence);
        assert!(checkpoint.queue.traversal.is_empty());

        let shuffled = restore_checkpoint(&checkpoint, None, RepeatMode::One, true, 81)
            .expect("restore shuffled");
        assert_eq!(shuffled.repeat_mode(), RepeatMode::One);
        assert!(shuffled.shuffle_enabled());
        assert_eq!(
            shuffled
                .traversal()
                .first()
                .map(|occurrence| occurrence.as_str()),
            shuffled.selected().map(|entry| entry.occurrence.as_str())
        );
        assert_eq!(shuffled.revision(), checkpoint.revision);

        let unshuffled = restore_checkpoint(&checkpoint, None, RepeatMode::All, false, 81)
            .expect("restore unshuffled");
        assert_eq!(unshuffled.repeat_mode(), RepeatMode::All);
        assert!(!unshuffled.shuffle_enabled());
        assert_eq!(
            unshuffled
                .traversal()
                .into_iter()
                .map(OccurrenceId::as_str)
                .collect::<Vec<_>>(),
            checkpoint
                .queue
                .occurrences
                .iter()
                .map(|occurrence| occurrence.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    fn item(number: u32) -> BatchItem {
        BatchItem::new(
            Track::new(TrackData {
                id: TrackId::fake(number),
                album_id: Some(AlbumId::fake(1)),
                title: format!("Track {number}"),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                album_artwork: None,
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
                local_artwork: None,
                musicbrainz_recording_id: None,
                musicbrainz_release_track_id: None,
                source_path: None,
                cue: None,
                source_format: None,
                comment: None,
                skip_count: None,
                bpm: None,
                relations: TrackRelations::default(),
            }),
            Provenance::Manual,
        )
    }
}
