use library::{SourceId, Track, TrackId};

use crate::RunId;

/// The immutable submission facts captured for one playback run.
///
/// This is intentionally narrower than a library item. Later library or
/// metadata changes must not rewrite a run that has already started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListeningTrack {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub recording_id: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub duration_millis: u64,
}

impl ListeningTrack {
    pub fn capture(source_id: SourceId, track: &Track) -> Self {
        let artists = if track.artist_credits.is_empty() {
            vec![track.artist.clone()]
        } else {
            track
                .artist_credits
                .iter()
                .map(|credit| credit.name.clone())
                .collect()
        };
        Self {
            source_id,
            track_id: track.id.clone(),
            recording_id: track.musicbrainz_recording_id.clone(),
            title: track.title.clone(),
            artists,
            album: (!track.album.trim().is_empty()).then(|| track.album.clone()),
            track_number: (track.track_number > 0).then_some(track.track_number),
            disc_number: (track.disc_number > 0).then_some(track.disc_number),
            duration_millis: u64::from(track.duration_seconds).saturating_mul(1_000),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunEndReason {
    Completed,
    ManualSkip,
    Stopped,
    Replaced,
    SourceSwitch,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListeningFact {
    Started {
        run: RunId,
        started_at_unix_seconds: i64,
        local_period: String,
        track: ListeningTrack,
    },
    Progress {
        run: RunId,
        audible_millis: u64,
        playhead_millis: u64,
    },
    Ended {
        run: RunId,
        reason: RunEndReason,
        audible_millis: u64,
        playhead_millis: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListeningOutcome {
    pub run: RunId,
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub local_period: String,
    pub qualified_plays: u32,
    pub skips: u32,
    pub last_played_at_unix_seconds: Option<i64>,
}

pub fn qualified_play_threshold_millis(duration_millis: u64) -> u64 {
    let duration_seconds = duration_millis / 1_000;
    let threshold_seconds = if duration_seconds <= 10 {
        duration_seconds
    } else {
        let half = duration_seconds / 2;
        if duration_seconds < 60 {
            half.max(5)
        } else {
            half.clamp(30, 240)
        }
    };
    threshold_seconds.saturating_mul(1_000)
}

pub fn manual_end_is_skip(
    reason: RunEndReason,
    duration_millis: u64,
    audible_millis: u64,
    playhead_millis: u64,
) -> bool {
    if !matches!(reason, RunEndReason::ManualSkip | RunEndReason::Replaced) {
        return false;
    }
    audible_millis < qualified_play_threshold_millis(duration_millis)
        && duration_millis.saturating_sub(playhead_millis) > 5_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_play_threshold_preserves_short_half_and_clamped_law() {
        assert_eq!(qualified_play_threshold_millis(10_000), 10_000);
        assert_eq!(qualified_play_threshold_millis(20_000), 10_000);
        assert_eq!(qualified_play_threshold_millis(180_000), 90_000);
        assert_eq!(qualified_play_threshold_millis(900_000), 240_000);
    }

    #[test]
    fn seeked_playhead_does_not_qualify_a_play_or_hide_an_early_skip() {
        assert!(manual_end_is_skip(
            RunEndReason::ManualSkip,
            180_000,
            4_000,
            170_000,
        ));
        assert!(!manual_end_is_skip(
            RunEndReason::ManualSkip,
            180_000,
            90_000,
            170_000,
        ));
    }

    #[test]
    fn natural_end_and_explicit_stop_are_not_skips() {
        for reason in [RunEndReason::Completed, RunEndReason::Stopped] {
            assert!(!manual_end_is_skip(reason, 180_000, 4_000, 10_000));
        }
    }
}
