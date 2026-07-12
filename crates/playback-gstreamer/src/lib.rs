use gst::glib;
use gst::prelude::*;
use gstreamer as gst;
use playback::*;
use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, instrument, warn};

mod analysis;
mod audio;
mod engine;
mod pipeline;

pub use analysis::generate_waveform_peaks_cancellable;
pub use audio::available_audio_outputs;
pub use engine::GStreamerPlaybackBackend;

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

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
