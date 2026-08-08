//! Source-scoped loudness facts produced by Rufin's audio analysis.

use crate::{AlbumId, Track, TrackId};
use std::sync::Arc;

pub const LOUDNESS_ANALYSIS_VERSION: u32 = 1;

/// A target-neutral loudness observation.
///
/// `true_peak_ratio` is linear full scale, where `1.0` is 0 dBFS. Integrated
/// loudness is absent for successfully analyzed silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessMeasurement {
    pub integrated_lufs: Option<f64>,
    pub true_peak_ratio: f64,
}

impl LoudnessMeasurement {
    pub fn new(integrated_lufs: Option<f64>, true_peak_ratio: f64) -> Result<Self, String> {
        if integrated_lufs.is_some_and(|value| !value.is_finite()) {
            return Err("integrated loudness must be finite".to_string());
        }
        if !true_peak_ratio.is_finite() || true_peak_ratio < 0.0 {
            return Err("true peak must be a finite non-negative ratio".to_string());
        }
        Ok(Self {
            integrated_lufs,
            true_peak_ratio,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoudnessItemId {
    Track(TrackId),
    Album(AlbumId),
}

impl LoudnessItemId {
    pub(crate) const fn scope(&self) -> &'static str {
        match self {
            Self::Track(_) => "track",
            Self::Album(_) => "album",
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Track(id) => id.as_str(),
            Self::Album(id) => id.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoudnessMeasurementWrite {
    pub item: LoudnessItemId,
    pub analysis_key: [u8; 32],
    pub measurement: LoudnessMeasurement,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackLoudness {
    pub track: Option<LoudnessMeasurement>,
    pub album: Option<LoudnessMeasurement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoudnessTrackInput {
    pub track: Track,
    pub analysis_key: [u8; 32],
    pub current: Option<LoudnessMeasurement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoudnessAlbumInput {
    pub album_id: AlbumId,
    pub analysis_key: [u8; 32],
    pub track_ids: Arc<[TrackId]>,
    pub current: Option<LoudnessMeasurement>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoudnessAnalysisSnapshot {
    pub tracks: Arc<[LoudnessTrackInput]>,
    pub albums: Arc<[LoudnessAlbumInput]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StoredLoudnessMeasurement {
    pub(crate) analysis_key: [u8; 32],
    pub(crate) measurement: LoudnessMeasurement,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_measurements_reject_non_finite_values() {
        assert!(LoudnessMeasurement::new(Some(f64::NAN), 0.5).is_err());
        assert!(LoudnessMeasurement::new(Some(-18.0), f64::INFINITY).is_err());
        assert!(LoudnessMeasurement::new(Some(-18.0), -0.1).is_err());
        assert_eq!(
            LoudnessMeasurement::new(None, 0.0).expect("silent measurement"),
            LoudnessMeasurement {
                integrated_lufs: None,
                true_peak_ratio: 0.0,
            }
        );
    }
}
