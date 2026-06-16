use gst::prelude::*;
use gstreamer as gst;
use std::time::{Duration, Instant};

const WAVEFORM_GENERATION_TIMEOUT: Duration = Duration::from_secs(180);
const WAVEFORM_BUS_POLL: gst::ClockTime = gst::ClockTime::from_mseconds(250);

pub fn generate_waveform_peaks(uri: &str) -> Result<Vec<(f64, f64)>, String> {
    generate_waveform_peaks_cancellable(uri, || false)
}

pub fn generate_waveform_peaks_cancellable(
    uri: &str,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<(f64, f64)>, String> {
    gst::init().map_err(|error| error.to_string())?;
    if cancelled() {
        return Err("waveform generation cancelled".to_string());
    }

    let pipeline =
        gst::parse::launch("uridecodebin name=decoder ! audioconvert ! audio/x-raw,channels=2 ! level name=level interval=250000000 ! fakesink name=sink")
            .map_err(|error| error.to_string())?;
    let bin = pipeline
        .downcast_ref::<gst::Bin>()
        .ok_or_else(|| "waveform pipeline is not a bin".to_string())?;
    let decoder = bin
        .by_name("decoder")
        .ok_or_else(|| "waveform pipeline is missing decoder".to_string())?;
    decoder.set_property("uri", uri);

    let sink = bin
        .by_name("sink")
        .ok_or_else(|| "waveform pipeline is missing sink".to_string())?;
    sink.set_property("qos", false);
    sink.set_property("sync", false);

    let bus = pipeline
        .bus()
        .ok_or_else(|| "waveform pipeline is missing bus".to_string())?;

    if let Err(error) = pipeline.set_state(gst::State::Playing) {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(error.to_string());
    }

    let mut peaks = Vec::new();
    let started = Instant::now();
    let result = loop {
        if cancelled() {
            break Err("waveform generation cancelled".to_string());
        }
        if started.elapsed() > WAVEFORM_GENERATION_TIMEOUT {
            break Err("waveform generation timed out".to_string());
        }
        let Some(message) = bus.timed_pop(WAVEFORM_BUS_POLL) else {
            continue;
        };
        use gst::MessageView;
        match message.view() {
            MessageView::Eos(..) => break Ok(peaks),
            MessageView::Error(error) => break Err(error.error().to_string()),
            MessageView::Element(element) => {
                if let Some(structure) = element.structure()
                    && structure.has_name("level")
                    && let Ok(values) = structure.get::<&gst::glib::ValueArray>("peak")
                    && values.len() >= 2
                    && let (Ok(left), Ok(right)) = (values[0].get::<f64>(), values[1].get::<f64>())
                {
                    peaks.push((db_to_amplitude(left), db_to_amplitude(right)));
                }
            }
            _ => {}
        }
    };

    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn db_to_amplitude(value: f64) -> f64 {
    10.0_f64.powf(value / 20.0).clamp(0.0, 1.0)
}
