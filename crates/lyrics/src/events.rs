use std::path::PathBuf;
use std::sync::Arc;

use playback::CurrentMediaId;

use crate::{LyricsDocument, LyricsQuery, LyricsSearchResult};

#[derive(Clone, Debug)]
pub enum CurrentLyrics {
    Cleared,
    Loading {
        media_id: CurrentMediaId,
    },
    Ready {
        media_id: CurrentMediaId,
        document: Option<Arc<LyricsDocument>>,
    },
}

impl Default for CurrentLyrics {
    fn default() -> Self {
        Self::Cleared
    }
}

#[derive(Clone, Debug)]
pub enum LyricsEvent {
    Current(CurrentLyrics),
    SearchFinished {
        media_id: CurrentMediaId,
        query: LyricsQuery,
        result: Result<Vec<LyricsSearchResult>, String>,
    },
    Saved {
        media_id: CurrentMediaId,
        path: PathBuf,
    },
}
