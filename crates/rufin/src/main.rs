#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod controller;
mod settings;
mod source_setup;

pub(crate) use settings::StoredSettings;

use std::process::ExitCode;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> ExitCode {
    init_tracing();
    info!("starting Rufin native shell");

    ui::run_application(controller::runtime_inputs)
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
