use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use tracing::error;

use crate::runtime::RuntimeInputs;

pub(crate) mod style;

const APP_ID: &str = "io.github.screwys.Rufin";

pub fn run_application<F>(bootstrap: F) -> ExitCode
where
    F: FnOnce() -> Result<RuntimeInputs, String> + 'static,
{
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
    let startup_succeeded = Rc::new(Cell::new(true));
    app.connect_startup(|_| configure_app_icon());
    let bootstrap = Rc::new(RefCell::new(Some(bootstrap)));
    let startup_succeeded_check = Rc::clone(&startup_succeeded);
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let Some(bootstrap) = bootstrap.borrow_mut().take() else {
            return;
        };
        match bootstrap() {
            Ok(inputs) => crate::shell::build::build(app, inputs),
            Err(error) => {
                error!(%error, "failed to start Rufin");
                startup_succeeded_check.set(false);
                app.quit();
            }
        }
    });

    let exit_code = app.run();
    if !startup_succeeded.get() {
        ExitCode::FAILURE
    } else {
        exit_code.into()
    }
}

fn configure_app_icon() {
    gtk::Window::set_default_icon_name(APP_ID);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_flatpak_icon_search_path() {
        let paths =
            app_icon_search_paths_for(None, Some(PathBuf::from("/app/bin/rufin.bin")), None);

        assert!(paths.contains(&PathBuf::from("/app/share/icons")));
    }
}
