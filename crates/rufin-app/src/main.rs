mod controller;
mod i18n;
mod ui;

use adw::prelude::*;
use clap::{Parser, ValueEnum};
use rufin_test_support::FakeScale;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

const APP_ID: &str = "io.github.screwys.Rufin";

#[derive(Clone, Debug, Parser)]
#[command(name = "rufin", about = "Native GTK music client shell")]
struct Cli {
    #[arg(long, value_enum)]
    fake_scale: Option<FakeScaleArg>,

    #[arg(long)]
    smoke_exit_ms: Option<u64>,

    #[arg(long)]
    clear_cache: bool,

    #[arg(long)]
    forget_active_server: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FakeScaleArg {
    Small,
    Large,
}

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
    init_tracing();
    i18n::init();

    if cli.clear_cache && cli.forget_active_server {
        eprintln!("Use only one maintenance flag at a time.");
        std::process::exit(2);
    }

    if cli.clear_cache {
        match controller::AppController::clear_active_server_cache_for_app() {
            Ok(()) => info!("cleared active Jellyfin cache"),
            Err(error) => {
                eprintln!("Failed to clear active Jellyfin cache: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.forget_active_server {
        match controller::AppController::forget_active_server_for_app() {
            Ok(()) => info!("forgot active Jellyfin server"),
            Err(error) => {
                eprintln!("Failed to forget active Jellyfin server: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    let options = ui::AppOptions {
        fake_scale: cli.fake_scale.map(Into::into),
        smoke_exit_ms: cli.smoke_exit_ms,
    };

    info!(?options, "starting Rufin native shell");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| ui::build(app, options.clone()));

    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "rufin".to_string());
    let _exit_code = app.run_with_args(&[program]);
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rufin=info,rufin_app=info"));
    fmt().with_env_filter(filter).compact().init();
}
