//! Rufin-owned accepted listening activity.
//!
//! Playback qualifies a play once and supplies its original time. Library
//! records the promised lifetime/monthly totals and bounded recent history;
//! it does not redraw a mounted Home or create a general event log.

use crate::{
    AcceptedLibraryChange, ArtistId, GenreId, Library, LibraryError, LibraryQueryError,
    LibraryResult, Track, TrackId,
};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedPlay {
    pub play_id: String,
    pub track_id: TrackId,
    pub played_at: i64,
    pub month: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSkip {
    pub track_id: TrackId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityPeriod {
    Lifetime,
    Month(String),
    Year(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityItemId {
    Track(TrackId),
    Artist(ArtistId),
    Genre(GenreId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityItem {
    pub id: ActivityItemId,
    pub name: String,
    pub context: Option<String>,
    pub play_count: u64,
    pub skip_count: Option<u64>,
    pub last_played_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivitySummary {
    pub tracks: Vec<ActivityItem>,
    pub artists: Vec<ActivityItem>,
    pub genres: Vec<ActivityItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentPlay {
    pub play_id: String,
    pub track_id: TrackId,
    pub track_title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub played_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct ActivityCredit {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub context: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActivityWrite {
    pub play_id: Option<String>,
    pub track_id: TrackId,
    pub track_title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub played_at: Option<i64>,
    pub month: Option<String>,
    pub credits: Vec<ActivityCredit>,
    pub skipped: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrackActivity {
    pub(crate) track_id: TrackId,
    pub(crate) play_count: u32,
    pub(crate) skip_count: u32,
    pub(crate) last_played: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedActivity {
    activity: TrackActivity,
    recent_play: Option<RecentPlay>,
}

impl Library {
    pub fn record_play(&self, play: AcceptedPlay) -> LibraryResult<Option<RecordedActivity>> {
        if play.play_id.is_empty() {
            return Err(LibraryError::Persistence(
                "accepted play ID cannot be empty".to_string(),
            ));
        }
        if !valid_month(&play.month) {
            return Err(LibraryError::Persistence(format!(
                "accepted play month is invalid: {}",
                play.month
            )));
        }
        let track = self
            .track(&play.track_id)?
            .ok_or_else(|| missing_track(&play.track_id))?;
        let primary = if track.relations.artists.is_empty() {
            &track.relations.album_artists
        } else {
            &track.relations.artists
        };
        let mut seen_artists = HashSet::new();
        let mut credits = primary
            .iter()
            .filter(|credit| seen_artists.insert(credit.id.clone()))
            .map(|credit| ActivityCredit {
                kind: "artist",
                id: credit.id.to_string(),
                name: credit.name.clone(),
                context: None,
            })
            .collect::<Vec<_>>();
        let mut seen_genres = HashSet::new();
        credits.extend(
            track
                .relations
                .genres
                .iter()
                .filter(|credit| seen_genres.insert(credit.id.clone()))
                .map(|credit| ActivityCredit {
                    kind: "genre",
                    id: credit.id.to_string(),
                    name: credit.name.clone(),
                    context: None,
                }),
        );
        credits.push(ActivityCredit {
            kind: "track",
            id: track.id.to_string(),
            name: track.title.clone(),
            context: Some(track.artist.clone()),
        });
        let replacement = self.store.record_activity(
            self.source_id().clone(),
            ActivityWrite {
                play_id: Some(play.play_id.clone()),
                track_id: track.id.clone(),
                track_title: track.title.clone(),
                artist_name: track.artist.clone(),
                album_title: (!track.album.trim().is_empty()).then(|| track.album.clone()),
                played_at: Some(play.played_at),
                month: Some(play.month),
                credits,
                skipped: false,
            },
        )?;
        let Some(activity) = replacement else {
            return Ok(None);
        };
        Ok(Some(RecordedActivity {
            activity,
            recent_play: Some(RecentPlay {
                play_id: play.play_id,
                track_id: track.id.clone(),
                track_title: track.title.clone(),
                artist_name: track.artist.clone(),
                album_title: (!track.album.trim().is_empty()).then(|| track.album.clone()),
                played_at: play.played_at,
            }),
        }))
    }

    pub fn activity_summary(&self, period: ActivityPeriod) -> LibraryResult<ActivitySummary> {
        if matches!(&period, ActivityPeriod::Month(month) if !valid_month(month))
            || matches!(&period, ActivityPeriod::Year(year) if !(1970..=9999).contains(year))
        {
            return Err(LibraryError::Persistence(
                "activity period is invalid".to_string(),
            ));
        }
        Ok(self
            .store
            .activity_summary(self.source_id().clone(), period)?)
    }

    pub fn record_skip(&self, skip: AcceptedSkip) -> LibraryResult<RecordedActivity> {
        let track = self
            .track(&skip.track_id)?
            .ok_or_else(|| missing_track(&skip.track_id))?;
        let replacement = self.store.record_activity(
            self.source_id().clone(),
            ActivityWrite {
                play_id: None,
                track_id: track.id.clone(),
                track_title: track.title.clone(),
                artist_name: track.artist.clone(),
                album_title: (!track.album.trim().is_empty()).then(|| track.album.clone()),
                played_at: None,
                month: None,
                credits: Vec::new(),
                skipped: true,
            },
        )?;
        let activity = replacement.ok_or_else(|| {
            LibraryError::Persistence("accepted skip did not update track activity".to_string())
        })?;
        Ok(RecordedActivity {
            activity,
            recent_play: None,
        })
    }

    pub fn apply_recorded_activity(
        &self,
        update: &RecordedActivity,
    ) -> LibraryResult<Option<AcceptedLibraryChange>> {
        self.replace_track_activity(update.activity.clone(), update.recent_play.clone())
            .map_err(Into::into)
    }
}

pub(crate) fn apply_track_activity_value(track: &mut Track, activity: &TrackActivity) {
    track.play_count = Some(activity.play_count);
    track.skip_count = Some(activity.skip_count);
    track.last_played.clone_from(&activity.last_played);
}

fn valid_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 7
        || bytes[4] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let year = bytes[..4]
        .iter()
        .fold(0_u16, |year, digit| year * 10 + u16::from(digit - b'0'));
    let month = (bytes[5] - b'0') * 10 + bytes[6] - b'0';
    year >= 1970 && (1..=12).contains(&month)
}

fn missing_track(id: &TrackId) -> LibraryError {
    LibraryQueryError::MissingItem {
        kind: "track",
        id: id.to_string(),
    }
    .into()
}
