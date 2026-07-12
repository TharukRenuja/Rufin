use gtk::glib::prelude::*;

use super::*;

const APP_ID: &str = "io.github.screwys.Rufin";
const APP_NAME: &str = "Rufin";
const DBUS_TIMEOUT_MSEC: i32 = 1_000;
const NOTIFICATIONS_BUS_NAME: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const NOW_PLAYING_NOTIFICATION_ID: &str = "now-playing";
const NOW_PLAYING_NOTIFICATION_TIMEOUT_MSEC: i32 = -1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) struct NativeNowPlayingNotification {
    pub(in crate::ui) title: String,
    pub(in crate::ui) body: String,
    pub(in crate::ui) artwork_uri: Option<String>,
}

impl Shell {
    pub(in crate::ui) fn notify_now_playing(self: &Rc<Self>, player: Option<&PlaybackView>) {
        self.notify_now_playing_inner(player, false);
    }

    pub(in crate::ui) fn refresh_now_playing_notification(
        self: &Rc<Self>,
        player: Option<&PlaybackView>,
    ) {
        self.notify_now_playing_inner(player, true);
    }

    fn notify_now_playing_inner(
        self: &Rc<Self>,
        player: Option<&PlaybackView>,
        refresh_current: bool,
    ) {
        let settings = self.state.settings.borrow().clone();
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
        if !refresh_current && self.state.now_playing_notification_run.get() == Some(run) {
            return;
        }
        self.state.now_playing_notification_run.set(Some(run));
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
                let settings = shell.state.settings.borrow();
                crate::external_activity::notifications(&settings)
            };
            if !notifications_enabled {
                shell.withdraw_now_playing_notification();
                return;
            }
            if !now_playing_notification_matches_current(shell.state.player.borrow().as_ref(), run)
            {
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
            let replaces_id = shell.state.now_playing_native_notification_id.get();
            match send_native_now_playing_notification(&native_notification, replaces_id).await {
                Ok(notification_id) => {
                    if now_playing_notification_is_still_sendable(&shell, run) {
                        shell
                            .application
                            .withdraw_notification(NOW_PLAYING_NOTIFICATION_ID);
                        shell
                            .state
                            .now_playing_native_notification_id
                            .set(notification_id);
                    } else {
                        close_native_now_playing_notification(notification_id).await;
                    }
                }
                Err(error) => {
                    warn!(%error, "failed to send native now-playing notification");
                    if replaces_id != 0 {
                        shell.state.now_playing_native_notification_id.set(0);
                        close_native_now_playing_notification(replaces_id).await;
                    }
                    if now_playing_notification_is_still_sendable(&shell, run) {
                        send_gio_now_playing_notification(
                            &shell.application,
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

    pub(in crate::ui) fn withdraw_now_playing_notification(&self) {
        self.application
            .withdraw_notification(NOW_PLAYING_NOTIFICATION_ID);
        let notification_id = self.state.now_playing_native_notification_id.replace(0);
        self.state.now_playing_notification_run.set(None);
        if notification_id != 0 {
            glib::spawn_future_local(async move {
                close_native_now_playing_notification(notification_id).await;
            });
        }
    }
}

fn now_playing_notification_is_still_sendable(shell: &Shell, run: playback::RunId) -> bool {
    let notifications_enabled = {
        let settings = shell.state.settings.borrow();
        crate::external_activity::notifications(&settings)
    };
    notifications_enabled
        && now_playing_notification_matches_current(shell.state.player.borrow().as_ref(), run)
}

pub(in crate::ui) fn now_playing_notification_artwork_uri(path: &Path) -> Option<String> {
    glib::filename_to_uri(path, None)
        .ok()
        .map(|uri| uri.to_string())
}

pub(in crate::ui) fn now_playing_notification_hints(
    artwork_uri: Option<&str>,
) -> glib::VariantDict {
    let hints = glib::VariantDict::new(None);
    hints.insert("desktop-entry", APP_ID);
    hints.insert("transient", true);
    if let Some(uri) = artwork_uri {
        hints.insert("image-path", uri);
        hints.insert("image_path", uri);
    }
    hints
}

pub(in crate::ui) fn now_playing_notification_parameters(
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

pub(in crate::ui) fn now_playing_notification_can_send(
    settings: &StoredSettings,
    player: Option<&PlaybackView>,
) -> bool {
    crate::external_activity::notifications(settings)
        && player.is_some_and(|player| {
            matches!(
                player.transport.state,
                TransportStatus::Playing | TransportStatus::Buffering
            ) && player.transport.current.is_some()
        })
}

pub(in crate::ui) fn now_playing_notification_should_withdraw(
    settings: &StoredSettings,
    player: Option<&PlaybackView>,
) -> bool {
    !crate::external_activity::notifications(settings)
        || player.is_none_or(|player| {
            player.transport.current.is_none() || player.transport.state == TransportStatus::Stopped
        })
}

pub(in crate::ui) fn now_playing_notification_matches_current(
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
