use std::sync::Arc;

use library::{SourceId, Track};

use crate::sequence::{OccurrenceId, RepeatMode, Sequence, SequenceEntry};
use crate::{PlaybackSession, Provenance, RunId, SourceSessionEpoch, TransportStatus};

pub const MAX_QUEUE_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuePageQuery {
    kind: QueuePageQueryKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum QueuePageQueryKind {
    Current,
    Search { text: String },
}

impl QueuePageQuery {
    pub fn current() -> Self {
        Self {
            kind: QueuePageQueryKind::Current,
        }
    }

    pub fn search(text: &str) -> Self {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return Self::current();
        }
        Self {
            kind: QueuePageQueryKind::Search { text },
        }
    }

    pub fn follows_current(&self) -> bool {
        matches!(self.kind, QueuePageQueryKind::Current)
    }

    pub fn is_filtered(&self) -> bool {
        matches!(self.kind, QueuePageQueryKind::Search { .. })
    }

    pub fn search_text(&self) -> Option<&str> {
        match &self.kind {
            QueuePageQueryKind::Search { text } => Some(text),
            QueuePageQueryKind::Current => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueSummaryView {
    pub revision: u64,
    pub total: usize,
    pub current_occurrence: Option<OccurrenceId>,
    pub current_index: Option<usize>,
    pub next_occurrence: Option<OccurrenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuePageRow {
    pub absolute_index: usize,
    pub entry: SequenceEntry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuePage {
    pub revision: u64,
    pub query: QueuePageQuery,
    pub total: usize,
    pub current_absolute_index: Option<usize>,
    pub rows: Vec<QueuePageRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMedia {
    pub id: CurrentMediaId,
    pub track: Track,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CurrentMediaId {
    pub source_id: SourceId,
    pub source_session_epoch: SourceSessionEpoch,
    pub run: Option<RunId>,
    pub occurrence: OccurrenceId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransportView {
    pub source_id: SourceId,
    pub current: Option<Arc<CurrentMedia>>,
    pub state: TransportStatus,
    pub desired_playing: bool,
    pub position_millis: u64,
    pub duration_millis: u64,
    pub can_seek: bool,
    pub buffering_percent: Option<u8>,
    pub error: Option<String>,
}

impl TransportView {
    pub fn effective_state(&self) -> TransportStatus {
        effective_transport_state(self.state, self.desired_playing)
    }
}

fn effective_transport_state(state: TransportStatus, desired_playing: bool) -> TransportStatus {
    match state {
        TransportStatus::Stopped | TransportStatus::Failed => state,
        _ if !desired_playing => TransportStatus::Paused,
        TransportStatus::Paused => TransportStatus::Buffering,
        state => state,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlsView {
    pub repeat_mode: RepeatMode,
    pub shuffle_enabled: bool,
    pub auto_dj_enabled: bool,
    pub volume: f64,
    pub muted: bool,
    pub audio_output: Option<String>,
    pub playback_output: PlaybackOutput,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RemoteOutputProtocol {
    Upnp,
    GoogleCast,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RemoteOutput {
    pub id: String,
    pub name: String,
    pub protocol: RemoteOutputProtocol,
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum PlaybackOutput {
    #[default]
    Local,
    Remote(RemoteOutput),
}

impl PlaybackOutput {
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackView {
    pub queue: QueueSummaryView,
    pub transport: TransportView,
    pub controls: ControlsView,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackNotice {
    RunStarted(RunId),
    PositionDiscontinuity(crate::PositionDiscontinuity),
    Visualizer { run: RunId, levels: Vec<f64> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackProjection {
    pub view: PlaybackView,
    pub queue_page: Option<QueuePage>,
    pub notices: Vec<PlaybackNotice>,
}

impl Sequence {
    pub fn summary(&self) -> QueueSummaryView {
        let next_index = self.next_index_eos();
        QueueSummaryView {
            revision: self.revision(),
            total: self.entries().len(),
            current_occurrence: self.selected().map(|entry| entry.occurrence.clone()),
            current_index: self.selected_index(),
            next_occurrence: next_index
                .and_then(|index| self.entries().get(index))
                .map(|entry| entry.occurrence.clone()),
        }
    }

    pub fn page(&self, query: QueuePageQuery) -> QueuePage {
        let total = self.entries().len();
        let start = match query.kind {
            QueuePageQueryKind::Current => self
                .selected_index()
                .map(|index| index.saturating_sub(20))
                .unwrap_or_default(),
            QueuePageQueryKind::Search { .. } => 0,
        }
        .min(total);
        let rows = match &query.kind {
            QueuePageQueryKind::Current => self.entries()[start..]
                .iter()
                .take(MAX_QUEUE_PAGE_SIZE)
                .enumerate()
                .map(|(offset, entry)| QueuePageRow {
                    absolute_index: start + offset,
                    entry: entry.clone(),
                })
                .collect(),
            QueuePageQueryKind::Search { text } => self
                .entries()
                .iter()
                .enumerate()
                .filter(|(_, entry)| queue_entry_matches_search(entry, text))
                .take(MAX_QUEUE_PAGE_SIZE)
                .map(|(absolute_index, entry)| QueuePageRow {
                    absolute_index,
                    entry: entry.clone(),
                })
                .collect(),
        };
        QueuePage {
            revision: self.revision(),
            query,
            total,
            current_absolute_index: self.selected_index(),
            rows,
        }
    }

    pub fn current_page(&self) -> QueuePage {
        self.page(QueuePageQuery::current())
    }
}

fn queue_entry_matches_search(entry: &SequenceEntry, text: &str) -> bool {
    entry.track.title.to_lowercase().contains(text)
        || entry.track.artist.to_lowercase().contains(text)
        || entry.track.album.to_lowercase().contains(text)
        || (entry.track.year != 0 && entry.track.year.to_string().contains(text))
}

impl PlaybackSession {
    pub fn view(&self) -> PlaybackView {
        let sequence = self.sequence();
        let settings = self.settings();
        PlaybackView {
            queue: sequence.summary(),
            transport: TransportView {
                source_id: sequence.source_id().clone(),
                current: sequence.selected().map(|entry| {
                    Arc::new(CurrentMedia {
                        id: CurrentMediaId {
                            source_id: sequence.source_id().clone(),
                            source_session_epoch: self.source_session_epoch(),
                            run: self.current_run(),
                            occurrence: entry.occurrence.clone(),
                        },
                        track: entry.track.clone(),
                        provenance: entry.provenance.clone(),
                    })
                }),
                state: self.status(),
                desired_playing: self.desired_playing(),
                position_millis: self.position_millis(),
                duration_millis: self.duration_millis(),
                can_seek: self.can_seek(),
                buffering_percent: self.buffering_percent(),
                error: self.last_error().map(str::to_string),
            },
            controls: ControlsView {
                repeat_mode: sequence.repeat_mode(),
                shuffle_enabled: sequence.shuffle_enabled(),
                auto_dj_enabled: self.auto_dj_enabled(),
                volume: if self.output_muted() {
                    0.0
                } else {
                    self.output_volume()
                },
                muted: self.output_muted(),
                audio_output: settings.audio_output.clone(),
                playback_output: self.playback_output().clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use library::{AlbumId, SourceId, Track, TrackId};

    use super::*;
    use crate::sequence::{Batch, BatchItem, Placement, Provenance};

    #[test]
    fn effective_transport_state_follows_play_pause_intent_before_backend_confirmation() {
        assert_eq!(
            effective_transport_state(TransportStatus::Playing, false),
            TransportStatus::Paused
        );
        assert_eq!(
            effective_transport_state(TransportStatus::Paused, true),
            TransportStatus::Buffering
        );
        assert_eq!(
            effective_transport_state(TransportStatus::Stopped, true),
            TransportStatus::Stopped
        );
        assert_eq!(
            effective_transport_state(TransportStatus::Failed, true),
            TransportStatus::Failed
        );
    }

    #[test]
    fn queue_pages_bound_projection_work_without_truncating_the_sequence() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(
                Batch::new(
                    (0..219)
                        .map(|number| BatchItem::new(track(number), Provenance::Manual))
                        .collect(),
                ),
                Placement::Replace { anchor_index: 150 },
            )
            .expect("replace sequence");

        let page = sequence.current_page();
        assert_eq!(sequence.entries().len(), 219);
        assert_eq!(page.rows.len(), 89);
        assert_eq!(page.current_absolute_index, Some(150));
        assert_eq!(page.rows.first().map(|row| row.absolute_index), Some(130));
        assert_eq!(page.rows.last().map(|row| row.absolute_index), Some(218));

        let summary = sequence.summary();
        assert_eq!(summary.total, 219);
        assert_eq!(summary.current_index, Some(150));
        assert!(summary.next_occurrence.is_some());
    }

    #[test]
    fn queue_search_scans_the_full_sequence_but_bounds_its_projection() {
        let mut sequence = Sequence::new(SourceId::fake(1));
        sequence
            .apply_batch(
                Batch::new(
                    (0..300)
                        .map(|number| {
                            let mut track = track(number);
                            if number >= 150 {
                                track.title = format!("Needle {number}");
                            }
                            BatchItem::new(track, Provenance::Manual)
                        })
                        .collect(),
                ),
                Placement::Replace { anchor_index: 0 },
            )
            .expect("replace sequence");

        let page = sequence.page(QueuePageQuery::search("  nEeDlE "));

        assert_eq!(page.total, 300);
        assert_eq!(page.rows.len(), MAX_QUEUE_PAGE_SIZE);
        assert_eq!(page.rows.first().map(|row| row.absolute_index), Some(150));
        assert_eq!(page.rows.last().map(|row| row.absolute_index), Some(249));
    }

    fn track(number: u32) -> Track {
        Track::new(library::TrackData {
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
            relations: library::TrackRelations::default(),
        })
    }
}
