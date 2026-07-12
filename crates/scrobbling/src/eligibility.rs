use playback::{ListeningFact, ListeningTrack, RunId};

const MIN_SCROBBLE_DURATION_MILLIS: u64 = 30_000;
const MAX_SCROBBLE_THRESHOLD_MILLIS: u64 = 4 * 60 * 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SubmissionTrack {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration_millis: u64,
}

impl SubmissionTrack {
    fn capture(track: &ListeningTrack) -> Option<Self> {
        let title = track.title.trim();
        let artists = track
            .artists
            .iter()
            .map(|artist| artist.trim())
            .filter(|artist| !artist.is_empty())
            .collect::<Vec<_>>();
        if title.is_empty() || artists.is_empty() {
            return None;
        }
        Some(Self {
            title: title.to_string(),
            artist: artists.join(", "),
            album: track
                .album
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            duration_millis: track.duration_millis,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Submission {
    NowPlaying(SubmissionTrack),
    Scrobble {
        track: SubmissionTrack,
        started_at_unix_seconds: i64,
    },
}

impl Submission {
    pub(crate) fn is_now_playing(&self) -> bool {
        matches!(self, Self::NowPlaying(_))
    }
}

#[derive(Clone, Debug)]
struct ActiveRun {
    track: SubmissionTrack,
    started_at_unix_seconds: i64,
    submitted: bool,
}

#[derive(Default)]
pub(crate) struct Eligibility {
    active: Option<(RunId, ActiveRun)>,
}

impl Eligibility {
    pub(crate) fn observe(
        &mut self,
        fact: &ListeningFact,
        dispatch_enabled: bool,
    ) -> Option<Submission> {
        match fact {
            ListeningFact::Started {
                run,
                started_at_unix_seconds,
                track,
                ..
            } => {
                let track = SubmissionTrack::capture(track)?;
                let submission = self.start(*run, *started_at_unix_seconds, track);
                dispatch_enabled.then_some(submission)
            }
            ListeningFact::Progress {
                run,
                audible_millis,
                ..
            } => dispatch_enabled
                .then(|| self.qualify(*run, *audible_millis))
                .flatten(),
            ListeningFact::Ended {
                run,
                audible_millis,
                ..
            } => {
                let submission = dispatch_enabled
                    .then(|| self.qualify(*run, *audible_millis))
                    .flatten();
                if self
                    .active
                    .as_ref()
                    .is_some_and(|(active_run, _)| active_run == run)
                {
                    self.active = None;
                }
                submission
            }
        }
    }

    fn start(
        &mut self,
        run: RunId,
        started_at_unix_seconds: i64,
        track: SubmissionTrack,
    ) -> Submission {
        self.active = Some((
            run,
            ActiveRun {
                track: track.clone(),
                started_at_unix_seconds,
                submitted: false,
            },
        ));
        Submission::NowPlaying(track)
    }

    fn qualify(&mut self, run: RunId, audible_millis: u64) -> Option<Submission> {
        let (_, active) = self
            .active
            .as_mut()
            .filter(|(active_run, _)| *active_run == run)?;
        if active.submitted
            || !reaches_scrobble_threshold(active.track.duration_millis, audible_millis)
        {
            return None;
        }
        active.submitted = true;
        Some(Submission::Scrobble {
            track: active.track.clone(),
            started_at_unix_seconds: active.started_at_unix_seconds,
        })
    }
}

pub fn scrobble_threshold_millis(duration_millis: u64) -> Option<u64> {
    if duration_millis <= MIN_SCROBBLE_DURATION_MILLIS {
        return None;
    }
    Some((duration_millis / 2).min(MAX_SCROBBLE_THRESHOLD_MILLIS))
}

fn reaches_scrobble_threshold(duration_millis: u64, audible_millis: u64) -> bool {
    scrobble_threshold_millis(duration_millis).is_some_and(|threshold| audible_millis >= threshold)
}

#[cfg(test)]
mod tests {
    use playback::RunEndReason;

    use super::*;

    #[test]
    fn threshold_uses_audible_half_with_four_minute_cap() {
        assert_eq!(scrobble_threshold_millis(30_000), None);
        assert_eq!(scrobble_threshold_millis(31_000), Some(15_500));
        assert_eq!(scrobble_threshold_millis(180_000), Some(90_000));
        assert_eq!(scrobble_threshold_millis(900_000), Some(240_000));
    }

    #[test]
    fn each_run_submits_at_most_once_even_for_the_same_track() {
        let mut state = Eligibility::default();
        for run in [RunId::new(1), RunId::new(2)] {
            assert!(matches!(
                state.start(run, 1_700_000_000, track()),
                Submission::NowPlaying(_)
            ));
            assert!(matches!(
                state.observe(&progress(run, 90_000), true),
                Some(Submission::Scrobble { .. })
            ));
            assert_eq!(state.observe(&progress(run, 120_000), true), None);
        }
    }

    #[test]
    fn ended_can_qualify_then_forgets_the_run_without_cancelling_the_submission() {
        let run = RunId::new(3);
        let mut state = Eligibility::default();
        state.start(run, 1_700_000_000, track());
        let submission = state.observe(
            &ListeningFact::Ended {
                run,
                reason: RunEndReason::Completed,
                audible_millis: 90_000,
                playhead_millis: 180_000,
            },
            true,
        );
        assert!(matches!(submission, Some(Submission::Scrobble { .. })));
        assert_eq!(
            state.observe(
                &ListeningFact::Ended {
                    run,
                    reason: RunEndReason::Completed,
                    audible_millis: 180_000,
                    playhead_millis: 180_000,
                },
                true,
            ),
            None
        );
    }

    fn track() -> SubmissionTrack {
        SubmissionTrack {
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_millis: 180_000,
        }
    }

    fn progress(run: RunId, audible_millis: u64) -> ListeningFact {
        ListeningFact::Progress {
            run,
            audible_millis,
            playhead_millis: audible_millis,
        }
    }
}
