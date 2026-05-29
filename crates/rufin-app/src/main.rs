#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod controller;
mod external_metadata;
mod external_scrobbling;
mod i18n;
mod lyrics;
mod providers;
mod ui;

use std::path::PathBuf;

use adw::prelude::*;
use clap::Parser;
#[cfg(feature = "dev-tools")]
use clap::ValueEnum;
#[cfg(feature = "dev-tools")]
use rufin_test_support::FakeScale;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

const APP_ID: &str = "io.github.screwys.Rufin";
const APP_ICON_NAME: &str = APP_ID;

#[derive(Clone, Debug, Parser)]
#[command(name = "rufin", about = "Native GTK music client shell")]
struct Cli {
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_enum)]
    fake_scale: Option<FakeScaleArg>,

    #[cfg(feature = "dev-tools")]
    #[arg(long)]
    smoke_exit_ms: Option<u64>,

    #[cfg(feature = "dev-tools")]
    #[arg(long)]
    ui_perf_run: bool,

    #[cfg(feature = "dev-tools")]
    #[arg(long)]
    ui_perf_observe: bool,

    #[cfg(feature = "dev-tools")]
    #[arg(long, default_value_t = 120)]
    ui_perf_max_gap_ms: u64,

    #[cfg(feature = "dev-tools")]
    #[arg(long, default_value_t = 650)]
    ui_perf_route_ms: u64,

    #[cfg(feature = "dev-tools")]
    #[arg(long, default_value_t = 15_000)]
    ui_perf_duration_ms: u64,

    #[cfg(feature = "dev-tools")]
    #[arg(long, default_value_t = 300)]
    ui_perf_asset_ms: u64,

    #[cfg(feature = "dev-tools")]
    #[arg(long)]
    ui_perf_output: Option<PathBuf>,

    #[arg(long)]
    clear_cache: bool,

    #[arg(long)]
    forget_active_server: bool,

    #[arg(long, hide = true)]
    validate_runtime: bool,
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FakeScaleArg {
    Small,
    Large,
}

#[cfg(feature = "dev-tools")]
impl From<FakeScaleArg> for FakeScale {
    fn from(value: FakeScaleArg) -> Self {
        match value {
            FakeScaleArg::Small => Self::Small,
            FakeScaleArg::Large => Self::Large,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if cli.validate_runtime {
        return;
    }

    init_tracing();
    i18n::init(&i18n::startup_language_preference());

    if cli.clear_cache && cli.forget_active_server {
        eprintln!("Use only one maintenance flag at a time.");
        std::process::exit(2);
    }
    #[cfg(feature = "dev-tools")]
    if cli.ui_perf_run && cli.ui_perf_observe {
        eprintln!("Use only one UI perf mode at a time.");
        std::process::exit(2);
    }

    if cli.clear_cache {
        match controller::AppController::clear_active_server_cache_for_app() {
            Ok(()) => info!("cleared active server cache"),
            Err(error) => {
                eprintln!("Failed to clear active server cache: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.forget_active_server {
        match controller::AppController::forget_active_server_for_app() {
            Ok(()) => info!("forgot active server"),
            Err(error) => {
                eprintln!("Failed to forget active server: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    #[cfg(feature = "dev-tools")]
    let mut options = ui::AppOptions::default();
    #[cfg(not(feature = "dev-tools"))]
    let options = ui::AppOptions::default();
    #[cfg(feature = "dev-tools")]
    {
        options.fake_scale = cli.fake_scale.map(Into::into);
        options.smoke_exit_ms = cli.smoke_exit_ms;
        options.ui_perf_run = cli.ui_perf_run;
        options.ui_perf_observe = cli.ui_perf_observe;
        options.ui_perf_max_gap_ms = cli.ui_perf_max_gap_ms;
        options.ui_perf_route_ms = cli.ui_perf_route_ms;
        options.ui_perf_duration_ms = cli.ui_perf_duration_ms;
        options.ui_perf_asset_ms = cli.ui_perf_asset_ms;
        options.ui_perf_output = cli.ui_perf_output;
    }

    info!(?options, "starting Rufin native shell");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("rufin-async")
        .build()
        .expect("failed to create async runtime");
    let _runtime_guard = runtime.enter();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| configure_app_icon());
    app.connect_activate(move |app| ui::build(app, options.clone()));

    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "rufin".to_string());
    let _exit_code = app.run_with_args(&[program]);
}

fn configure_app_icon() {
    gtk::Window::set_default_icon_name(APP_ICON_NAME);

    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let icon_theme = gtk::IconTheme::for_display(&display);
    for path in app_icon_search_paths() {
        if path.exists() {
            icon_theme.add_search_path(path);
        }
    }
}

fn app_icon_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = option_env!("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map(|path| path.join("../../data/icons"))
    {
        paths.push(path);
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        paths.push(exe_dir.join("data/icons"));
        paths.push(exe_dir.join("share/icons"));
        if let Some(repo_root) = exe_dir.parent().and_then(|path| path.parent()) {
            paths.push(repo_root.join("data/icons"));
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        paths.push(current_dir.join("data/icons"));
    }

    paths
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rufin=info,rufin_app=info"));
    fmt().with_env_filter(filter).compact().init();
}
