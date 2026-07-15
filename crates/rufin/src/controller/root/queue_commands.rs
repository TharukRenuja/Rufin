use std::sync::Arc;

use library::play_context::{
    PlayContext, PlayContextAnchor, PlayContextDescriptor, PlayContextItem, PlayContextOrder,
    PlaylistSort, TrackFilter, context_id, smart_playlist_definition_fingerprint,
};
use library::{MusicFolderId, PlaylistId, SourceId, Track, TrackId};
use playback::{
    AlbumPlayRequest, ArtistWindowPlayRequest, Batch, BatchItem, CachedPlaylistPlayRequest,
    FolderWindowPlayRequest, GenreWindowPlayRequest, LibraryWindowPlayRequest, MaterializationId,
    MoodWindowPlayRequest, OccurrenceId, Placement, PlaylistEntryPlayRequest, Provenance,
    QueuePlacement, RepeatMode, SessionCommand, SmartPlaylistPlayRequest,
};
use tracing::warn;

use super::{PlaybackCommands, PlaybackProduct, StoreHandle, shuffle_seed};

#[derive(Clone)]
pub(in crate::controller) struct ReservedQueueMaterialization {
    product: Arc<PlaybackProduct>,
    id: MaterializationId,
    source_id: SourceId,
    placement: Placement,
    current_track_id: Option<TrackId>,
}

impl ReservedQueueMaterialization {
    pub(in crate::controller) fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub(in crate::controller) fn current_track_id(&self) -> Option<&TrackId> {
        self.current_track_id.as_ref()
    }
}

impl PlaybackCommands {
    pub(in crate::controller) fn reserve_queue_materialization(
        &self,
        placement: Placement,
    ) -> Result<ReservedQueueMaterialization, String> {
        let product = self.playback_product()?;
        let reservation = product.reserve_materialization(placement)?;
        Ok(ReservedQueueMaterialization {
            product,
            id: reservation.id,
            source_id: reservation.source_id,
            placement,
            current_track_id: reservation.current_track_id,
        })
    }

    pub(in crate::controller) fn apply_reserved_tracks(
        &self,
        reservation: ReservedQueueMaterialization,
        tracks: Vec<Track>,
        provenance: Provenance,
    ) -> Result<bool, String> {
        let items = tracks
            .into_iter()
            .map(|track| BatchItem::new(track, provenance.clone()))
            .collect();
        self.apply_reserved_batch(reservation, items, false)
    }

    pub(in crate::controller) fn reject_queue_materialization(
        &self,
        reservation: ReservedQueueMaterialization,
        error: impl Into<String>,
    ) {
        if reservation.product.fail_materialization(
            reservation.id,
            &reservation.source_id,
            reservation.placement,
        ) {
            self.queue_error(error);
        }
    }

    pub fn play_tracks_now(&self, tracks: Vec<Track>) {
        self.apply_tracks(
            tracks,
            Placement::Replace { anchor_index: 0 },
            Provenance::Manual,
        );
    }

    pub fn play_now(&self, track: Track) {
        self.play_tracks_now(vec![track]);
    }

    pub fn play_album(&self, request: AlbumPlayRequest) {
        let Some(anchor_track) = request.tracks.get(request.anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return;
        };
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Album {
                album_id: request.album_id,
                music_folder_id: Self::active_music_folder(&self.store),
            },
            order: PlayContextOrder::Canonical,
        };
        let anchor = PlayContextAnchor {
            track_id: anchor_track.id.clone(),
            source_rank: request.anchor_index,
            source_item_id: None,
        };
        self.play_store_context(context, anchor, request.shuffled_start);
    }

    pub fn play_playlist_entry(&self, request: PlaylistEntryPlayRequest) {
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Playlist {
                playlist_id: request.playlist_id,
            },
            order: PlayContextOrder::Playlist {
                query: request.query,
                sort: request.sort,
                descending: request.descending,
            },
        };
        let anchor = PlayContextAnchor {
            track_id: request.entry.track.id,
            source_rank: request.source_index,
            source_item_id: Some(request.entry.entry_id),
        };
        self.play_store_context(context, anchor, request.shuffled_start);
    }

    pub fn play_cached_playlist(&self, request: CachedPlaylistPlayRequest) {
        let shuffled_start = request.placement == QueuePlacement::Now;
        self.play_cached_playlist_at(
            request.playlist_id,
            request.placement.into(),
            shuffled_start,
        );
    }

    pub fn play_smart_playlist(&self, request: SmartPlaylistPlayRequest) {
        let context = PlayContext {
            descriptor: PlayContextDescriptor::SmartPlaylist {
                smart_playlist_id: request.playlist.id,
                definition_fingerprint: smart_playlist_definition_fingerprint(
                    &request.playlist.definition,
                ),
                music_folder_id: request.music_folder_id,
            },
            order: PlayContextOrder::SmartPlaylist,
        };
        let Some(track_id) = request.anchor_track_id else {
            self.queue_error("No tracks are available to play.");
            return;
        };
        let anchor = PlayContextAnchor {
            track_id,
            source_rank: 0,
            source_item_id: None,
        };
        self.play_store_context(context, anchor, true);
    }

    pub fn play_library_window(&self, mut request: LibraryWindowPlayRequest) -> bool {
        if request.total_items == 0 || request.anchor_index >= request.total_items {
            self.queue_error("The selected track is no longer available.");
            return false;
        }
        let Some(track) = (request.track_at)(request.anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        let order = if matches!(
            &request.descriptor,
            PlayContextDescriptor::SmartPlaylist { .. }
        ) {
            PlayContextOrder::SmartPlaylist
        } else {
            PlayContextOrder::Tracks {
                filter: TrackFilter {
                    query: source_query(&request.query),
                    favorites_only: request.favorites_only,
                },
                sort: request.sort,
                descending: request.descending,
                favorite_first: request.favorite_first,
            }
        };
        let context = PlayContext {
            descriptor: request.descriptor,
            order,
        };
        let anchor = PlayContextAnchor {
            track_id: track.id,
            source_rank: request.anchor_index,
            source_item_id: None,
        };
        self.play_store_context(context, anchor, false)
    }

    pub fn play_folder_window(&self, request: FolderWindowPlayRequest) -> bool {
        let Some(anchor_track) = request.tracks.get(request.anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Folder {
                path: request.path,
                music_folder_id: Self::active_music_folder(&self.store),
            },
            order: PlayContextOrder::Tracks {
                filter: TrackFilter {
                    query: source_query(&request.query),
                    favorites_only: false,
                },
                sort: request.sort,
                descending: request.descending,
                favorite_first: false,
            },
        };
        let anchor = PlayContextAnchor {
            track_id: anchor_track.id.clone(),
            source_rank: request.anchor_index,
            source_item_id: None,
        };
        self.play_loaded_context(context, request.tracks, anchor, false)
    }

    pub fn play_artist_window(&self, mut request: ArtistWindowPlayRequest) -> bool {
        let Some(anchor) = context_anchor(
            request.total_items,
            request.anchor_index,
            &mut request.track_at,
        ) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Artist {
                    artist_id: request.artist_id,
                    scope: request.scope,
                    music_folder_id: Self::active_music_folder(&self.store),
                },
                order: PlayContextOrder::Canonical,
            },
            anchor,
            true,
        )
    }

    pub fn play_genre_window(&self, mut request: GenreWindowPlayRequest) -> bool {
        let Some(anchor) = context_anchor(
            request.total_items,
            request.anchor_index,
            &mut request.track_at,
        ) else {
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Genre {
                    genre_id: request.genre_id,
                    music_folder_id: Self::active_music_folder(&self.store),
                },
                order: PlayContextOrder::Canonical,
            },
            anchor,
            true,
        )
    }

    pub fn play_mood_window(&self, mut request: MoodWindowPlayRequest) -> bool {
        let Some(anchor) = context_anchor(
            request.total_items,
            request.anchor_index,
            &mut request.track_at,
        ) else {
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Mood {
                    mood_id: request.mood_id,
                    music_folder_id: Self::active_music_folder(&self.store),
                },
                order: PlayContextOrder::Canonical,
            },
            anchor,
            true,
        )
    }

    pub fn play_next(&self, track: Track) {
        self.apply_tracks(vec![track], Placement::AfterCurrent, Provenance::Manual);
    }

    pub fn play_last(&self, tracks: Vec<Track>) {
        self.apply_tracks(tracks, Placement::End, Provenance::Manual);
    }

    pub fn remove_from_queue(&self, occurrence: OccurrenceId) {
        self.send_session_command(SessionCommand::Remove(occurrence));
    }

    pub fn activate_queue_entry(&self, occurrence: OccurrenceId) {
        self.send_session_command(SessionCommand::Activate(occurrence));
    }

    pub fn move_queue_entry_after_current(&self, occurrence: OccurrenceId) {
        self.send_session_command(SessionCommand::MoveAfterCurrent(occurrence));
    }

    pub fn reorder_queue_entry(&self, occurrence: OccurrenceId, target_index: usize, after: bool) {
        self.send_session_command(SessionCommand::Reorder {
            occurrence,
            target_index,
            after,
        });
    }

    pub fn clear_queue(&self) {
        self.send_session_command(SessionCommand::ClearUpcoming);
    }

    pub fn toggle_shuffle(&self) {
        self.send_session_command(SessionCommand::ToggleShuffle {
            seed: shuffle_seed(),
        });
    }

    pub fn set_shuffle(&self, enabled: bool) {
        self.send_session_command(SessionCommand::SetShuffle {
            enabled,
            seed: shuffle_seed(),
        });
    }

    pub fn set_repeat(&self, repeat: RepeatMode) {
        self.send_session_command(SessionCommand::SetRepeat(repeat));
    }

    pub fn cycle_repeat(&self) {
        self.send_session_command(SessionCommand::CycleRepeat);
    }

    fn play_cached_playlist_at(
        &self,
        playlist_id: PlaylistId,
        placement: Placement,
        shuffled_start: bool,
    ) {
        let reservation = match self.reserve_queue_materialization(placement) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.queue_error(error);
                return;
            }
        };
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Playlist {
                playlist_id: playlist_id.clone(),
            },
            order: PlayContextOrder::Playlist {
                query: None,
                sort: PlaylistSort::Position,
                descending: false,
            },
        };
        let context_id = context_id(&context);
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            let result = controller.load_playlist_context_items(
                &reservation.source_id,
                &playlist_id,
                &context_id,
            );
            match result {
                Ok(items) => {
                    if let Err(error) =
                        controller.apply_reserved_batch(reservation.clone(), items, shuffled_start)
                    {
                        controller.reject_queue_materialization(reservation, error);
                    }
                }
                Err(error) => controller.reject_queue_materialization(reservation, error),
            }
        }) {
            self.reject_queue_materialization(rejected, error);
        }
    }

    fn load_playlist_context_items(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        context_id: &str,
    ) -> Result<Vec<BatchItem>, String> {
        let detail = self
            .store
            .with_store(|store| store.load_playlist_detail(source_id, playlist_id))?
            .ok_or_else(|| "The selected cached playlist was not found.".to_string())?;
        if detail.entries.is_empty() {
            return Err("No tracks are available to add to the queue.".to_string());
        }
        let tracks = detail
            .entries
            .iter()
            .map(|entry| entry.track.clone())
            .collect::<Vec<_>>();
        Ok(tracks
            .into_iter()
            .enumerate()
            .map(|(source_rank, track)| {
                BatchItem::new(
                    track,
                    Provenance::Context {
                        context_id: context_id.to_string(),
                        source_rank,
                    },
                )
            })
            .collect())
    }

    fn play_store_context(
        &self,
        context: PlayContext,
        anchor: PlayContextAnchor,
        shuffled_start: bool,
    ) -> bool {
        let context_id = context_id(&context);
        if !shuffled_start && self.activate_context_occurrence(&context_id, &anchor) {
            return true;
        }
        let reservation = match self.reserve_queue_materialization(Placement::Replace {
            anchor_index: anchor.source_rank,
        }) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.queue_error(error);
                return false;
            }
        };
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            let result = controller.materialize_store_context(
                &reservation.source_id,
                &context,
                &anchor,
                &context_id,
            );
            match result {
                Ok((items, anchor_index)) => {
                    if let Err(error) = controller.apply_reserved_context(
                        reservation.clone(),
                        items,
                        anchor_index,
                        shuffled_start,
                    ) {
                        controller.reject_queue_materialization(reservation, error);
                    }
                }
                Err(error) => controller.reject_queue_materialization(reservation, error),
            }
        }) {
            self.reject_queue_materialization(rejected, error);
            return false;
        }
        true
    }

    fn play_loaded_context(
        &self,
        context: PlayContext,
        tracks: Arc<Vec<Track>>,
        anchor: PlayContextAnchor,
        shuffled_start: bool,
    ) -> bool {
        let context_id = context_id(&context);
        if !shuffled_start && self.activate_context_occurrence(&context_id, &anchor) {
            return true;
        }
        let anchor_index = anchor.source_rank;
        if tracks
            .get(anchor_index)
            .is_none_or(|track| track.id != anchor.track_id)
        {
            self.queue_error("The selected track is no longer available.");
            return false;
        }
        let reservation =
            match self.reserve_queue_materialization(Placement::Replace { anchor_index }) {
                Ok(reservation) => reservation,
                Err(error) => {
                    self.queue_error(error);
                    return false;
                }
            };
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            let tracks = Arc::try_unwrap(tracks).unwrap_or_else(|tracks| tracks.as_ref().clone());
            let batch_items = tracks
                .into_iter()
                .enumerate()
                .map(|(source_rank, track)| {
                    BatchItem::new(
                        track,
                        Provenance::Context {
                            context_id: context_id.clone(),
                            source_rank,
                        },
                    )
                })
                .collect();
            if let Err(error) = controller.apply_reserved_context(
                reservation.clone(),
                batch_items,
                anchor_index,
                shuffled_start,
            ) {
                controller.reject_queue_materialization(reservation, error);
            }
        }) {
            self.reject_queue_materialization(rejected, error);
            return false;
        }
        true
    }

    fn materialize_store_context(
        &self,
        source_id: &SourceId,
        context: &PlayContext,
        anchor: &PlayContextAnchor,
        context_id: &str,
    ) -> Result<(Vec<BatchItem>, usize), String> {
        let materialized = self
            .store
            .with_store(|store| store.materialize_play_context(source_id, context, anchor))?;
        let anchor_index = materialized.anchor_index;
        let (positions, tracks): (Vec<_>, Vec<_>) = materialized
            .items
            .into_iter()
            .map(|item| ((item.source_rank, item.source_item_id), item.track))
            .unzip();
        let items = positions
            .into_iter()
            .zip(tracks)
            .map(|((source_rank, source_item_id), track)| PlayContextItem {
                track,
                source_rank,
                source_item_id,
            })
            .collect();
        Ok((context_batch_items(items, context_id), anchor_index))
    }

    fn activate_context_occurrence(&self, context_id: &str, anchor: &PlayContextAnchor) -> bool {
        let Some(product) = self.playback_product_if_present() else {
            return false;
        };
        match product.activate_context_occurrence(context_id, &anchor.track_id, anchor.source_rank)
        {
            Ok(activated) => activated,
            Err(error) => {
                self.queue_error(error);
                false
            }
        }
    }

    fn apply_reserved_context(
        &self,
        mut reservation: ReservedQueueMaterialization,
        items: Vec<BatchItem>,
        anchor_index: usize,
        shuffled_start: bool,
    ) -> Result<bool, String> {
        reservation.placement = Placement::Replace { anchor_index };
        self.apply_reserved_batch(reservation, items, shuffled_start)
    }

    fn apply_reserved_batch(
        &self,
        reservation: ReservedQueueMaterialization,
        items: Vec<BatchItem>,
        shuffled_start: bool,
    ) -> Result<bool, String> {
        if items.is_empty() {
            return Err("No tracks are available to add to the queue.".to_string());
        }
        let batch = Batch::new(items).with_shuffle_intent(shuffle_seed(), shuffled_start);
        reservation.product.apply_materialization(
            reservation.id,
            reservation.source_id,
            batch,
            reservation.placement,
        )
    }

    fn apply_tracks(&self, tracks: Vec<Track>, placement: Placement, provenance: Provenance) {
        if tracks.is_empty() {
            self.queue_error(if matches!(placement, Placement::Replace { .. }) {
                "No tracks are available to play."
            } else {
                "No tracks are available to add to the queue."
            });
            return;
        }
        let reservation = match self.reserve_queue_materialization(placement) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.queue_error(error);
                return;
            }
        };
        if let Err(error) = self.apply_reserved_tracks(reservation.clone(), tracks, provenance) {
            self.reject_queue_materialization(reservation, error);
        }
    }

    fn active_music_folder(store: &StoreHandle) -> Option<MusicFolderId> {
        store
            .with_store(|store| {
                let Some(saved) = store.active_source()? else {
                    return Ok(None);
                };
                store.selected_music_folder_id(&saved.source_id)
            })
            .ok()
            .flatten()
    }

    fn queue_error(&self, error: impl Into<String>) {
        let error = error.into();
        warn!(%error, "queue command failed");
    }
}

fn context_anchor(
    total_items: usize,
    anchor_index: usize,
    track_at: &mut impl FnMut(usize) -> Option<Track>,
) -> Option<PlayContextAnchor> {
    if total_items == 0 || anchor_index >= total_items {
        return None;
    }
    Some(PlayContextAnchor {
        track_id: track_at(anchor_index)?.id,
        source_rank: anchor_index,
        source_item_id: None,
    })
}

fn context_batch_items(items: Vec<PlayContextItem>, context_id: &str) -> Vec<BatchItem> {
    items
        .into_iter()
        .map(|item| {
            BatchItem::new(
                item.track,
                Provenance::Context {
                    context_id: context_id.to_string(),
                    source_rank: item.source_rank,
                },
            )
        })
        .collect()
}

fn source_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| query.to_string())
}
