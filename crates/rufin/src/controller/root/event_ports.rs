use super::*;
use async_channel::{Receiver, unbounded};

#[derive(Clone)]
pub(in crate::controller) struct SourceEventSenders {
    pub(in crate::controller) presentation: Sender<SourcePresentationState>,
    pub(in crate::controller) local_access: Sender<sources::SourceLocalAccessPresentation>,
    pub(in crate::controller) selection: Sender<sources::SourceSelectionChanged>,
    pub(in crate::controller) discovery: Sender<sources::ServerDiscoveryUpdate>,
    pub(in crate::controller) notice: Sender<sources::SourceNotice>,
    pub(in crate::controller) transition_failure: Sender<sources::SourceTransitionFailed>,
    pub(in crate::controller) sync: Sender<library_sync::LibrarySyncEvent>,
}

#[derive(Clone)]
pub(in crate::controller) struct PlaybackEventSenders {
    pub(in crate::controller) projection: Sender<playback::PlaybackProjection>,
    pub(in crate::controller) waveform: Sender<playback::WaveformProjection>,
}

pub(in crate::controller) type LibraryEventSender = Sender<library::LibraryEvent>;
pub(in crate::controller) type LyricsEventSender = Sender<metadata::LyricsEvent>;

pub(in crate::controller) fn product_event_channels(
    artwork: Receiver<artwork::ArtworkEvent>,
) -> (
    SourceEventSenders,
    LibraryEventSender,
    PlaybackEventSenders,
    LyricsEventSender,
    ui::runtime::ProductReceivers,
) {
    let (source_presentation, source_presentation_receiver) = unbounded();
    let (source_local_access, source_local_access_receiver) = unbounded();
    let (source_selection, source_selection_receiver) = unbounded();
    let (source_discovery, source_discovery_receiver) = unbounded();
    let (source_notice, source_notice_receiver) = unbounded();
    let (source_transition_failure, source_transition_failure_receiver) = unbounded();
    let (library_sync, library_sync_receiver) = unbounded();
    let (library_fact, library_fact_receiver) = unbounded();
    let (playback_projection, playback_projection_receiver) = unbounded();
    let (waveform, waveform_receiver) = unbounded();
    let (metadata_lyrics, metadata_lyrics_receiver) = unbounded();
    (
        SourceEventSenders {
            presentation: source_presentation,
            local_access: source_local_access,
            selection: source_selection,
            discovery: source_discovery,
            notice: source_notice,
            transition_failure: source_transition_failure,
            sync: library_sync,
        },
        library_fact,
        PlaybackEventSenders {
            projection: playback_projection,
            waveform,
        },
        metadata_lyrics,
        ui::runtime::ProductReceivers {
            source_presentation: source_presentation_receiver,
            source_local_access: source_local_access_receiver,
            source_selection: source_selection_receiver,
            source_discovery: source_discovery_receiver,
            source_notice: source_notice_receiver,
            source_transition_failure: source_transition_failure_receiver,
            library_sync: library_sync_receiver,
            library_fact: library_fact_receiver,
            playback_projection: playback_projection_receiver,
            waveform: waveform_receiver,
            metadata_lyrics: metadata_lyrics_receiver,
            artwork,
        },
    )
}
