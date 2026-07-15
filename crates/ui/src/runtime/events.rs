use async_channel::Receiver;

use sources::SourcePresentationState;

pub struct ProductReceivers {
    pub source_presentation: Receiver<SourcePresentationState>,
    pub source_local_access: Receiver<sources::SourceLocalAccessPresentation>,
    pub source_selection: Receiver<sources::SourceSelectionChanged>,
    pub source_discovery: Receiver<sources::ServerDiscoveryUpdate>,
    pub source_notice: Receiver<sources::SourceNotice>,
    pub source_transition_failure: Receiver<sources::SourceTransitionFailed>,
    pub library_sync: Receiver<library_sync::LibrarySyncEvent>,
    pub library_fact: Receiver<library::LibraryEvent>,
    pub playback_projection: Receiver<playback::PlaybackProjection>,
    pub waveform: Receiver<playback::WaveformProjection>,
    pub metadata_lyrics: Receiver<metadata::LyricsEvent>,
    pub artwork: Receiver<artwork::ArtworkEvent>,
}
