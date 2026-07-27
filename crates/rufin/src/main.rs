#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod album_release;
mod app;
mod diagnostics;
mod paths;
mod playback;
mod radio;
mod release_update;
mod schema30_migration;
mod scrobbling;
mod settings;
mod source;
mod waveform;

use std::process::ExitCode;
use tracing::info;
fn main() -> ExitCode {
    let diagnostics = diagnostics::Diagnostics::install(paths::state_dir());
    info!("starting Rufin native shell");

    ui::run_application(move || app::runtime_inputs(diagnostics))
}
