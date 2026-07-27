#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod album_release;
mod app;
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
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> ExitCode {
    init_tracing();
    info!("starting Rufin native shell");

    ui::run_application(app::runtime_inputs)
}

fn init_tracing() {
    let mut filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rufin=info,rufin_app=info,playback=info"));
    if std::env::var("RUST_LOG").map_or(true, |value| !value.contains("lofty"))
        && let Ok(directive) = "lofty=error".parse()
    {
        filter = filter.add_directive(directive);
    }
    fmt().with_env_filter(filter).compact().init();
}
