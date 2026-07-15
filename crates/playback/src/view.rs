use std::sync::Arc;

use library::SourceId;

use crate::sequence::{OccurrenceId, RepeatMode, Sequence, SequenceEntry};
use crate::{PlaybackSession, RunId, TransportStatus};

pub const MAX_QUEUE_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuePageQuery {
    kind: QueuePageQueryKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum QueuePageQueryKind {
    Current,
    At { start: usize },
    Search { text: String },
}

impl QueuePageQuery {
    pub fn current() -> Self {
        Self {
            kind: QueuePageQueryKind::Current,
        }
    }

    pub fn at(start: usize) -> Self {
        Self {
            kind: QueuePageQueryKind::At { start },
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
            QueuePageQueryKind::Current | QueuePageQueryKind::At { .. } => None,
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
    pub entry: Arc<SequenceEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuePage {
    pub revision: u64,
    pub query: QueuePageQuery,
    pub start: usize,
    pub total: usize,
    pub current_absolute_index: Option<usize>,
    pub rows: Vec<QueuePageRow>,
}

impl QueuePage {
    pub fn previous_query(&self) -> Option<QueuePageQuery> {
        (!self.query.is_filtered() && self.start != 0)
            .then(|| QueuePageQuery::at(self.start.saturating_sub(MAX_QUEUE_PAGE_SIZE)))
    }

    pub fn next_query(&self) -> Option<QueuePageQuery> {
        (!self.query.is_filtered()
            && !self.rows.is_empty()
            && self.start + self.rows.len() < self.total)
            .then(|| QueuePageQuery::at(self.start + self.rows.len()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransportView {
    pub source_id: SourceId,
    pub run: Option<RunId>,
    pub current: Option<Arc<SequenceEntry>>,
    pub state: TransportStatus,
    pub position_millis: u64,
    pub duration_millis: u64,
    pub buffering_percent: Option<u8>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlsView {
    pub repeat_mode: RepeatMode,
    pub shuffle_enabled: bool,
    pub auto_dj_enabled: bool,
    pub volume: f64,
    pub muted: bool,
    pub audio_output: Option<String>,
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
    MediaChanged(crate::MediaChanged),
    PositionDiscontinuity(crate::PositionDiscontinuity),
    Visualizer { run: RunId, levels: Vec<f64> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackProjection {
    pub view: PlaybackView,
    pub queue_page: Option<QueuePage>,
    pub notices: Vec<PlaybackNotice>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveformProjection {
    pub key: Option<String>,
    pub peaks: Option<Arc<Vec<(f64, f64)>>>,
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
            QueuePageQueryKind::At { start } => start,
            QueuePageQueryKind::Search { .. } => 0,
        }
        .min(total);
        let rows = match &query.kind {
            QueuePageQueryKind::Current | QueuePageQueryKind::At { .. } => self.entries()[start..]
                .iter()
                .take(MAX_QUEUE_PAGE_SIZE)
                .enumerate()
                .map(|(offset, entry)| QueuePageRow {
                    absolute_index: start + offset,
                    entry: Arc::new(entry.clone()),
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
                    entry: Arc::new(entry.clone()),
                })
                .collect(),
        };
        QueuePage {
            revision: self.revision(),
            query,
            start,
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
                run: self.current_run(),
                current: sequence.selected().cloned().map(Arc::new),
                state: self.status(),
                position_millis: self.position_millis(),
                duration_millis: self.duration_millis(),
                buffering_percent: self.buffering_percent(),
                error: self.last_error().map(str::to_string),
            },
            controls: ControlsView {
                repeat_mode: sequence.repeat_mode(),
                shuffle_enabled: sequence.shuffle_enabled(),
                auto_dj_enabled: self.auto_dj_enabled(),
                volume: settings.volume,
                muted: settings.muted,
                audio_output: settings.audio_output.clone(),
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
    fn queue_pages_are_bounded_without_truncating_the_sequence() {
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
        assert_eq!(page.start, 130);
        assert_eq!(page.rows.len(), 89);
        assert_eq!(page.current_absolute_index, Some(150));
        assert_eq!(
            sequence.page(QueuePageQuery::at(0)).rows.len(),
            MAX_QUEUE_PAGE_SIZE
        );
        let middle = sequence.page(QueuePageQuery::at(100));
        let following = sequence.page(middle.next_query().expect("following page"));
        assert_eq!(following.start, 200);
        assert_eq!(following.rows.len(), 19);
        assert_eq!(
            following.rows.first().map(|row| row.absolute_index),
            Some(200)
        );
        let preceding = sequence.page(middle.previous_query().expect("preceding page"));
        assert_eq!(preceding.start, 0);
        assert_eq!(preceding.rows.len(), MAX_QUEUE_PAGE_SIZE);

        let summary = sequence.summary();
        assert_eq!(summary.total, 219);
        assert_eq!(summary.current_index, Some(150));
        assert!(summary.next_occurrence.is_some());
    }

    #[test]
    fn queue_search_scans_the_full_sequence_case_insensitively_and_stays_bounded() {
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
        assert!(page.previous_query().is_none());
        assert!(page.next_query().is_none());
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
}
