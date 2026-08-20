//! Bounded persisted lyrics payloads.
//!
//! Lyrics owns selection and provider policy. Library owns only cache identity,
//! input compatibility, durable payloads, and the global size bound.

use crate::{Libraries, LibraryError, LibraryResult, SourceId, TrackId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LyricsCacheAuthority {
    Source,
    External,
}

impl LyricsCacheAuthority {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::External => "external",
        }
    }

    pub(crate) fn from_stored(value: &str) -> Option<Self> {
        match value {
            "source" => Some(Self::Source),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LyricsCacheKey {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub role: String,
    pub language: String,
    pub script: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsCacheInput {
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedLyrics {
    pub key: LyricsCacheKey,
    pub authority: LyricsCacheAuthority,
    pub input: LyricsCacheInput,
    pub payload: String,
    pub cached_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsCacheWrite {
    pub key: LyricsCacheKey,
    pub authority: LyricsCacheAuthority,
    pub input: LyricsCacheInput,
    pub payload: String,
    pub cached_at: i64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LyricsCacheTrim {
    pub rows_removed: u64,
    pub bytes_removed: u64,
}

impl Libraries {
    pub fn cached_lyrics(
        &self,
        key: LyricsCacheKey,
        expected_input: LyricsCacheInput,
    ) -> LibraryResult<Option<CachedLyrics>> {
        validate_key(&key)?;
        Ok(self.store.cached_lyrics(key, expected_input)?)
    }

    pub fn store_lyrics(&self, write: LyricsCacheWrite) -> LibraryResult<LyricsCacheTrim> {
        validate_key(&write.key)?;
        if write.cached_at < 0 {
            return Err(LibraryError::Persistence(
                "lyrics cache input is invalid".to_string(),
            ));
        }
        Ok(self.store.store_lyrics(write)?)
    }

    pub fn remove_lyrics_if_authority(
        &self,
        key: LyricsCacheKey,
        authority: LyricsCacheAuthority,
    ) -> LibraryResult<bool> {
        validate_key(&key)?;
        Ok(self.store.remove_lyrics_if_authority(key, authority)?)
    }

    pub fn remove_track_lyrics_by_authority(
        &self,
        source_id: SourceId,
        track_id: TrackId,
        authority: LyricsCacheAuthority,
    ) -> LibraryResult<u64> {
        Ok(self
            .store
            .remove_track_lyrics_by_authority(source_id, track_id, authority)?)
    }
}

fn validate_key(key: &LyricsCacheKey) -> LibraryResult<()> {
    if key.role.is_empty()
        || key.role.len() > 64
        || key.language.len() > 64
        || key.script.len() > 64
        || key.role.trim() != key.role
        || key.language.trim() != key.language
        || key.script.trim() != key.script
    {
        return Err(LibraryError::Persistence(
            "lyrics cache identity is invalid".to_string(),
        ));
    }
    Ok(())
}
