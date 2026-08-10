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
#[cfg(target_os = "windows")]
use tracing::error;
use tracing::info;

fn main() -> ExitCode {
    #[cfg(target_os = "windows")]
    configure_windows_native_decorations();
    if let Some(result) = verify_media_argument() {
        return result;
    }
    let updated_restart = match updated_restart_argument() {
        Some(Ok(())) => true,
        Some(Err(message)) => {
            let _ = writeln!(io::stderr().lock(), "{message}");
            return ExitCode::FAILURE;
        }
        None => false,
    };
    let _desktop_platform = desktop_integration::Platform::initialize();
    let diagnostics = diagnostics::Diagnostics::install(paths::state_dir());
    info!("starting Rufin native shell");

    let bootstrap = move || app::runtime_inputs(diagnostics, !updated_restart);
    if updated_restart {
        ui::run_application_after_update(bootstrap, || {
            #[cfg(target_os = "windows")]
            if let Err(report_error) = windows_updater::report_updated_restart_visible() {
                error!(%report_error, "could not acknowledge the reopened Rufin window");
            }
        })
    } else {
        ui::run_application(bootstrap)
    }
}

#[cfg(target_os = "windows")]
fn configure_windows_native_decorations() {
    // Rufin is still single-threaded here and GTK has not been initialized.
    unsafe { env::set_var("GTK_CSD", "0") };
}

#[cfg(target_os = "windows")]
fn updated_restart_argument() -> Option<Result<(), String>> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new("--updated-restart")) {
        return None;
    }
    Some((|| {
        let version = arguments
            .next()
            .ok_or("Usage: rufin --updated-restart VERSION")?;
        if arguments.next().is_some() {
            return Err("Usage: rufin --updated-restart VERSION".to_string());
        }
        if version != OsStr::new(env!("CARGO_PKG_VERSION")) {
            return Err(
                "The reopened Rufin version does not match the installed update.".to_string(),
            );
        }
        windows_updater::wait_for_updated_restart()
    })())
}

#[cfg(not(target_os = "windows"))]
fn updated_restart_argument() -> Option<Result<(), String>> {
    None
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
        ui::verify_interface_resources()?;
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
