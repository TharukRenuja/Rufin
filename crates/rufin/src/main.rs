#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod album_release;
mod app;
mod diagnostics;
mod loudness;
mod paths;
mod playback;
mod radio;
mod release_update;
mod scrobbling;
mod settings;
mod source;
mod waveform;

use std::env;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing::info;

fn main() -> ExitCode {
    if let Some(result) = verify_media_argument() {
        return result;
    }
    let _desktop_platform = desktop_integration::Platform::initialize();
    let diagnostics = diagnostics::Diagnostics::install(paths::state_dir());
    info!("starting Rufin native shell");

    ui::run_application(move || app::runtime_inputs(diagnostics))
}

fn verify_media_argument() -> Option<ExitCode> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new("--verify-media")) {
        return None;
    }
    let result = (|| {
        let path = PathBuf::from(arguments.next().ok_or("Usage: rufin --verify-media PATH")?);
        if arguments.next().is_some() {
            return Err("Usage: rufin --verify-media PATH".to_string());
        }
        sources::verify_local_media_file(&path).map_err(|error| error.to_string())?;
        playback_gstreamer::verify_audio_file(&path)?;
        Ok(())
    })();
    match result {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            Some(ExitCode::FAILURE)
        }
    }
}
