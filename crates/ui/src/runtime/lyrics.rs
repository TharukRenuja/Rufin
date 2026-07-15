use std::path::PathBuf;
use std::sync::Arc;

use metadata::{Lyrics, LyricsRequestKind, LyricsSearchResult};
use playback::MediaKey;

pub trait LyricsPort: Send + Sync {
    fn request(&self, media_key: MediaKey, kind: LyricsRequestKind);
    fn refresh_current(&self);
    fn clear_remote_current(&self);
    fn search_current(&self, artist_name: String, track_name: String);
    fn save_search_result(&self, media_key: MediaKey, result: LyricsSearchResult, path: PathBuf);
    fn save_current(&self, media_key: MediaKey, lyrics: Lyrics, offset_millis: i64, path: PathBuf);
    fn preview_search_result(&self, media_key: MediaKey, result: LyricsSearchResult);
    fn accepts_generation(&self, generation: u64) -> bool;
}

pub type LyricsHandle = Arc<dyn LyricsPort>;
