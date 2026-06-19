#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod controller;
mod cover_art_policy;
mod external_activity;
mod external_metadata;
mod external_scrobbling;
mod i18n;
mod lyrics;
mod providers;
mod ui;

#[cfg(feature = "dev-tools")]
use ::test_support::FakeScale;
use adw::prelude::*;
use clap::Parser;
#[cfg(feature = "dev-tools")]
use clap::ValueEnum;
use gtk::gio;
use std::cell::Cell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

const APP_ID: &str = "io.github.screwys.Rufin";
const APP_ICON_NAME: &str = APP_ID;

#[derive(Clone, Debug, Parser)]
#[command(name = "rufin", about = "Native GTK music client shell")]
struct Cli {
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_enum)]
    fake_scale: Option<FakeScaleArg>,

    #[arg(long, hide = true)]
    startup_check: bool,
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, Copy, Debug, ValueEnum)]
enum FakeScaleArg {
    Small,
    Large,
    Stress,
    ThirtyK,
}

#[cfg(feature = "dev-tools")]
impl From<FakeScaleArg> for FakeScale {
    fn from(value: FakeScaleArg) -> Self {
        match value {
            FakeScaleArg::Small => Self::Small,
            FakeScaleArg::Large => Self::Large,
            FakeScaleArg::Stress => Self::Stress,
            FakeScaleArg::ThirtyK => Self::ThirtyK,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    init_tracing();
    i18n::init(&i18n::startup_language_preference());

    let options = ui::AppOptions {
        #[cfg(feature = "dev-tools")]
        fake_scale: cli.fake_scale.map(Into::into),
    };

    info!(?options, "starting Rufin native shell");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("rufin-async")
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "failed to create async runtime");
            return ExitCode::FAILURE;
        }
    };
    let _runtime_guard = runtime.enter();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();
    let startup_check = cli.startup_check;
    let startup_display_ready = Rc::new(Cell::new(true));
    let startup_display_ready_check = Rc::clone(&startup_display_ready);
    app.connect_startup(move |app| {
        let display_ready = configure_app_icon();
        if startup_check {
            if !display_ready {
                error!("GTK display is not available");
                startup_display_ready_check.set(false);
            }
            app.quit();
        }
    });
    if startup_check {
        app.connect_activate(|app| app.quit());
    } else {
        app.connect_activate(move |app| ui::build(app, options.clone()));
    }

    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "rufin".to_string());
    let _exit_code = app.run_with_args(&[program]);
    if startup_display_ready.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn configure_app_icon() -> bool {
    gtk::Window::set_default_icon_name(APP_ICON_NAME);

    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let icon_theme = gtk::IconTheme::for_display(&display);
    for path in app_icon_search_paths() {
        if path.exists() {
            icon_theme.add_search_path(path);
        }
    }
    true
}

fn app_icon_search_paths() -> Vec<PathBuf> {
    app_icon_search_paths_for(
        option_env!("CARGO_MANIFEST_DIR").map(PathBuf::from),
        std::env::current_exe().ok(),
        std::env::current_dir().ok(),
    )
}

fn app_icon_search_paths_for(
    manifest_dir: Option<PathBuf>,
    exe: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = manifest_dir.map(|path| path.join("../../data/icons")) {
        paths.push(path);
    }

    if let Some(exe) = exe
        && let Some(exe_dir) = exe.parent()
    {
        paths.push(exe_dir.join("data/icons"));
        paths.push(exe_dir.join("share/icons"));
        if let Some(install_prefix) = exe_dir.parent() {
            paths.push(install_prefix.join("share/icons"));
        }
        if let Some(repo_root) = exe_dir.parent().and_then(|path| path.parent()) {
            paths.push(repo_root.join("data/icons"));
        }
    }

    if let Some(current_dir) = current_dir {
        paths.push(current_dir.join("data/icons"));
    }

    paths
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_flatpak_search() {
        let paths =
            app_icon_search_paths_for(None, Some(PathBuf::from("/app/bin/rufin.bin")), None);

        assert!(paths.contains(&PathBuf::from("/app/share/icons")));
    }
}
