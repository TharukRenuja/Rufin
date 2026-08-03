use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use library::ResolvedStream;
use playback::*;
use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, instrument, warn};

mod audio;
mod engine;
mod pipeline;
mod waveform;

pub use audio::available_audio_outputs;
pub use engine::GStreamerPlaybackBackend;
pub use waveform::generate_waveform_peaks_cancellable;

pub fn verify_audio_file(path: &Path) -> Result<(), String> {
    let uri = glib::filename_to_uri(path, None).map_err(|error| error.to_string())?;
    let stream = ResolvedStream::new(uri.as_str());
    generate_waveform_peaks_cancellable(&stream, || false).map(|_| ())
}

const SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(1_000);
const TRACK_START_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const STARTUP_SEEK_SETTLE_WINDOW: Duration = Duration::from_millis(10_000);
const SEEK_POSITION_TOLERANCE_MILLIS: u64 = 1_500;

/// Initialize GStreamer once before playback or waveform work starts.
fn ensure_gstreamer_initialized() -> Result<(), String> {
    static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZED
        .get_or_init(|| gst::init().map_err(|error| error.to_string()))
        .clone()
}

fn connect_server_certificate_policy(
    element: &gst::Element,
    trust_invalid_certificate: impl Fn() -> bool + Send + Sync + 'static,
) {
    let _ = element.connect("source-setup", false, move |values| {
        if let Some(source) = values
            .get(1)
            .and_then(|value| value.get::<gst::Element>().ok())
        {
            apply_server_certificate_policy(&source, trust_invalid_certificate());
        }
        None
    });
}

fn apply_server_certificate_policy(source: &gst::Element, trust_invalid_certificate: bool) {
    if source.find_property("ssl-strict").is_some() {
        source.set_property("ssl-strict", !trust_invalid_certificate);
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_source_uses_the_prepared_certificate_policy() {
        ensure_gstreamer_initialized().expect("initialize GStreamer");
        let playbin = gst::ElementFactory::make("playbin")
            .build()
            .expect("GStreamer playbin");
        let source = gst::ElementFactory::make("souphttpsrc")
            .build()
            .expect("GStreamer HTTP source");
        let trust_invalid_certificate = Arc::new(AtomicBool::new(false));
        let policy = Arc::clone(&trust_invalid_certificate);
        connect_server_certificate_policy(&playbin, move || policy.load(Ordering::SeqCst));

        playbin.emit_by_name::<()>("source-setup", &[&source]);
        assert!(source.property::<bool>("ssl-strict"));

        trust_invalid_certificate.store(true, Ordering::SeqCst);
        playbin.emit_by_name::<()>("source-setup", &[&source]);
        assert!(!source.property::<bool>("ssl-strict"));
    }
}
