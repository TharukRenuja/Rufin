pub(crate) mod lifecycle;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use playback::RunId;
#[cfg(unix)]
mod mpris;
mod notification;
#[cfg(unix)]
mod tray;

#[cfg(unix)]
pub(crate) use mpris::{MprisAdapter, install_mpris};
pub(crate) use notification::{
    now_playing_notification_can_send, now_playing_notification_should_withdraw,
};
#[cfg(unix)]
pub(crate) use tray::{TrayHandle, install_tray, present_initial_window};

pub(crate) struct DesktopState {
    #[cfg(unix)]
    pub(crate) mpris: Rc<MprisAdapter>,
    pub(crate) notification_id: Cell<u32>,
    pub(crate) notification_run: Cell<Option<RunId>>,
    #[cfg(unix)]
    pub(crate) tray: RefCell<Option<TrayHandle>>,
    #[cfg(unix)]
    pub(crate) tray_command_source: RefCell<Option<gtk::glib::SourceId>>,
}
