use super::ensure_gstreamer_initialized;
use gst::prelude::*;
use gstreamer as gst;
use playback::{
    AudioOutput, BackendAudioSettings, EQUALIZER_BAND_COUNT, EqualizerSettings, ReplayGainMode,
};
use std::collections::HashSet;

const AUDIO_OUTPUT_DEVICE_PREFIX: &str = "gst-device:";
const CLASSIC_EQUALIZER_FREQUENCIES: [f64; EQUALIZER_BAND_COUNT] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];
const EQUALIZER_DUMMY_LOW_FREQUENCY: f64 = 20.0;
const EQUALIZER_DUMMY_HIGH_FREQUENCY: f64 = 20_000.0;

#[derive(Clone, Debug, PartialEq)]
struct AudioGraphConfig {
    replay_gain: ReplayGainMode,
    audio_output: Option<String>,
}

impl AudioGraphConfig {
    fn new(settings: &BackendAudioSettings) -> Self {
        Self {
            replay_gain: settings.replay_gain,
            audio_output: settings.audio_output.clone(),
        }
    }
}

pub(super) struct AudioGraph {
    root: gst::Element,
    config: AudioGraphConfig,
    output: gst::Element,
    equalizer: gst::Element,
    visualizer_pad: Option<gst::Pad>,
}

impl AudioGraph {
    pub(super) fn new(settings: &BackendAudioSettings) -> Result<Self, String> {
        let has_replay_gain = settings.replay_gain != ReplayGainMode::Off;
        let bin = gst::Bin::new();
        let convert_in = make_element("audioconvert", "rufin-audio-convert-in")?;
        let convert_out = make_element("audioconvert", "rufin-audio-convert-out")?;
        let output = make_audio_output(settings.audio_output.as_deref())?;
        let mut elements = vec![convert_in.clone()];

        let equalizer = make_element("equalizer-nbands", "rufin-equalizer")?;
        equalizer.set_property("num-bands", (EQUALIZER_BAND_COUNT + 2) as u32);
        configure_equalizer(&equalizer, &settings.equalizer);
        elements.push(equalizer.clone());

        if has_replay_gain {
            let rgvolume = make_element("rgvolume", "rufin-replaygain")?;
            if settings.replay_gain == ReplayGainMode::Album {
                rgvolume.set_property("album-mode", true);
            }
            elements.push(rgvolume);
            elements.push(make_element("rglimiter", "rufin-replaygain-limiter")?);
        }

        let visualizer_pad = convert_out.static_pad("src");
        elements.push(convert_out.clone());
        elements.push(output.clone());
        for element in &elements {
            bin.add(element).map_err(|error| error.to_string())?;
        }
        let refs = elements.iter().collect::<Vec<_>>();
        gst::Element::link_many(&refs).map_err(|error| error.to_string())?;

        let sink_pad = convert_in
            .static_pad("sink")
            .ok_or_else(|| "audio chain is missing an input pad".to_string())?;
        let ghost_sink =
            gst::GhostPad::with_target(&sink_pad).map_err(|error| error.to_string())?;
        ghost_sink
            .set_active(true)
            .map_err(|error| error.to_string())?;
        bin.add_pad(&ghost_sink)
            .map_err(|error| error.to_string())?;

        Ok(Self {
            root: bin.upcast(),
            config: AudioGraphConfig::new(settings),
            output,
            equalizer,
            visualizer_pad,
        })
    }

    pub(super) fn root(&self) -> &gst::Element {
        &self.root
    }

    pub(super) fn reconfigure(&mut self, settings: &BackendAudioSettings) -> Result<bool, String> {
        let config = AudioGraphConfig::new(settings);
        if self.config == config {
            self.apply_equalizer(&settings.equalizer);
            return Ok(true);
        }
        if self.config.replay_gain != config.replay_gain {
            return Ok(false);
        }
        if !self.update_output(config.audio_output.as_deref())? {
            return Ok(false);
        }
        self.config = config;
        self.apply_equalizer(&settings.equalizer);
        Ok(true)
    }

    pub(super) fn visualizer_pad(&self) -> Option<&gst::Pad> {
        self.visualizer_pad.as_ref()
    }

    pub(super) fn output_factory(&self) -> Option<String> {
        self.output
            .factory()
            .map(|factory| factory.name().to_string())
    }

    fn apply_equalizer(&self, settings: &EqualizerSettings) {
        configure_equalizer(&self.equalizer, settings);
    }

    fn update_output(&self, selected: Option<&str>) -> Result<bool, String> {
        let Some(selected) = selected else {
            return Ok(false);
        };
        let Some(target) = audio_output_device_target(selected) else {
            if gst::ElementFactory::find(selected).is_none() {
                return Err(selected_output_unavailable(selected));
            }
            return Ok(false);
        };
        if device_output_factory().is_none() {
            return Err(selected_output_unavailable(selected));
        }
        Ok(set_output_target(&self.output, target))
    }
}

pub fn available_audio_outputs() -> Vec<AudioOutput> {
    if ensure_gstreamer_initialized().is_err() {
        return Vec::new();
    }
    let devices = available_audio_output_devices();
    if !devices.is_empty() {
        return devices;
    }

    let candidates = [
        ("autoaudiosink", "System default"),
        ("pipewiresink", "PipeWire"),
        ("pulsesink", "PulseAudio"),
        ("alsasink", "ALSA"),
        ("jackaudiosink", "JACK"),
        ("osxaudiosink", "macOS"),
        ("wasapisink", "WASAPI"),
        ("directsoundsink", "DirectSound"),
    ];
    candidates
        .into_iter()
        .filter(|(id, _)| gst::ElementFactory::find(id).is_some())
        .map(|(id, name)| AudioOutput {
            id: id.to_string(),
            name: name.to_string(),
        })
        .collect()
}

fn audio_output_device_id(node_name: &str) -> String {
    format!("{AUDIO_OUTPUT_DEVICE_PREFIX}{node_name}")
}

fn audio_output_device_target(id: &str) -> Option<&str> {
    id.strip_prefix(AUDIO_OUTPUT_DEVICE_PREFIX)
        .filter(|target| !target.is_empty())
}

fn available_audio_output_devices() -> Vec<AudioOutput> {
    let monitor = gst::DeviceMonitor::new();
    let _filter_id = monitor.add_filter(Some("Audio/Sink"), None);
    if monitor.start().is_err() {
        return Vec::new();
    }

    let mut seen = HashSet::new();
    let mut outputs = monitor
        .devices()
        .into_iter()
        .filter_map(|device| {
            let properties = device.properties()?;
            let node_name = audio_output_device_node_name(&properties)?;
            if node_name.trim().is_empty() || !seen.insert(node_name.clone()) {
                return None;
            }
            let name = properties
                .get::<String>("node.description")
                .ok()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| device.display_name().to_string());
            Some(AudioOutput {
                id: audio_output_device_id(&node_name),
                name,
            })
        })
        .collect::<Vec<_>>();
    monitor.stop();
    outputs.sort_by_key(|output| output.name.to_lowercase());
    outputs
}

fn audio_output_device_node_name(properties: &gst::StructureRef) -> Option<String> {
    ["node.name", "device"]
        .into_iter()
        .find_map(|name| properties.get::<String>(name).ok())
        .filter(|name| !name.trim().is_empty())
}

fn make_audio_output(selected: Option<&str>) -> Result<gst::Element, String> {
    match selected {
        None => make_element("autoaudiosink", "rufin-audio-output"),
        Some(selected) => {
            if let Some(target) = audio_output_device_target(selected) {
                return make_device_audio_output(target)
                    .ok_or_else(|| selected_output_unavailable(selected));
            }
            if gst::ElementFactory::find(selected).is_none() {
                return Err(selected_output_unavailable(selected));
            }
            make_element(selected, "rufin-audio-output")
                .map_err(|_| selected_output_unavailable(selected))
        }
    }
}

fn make_device_audio_output(target: &str) -> Option<gst::Element> {
    let factory = device_output_factory()?;
    let sink = make_element(factory, "rufin-audio-output").ok()?;
    let property = if factory == "pulsesink" {
        "device"
    } else {
        "target-object"
    };
    sink.set_property(property, target);
    Some(sink)
}

fn device_output_factory() -> Option<&'static str> {
    ["pulsesink", "pipewiresink"]
        .into_iter()
        .find(|factory| gst::ElementFactory::find(factory).is_some())
}

fn set_output_target(output: &gst::Element, target: &str) -> bool {
    if output.find_property("device").is_some() {
        output.set_property("device", target);
        return true;
    }
    if output.find_property("target-object").is_some() {
        output.set_property("target-object", target);
        return true;
    }
    if let Some(proxy) = output.dynamic_cast_ref::<gst::ChildProxy>()
        && let Some(child) = proxy.child_by_index(0)
    {
        if child.find_property("device").is_some() {
            child.set_property("device", target);
            return true;
        }
        if child.find_property("target-object").is_some() {
            child.set_property("target-object", target);
            return true;
        }
    }
    false
}

fn selected_output_unavailable(selected: &str) -> String {
    format!("Selected audio output is unavailable: {selected}")
}

fn set_equalizer_band(
    equalizer: &gst::Element,
    index: usize,
    frequency: f64,
    bandwidth: f64,
    gain: f64,
) {
    let Some(proxy) = equalizer.dynamic_cast_ref::<gst::ChildProxy>() else {
        return;
    };
    if let Some(band) = proxy.child_by_index(index as u32) {
        band.set_property("freq", frequency);
        band.set_property("bandwidth", bandwidth);
        band.set_property("gain", gain);
    }
}

fn configure_equalizer(equalizer: &gst::Element, settings: &EqualizerSettings) {
    set_equalizer_band(equalizer, 0, EQUALIZER_DUMMY_LOW_FREQUENCY, 0.0, 0.0);
    set_equalizer_band(
        equalizer,
        EQUALIZER_BAND_COUNT + 1,
        EQUALIZER_DUMMY_HIGH_FREQUENCY,
        0.0,
        0.0,
    );
    let mut previous = 0.0;
    for (index, frequency) in CLASSIC_EQUALIZER_FREQUENCIES.iter().copied().enumerate() {
        let gain = if settings.enabled {
            settings.bands.get(index).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        set_equalizer_band(equalizer, index + 1, frequency, frequency - previous, gain);
        previous = frequency;
    }
}

fn make_element(factory: &str, name: &str) -> Result<gst::Element, String> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize_gstreamer() {
        ensure_gstreamer_initialized().expect("initialize GStreamer");
    }

    #[test]
    fn explicit_unavailable_output_does_not_fall_back_to_default() {
        initialize_gstreamer();
        let result = make_audio_output(Some("rufin-output-that-does-not-exist"));
        assert!(result.is_err_and(|error| error.contains("unavailable")));
    }

    #[test]
    fn no_output_preference_uses_the_system_default_sink() {
        initialize_gstreamer();
        if gst::ElementFactory::find("autoaudiosink").is_none() {
            return;
        }
        let output = make_audio_output(None).expect("system default output");
        assert_eq!(
            output.factory().map(|factory| factory.name().to_string()),
            Some("autoaudiosink".to_string())
        );
    }

    #[test]
    fn selected_device_id_targets_a_device_capable_sink() {
        initialize_gstreamer();
        let Some(factory) = device_output_factory() else {
            return;
        };
        let output = make_audio_output(Some(&audio_output_device_id("alsa_output.test")))
            .expect("selected device output");
        assert_eq!(
            output.factory().map(|output| output.name().to_string()),
            Some(factory.to_string())
        );
        let property = if factory == "pulsesink" {
            "device"
        } else {
            "target-object"
        };
        assert_eq!(output.property::<String>(property), "alsa_output.test");
    }

    #[test]
    fn equalizer_changes_apply_live_and_disable_to_zero_gain() {
        initialize_gstreamer();
        let equalizer =
            make_element("equalizer-nbands", "test-live-equalizer").expect("packaged equalizer");
        equalizer.set_property("num-bands", (EQUALIZER_BAND_COUNT + 2) as u32);
        let mut settings = EqualizerSettings {
            enabled: true,
            bands: vec![5.0; EQUALIZER_BAND_COUNT],
            ..EqualizerSettings::default()
        };
        configure_equalizer(&equalizer, &settings);
        assert_eq!(equalizer_band_gain(&equalizer, 1), Some(5.0));

        settings.enabled = false;
        configure_equalizer(&equalizer, &settings);
        assert_eq!(equalizer_band_gain(&equalizer, 1), Some(0.0));
    }

    fn equalizer_band_gain(equalizer: &gst::Element, index: usize) -> Option<f64> {
        equalizer
            .dynamic_cast_ref::<gst::ChildProxy>()?
            .child_by_index(index as u32)
            .map(|band| band.property::<f64>("gain"))
    }
}
