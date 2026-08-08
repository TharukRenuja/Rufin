use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::OnceLock;

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
    run_application_with_presentation(bootstrap, false, None)
}

pub fn run_application_after_update<F>(bootstrap: F, presented: impl FnOnce() + 'static) -> ExitCode
where
    F: FnOnce() -> Result<RuntimeInputs, String> + 'static,
{
    run_application_with_presentation(bootstrap, true, Some(Box::new(presented)))
}

fn run_application_with_presentation<F>(
    bootstrap: F,
    force_initial_presentation: bool,
    presented: Option<Box<dyn FnOnce()>>,
) -> ExitCode
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
            return run_startup_error_application(error.to_string());
        }
    };
    let _runtime_guard = runtime.enter();

    let app = application();
    app.connect_startup(|_| configure_app_icon());
    let bootstrap = Rc::new(RefCell::new(Some(bootstrap)));
    let presented = Rc::new(RefCell::new(presented));
    app.connect_activate(move |app| {
        if let Some(window) = app
            .active_window()
            .or_else(|| app.windows().into_iter().next())
        {
            present_window(&window);
            return;
        }
        let Some(bootstrap) = bootstrap.borrow_mut().take() else {
            return;
        };
        match bootstrap() {
            Ok(inputs) => crate::shell::build::build(
                app,
                inputs,
                force_initial_presentation,
                presented.borrow_mut().take(),
            ),
            Err(error) => {
                error!(%error, "failed to start Rufin");
                present_startup_error(app, &error);
            }
        }
    });

    app.run().into()
}

fn run_startup_error_application(error: String) -> ExitCode {
    let app = application();
    app.connect_startup(|_| configure_app_icon());
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            present_window(&window);
        } else {
            present_startup_error(app, &error);
        }
    });
    app.run().into()
}

fn application() -> adw::Application {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();
    let quit = gio::SimpleAction::new("quit", None);
    let quit_app = app.clone();
    quit.connect_activate(move |_, _| quit_app.quit());
    app.add_action(&quit);
    #[cfg(target_os = "macos")]
    {
        app.set_accels_for_action("app.quit", &["<Meta>q"]);
        app.set_accels_for_action("window.close", &["<Meta>w"]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
    }
    app
}

pub(crate) fn present_window(window: &impl IsA<gtk::Window>) {
    let window = window.as_ref();
    let had_focus = gtk::prelude::RootExt::focus(window).is_some();
    window.present();
    if !had_focus {
        gtk::prelude::RootExt::set_focus(window, None::<&gtk::Widget>);
    }
}

fn present_startup_error(app: &adw::Application, error: &str) {
    let status = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title("Rufin")
        .description(error)
        .build();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rufin")
        .default_width(480)
        .default_height(320)
        .content(&status)
        .build();
    present_window(&window);
}

fn configure_app_icon() {
    if let Err(error) = register_resources() {
        error!(%error, "failed to register Rufin's interface resources");
    }
    gtk::Window::set_default_icon_name(APP_ID);
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/screwys/Rufin/icons");
}

fn register_resources() -> Result<(), String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            gio::resources_register_include!("rufin.gresource").map_err(|error| error.to_string())
        })
        .clone()
}

pub(crate) fn verify_interface_resources() -> Result<(), String> {
    register_resources()?;
    for path in [
        "/io/github/screwys/Rufin/icons/hicolor/scalable/apps/io.github.screwys.Rufin.svg",
        "/io/github/screwys/Rufin/icons/hicolor/scalable/actions/rufin-play-symbolic.svg",
        "/io/github/screwys/Rufin/icons/hicolor/scalable/status/io.github.screwys.Rufin.scrobbling-symbolic.svg",
    ] {
        gio::resources_lookup_data(path, gio::ResourceLookupFlags::NONE)
            .map_err(|error| format!("missing compiled Rufin resource {path}: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_rufin_icons_are_compiled_resources() {
        verify_interface_resources().expect("compiled interface resources");
    }
}
