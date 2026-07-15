use std::path::PathBuf;

use metadata::{Lyrics, LyricsRequestKind, LyricsSearchResult};
use playback::MediaKey;
use ui::runtime::lyrics::LyricsPort;

use super::super::root::LyricsCommands;

impl LyricsPort for LyricsCommands {
    fn request(&self, media_key: MediaKey, kind: LyricsRequestKind) {
        self.request_lyrics_for_media(media_key, kind);
    }

    fn refresh_current(&self) {
        self.refresh_lyrics_for_current();
    }

    fn clear_remote_current(&self) {
        self.clear_remote_lyrics_for_current();
    }

    fn search_current(&self, artist_name: String, track_name: String) {
        self.search_lyrics_for_current(artist_name, track_name);
    }

    fn save_search_result(&self, media_key: MediaKey, result: LyricsSearchResult, path: PathBuf) {
        self.save_lyrics_search_result(media_key, result, path);
    }

    fn save_current(&self, media_key: MediaKey, lyrics: Lyrics, offset_millis: i64, path: PathBuf) {
        self.save_current_lyrics(media_key, lyrics, offset_millis, path);
    }

    fn preview_search_result(&self, media_key: MediaKey, result: LyricsSearchResult) {
        self.preview_lyrics_search_result(media_key, result);
    }

    fn accepts_generation(&self, generation: u64) -> bool {
        self.lyrics_result_is_current(generation)
    }
}
