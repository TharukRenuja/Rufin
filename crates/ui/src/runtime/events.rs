use async_channel::Receiver;

use super::source::DiscoveryUpdate;
use super::{ReleaseUpdate, SourceEvent, WaveformProjection};

pub struct ProductReceivers {
    pub source: Receiver<SourceEvent>,
    pub source_discovery: Receiver<DiscoveryUpdate>,
    pub waveform: Receiver<WaveformProjection>,
    pub lyrics: Receiver<lyrics::LyricsEvent>,
    pub release_updates: Receiver<ReleaseUpdate>,
}
