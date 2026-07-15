mod artwork;
mod library;
mod lyrics;
mod playback;
mod settings;
mod source;

use std::{rc::Rc, sync::Arc};

use super::root::bootstrap;

pub(crate) fn runtime_inputs() -> Result<ui::runtime::RuntimeInputs, String> {
    let (owners, receivers, source, playback) = bootstrap()?;
    let products = ui::runtime::ProductHandles {
        source: Arc::new(owners.source),
        library: Arc::new(owners.library),
        playback: playback::handles(owners.playback),
        artwork: Arc::new(owners.artwork),
        lyrics: Arc::new(owners.lyrics),
    };
    Ok(ui::runtime::RuntimeInputs {
        products,
        settings: Rc::new(owners.settings),
        receivers,
        source,
        playback,
    })
}
