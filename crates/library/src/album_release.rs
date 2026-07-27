//! Exact Album release lookup state.
//!
//! Source Album facts remain canonical. A matching found result is overlaid on
//! hydration or patched into the selected LoadedLibrary; missing results only
//! prevent repeated lookup of the same exact identity.

use std::sync::Arc;

use crate::{
    AcceptedLibraryChange, AlbumId, Library, LibraryError, LibraryResult, LoadedLibrary, SourceId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumReleaseIdentity {
    ReleaseGroup(String),
    Release(String),
}

impl AlbumReleaseIdentity {
    pub(crate) fn stored_key(&self) -> String {
        match self {
            Self::ReleaseGroup(id) => format!("release-group:{id}"),
            Self::Release(id) => format!("release:{id}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumReleaseCandidate {
    pub source_id: SourceId,
    pub album_id: AlbumId,
    pub title: String,
    pub artist: String,
    pub identity: AlbumReleaseIdentity,
    pub(crate) library_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumReleaseResult {
    Found {
        release_types: Vec<String>,
        is_compilation: Option<bool>,
    },
    Missing,
}

impl Library {
    pub fn take_album_release_lookups(
        &self,
        loaded: &Arc<LoadedLibrary>,
        limit: usize,
    ) -> LibraryResult<Vec<AlbumReleaseCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let source_id = loaded.source_id().clone();
        let library_id = loaded.library_id();
        let state = loaded.read_state()?;
        let mut album_ids = state
            .unresolved_album_releases
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        album_ids.sort();
        let mut candidates = Vec::new();
        for album_id in album_ids.into_iter().take(limit.min(500)) {
            let Some(album) = state.albums.get(&album_id) else {
                continue;
            };
            if !album.release_types.is_empty() {
                continue;
            }
            let Some(identity) = release_identity(album) else {
                continue;
            };
            candidates.push(AlbumReleaseCandidate {
                source_id: source_id.clone(),
                album_id: album.id.clone(),
                title: album.title.clone(),
                artist: album.artist.clone(),
                identity,
                library_id,
            });
        }
        Ok(candidates)
    }

    pub fn accept_album_release_result(
        &self,
        loaded: &Arc<LoadedLibrary>,
        candidate: AlbumReleaseCandidate,
        result: AlbumReleaseResult,
    ) -> LibraryResult<Option<AcceptedLibraryChange>> {
        if loaded.source_id() != &candidate.source_id || loaded.library_id() != candidate.library_id
        {
            return Ok(None);
        }
        if matches!(
            &result,
            AlbumReleaseResult::Found { release_types, .. }
                if release_types.is_empty()
                    || release_types.iter().any(|value| value.trim().is_empty())
        ) {
            return Err(LibraryError::Persistence(
                "found Album release result cannot be empty".to_string(),
            ));
        }
        let accepted = self
            .store
            .accept_album_release(candidate.clone(), result.clone())?;
        if !accepted {
            return Ok(None);
        }
        loaded.mark_album_release_resolved(&candidate.album_id)?;
        let AlbumReleaseResult::Found {
            release_types,
            is_compilation,
        } = result
        else {
            return Ok(None);
        };
        Ok(Some(loaded.replace_album_release(
            &candidate.album_id,
            release_types,
            is_compilation,
        )?))
    }
}

pub(crate) fn release_identity(album: &crate::Album) -> Option<AlbumReleaseIdentity> {
    album
        .musicbrainz_release_group_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .cloned()
        .map(AlbumReleaseIdentity::ReleaseGroup)
        .or_else(|| {
            album
                .musicbrainz_album_id
                .as_ref()
                .filter(|id| !id.is_empty())
                .cloned()
                .map(AlbumReleaseIdentity::Release)
        })
}
