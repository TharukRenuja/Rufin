mod i18n;
mod ui;

use adw::prelude::*;
use clap::{Parser, ValueEnum};
use rufin_test_support::FakeScale;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

const APP_ID: &str = "io.github.screwys.Rufin.Devel";

#[derive(Clone, Debug, Parser)]
#[command(name = "rufin", about = "Native GTK music client shell")]
struct Cli {
    #[arg(long, value_enum, default_value = "small")]
    fake_scale: FakeScaleArg,

    #[arg(long)]
    smoke_exit_ms: Option<u64>,
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

    let options = ui::AppOptions {
        fake_scale: cli.fake_scale.into(),
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
