use library::StreamQuality;
use serde::{Deserialize, Serialize};

pub const EQUALIZER_BAND_COUNT: usize = 10;
pub const MIN_CROSSFADE_SECONDS: u8 = 1;
pub const MAX_CROSSFADE_SECONDS: u8 = 30;
pub const DEFAULT_AUTO_DJ_REFILL_THRESHOLD: u8 = 1;
pub const MIN_AUTO_DJ_REFILL_THRESHOLD: u8 = 1;
pub const MAX_AUTO_DJ_REFILL_THRESHOLD: u8 = 10;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlaybackTransitionMode {
    #[default]
    #[serde(alias = "Default")]
    Gapless,
    Crossfade,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReplayGainMode {
    Off,
    Track,
    #[default]
    Album,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum VolumeScale {
    #[default]
    Perceptual,
    Linear,
}

impl VolumeScale {
    pub fn gain(self, position: f64) -> f64 {
        let position = sanitize_volume(position);
        match self {
            Self::Perceptual => position.powi(3),
            Self::Linear => position,
        }
    }

    pub fn position_for_gain(self, gain: f64) -> f64 {
        let gain = sanitize_volume(gain);
        match self {
            Self::Perceptual => gain.cbrt(),
            Self::Linear => gain,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EqualizerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_equalizer_selected_preset")]
    pub selected_preset: String,
    #[serde(default = "default_equalizer_bands")]
    pub bands: Vec<f64>,
}

impl Default for EqualizerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            selected_preset: "Flat".to_string(),
            bands: default_equalizer_bands(),
        }
    }
}

impl EqualizerSettings {
    pub fn sanitize(&mut self) {
        if self.selected_preset.trim().is_empty() {
            self.selected_preset = default_equalizer_selected_preset();
        }
        sanitize_equalizer_bands(&mut self.bands);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlaybackSettings {
    pub transition_mode: PlaybackTransitionMode,
    pub crossfade_seconds: u8,
    pub skip_same_album_crossfade: bool,
    pub audio_fade_on_status_change: bool,
    pub replay_gain: ReplayGainMode,
    pub stream_quality: StreamQuality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<String>,
    pub equalizer: EqualizerSettings,
    pub volume: f64,
    pub volume_scale: VolumeScale,
    pub muted: bool,
}

#[derive(Deserialize)]
struct SavedPlaybackSettings {
    #[serde(default)]
    transition_mode: PlaybackTransitionMode,
    #[serde(default = "default_crossfade_seconds")]
    crossfade_seconds: u8,
    #[serde(default)]
    skip_same_album_crossfade: bool,
    #[serde(default = "default_true")]
    audio_fade_on_status_change: bool,
    #[serde(default)]
    replay_gain: ReplayGainMode,
    #[serde(default)]
    stream_quality: StreamQuality,
    #[serde(default)]
    audio_output: Option<String>,
    #[serde(default)]
    equalizer: EqualizerSettings,
    #[serde(default = "default_volume")]
    volume: f64,
    #[serde(default)]
    volume_scale: Option<VolumeScale>,
    #[serde(default)]
    muted: bool,
}

impl<'de> Deserialize<'de> for PlaybackSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let saved = SavedPlaybackSettings::deserialize(deserializer)?;
        let legacy_gain = sanitize_volume(saved.volume);
        let (volume, volume_scale) = match saved.volume_scale {
            Some(scale) => (saved.volume, scale),
            None => (
                VolumeScale::Perceptual.position_for_gain(legacy_gain),
                VolumeScale::Perceptual,
            ),
        };
        Ok(Self {
            transition_mode: saved.transition_mode,
            crossfade_seconds: saved.crossfade_seconds,
            skip_same_album_crossfade: saved.skip_same_album_crossfade,
            audio_fade_on_status_change: saved.audio_fade_on_status_change,
            replay_gain: saved.replay_gain,
            stream_quality: saved.stream_quality,
            audio_output: saved.audio_output,
            equalizer: saved.equalizer,
            volume,
            volume_scale,
            muted: saved.muted,
        })
    }
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            transition_mode: PlaybackTransitionMode::Gapless,
            crossfade_seconds: default_crossfade_seconds(),
            skip_same_album_crossfade: false,
            audio_fade_on_status_change: true,
            replay_gain: ReplayGainMode::Album,
            stream_quality: StreamQuality::Original,
            audio_output: None,
            equalizer: EqualizerSettings::default(),
            volume: default_volume(),
            volume_scale: VolumeScale::Perceptual,
            muted: false,
        }
    }
}

impl PlaybackSettings {
    pub fn set_volume_scale_preserving_gain(&mut self, volume_scale: VolumeScale) {
        if self.volume_scale == volume_scale {
            return;
        }
        let gain = self.volume_scale.gain(self.volume);
        self.volume = volume_scale.position_for_gain(gain);
        self.volume_scale = volume_scale;
    }

    pub fn sanitize(&mut self) {
        self.crossfade_seconds = self
            .crossfade_seconds
            .clamp(MIN_CROSSFADE_SECONDS, MAX_CROSSFADE_SECONDS);
        self.volume = sanitize_volume(self.volume);
        if self
            .audio_output
            .as_deref()
            .is_some_and(|output| output.trim().is_empty())
        {
            self.audio_output = None;
        }
        self.equalizer.sanitize();
    }
}

fn default_true() -> bool {
    true
}

fn default_volume() -> f64 {
    1.0
}

fn sanitize_volume(volume: f64) -> f64 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        default_volume()
    }
}

fn default_crossfade_seconds() -> u8 {
    5
}

fn default_equalizer_bands() -> Vec<f64> {
    vec![0.0; EQUALIZER_BAND_COUNT]
}

fn default_equalizer_selected_preset() -> String {
    "Custom".to_string()
}

fn sanitize_equalizer_bands(bands: &mut Vec<f64>) {
    if bands.len() != EQUALIZER_BAND_COUNT {
        bands.resize(EQUALIZER_BAND_COUNT, 0.0);
    }
    for gain in bands {
        if !gain.is_finite() {
            *gain = 0.0;
        }
        *gain = gain.clamp(-12.0, 12.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_default_transition_migrates_to_gapless() {
        let mode = serde_json::from_str::<PlaybackTransitionMode>(r#""Default""#)
            .expect("deserialize the legacy transition mode");

        assert_eq!(mode, PlaybackTransitionMode::Gapless);
        assert_eq!(
            serde_json::to_string(&mode).expect("serialize the migrated transition mode"),
            r#""Gapless""#
        );
        assert_eq!(
            PlaybackSettings::default().transition_mode,
            PlaybackTransitionMode::Gapless
        );
    }

    #[test]
    fn playback_settings_clamp_crossfade_range() {
        let mut settings = PlaybackSettings {
            crossfade_seconds: 0,
            ..PlaybackSettings::default()
        };
        settings.sanitize();
        assert_eq!(settings.crossfade_seconds, MIN_CROSSFADE_SECONDS);

        settings.crossfade_seconds = MAX_CROSSFADE_SECONDS + 1;
        settings.sanitize();
        assert_eq!(settings.crossfade_seconds, MAX_CROSSFADE_SECONDS);
    }

    #[test]
    fn replay_gain_defaults_to_album() {
        assert_eq!(ReplayGainMode::default(), ReplayGainMode::Album);
        assert_eq!(
            PlaybackSettings::default().replay_gain,
            ReplayGainMode::Album
        );

        let restored = serde_json::from_str::<PlaybackSettings>("{}")
            .expect("restore playback settings without ReplayGain");
        assert_eq!(restored.replay_gain, ReplayGainMode::Album);
    }

    #[test]
    fn perceptual_volume_is_cubic_and_reversible() {
        for position in [0.0, 0.1, 0.5, 1.0] {
            let gain = VolumeScale::Perceptual.gain(position);
            assert!((gain - position.powi(3)).abs() < f64::EPSILON);
            assert!((VolumeScale::Perceptual.position_for_gain(gain) - position).abs() < 1e-12);
        }
    }

    #[test]
    fn legacy_linear_volume_migrates_to_perceptual_without_changing_gain() {
        let mut value =
            serde_json::to_value(PlaybackSettings::default()).expect("serialize settings");
        value
            .as_object_mut()
            .expect("playback settings object")
            .remove("volume_scale");
        value["volume"] = 0.5.into();

        let migrated =
            serde_json::from_value::<PlaybackSettings>(value).expect("migrate playback settings");

        assert_eq!(migrated.volume_scale, VolumeScale::Perceptual);
        assert!((migrated.volume_scale.gain(migrated.volume) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn explicit_linear_volume_round_trips_without_migration() {
        let settings = PlaybackSettings {
            volume: 0.5,
            volume_scale: VolumeScale::Linear,
            ..PlaybackSettings::default()
        };
        let restored = serde_json::from_value::<PlaybackSettings>(
            serde_json::to_value(settings).expect("serialize linear volume"),
        )
        .expect("restore linear volume");

        assert_eq!(restored.volume_scale, VolumeScale::Linear);
        assert_eq!(restored.volume, 0.5);
        assert_eq!(restored.volume_scale.gain(restored.volume), 0.5);
    }

    #[test]
    fn changing_volume_scale_preserves_output_gain() {
        let mut settings = PlaybackSettings {
            volume: 0.5,
            volume_scale: VolumeScale::Linear,
            ..PlaybackSettings::default()
        };

        settings.set_volume_scale_preserving_gain(VolumeScale::Perceptual);
        assert!((settings.volume - 0.5_f64.cbrt()).abs() < 1e-12);
        assert!((settings.volume_scale.gain(settings.volume) - 0.5).abs() < 1e-12);

        settings.set_volume_scale_preserving_gain(VolumeScale::Linear);
        assert!((settings.volume - 0.5).abs() < 1e-12);
        assert!((settings.volume_scale.gain(settings.volume) - 0.5).abs() < 1e-12);
    }
}
