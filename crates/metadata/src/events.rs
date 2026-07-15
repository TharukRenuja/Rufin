use std::path::PathBuf;

use playback::MediaKey;

use crate::{Lyrics, LyricsSearchResult};

#[derive(Clone, Debug)]
pub enum LyricsEvent {
    Loaded {
        media_key: MediaKey,
        generation: u64,
        lyrics: Box<Option<Lyrics>>,
    },
    SearchResults {
        media_key: MediaKey,
        generation: u64,
        artist_name: String,
        track_name: String,
        results: Vec<LyricsSearchResult>,
    },
    SearchFailed {
        media_key: MediaKey,
        generation: u64,
        artist_name: String,
        track_name: String,
        error: String,
    },
    Saved {
        media_key: MediaKey,
        generation: u64,
        path: PathBuf,
        lyrics: Lyrics,
    },
    FileSaved {
        media_key: MediaKey,
        generation: u64,
        path: PathBuf,
    },
}
