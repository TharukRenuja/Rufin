pub(crate) mod lifecycle;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use library::SourceId;
use playback::{CurrentMedia, PlaybackView, PositionDiscontinuity, TransportHandle};
use tracing::warn;

use crate::Settings as UiSettings;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;

const TRAY_POLL_INTERVAL: Duration = Duration::from_millis(120);

pub(crate) struct DesktopState {
    pub(crate) media_controls: Rc<desktop_integration::MediaControls>,
    pub(crate) notifications: Rc<desktop_integration::Notifications>,
    tray: RefCell<Option<desktop_integration::Tray>>,
    tray_command_source: RefCell<Option<glib::SourceId>>,
}

impl DesktopState {
    pub(crate) fn new(application: &adw::Application, transport: TransportHandle) -> Self {
        Self {
            media_controls: desktop_integration::MediaControls::start(transport),
            notifications: desktop_integration::Notifications::new(application.clone().upcast()),
            tray: RefCell::new(None),
            tray_command_source: RefCell::new(None),
        }
    }
}

pub(crate) fn now_playing_notification_can_send(
    settings: &UiSettings,
    player: Option<&PlaybackView>,
) -> bool {
    desktop_integration::now_playing_notification_can_send(settings.allows_notifications(), player)
}

pub(crate) fn now_playing_notification_should_withdraw(
    settings: &UiSettings,
    player: Option<&PlaybackView>,
) -> bool {
    desktop_integration::now_playing_notification_should_withdraw(
        settings.allows_notifications(),
        player,
    )
}

impl Shell {
    pub(crate) fn notify_now_playing(self: &Rc<Self>, player: Option<&PlaybackView>) {
        self.observe_now_playing_notification(player, false);
    }

    pub(crate) fn refresh_now_playing_notification(self: &Rc<Self>, player: Option<&PlaybackView>) {
        self.observe_now_playing_notification(player, true);
    }

    fn observe_now_playing_notification(
        self: &Rc<Self>,
        player: Option<&PlaybackView>,
        refresh_current: bool,
    ) {
        let artwork = player.and_then(|player| {
            player.transport.current.as_deref().and_then(|media| {
                self.current_playback_cached_artwork_path(
                    &player.transport.source_id,
                    media,
                    THUMB_COVER_SIZE,
                )
                .map(|artwork| artwork.path)
            })
        });
        self.desktop.notifications.observe(
            player,
            self.settings.current.borrow().allows_notifications(),
            artwork,
            refresh_current,
        );
    }

    pub(crate) fn withdraw_now_playing_notification(&self) {
        self.desktop.notifications.withdraw();
    }

    pub(crate) fn update_media_controls(&self) {
        self.update_media_controls_after(None);
    }

    pub(crate) fn update_media_controls_after(&self, discontinuity: Option<PositionDiscontinuity>) {
        let playback = self.playback.player.borrow();
        let art_url = playback.as_ref().and_then(|playback| {
            playback
                .transport
                .current
                .as_deref()
                .and_then(|media| self.current_art_url(&playback.transport.source_id, media))
        });
        self.desktop
            .media_controls
            .observe(playback.as_ref(), art_url, discontinuity);
    }

    pub(crate) fn update_media_controls_position_after(
        &self,
        position_millis: Option<u64>,
        discontinuity: Option<PositionDiscontinuity>,
    ) {
        self.desktop
            .media_controls
            .observe_position(position_millis, discontinuity);
    }

    fn current_art_url(&self, source_id: &SourceId, media: &CurrentMedia) -> Option<String> {
        let artwork =
            self.current_playback_cached_artwork_path(source_id, media, THUMB_COVER_SIZE)?;
        glib::filename_to_uri(artwork.path, None)
            .ok()
            .map(|uri| uri.to_string())
    }

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
                if settings.exit_to_tray == enabled || (enabled && !settings.tray_enabled) {
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
                if settings.start_minimized == enabled || (enabled && !settings.tray_enabled) {
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
        let private_mode = self.settings.current.borrow().private_mode;
        let (tray, receiver) = match desktop_integration::Tray::start(private_mode) {
            Ok(started) => started,
            Err(error) => {
                warn!(%error);
                return false;
            }
        };
        *self.desktop.tray.borrow_mut() = Some(tray);
        self.install_tray_command_pump(receiver);
        true
    }

    fn shutdown_tray(&self) {
        if let Some(source) = self.desktop.tray_command_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(tray) = self.desktop.tray.borrow_mut().take() {
            tray.shutdown();
        }
    }

    pub(crate) fn refresh_tray_private_mode(&self) {
        let private_mode = self.settings.current.borrow().private_mode;
        if let Some(tray) = self.desktop.tray.borrow().as_ref() {
            tray.set_private_mode(private_mode);
        }
    }

    fn install_tray_command_pump(
        self: &Rc<Self>,
        receiver: Receiver<desktop_integration::TrayIntent>,
    ) {
        if let Some(source) = self.desktop.tray_command_source.borrow_mut().take() {
            source.remove();
        }
        let shell = Rc::clone(self);
        let source = glib::timeout_add_local(TRAY_POLL_INTERVAL, move || {
            while let Ok(intent) = receiver.try_recv() {
                match intent {
                    desktop_integration::TrayIntent::Present => shell.present_from_tray(),
                    desktop_integration::TrayIntent::PlayPause => {
                        shell.products.playback.transport.play_pause();
                    }
                    desktop_integration::TrayIntent::PreviousTrack => {
                        shell.products.playback.transport.previous();
                    }
                    desktop_integration::TrayIntent::NextTrack => {
                        shell.products.playback.transport.next();
                    }
                    desktop_integration::TrayIntent::TogglePrivateMode => {
                        let enabled = !shell.settings.current.borrow().private_mode;
                        shell.set_private_mode(enabled);
                    }
                    desktop_integration::TrayIntent::Quit => {
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

pub(crate) fn install_tray(shell: &Rc<Shell>) {
    if shell.settings.current.borrow().tray_enabled {
        shell.ensure_tray();
    }
    let close_shell = Rc::clone(shell);
    shell.chrome.window.connect_close_request(move |_| {
        let settings = close_shell.settings.current.borrow().clone();
        let tray_available =
            settings.tray_enabled && settings.exit_to_tray && close_shell.ensure_tray();
        if settings.tray_enabled && settings.exit_to_tray && tray_available {
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

pub(crate) fn present_initial_window(shell: &Rc<Shell>) {
    let settings = shell.settings.current.borrow().clone();
    let tray_available = settings.tray_enabled && settings.start_minimized && shell.ensure_tray();
    if settings.tray_enabled && settings.start_minimized && tray_available {
        shell.chrome.window.set_visible(false);
    } else {
        shell.chrome.window.present();
    }
}
