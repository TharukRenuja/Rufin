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
    pub(in crate::ui) fn notify_now_playing(self: &Rc<Self>, snapshot: &PlaybackSnapshot) {
        let settings = self.state.settings.borrow().clone();
        if now_playing_notification_should_withdraw(&settings, snapshot) {
            self.withdraw_now_playing_notification();
            return;
        }
        if !now_playing_notification_can_send(&settings, snapshot) {
            return;
        }
        let Some(entry) = snapshot.current.as_ref() else {
            return;
        };
        let title = entry.title.clone();
        let body = format!("{} - {}", entry.artist, entry.album);
        let track_id = entry.track_id.clone();
        let artwork_lookup = self.current_playback_artwork_lookup(entry, THUMB_COVER_SIZE);
        let controller = self.controller.clone();
        let shell = Rc::clone(self);
        glib::spawn_future_local(async move {
            let artwork_path = match artwork_lookup {
                Some(lookup) => gtk::gio::spawn_blocking(move || {
                    let key_controller = controller.clone();
                    let fallback_controller = controller;
                    playback_artwork_path_from_lookup_context(
                        lookup,
                        |key| key_controller.cached_cover_path_for_key(key),
                        |image_ref, size| fallback_controller.cached_cover_path(image_ref, size),
                    )
                })
                .await
                .ok()
                .flatten(),
                None => None,
            };
            let notifications_enabled = {
                let settings = shell.state.settings.borrow();
                crate::external_activity::notifications(&settings)
            };
            if !notifications_enabled {
                shell.withdraw_now_playing_notification();
                return;
            }
            if !now_playing_notification_matches_current(&shell.state.player.borrow(), &track_id) {
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
                    if now_playing_notification_is_still_sendable(&shell, &track_id) {
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
                    if now_playing_notification_is_still_sendable(&shell, &track_id) {
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
        if notification_id != 0 {
            glib::spawn_future_local(async move {
                close_native_now_playing_notification(notification_id).await;
            });
        }
    }
}

fn now_playing_notification_is_still_sendable(shell: &Shell, track_id: &TrackId) -> bool {
    let notifications_enabled = {
        let settings = shell.state.settings.borrow();
        crate::external_activity::notifications(&settings)
    };
    notifications_enabled
        && now_playing_notification_matches_current(&shell.state.player.borrow(), track_id)
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
    settings: &AppSettings,
    snapshot: &PlaybackSnapshot,
) -> bool {
    crate::external_activity::notifications(settings)
        && matches!(
            snapshot.state,
            PlaybackState::Playing | PlaybackState::Buffering
        )
        && snapshot.current.is_some()
}

pub(in crate::ui) fn now_playing_notification_should_withdraw(
    settings: &AppSettings,
    snapshot: &PlaybackSnapshot,
) -> bool {
    !crate::external_activity::notifications(settings)
        || snapshot.current.is_none()
        || snapshot.state == PlaybackState::Stopped
}

pub(in crate::ui) fn now_playing_notification_matches_current(
    snapshot: &PlaybackSnapshot,
    track_id: &TrackId,
) -> bool {
    matches!(
        snapshot.state,
        PlaybackState::Playing | PlaybackState::Buffering
    ) && snapshot
        .current
        .as_ref()
        .is_some_and(|current| current.track_id == track_id.clone())
}
