use playback::PlaybackProjection;
use sources::SourcePresentationState;

use crate::SettingsHandle;

use super::{ProductHandles, ProductReceivers};

pub struct RuntimeInputs {
    pub products: ProductHandles,
    pub settings: SettingsHandle,
    pub receivers: ProductReceivers,
    pub source: SourcePresentationState,
    pub playback: Option<PlaybackProjection>,
}
