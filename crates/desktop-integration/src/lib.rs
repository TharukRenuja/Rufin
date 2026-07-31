//! Desktop protocols driven by Rufin's accepted playback projection.
//!
//! Each integration keeps only the state required by its external protocol.
//! GTK window policy and Rufin's current-media authority remain with their
//! existing owners.

mod discord;
mod media_controls;
mod notification;
mod tray;

pub use discord::{DEFAULT_CLIENT_ID, Discord, DisplayType, LinkType, Settings};
pub use media_controls::MediaControls;
pub use notification::{
    Notifications, now_playing_notification_can_send, now_playing_notification_should_withdraw,
};
pub use tray::{Tray, TrayIntent};
