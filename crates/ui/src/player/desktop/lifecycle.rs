use adw::prelude::*;

use playback::TransportHandle;

pub(crate) fn install_playback_shutdown(
    application: &adw::Application,
    transport: &TransportHandle,
) {
    let transport = transport.clone();
    application.connect_shutdown(move |_| transport.shutdown());
}
