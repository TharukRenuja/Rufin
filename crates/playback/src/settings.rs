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
    #[default]
    Off,
    Track,
    Album,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlaybackSettings {
    #[serde(default)]
    pub transition_mode: PlaybackTransitionMode,
    #[serde(default = "default_crossfade_seconds")]
    pub crossfade_seconds: u8,
    #[serde(default)]
    pub skip_same_album_crossfade: bool,
    #[serde(default = "default_true")]
    pub audio_fade_on_status_change: bool,
    #[serde(default)]
    pub replay_gain: ReplayGainMode,
    #[serde(default)]
    pub stream_quality: StreamQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<String>,
    #[serde(default)]
    pub equalizer: EqualizerSettings,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            transition_mode: PlaybackTransitionMode::Gapless,
            crossfade_seconds: default_crossfade_seconds(),
            skip_same_album_crossfade: false,
            audio_fade_on_status_change: true,
            replay_gain: ReplayGainMode::Off,
            stream_quality: StreamQuality::Original,
            audio_output: None,
            equalizer: EqualizerSettings::default(),
            volume: default_volume(),
            muted: false,
        }
    }
}

impl PlaybackSettings {
    pub fn sanitize(&mut self) {
        self.crossfade_seconds = self
            .crossfade_seconds
            .clamp(MIN_CROSSFADE_SECONDS, MAX_CROSSFADE_SECONDS);
        if !self.volume.is_finite() {
            self.volume = default_volume();
        }
        self.volume = self.volume.clamp(0.0, 1.0);
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
}
