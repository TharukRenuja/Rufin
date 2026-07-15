use crate::Settings as UiSettings;
use crate::shell::Shell;
use adw::prelude::*;
use gtk::glib;
use ksni::blocking::TrayMethods;
use localization::tr;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
use tracing::warn;

const TRAY_POLL_INTERVAL_MS: u64 = 120;
const TRAY_ICON_SIZES: [i32; 5] = [16, 22, 24, 32, 48];
const APP_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/icons/hicolor/512x512/apps/io.github.screwys.Rufin.png"
));

pub(crate) type TrayHandle = ksni::blocking::Handle<RufinTray>;

#[derive(Clone)]
pub(crate) struct RufinTray {
    sender: Sender<TrayCommand>,
    show_label: String,
    play_pause_label: String,
    previous_label: String,
    next_label: String,
    enable_private_mode_label: String,
    disable_private_mode_label: String,
    quit_label: String,
    tooltip: String,
    private_mode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayCommand {
    Present,
    PlayPause,
    PreviousTrack,
    NextTrack,
    TogglePrivateMode,
    Quit,
}

impl RufinTray {
    fn new(sender: Sender<TrayCommand>, private_mode: bool) -> Self {
        Self {
            sender,
            show_label: tr("Show Rufin"),
            play_pause_label: tr("Play/Pause"),
            previous_label: tr("Previous Track"),
            next_label: tr("Next Track"),
            enable_private_mode_label: tr("Enable private mode"),
            disable_private_mode_label: tr("Disable private mode"),
            quit_label: tr("Quit"),
            tooltip: tr("Rufin is running in the tray"),
            private_mode,
        }
    }

    fn send_command(&self, command: TrayCommand) {
        let _ = self.sender.send(command);
    }

    fn private_mode_label(&self) -> String {
        if self.private_mode {
            self.disable_private_mode_label.clone()
        } else {
            self.enable_private_mode_label.clone()
        }
    }
}

impl ksni::Tray for RufinTray {
    fn id(&self) -> String {
        "io.github.screwys.Rufin".to_string()
    }

    fn title(&self) -> String {
        "Rufin".to_string()
    }

    fn icon_name(&self) -> String {
        if tray_icon_pixmaps().is_empty() {
            "io.github.screwys.Rufin".to_string()
        } else {
            String::new()
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        tray_icon_pixmaps().clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: self.icon_name(),
            icon_pixmap: tray_icon_pixmaps().clone(),
            title: self.title(),
            description: self.tooltip.clone(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send_command(TrayCommand::Present);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};

        vec![
            StandardItem {
                label: self.show_label.clone(),
                activate: Box::new(|tray: &mut RufinTray| tray.send_command(TrayCommand::Present)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.play_pause_label.clone(),
                activate: Box::new(|tray: &mut RufinTray| {
                    tray.send_command(TrayCommand::PlayPause)
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.previous_label.clone(),
                activate: Box::new(|tray: &mut RufinTray| {
                    tray.send_command(TrayCommand::PreviousTrack)
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.next_label.clone(),
                activate: Box::new(|tray: &mut RufinTray| {
                    tray.send_command(TrayCommand::NextTrack)
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.private_mode_label(),
                activate: Box::new(|tray: &mut RufinTray| {
                    tray.send_command(TrayCommand::TogglePrivateMode)
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.quit_label.clone(),
                activate: Box::new(|tray: &mut RufinTray| tray.send_command(TrayCommand::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub(crate) fn install_tray(shell: &Rc<Shell>) {
    if shell.settings.current.borrow().tray_enabled {
        shell.ensure_tray();
    }

    let close_shell = Rc::clone(shell);
    shell.chrome.window.connect_close_request(move |_| {
        let settings = close_shell.settings.current.borrow().clone();
        let tray_available = if settings.tray_enabled && settings.exit_to_tray {
            close_shell.ensure_tray()
        } else {
            false
        };
        if exit_tray_hide(&settings, tray_available) {
            close_shell.save_window_state();
            close_shell.chrome.window.set_visible(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    let shutdown_shell = Rc::clone(shell);
    shell.chrome.application.connect_shutdown(move |_| {
        shutdown_shell.shutdown_tray();
    });
}

fn exit_tray_hide(settings: &UiSettings, tray_available: bool) -> bool {
    settings.tray_enabled && settings.exit_to_tray && tray_available
}

fn should_start_minimized(settings: &UiSettings, tray_available: bool) -> bool {
    settings.tray_enabled && settings.start_minimized && tray_available
}

pub(crate) fn present_initial_window(shell: &Rc<Shell>) {
    let settings = shell.settings.current.borrow().clone();
    let tray_available = if settings.tray_enabled && settings.start_minimized {
        shell.ensure_tray()
    } else {
        false
    };
    if should_start_minimized(&settings, tray_available) {
        shell.chrome.window.set_visible(false);
    } else {
        shell.chrome.window.present();
    }
}

impl Shell {
    pub(crate) fn set_tray_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("tray setting", |settings| {
                if settings.tray_enabled == enabled
                    && (enabled || (!settings.exit_to_tray && !settings.start_minimized))
                {
                    return false;
                }
                settings.tray_enabled = enabled;
                if !enabled {
                    settings.exit_to_tray = false;
                    settings.start_minimized = false;
                }
                true
            })
            .is_none()
        {
            return;
        }

        if enabled {
            self.ensure_tray();
        } else {
            self.shutdown_tray();
        }
    }

    pub(crate) fn set_exit_to_tray_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("exit to tray setting", |settings| {
                if settings.exit_to_tray == enabled {
                    return false;
                }
                if enabled && !settings.tray_enabled {
                    return false;
                }
                settings.exit_to_tray = enabled;
                true
            })
            .is_none()
        {
            return;
        }

        if enabled {
            self.ensure_tray();
        }
    }

    pub(crate) fn set_start_minimized_enabled(self: &Rc<Self>, enabled: bool) {
        if self
            .update_app_settings("start minimized setting", |settings| {
                if settings.start_minimized == enabled {
                    return false;
                }
                if enabled && !settings.tray_enabled {
                    return false;
                }
                settings.start_minimized = enabled;
                true
            })
            .is_none()
        {
            return;
        }

        if enabled {
            self.ensure_tray();
        }
    }

    fn ensure_tray(self: &Rc<Self>) -> bool {
        if self.desktop.tray.borrow().is_some() {
            return true;
        }

        let (sender, receiver) = channel();
        let tray = RufinTray::new(sender, self.settings.current.borrow().private_mode);
        let handle = match tray.disable_dbus_name(true).spawn() {
            Ok(handle) => handle,
            Err(error) => {
                warn!(?error, "failed to create status notifier tray item");
                return false;
            }
        };

        *self.desktop.tray.borrow_mut() = Some(handle);
        self.install_tray_command_pump(receiver);
        true
    }

    fn shutdown_tray(&self) {
        if let Some(source) = self.desktop.tray_command_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(handle) = self.desktop.tray.borrow_mut().take() {
            handle.shutdown().wait();
        }
    }

    pub(crate) fn refresh_tray_private_mode(&self) {
        let private_mode = self.settings.current.borrow().private_mode;
        if let Some(handle) = self.desktop.tray.borrow().as_ref().cloned() {
            let _updated = handle.update(|tray| {
                tray.private_mode = private_mode;
            });
        }
    }

    fn install_tray_command_pump(self: &Rc<Self>, receiver: Receiver<TrayCommand>) {
        if let Some(source) = self.desktop.tray_command_source.borrow_mut().take() {
            source.remove();
        }

        let shell = Rc::clone(self);
        let source =
            glib::timeout_add_local(Duration::from_millis(TRAY_POLL_INTERVAL_MS), move || {
                while let Ok(command) = receiver.try_recv() {
                    match command {
                        TrayCommand::Present => shell.present_from_tray(),
                        TrayCommand::PlayPause => {
                            shell.products.playback.transport.play_pause();
                        }
                        TrayCommand::PreviousTrack => {
                            shell.products.playback.transport.previous();
                        }
                        TrayCommand::NextTrack => {
                            shell.products.playback.transport.next();
                        }
                        TrayCommand::TogglePrivateMode => {
                            let enabled = !shell.settings.current.borrow().private_mode;
                            shell.set_private_mode(enabled);
                        }
                        TrayCommand::Quit => {
                            shell.shutdown_tray();
                            shell.chrome.application.quit();
                            return glib::ControlFlow::Break;
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        *self.desktop.tray_command_source.borrow_mut() = Some(source);
    }

    fn present_from_tray(&self) {
        self.chrome.window.set_visible(true);
        self.chrome.window.present();
    }
}

fn tray_icon_pixmaps() -> &'static Vec<ksni::Icon> {
    static ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    ICONS.get_or_init(build_tray_icon_pixmaps)
}

fn build_tray_icon_pixmaps() -> Vec<ksni::Icon> {
    let source = match artwork::decode_rgba(APP_ICON_BYTES, u32::MAX) {
        Ok(source) => source,
        Err(error) => {
            warn!(%error, "failed to load tray icon pixmap");
            return Vec::new();
        }
    };

    TRAY_ICON_SIZES
        .iter()
        .filter_map(|size| {
            source
                .resized_exact(u32::try_from(*size).ok()?, u32::try_from(*size).ok()?)
                .ok()
                .and_then(|image| tray_icon_from_rgba(&image))
        })
        .collect()
}

fn tray_icon_from_rgba(image: &artwork::RgbaImage) -> Option<ksni::Icon> {
    let width = i32::try_from(image.width()).ok()?;
    let height = i32::try_from(image.height()).ok()?;
    let rowstride = usize::try_from(image.row_stride()).ok()?;
    let width_usize = usize::try_from(width).ok()?;
    let height_usize = usize::try_from(height).ok()?;
    let pixels = image.rgba();
    let mut data = Vec::with_capacity(width_usize * height_usize * 4);
    for y in 0..height_usize {
        let row = y.checked_mul(rowstride)?;
        for x in 0..width_usize {
            let offset = row.checked_add(x.checked_mul(4)?)?;
            let end = offset.checked_add(4)?;
            let pixel = pixels.get(offset..end)?;
            data.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
    }
    Some(ksni::Icon {
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::{RufinTray, TRAY_ICON_SIZES, TrayCommand, tray_icon_pixmaps};
    use ksni::Tray;
    use ksni::menu::MenuItem;
    use std::sync::mpsc::channel;

    #[test]
    fn tray_use_controls() {
        let (sender, _receiver) = channel();
        let tray = RufinTray::new(sender, false);
        let items = tray.menu();
        let labels = standard_items(&items)
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Show Rufin",
                "Play/Pause",
                "Previous Track",
                "Next Track",
                "Enable private mode",
                "Quit"
            ]
        );
        assert!(
            standard_items(&items)
                .iter()
                .all(|item| item.icon_name.is_empty() && item.shortcut.is_empty())
        );
    }

    #[test]
    fn tray_match_state() {
        let (sender, _receiver) = channel();
        let disabled = RufinTray::new(sender.clone(), false);
        let enabled = RufinTray::new(sender, true);

        assert!(standard_labels(&disabled.menu()).contains(&"Enable private mode"));
        assert!(standard_labels(&enabled.menu()).contains(&"Disable private mode"));
    }

    #[test]
    fn tray_playback_command() {
        let (sender, receiver) = channel();
        let mut tray = RufinTray::new(sender, false);
        let mut items = tray.menu();

        activate_standard_item(&mut items, &mut tray, "Play/Pause");
        activate_standard_item(&mut items, &mut tray, "Previous Track");
        activate_standard_item(&mut items, &mut tray, "Next Track");
        activate_standard_item(&mut items, &mut tray, "Enable private mode");

        assert_eq!(receiver.recv().ok(), Some(TrayCommand::PlayPause));
        assert_eq!(receiver.recv().ok(), Some(TrayCommand::PreviousTrack));
        assert_eq!(receiver.recv().ok(), Some(TrayCommand::NextTrack));
        assert_eq!(receiver.recv().ok(), Some(TrayCommand::TogglePrivateMode));
    }

    #[test]
    fn tray_icon_size() {
        let sizes = tray_icon_pixmaps()
            .iter()
            .map(|icon| (icon.width, icon.height))
            .collect::<Vec<_>>();

        assert_eq!(
            sizes,
            TRAY_ICON_SIZES
                .iter()
                .map(|size| (*size, *size))
                .collect::<Vec<_>>()
        );
    }

    fn standard_items<T>(items: &[MenuItem<T>]) -> Vec<&ksni::menu::StandardItem<T>> {
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Standard(item) => Some(item),
                _ => None,
            })
            .collect()
    }

    fn standard_labels<T>(items: &[MenuItem<T>]) -> Vec<&str> {
        standard_items(items)
            .iter()
            .map(|item| item.label.as_str())
            .collect()
    }

    fn activate_standard_item(
        items: &mut [MenuItem<RufinTray>],
        tray: &mut RufinTray,
        label: &str,
    ) {
        let item = items
            .iter_mut()
            .find_map(|item| match item {
                MenuItem::Standard(item) if item.label == label => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing tray item {label}"));
        (item.activate)(tray);
    }
}
