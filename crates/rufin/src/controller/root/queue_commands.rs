use std::sync::Arc;

use library::play_context::{
    PlayContext, PlayContextAnchor, PlayContextDescriptor, PlayContextItem, PlayContextOrder,
    PlaylistSort, TrackFilter, context_id,
};
use library::{MusicFolderId, PlaylistId, SmartPlaylistId, SourceId, Track, TrackId};
use playback::{
    AlbumPlayRequest, ArtistWindowPlayRequest, Batch, BatchItem, CachedPlaylistPlayRequest,
    ContextPlayRequest, ContextTrackSource, FolderWindowPlayRequest, LibraryWindowPlayRequest,
    MaterializationId, OccurrenceId, Placement, PlaylistEntryPlayRequest, Provenance,
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
        let shuffled_start = request.placement == QueuePlacement::Now;
        self.play_smart_playlist_at(request.smart_playlist_id, request.placement, shuffled_start);
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

    pub fn play_context(&self, request: ContextPlayRequest) -> bool {
        let context = PlayContext {
            descriptor: request.descriptor,
            order: PlayContextOrder::Canonical,
        };
        self.enqueue_context(context, request.tracks, request.placement)
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

    fn play_smart_playlist_at(
        &self,
        smart_playlist_id: SmartPlaylistId,
        placement: QueuePlacement,
        shuffled_start: bool,
    ) {
        let reservation = match self.reserve_queue_materialization(placement.into()) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.queue_error(error);
                return;
            }
        };
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            let result = controller.load_smart_playlist_context_items(
                &reservation.source_id,
                &smart_playlist_id,
                placement,
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

    fn load_smart_playlist_context_items(
        &self,
        source_id: &SourceId,
        smart_playlist_id: &SmartPlaylistId,
        placement: QueuePlacement,
    ) -> Result<Vec<BatchItem>, String> {
        let materialized = self.store.with_store(|store| {
            store.materialize_saved_smart_playlist_context(source_id, smart_playlist_id)
        })?;
        let Some((context, items)) = materialized else {
            return Err("The selected smart playlist was not found.".to_string());
        };
        if items.is_empty() {
            return Err(if placement == QueuePlacement::Now {
                "No tracks are available to play.".to_string()
            } else {
                "No tracks are available to add to the queue.".to_string()
            });
        }
        Ok(context_batch_items(items, &context_id(&context)))
    }

    fn enqueue_context(
        &self,
        context: PlayContext,
        tracks: ContextTrackSource,
        placement: QueuePlacement,
    ) -> bool {
        if matches!(&tracks, ContextTrackSource::Loaded(tracks) if tracks.is_empty()) {
            self.queue_error(if placement == QueuePlacement::Now {
                "No tracks are available to play."
            } else {
                "No tracks are available to add to the queue."
            });
            return false;
        }
        let reservation = match self.reserve_queue_materialization(placement.into()) {
            Ok(reservation) => reservation,
            Err(error) => {
                self.queue_error(error);
                return false;
            }
        };
        let context_id = context_id(&context);
        let shuffled_start = placement == QueuePlacement::Now;
        let controller = self.clone();
        let rejected = reservation.clone();
        if let Err(error) = self.submit_playback_materialization(move || {
            let items = match tracks {
                ContextTrackSource::Store => controller
                    .store
                    .with_store(|store| {
                        store.materialize_play_context_items(&reservation.source_id, &context)
                    })
                    .map(|items| context_batch_items(items, &context_id)),
                ContextTrackSource::Loaded(tracks) => {
                    let tracks =
                        Arc::try_unwrap(tracks).unwrap_or_else(|tracks| tracks.as_ref().clone());
                    Ok(tracks
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
                        .collect())
                }
            };
            match items {
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
            return false;
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::root::test_support::{
        CachedLibraryObservation, commit_cached_library, library_album, library_track,
        owners_from_store_for_test, saved_source, wait_for_playback_projection,
    };
    use library::{
        MusicFolder, SmartPlaylistDefinition, SmartPlaylistMatchMode, SmartPlaylistRuleGroup,
        SmartPlaylistSortField,
    };

    #[test]
    fn smart_playlist_placement_materializes_one_ordered_batch() {
        let store = StoreHandle::open_memory().expect("open store");
        let saved = saved_source();
        let album = library_album(1, "Artist", "Album", None);
        let mut tracks = (1..=4)
            .map(|number| {
                library_track(
                    number,
                    album.artist_id.clone(),
                    album.id.clone(),
                    "Artist",
                    &[],
                )
            })
            .collect::<Vec<_>>();
        tracks[0].title = "Charlie".to_string();
        tracks[1].title = "Alpha".to_string();
        tracks[2].title = "Bravo".to_string();
        tracks[3].title = "Aardvark outside folder".to_string();
        let folder = MusicFolder {
            id: MusicFolderId::fake(1),
            name: "Selected".to_string(),
        };
        let smart_playlist_id = SmartPlaylistId::new("custom:queue-placement");
        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: Vec::new(),
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: Some(2),
        };
        store
            .with_store(|store| {
                store.save_source(&saved)?;
                store.set_active_source(&saved.source_id)?;
                let generation = store.begin_sync(&saved.source_id)?;
                commit_cached_library(
                    store,
                    &saved.source_id,
                    generation,
                    CachedLibraryObservation {
                        albums: vec![album],
                        tracks: tracks.clone(),
                        music_folders: vec![(folder.clone(), tracks[..3].to_vec())],
                        ..CachedLibraryObservation::default()
                    },
                )?;
                store.set_selected_music_folder_id(&saved.source_id, Some(&folder.id))?;
                store.save_smart_playlist(
                    &saved.source_id,
                    &smart_playlist_id,
                    "Queue placement",
                    &definition,
                )
            })
            .expect("seed smart playlist");

        let expected = [tracks[1].clone(), tracks[2].clone()];
        let (owners, events) = owners_from_store_for_test(store);
        let before_seed = sequence_snapshot(&owners).revision();
        owners.playback.play_tracks_now(vec![expected[0].clone()]);
        wait_for_queue_revision(&events, before_seed + 1);
        let before_shuffle = sequence_snapshot(&owners).revision();
        owners.playback.set_shuffle(true);
        wait_for_queue_revision(&events, before_shuffle + 1);

        let before_next = sequence_snapshot(&owners).revision();
        owners
            .playback
            .play_smart_playlist(SmartPlaylistPlayRequest::new(
                smart_playlist_id.clone(),
                QueuePlacement::Next,
            ));
        wait_for_queue_revision(&events, before_next + 1);
        let after_next = sequence_snapshot(&owners);
        assert_eq!(after_next.revision(), before_next + 1);
        assert_eq!(
            track_ids(&after_next),
            vec![
                expected[0].id.clone(),
                expected[0].id.clone(),
                expected[1].id.clone(),
            ]
        );
        assert_eq!(context_ranks(&after_next), vec![0, 1]);

        let before_last = after_next.revision();
        owners
            .playback
            .play_smart_playlist(SmartPlaylistPlayRequest::new(
                smart_playlist_id.clone(),
                QueuePlacement::Last,
            ));
        wait_for_queue_revision(&events, before_last + 1);
        let after_last = sequence_snapshot(&owners);
        assert_eq!(after_last.revision(), before_last + 1);
        assert_eq!(
            track_ids(&after_last),
            vec![
                expected[0].id.clone(),
                expected[0].id.clone(),
                expected[1].id.clone(),
                expected[0].id.clone(),
                expected[1].id.clone(),
            ]
        );
        assert_eq!(context_ranks(&after_last), vec![0, 1, 0, 1]);

        let before_now = after_last.revision();
        owners
            .playback
            .play_smart_playlist(SmartPlaylistPlayRequest::new(
                smart_playlist_id,
                QueuePlacement::Now,
            ));
        wait_for_queue_revision(&events, before_now + 1);
        let after_now = sequence_snapshot(&owners);
        assert_eq!(after_now.revision(), before_now + 1);
        assert_eq!(
            track_ids(&after_now),
            expected
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(context_ranks(&after_now), vec![0, 1]);
        assert_eq!(
            after_now.selected().map(|entry| &entry.track.id),
            Some(&expected[1].id)
        );
    }

    fn sequence_snapshot(owners: &super::super::ProductOwners) -> playback::Sequence {
        owners
            .playback
            .playback_product()
            .expect("playback product")
            .sequence_snapshot()
            .expect("sequence")
    }

    fn wait_for_queue_revision(events: &super::super::ProductReceivers, revision: u64) {
        loop {
            let projection = wait_for_playback_projection(events);
            if projection.view.queue.revision >= revision {
                return;
            }
        }
    }

    fn track_ids(sequence: &playback::Sequence) -> Vec<TrackId> {
        sequence
            .entries()
            .iter()
            .map(|entry| entry.track.id.clone())
            .collect()
    }

    fn context_ranks(sequence: &playback::Sequence) -> Vec<usize> {
        sequence
            .entries()
            .iter()
            .filter_map(|entry| match &entry.provenance {
                Provenance::Context { source_rank, .. } => Some(*source_rank),
                _ => None,
            })
            .collect()
    }
}
