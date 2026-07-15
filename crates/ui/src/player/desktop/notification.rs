use crate::Settings as UiSettings;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use gio::prelude::ApplicationExt;
use gtk::glib::prelude::*;
use gtk::{gio, glib};
use playback::{PlaybackView, TransportStatus};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tracing::warn;

const APP_ID: &str = "io.github.screwys.Rufin";
const APP_NAME: &str = "Rufin";
const DBUS_TIMEOUT_MSEC: i32 = 1_000;
const NOTIFICATIONS_BUS_NAME: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const NOW_PLAYING_NOTIFICATION_ID: &str = "now-playing";
const NOW_PLAYING_NOTIFICATION_TIMEOUT_MSEC: i32 = -1;

fn notification_icon_path(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::read(path).ok()?;
    notification_icon_bytes(&bytes)
}

fn notification_icon_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    artwork::square_thumbnail_png(bytes, THUMB_COVER_SIZE.clamp(1, 512)).ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeNowPlayingNotification {
    title: String,
    body: String,
    artwork_uri: Option<String>,
}

impl Shell {
    pub(crate) fn notify_now_playing(self: &Rc<Self>, player: Option<&PlaybackView>) {
        self.notify_now_playing_inner(player, false);
    }

    pub(crate) fn refresh_now_playing_notification(self: &Rc<Self>, player: Option<&PlaybackView>) {
        self.notify_now_playing_inner(player, true);
    }

    fn notify_now_playing_inner(
        self: &Rc<Self>,
        player: Option<&PlaybackView>,
        refresh_current: bool,
    ) {
        let settings = self.settings.current.borrow().clone();
        if now_playing_notification_should_withdraw(&settings, player) {
            self.withdraw_now_playing_notification();
            return;
        }
        if !now_playing_notification_can_send(&settings, player) {
            return;
        }
        let Some(player) = player else {
            return;
        };
        let Some(run) = player.transport.run else {
            return;
        };
        if !refresh_current && self.desktop.notification_run.get() == Some(run) {
            return;
        }
        self.desktop.notification_run.set(Some(run));
        let Some(entry) = player.transport.current.as_ref() else {
            return;
        };
        let title = entry.track.title.clone();
        let body = format!("{} - {}", entry.track.artist, entry.track.album);
        let artwork_path = self
            .current_playback_cached_artwork_path(
                &player.transport.source_id,
                entry,
                THUMB_COVER_SIZE,
            )
            .map(|artwork| artwork.path);
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let notifications_enabled = {
                let settings = shell.settings.current.borrow();
                settings.allows_notifications()
            };
            if !notifications_enabled {
                shell.withdraw_now_playing_notification();
                return;
            }
            if !now_playing_notification_matches_current(
                shell.playback.player.borrow().as_ref(),
                run,
            ) {
                return;
            }

            let artwork_uri = artwork_path
                .as_deref()
                .and_then(now_playing_notification_artwork_uri);
            let native_notification = NativeNowPlayingNotification {
                title: title.clone(),
                body: body.clone(),
                artwork_uri,
            };
            let replaces_id = shell.desktop.notification_id.get();
            match send_native_now_playing_notification(&native_notification, replaces_id).await {
                Ok(notification_id) => {
                    if now_playing_notification_is_still_sendable(&shell, run) {
                        shell
                            .chrome
                            .application
                            .withdraw_notification(NOW_PLAYING_NOTIFICATION_ID);
                        shell.desktop.notification_id.set(notification_id);
                    } else {
                        close_native_now_playing_notification(notification_id).await;
                    }
                }
                Err(error) => {
                    warn!(%error, "failed to send native now-playing notification");
                    if replaces_id != 0 {
                        shell.desktop.notification_id.set(0);
                        close_native_now_playing_notification(replaces_id).await;
                    }
                    if now_playing_notification_is_still_sendable(&shell, run) {
                        send_gio_now_playing_notification(
                            &shell.chrome.application,
                            title,
                            body,
                            artwork_path,
                        )
                        .await;
                    }
                }
            }
        });
    }

    pub(crate) fn withdraw_now_playing_notification(&self) {
        self.chrome
            .application
            .withdraw_notification(NOW_PLAYING_NOTIFICATION_ID);
        let notification_id = self.desktop.notification_id.replace(0);
        self.desktop.notification_run.set(None);
        if notification_id != 0 {
            glib::spawn_future_local(async move {
                close_native_now_playing_notification(notification_id).await;
            });
        }
    }
}

fn now_playing_notification_is_still_sendable(shell: &Shell, run: playback::RunId) -> bool {
    let notifications_enabled = {
        let settings = shell.settings.current.borrow();
        settings.allows_notifications()
    };
    notifications_enabled
        && now_playing_notification_matches_current(shell.playback.player.borrow().as_ref(), run)
}

fn now_playing_notification_artwork_uri(path: &Path) -> Option<String> {
    glib::filename_to_uri(path, None)
        .ok()
        .map(|uri| uri.to_string())
}

fn now_playing_notification_hints(artwork_uri: Option<&str>) -> glib::VariantDict {
    let hints = glib::VariantDict::new(None);
    hints.insert("desktop-entry", APP_ID);
    hints.insert("transient", true);
    if let Some(uri) = artwork_uri {
        hints.insert("image-path", uri);
        hints.insert("image_path", uri);
    }
    hints
}

fn now_playing_notification_parameters(
    notification: &NativeNowPlayingNotification,
    replaces_id: u32,
) -> glib::Variant {
    (
        APP_NAME.to_string(),
        replaces_id,
        APP_ID.to_string(),
        notification.title.clone(),
        notification.body.clone(),
        Vec::<String>::new(),
        now_playing_notification_hints(notification.artwork_uri.as_deref()),
        NOW_PLAYING_NOTIFICATION_TIMEOUT_MSEC,
    )
        .to_variant()
}

async fn send_native_now_playing_notification(
    notification: &NativeNowPlayingNotification,
    replaces_id: u32,
) -> Result<u32, glib::Error> {
    let connection = gio::bus_get_future(gio::BusType::Session).await?;
    let parameters = now_playing_notification_parameters(notification, replaces_id);
    let reply_type = glib::VariantTy::new("(u)").ok();
    let reply = connection
        .call_future(
            Some(NOTIFICATIONS_BUS_NAME),
            NOTIFICATIONS_OBJECT_PATH,
            NOTIFICATIONS_INTERFACE,
            "Notify",
            Some(&parameters),
            reply_type,
            gio::DBusCallFlags::NONE,
            DBUS_TIMEOUT_MSEC,
        )
        .await?;
    Ok(reply.try_child_get::<u32>(0).ok().flatten().unwrap_or(0))
}

async fn close_native_now_playing_notification(notification_id: u32) {
    if notification_id == 0 {
        return;
    }
    let Ok(connection) = gio::bus_get_future(gio::BusType::Session).await else {
        return;
    };
    let parameters = (notification_id,).to_variant();
    let _closed = connection
        .call_future(
            Some(NOTIFICATIONS_BUS_NAME),
            NOTIFICATIONS_OBJECT_PATH,
            NOTIFICATIONS_INTERFACE,
            "CloseNotification",
            Some(&parameters),
            None,
            gio::DBusCallFlags::NONE,
            DBUS_TIMEOUT_MSEC,
        )
        .await;
}

async fn send_gio_now_playing_notification(
    application: &adw::Application,
    title: String,
    body: String,
    artwork_path: Option<PathBuf>,
) {
    let icon_bytes = match artwork_path {
        Some(path) => gtk::gio::spawn_blocking(move || notification_icon_path(&path))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let notification = gio::Notification::new(&title);
    notification.set_body(Some(&body));
    if let Some(bytes) = icon_bytes {
        let bytes = glib::Bytes::from_owned(bytes);
        notification.set_icon(&gio::BytesIcon::new(&bytes));
    }
    application.send_notification(Some(NOW_PLAYING_NOTIFICATION_ID), &notification);
}

pub(crate) fn now_playing_notification_can_send(
    settings: &UiSettings,
    player: Option<&PlaybackView>,
) -> bool {
    settings.allows_notifications()
        && player.is_some_and(|player| {
            matches!(
                player.transport.state,
                TransportStatus::Playing | TransportStatus::Buffering
            ) && player.transport.current.is_some()
        })
}

pub(crate) fn now_playing_notification_should_withdraw(
    settings: &UiSettings,
    player: Option<&PlaybackView>,
) -> bool {
    !settings.allows_notifications()
        || player.is_none_or(|player| {
            player.transport.current.is_none() || player.transport.state == TransportStatus::Stopped
        })
}

fn now_playing_notification_matches_current(
    player: Option<&PlaybackView>,
    run: playback::RunId,
) -> bool {
    player.is_some_and(|player| {
        matches!(
            player.transport.state,
            TransportStatus::Playing | TransportStatus::Buffering
        ) && player.transport.run == Some(run)
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        APP_ID, APP_NAME, NOW_PLAYING_NOTIFICATION_TIMEOUT_MSEC, NativeNowPlayingNotification,
        notification_icon_bytes, now_playing_notification_artwork_uri,
        now_playing_notification_hints, now_playing_notification_parameters,
    };
    use crate::shell::cover::THUMB_COVER_SIZE;

    #[test]
    fn notification_artwork_is_square_and_thumbnail_sized() {
        let cover = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/icons/hicolor/512x512/apps/io.github.screwys.Rufin.png"
        ));
        let bytes = notification_icon_bytes(cover).expect("notification bytes");
        let icon = artwork::decode_rgba(&bytes, u32::MAX).expect("notification image");

        assert_eq!(icon.width(), THUMB_COVER_SIZE);
        assert_eq!(icon.height(), THUMB_COVER_SIZE);
    }

    #[test]
    fn native_notification_hints_include_identity_and_artwork() {
        let hints = now_playing_notification_hints(Some("file:///music/cover.png"));

        assert_eq!(
            hints.lookup::<String>("desktop-entry").unwrap().as_deref(),
            Some(APP_ID)
        );
        assert_eq!(hints.lookup::<bool>("transient").unwrap(), Some(true));
        assert_eq!(
            hints.lookup::<String>("image-path").unwrap().as_deref(),
            Some("file:///music/cover.png")
        );
        assert_eq!(
            hints.lookup::<String>("image_path").unwrap().as_deref(),
            Some("file:///music/cover.png")
        );
    }

    #[test]
    fn native_notification_parameters_replace_the_previous_notification() {
        let notification = NativeNowPlayingNotification {
            title: "Track".to_string(),
            body: "Artist - Album".to_string(),
            artwork_uri: None,
        };
        let parameters = now_playing_notification_parameters(&notification, 41);

        assert_eq!(
            parameters.try_child_get::<String>(0).unwrap().as_deref(),
            Some(APP_NAME)
        );
        assert_eq!(parameters.try_child_get::<u32>(1).unwrap(), Some(41));
        assert_eq!(
            parameters.try_child_get::<String>(3).unwrap().as_deref(),
            Some("Track")
        );
        assert_eq!(
            parameters.try_child_get::<String>(4).unwrap().as_deref(),
            Some("Artist - Album")
        );
        assert_eq!(
            parameters.try_child_get::<i32>(7).unwrap(),
            Some(NOW_PLAYING_NOTIFICATION_TIMEOUT_MSEC)
        );
    }

    #[test]
    fn notification_artwork_path_becomes_a_file_uri() {
        let uri = now_playing_notification_artwork_uri(Path::new("/tmp/cover art.png"))
            .expect("absolute artwork path");

        assert_eq!(uri, "file:///tmp/cover%20art.png");
    }
}
