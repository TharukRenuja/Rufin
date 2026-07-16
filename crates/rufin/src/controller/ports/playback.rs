use playback::{
    AlbumPlayRequest, ArtistWindowPlayRequest, CachedPlaylistPlayRequest, ContextPlayRequest,
    FolderWindowPlayRequest, LibraryWindowPlayRequest, PlaybackHandles, PlaylistEntryPlayRequest,
    QueueCommandPort as QueuePort, QueueReorderRequest, RadioCommandPort as RadioPort,
    RadioPlayRequest, RandomPlayRequest, SmartPlaylistPlayRequest,
    TransportCommandPort as TransportPort, WaveformCommandPort as WaveformPort,
};
use std::sync::Arc;

use super::super::root::PlaybackCommands;

pub(super) fn handles(controller: PlaybackCommands) -> PlaybackHandles {
    PlaybackHandles {
        transport: Arc::new(controller.clone()),
        queue: Arc::new(controller.clone()),
        radio: Arc::new(controller.clone()),
        waveform: Arc::new(controller),
    }
}

impl TransportPort for PlaybackCommands {
    fn play_pause(&self) {
        PlaybackCommands::play_pause(self);
    }

    fn play(&self) {
        PlaybackCommands::play(self);
    }

    fn pause(&self) {
        PlaybackCommands::pause(self);
    }

    fn stop(&self) {
        PlaybackCommands::stop(self);
    }

    fn next(&self) {
        self.next_track();
    }

    fn previous(&self) {
        self.previous_track();
    }

    fn seek_seconds(&self, seconds: u32) {
        self.seek(seconds);
    }

    fn seek_millis(&self, millis: u64) {
        PlaybackCommands::seek_millis(self, millis);
    }

    fn set_volume(&self, volume: f64) {
        PlaybackCommands::set_volume(self, volume);
    }

    fn persist_volume(&self, volume: f64) {
        PlaybackCommands::persist_volume(self, volume);
    }

    fn set_muted(&self, muted: bool) {
        PlaybackCommands::set_muted(self, muted);
    }

    fn toggle_shuffle(&self) {
        PlaybackCommands::toggle_shuffle(self);
    }

    fn set_shuffle(&self, enabled: bool) {
        PlaybackCommands::set_shuffle(self, enabled);
    }

    fn cycle_repeat(&self) {
        PlaybackCommands::cycle_repeat(self);
    }

    fn set_repeat(&self, repeat: playback::RepeatMode) {
        PlaybackCommands::set_repeat(self, repeat);
    }

    fn toggle_auto_dj(&self) {
        PlaybackCommands::toggle_auto_dj(self);
    }

    fn set_visualizer_enabled(&self, enabled: bool) {
        PlaybackCommands::set_visualizer_enabled(self, enabled);
    }

    fn available_audio_outputs(&self) -> Vec<playback::AudioOutput> {
        playback_gstreamer::available_audio_outputs()
    }

    fn poll_events(&self) {
        self.poll_playback_events();
    }

    fn shutdown(&self) {
        self.shutdown_playback();
    }
}

impl QueuePort for PlaybackCommands {
    fn play_tracks_now(&self, tracks: Vec<library::Track>) {
        PlaybackCommands::play_tracks_now(self, tracks);
    }

    fn play_now(&self, track: library::Track) {
        PlaybackCommands::play_now(self, track);
    }

    fn play_album(&self, request: AlbumPlayRequest) {
        PlaybackCommands::play_album(self, request);
    }

    fn play_playlist_entry(&self, request: PlaylistEntryPlayRequest) {
        PlaybackCommands::play_playlist_entry(self, request);
    }

    fn play_cached_playlist(&self, request: CachedPlaylistPlayRequest) {
        PlaybackCommands::play_cached_playlist(self, request);
    }

    fn play_smart_playlist(&self, request: SmartPlaylistPlayRequest) {
        PlaybackCommands::play_smart_playlist(self, request);
    }

    fn play_library_window(&self, request: LibraryWindowPlayRequest) -> bool {
        PlaybackCommands::play_library_window(self, request)
    }

    fn play_folder_window(&self, request: FolderWindowPlayRequest) -> bool {
        PlaybackCommands::play_folder_window(self, request)
    }

    fn play_artist_window(&self, request: ArtistWindowPlayRequest) -> bool {
        PlaybackCommands::play_artist_window(self, request)
    }

    fn play_context(&self, request: ContextPlayRequest) -> bool {
        PlaybackCommands::play_context(self, request)
    }

    fn play_next(&self, track: library::Track) {
        PlaybackCommands::play_next(self, track);
    }

    fn play_last(&self, tracks: Vec<library::Track>) {
        PlaybackCommands::play_last(self, tracks);
    }

    fn remove(&self, occurrence: playback::OccurrenceId) {
        self.remove_from_queue(occurrence);
    }

    fn activate(&self, occurrence: playback::OccurrenceId) {
        self.activate_queue_entry(occurrence);
    }

    fn move_after_current(&self, occurrence: playback::OccurrenceId) {
        self.move_queue_entry_after_current(occurrence);
    }

    fn reorder(&self, request: QueueReorderRequest) {
        self.reorder_queue_entry(request.occurrence, request.target_index, request.after);
    }

    fn clear(&self) {
        self.clear_queue();
    }

    fn request_page(&self, query: playback::QueuePageQuery) -> Option<playback::QueuePage> {
        self.request_queue_page(query)
    }
}

impl RadioPort for PlaybackCommands {
    fn random_track_domain(&self) -> Option<sources::RandomTrackDomain> {
        PlaybackCommands::random_track_domain(self)
    }

    fn play_random(&self, request: RandomPlayRequest) {
        PlaybackCommands::play_random(self, request);
    }

    fn manual_radio_supported(&self, kind: sources::GeneratedTrackSeedKind) -> bool {
        PlaybackCommands::manual_radio_supported(self, kind)
    }

    fn play_radio(&self, request: RadioPlayRequest) {
        PlaybackCommands::play_radio(self, request);
    }
}

impl WaveformPort for PlaybackCommands {
    fn request_current(&self) {
        self.request_waveform_for_current();
    }

    fn warm_queue(&self) {
        self.warm_waveforms_for_queue();
    }
}
