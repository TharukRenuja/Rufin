use async_channel::Receiver;

use super::source::DiscoveryUpdate;
use super::{PlaybackPublication, ReleaseUpdate, SourceEvent, WaveformProjection};

pub struct ProductReceivers {
    pub source: Receiver<SourceEvent>,
    pub source_discovery: Receiver<DiscoveryUpdate>,
    pub downloads: Receiver<downloads::DownloadEvent>,
    pub playback: Receiver<PlaybackPublication>,
    pub waveform: Receiver<WaveformProjection>,
    pub lyrics: Receiver<lyrics::LyricsEvent>,
    pub release_updates: Receiver<ReleaseUpdate>,
}
