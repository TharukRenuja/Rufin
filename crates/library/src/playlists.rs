//! Accepted manual playlist transactions.
//!
//! Local playlists are Rufin-owned ordered occurrences. Remote mutations are
//! accepted only from the source's exact affected-playlist readback. Revisions,
//! tombstones, and observation histories are deliberately absent.

use std::collections::HashSet;
use std::sync::Arc;

use crate::{
    AcceptedLibraryChange, Library, LibraryError, LibraryResult, LoadedLibrary, LoadedLibraryError,
    Playlist, PlaylistEntry, PlaylistId, PlaylistSnapshot, SourceLibraryUpdate, TrackId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaylistEdit {
    Create {
        name: String,
        track_ids: Vec<TrackId>,
    },
    Rename {
        playlist_id: PlaylistId,
        name: String,
    },
    Delete {
        playlist_id: PlaylistId,
    },
    AddTracks {
        playlist_id: PlaylistId,
        track_ids: Vec<TrackId>,
    },
    RemoveEntries {
        playlist_id: PlaylistId,
        occurrence_ids: Vec<String>,
    },
    MoveEntry {
        playlist_id: PlaylistId,
        occurrence_id: String,
        new_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaylistTrackAdd {
    pub playlist_id: PlaylistId,
    pub track_ids: Vec<TrackId>,
    pub skip_duplicates: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaylistAcceptance {
    RufinOwned(PlaylistEdit),
    SourceSnapshot(PlaylistSnapshot),
    SourceDeleted(PlaylistId),
}

impl Library {
    /// Resolves the user's duplicate policy against the accepted playlist.
    ///
    /// Existing occurrences are skipped when requested. Repeated tracks in
    /// the incoming order remain repeated because playlists preserve
    /// occurrences rather than collapsing them into a set.
    pub fn prepare_playlist_add(
        &self,
        loaded: &Arc<LoadedLibrary>,
        request: PlaylistTrackAdd,
    ) -> LibraryResult<Option<PlaylistEdit>> {
        require_tracks(loaded, &request.track_ids)?;
        let playlist = loaded
            .playlist(&request.playlist_id)?
            .ok_or_else(|| missing_playlist(&request.playlist_id))?;
        let track_ids = if request.skip_duplicates {
            let existing = playlist
                .entries
                .iter()
                .map(|entry| &entry.track_id)
                .collect::<HashSet<_>>();
            request
                .track_ids
                .into_iter()
                .filter(|track_id| !existing.contains(track_id))
                .collect()
        } else {
            request.track_ids
        };
        if track_ids.is_empty() {
            return Ok(None);
        }
        Ok(Some(PlaylistEdit::AddTracks {
            playlist_id: request.playlist_id,
            track_ids,
        }))
    }

    pub fn accept_playlist(
        &self,
        loaded: &Arc<LoadedLibrary>,
        acceptance: PlaylistAcceptance,
    ) -> LibraryResult<Option<AcceptedLibraryChange>> {
        match acceptance {
            PlaylistAcceptance::SourceSnapshot(snapshot) => self.accept_source_update(
                loaded,
                SourceLibraryUpdate {
                    playlists: vec![snapshot],
                    ..SourceLibraryUpdate::default()
                },
            ),
            PlaylistAcceptance::SourceDeleted(playlist_id) => self.accept_source_update(
                loaded,
                SourceLibraryUpdate {
                    removed_playlists: vec![playlist_id],
                    ..SourceLibraryUpdate::default()
                },
            ),
            PlaylistAcceptance::RufinOwned(edit) => self.accept_local_playlist(loaded, edit),
        }
    }

    fn accept_local_playlist(
        &self,
        loaded: &Arc<LoadedLibrary>,
        edit: PlaylistEdit,
    ) -> LibraryResult<Option<AcceptedLibraryChange>> {
        let result = match edit {
            PlaylistEdit::Create { name, track_ids } => {
                require_tracks(loaded, &track_ids)?;
                let playlist_id = PlaylistId::new(format!("rufin:playlist:{}", random_hex()?));
                let entries = entries_for_tracks(&playlist_id, &track_ids)?;
                let snapshot = PlaylistSnapshot {
                    playlist: Playlist {
                        id: playlist_id.clone(),
                        name: name.trim().to_string(),
                        image_ref: None,
                    },
                    entries,
                };
                self.replace_local_playlist_snapshot(loaded, snapshot)?
            }
            PlaylistEdit::Rename { playlist_id, name } => {
                let mut snapshot = local_playlist(loaded, &playlist_id)?;
                snapshot.playlist.name = name.trim().to_string();
                self.replace_local_playlist_snapshot(loaded, snapshot)?
            }
            PlaylistEdit::Delete { playlist_id } => {
                require_playlist(loaded, &playlist_id)?;
                self.store
                    .remove_local_playlist(loaded.source_id().clone(), playlist_id.clone())?;
                loaded.remove_playlist(&playlist_id)?;
                playlist_id
            }
            PlaylistEdit::AddTracks {
                playlist_id,
                track_ids,
            } => {
                require_tracks(loaded, &track_ids)?;
                let mut snapshot = local_playlist(loaded, &playlist_id)?;
                snapshot
                    .entries
                    .extend(entries_for_tracks(&playlist_id, &track_ids)?);
                self.replace_local_playlist_snapshot(loaded, snapshot)?
            }
            PlaylistEdit::RemoveEntries {
                playlist_id,
                occurrence_ids,
            } => {
                let mut snapshot = local_playlist(loaded, &playlist_id)?;
                let removed = occurrence_ids.into_iter().collect::<HashSet<_>>();
                snapshot
                    .entries
                    .retain(|entry| !removed.contains(&entry.occurrence_id));
                self.replace_local_playlist_snapshot(loaded, snapshot)?
            }
            PlaylistEdit::MoveEntry {
                playlist_id,
                occurrence_id,
                new_index,
            } => {
                let mut snapshot = local_playlist(loaded, &playlist_id)?;
                if let Some(old_index) = snapshot
                    .entries
                    .iter()
                    .position(|entry| entry.occurrence_id == occurrence_id)
                {
                    let entry = snapshot.entries.remove(old_index);
                    let new_index = new_index.min(snapshot.entries.len());
                    snapshot.entries.insert(new_index, entry);
                }
                self.replace_local_playlist_snapshot(loaded, snapshot)?
            }
        };
        Ok(Some(AcceptedLibraryChange {
            playlists: vec![result],
            ..AcceptedLibraryChange::default()
        }))
    }

    fn replace_local_playlist_snapshot(
        &self,
        loaded: &Arc<LoadedLibrary>,
        snapshot: PlaylistSnapshot,
    ) -> LibraryResult<PlaylistId> {
        let playlist_id = snapshot.playlist.id.clone();
        self.store
            .replace_local_playlist(loaded.source_id().clone(), snapshot.clone())?;
        loaded.replace_playlist(snapshot)?;
        Ok(playlist_id)
    }
}

fn local_playlist(
    loaded: &Arc<LoadedLibrary>,
    playlist_id: &PlaylistId,
) -> LibraryResult<PlaylistSnapshot> {
    let current = loaded
        .playlist(playlist_id)?
        .ok_or_else(|| missing_playlist(playlist_id))?;
    Ok(PlaylistSnapshot {
        playlist: current.playlist.as_ref().clone(),
        entries: current.entries.to_vec(),
    })
}

fn require_playlist(loaded: &Arc<LoadedLibrary>, playlist_id: &PlaylistId) -> LibraryResult<()> {
    loaded
        .playlist(playlist_id)?
        .ok_or_else(|| missing_playlist(playlist_id))
        .map(|_| ())
}

fn missing_playlist(playlist_id: &PlaylistId) -> LibraryError {
    LibraryError::Loaded(LoadedLibraryError::MissingItem {
        kind: "playlist",
        id: playlist_id.to_string(),
    })
}

fn require_tracks(loaded: &Arc<LoadedLibrary>, track_ids: &[TrackId]) -> LibraryResult<()> {
    for track_id in track_ids {
        if loaded.track(track_id)?.is_none() {
            return Err(LibraryError::Loaded(LoadedLibraryError::MissingItem {
                kind: "track",
                id: track_id.to_string(),
            }));
        }
    }
    Ok(())
}

fn entries_for_tracks(
    playlist_id: &PlaylistId,
    track_ids: &[TrackId],
) -> LibraryResult<Vec<PlaylistEntry>> {
    let batch = random_hex()?;
    track_ids
        .iter()
        .enumerate()
        .map(|(index, track_id)| {
            Ok(PlaylistEntry {
                occurrence_id: format!("{}:{batch}:{index}", playlist_id.as_str()),
                track_id: track_id.clone(),
            })
        })
        .collect()
}

fn random_hex() -> LibraryResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        LibraryError::Persistence(format!("could not create a playlist identity: {error}"))
    })?;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(value)
}
