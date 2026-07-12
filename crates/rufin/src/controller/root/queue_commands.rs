use std::sync::Arc;

use domain::{FolderPathItem, LibraryField, LibraryListSettings, TrackSortKey, TrackTableSettings};
use library::play_context::{
    ArtistTrackScope, PlayContext, PlayContextAnchor, PlayContextDescriptor, PlayContextItem,
    PlayContextOrder, PlaylistSort, TrackFilter,
};
use library::{
    AlbumId, ArtistId, GenreId, MoodId, MusicFolderId, PlaylistEntry, PlaylistId, SmartPlaylist,
    SourceId, Track, TrackId,
};
use playback::{
    Batch, BatchItem, MaterializationId, OccurrenceId, Placement, Provenance, RepeatMode,
    SessionCommand,
};

use super::{
    AppController, ControllerEvent, PlaybackProduct, StoreHandle, shuffle_seed,
    smart_playlist_definition_fingerprint,
};

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

impl AppController {
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

    pub fn play_album_tracks(
        &self,
        album_id: AlbumId,
        tracks: Vec<Track>,
        anchor_index: usize,
        shuffled_start: bool,
    ) {
        let Some(anchor_track) = tracks.get(anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return;
        };
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Album {
                album_id,
                music_folder_id: Self::active_music_folder(&self.store),
            },
            order: PlayContextOrder::Canonical,
        };
        let anchor = PlayContextAnchor {
            track_id: anchor_track.id.clone(),
            source_rank: anchor_index,
            source_item_id: None,
        };
        self.play_store_context(context, anchor, shuffled_start);
    }

    pub fn play_album_now(&self, album_id: AlbumId) {
        match self.cached_album_detail(&album_id) {
            Ok(Some((album, tracks))) if !tracks.is_empty() => {
                self.play_album_tracks(album.id, tracks, 0, true);
            }
            Ok(Some(_)) => self.queue_error("No tracks are available to play."),
            Ok(None) => self.queue_error("The selected cached album was not found."),
            Err(error) => self.queue_error(error),
        }
    }

    pub fn play_playlist_entry(
        &self,
        playlist_id: PlaylistId,
        entry: PlaylistEntry,
        source_index: usize,
        query: Option<String>,
        sort: (PlaylistSort, bool),
        shuffled_start: bool,
    ) {
        let (sort, descending) = sort;
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Playlist { playlist_id },
            order: PlayContextOrder::Playlist {
                query,
                sort,
                descending,
            },
        };
        let anchor = PlayContextAnchor {
            track_id: entry.track.id,
            source_rank: source_index,
            source_item_id: Some(entry.entry_id),
        };
        self.play_store_context(context, anchor, shuffled_start);
    }

    pub fn play_cached_playlist(&self, playlist_id: PlaylistId) {
        self.play_cached_playlist_at(playlist_id, Placement::Replace { anchor_index: 0 }, true);
    }

    pub fn play_cached_playlist_next(&self, playlist_id: PlaylistId) {
        self.play_cached_playlist_at(playlist_id, Placement::AfterCurrent, false);
    }

    pub fn play_cached_playlist_last(&self, playlist_id: PlaylistId) {
        self.play_cached_playlist_at(playlist_id, Placement::End, false);
    }

    pub fn play_smart_playlist(
        &self,
        smart_playlist: SmartPlaylist,
        anchor_track_id: Option<TrackId>,
        music_folder_id: Option<MusicFolderId>,
    ) {
        let context = PlayContext {
            descriptor: PlayContextDescriptor::SmartPlaylist {
                smart_playlist_id: smart_playlist.id,
                definition_fingerprint: smart_playlist_definition_fingerprint(
                    &smart_playlist.definition,
                ),
                music_folder_id,
            },
            order: PlayContextOrder::SmartPlaylist,
        };
        let Some(track_id) = anchor_track_id else {
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

    pub fn play_library_source_window(
        &self,
        descriptor: PlayContextDescriptor,
        source: (LibraryListSettings, String, bool, bool),
        total_items: usize,
        anchor_index: usize,
        mut track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        if total_items == 0 || anchor_index >= total_items {
            self.queue_error("The selected track is no longer available.");
            return false;
        }
        let Some(track) = track_at(anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        let (settings, query, favorites_only, favorite_first) = source;
        let order = if matches!(&descriptor, PlayContextDescriptor::SmartPlaylist { .. }) {
            PlayContextOrder::SmartPlaylist
        } else {
            PlayContextOrder::Tracks {
                filter: TrackFilter {
                    query: source_query(&query),
                    favorites_only,
                },
                sort: library_track_sort(settings.sort_key),
                descending: settings.descending,
                favorite_first,
            }
        };
        let context = PlayContext { descriptor, order };
        let anchor = PlayContextAnchor {
            track_id: track.id,
            source_rank: anchor_index,
            source_item_id: None,
        };
        self.play_store_context(context, anchor, false)
    }

    pub fn play_folder_window(
        &self,
        path: Vec<FolderPathItem>,
        query: String,
        settings: TrackTableSettings,
        tracks: Arc<Vec<Track>>,
        anchor_index: usize,
    ) -> bool {
        let Some(anchor_track) = tracks.get(anchor_index) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Folder {
                path: path.into_iter().map(|entry| entry.name).collect(),
                music_folder_id: Self::active_music_folder(&self.store),
            },
            order: PlayContextOrder::Tracks {
                filter: TrackFilter {
                    query: source_query(&query),
                    favorites_only: false,
                },
                sort: table_track_sort(settings.sort_key),
                descending: settings.descending,
                favorite_first: false,
            },
        };
        let anchor = PlayContextAnchor {
            track_id: anchor_track.id.clone(),
            source_rank: anchor_index,
            source_item_id: None,
        };
        self.play_loaded_context(context, tracks, anchor, false)
    }

    pub fn play_artist_tracks_window(
        &self,
        artist_id: ArtistId,
        scope: ArtistTrackScope,
        total_items: usize,
        anchor_index: usize,
        mut track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        let Some(anchor) = context_anchor(total_items, anchor_index, &mut track_at) else {
            self.queue_error("The selected track is no longer available.");
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Artist {
                    artist_id,
                    scope,
                    music_folder_id: Self::active_music_folder(&self.store),
                },
                order: PlayContextOrder::Canonical,
            },
            anchor,
            true,
        )
    }

    pub fn play_genre_tracks_window(
        &self,
        genre_id: GenreId,
        total_items: usize,
        anchor_index: usize,
        mut track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        let Some(anchor) = context_anchor(total_items, anchor_index, &mut track_at) else {
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Genre {
                    genre_id,
                    music_folder_id: Self::active_music_folder(&self.store),
                },
                order: PlayContextOrder::Canonical,
            },
            anchor,
            true,
        )
    }

    pub fn play_mood_tracks_window(
        &self,
        mood_id: MoodId,
        total_items: usize,
        anchor_index: usize,
        mut track_at: impl FnMut(usize) -> Option<Track>,
    ) -> bool {
        let Some(anchor) = context_anchor(total_items, anchor_index, &mut track_at) else {
            return false;
        };
        self.play_store_context(
            PlayContext {
                descriptor: PlayContextDescriptor::Mood {
                    mood_id,
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
        let _ = self.events.send(ControllerEvent::Error(error.into()));
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

fn context_id(context: &PlayContext) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in format!("{context:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("context:{hash:016x}")
}

fn source_query(query: &str) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| query.to_string())
}

fn table_track_sort(sort: TrackSortKey) -> library::TrackSort {
    match sort {
        TrackSortKey::TrackNumber => library::TrackSort::TrackNumber,
        TrackSortKey::Title => library::TrackSort::Title,
        TrackSortKey::Artist => library::TrackSort::Artist,
        TrackSortKey::Album => library::TrackSort::Album,
        TrackSortKey::Year => library::TrackSort::Year,
        TrackSortKey::Duration => library::TrackSort::Duration,
        TrackSortKey::Favorite => library::TrackSort::Favorite,
    }
}

pub(super) fn library_track_sort(sort: LibraryField) -> library::TrackSort {
    match sort {
        LibraryField::TrackNumber => library::TrackSort::TrackNumber,
        LibraryField::Artist => library::TrackSort::Artist,
        LibraryField::AlbumArtist => library::TrackSort::AlbumArtist,
        LibraryField::Album => library::TrackSort::Album,
        LibraryField::Year => library::TrackSort::Year,
        LibraryField::ReleaseDate => library::TrackSort::ReleaseDate,
        LibraryField::DateAdded => library::TrackSort::DateAdded,
        LibraryField::LastPlayed => library::TrackSort::LastPlayed,
        LibraryField::PlayCount => library::TrackSort::PlayCount,
        LibraryField::UserRating => library::TrackSort::UserRating,
        LibraryField::Genre => library::TrackSort::Genre,
        LibraryField::Bpm => library::TrackSort::Bpm,
        LibraryField::Duration => library::TrackSort::Duration,
        LibraryField::Favorite => library::TrackSort::Favorite,
        LibraryField::RowIndex
        | LibraryField::Image
        | LibraryField::Title
        | LibraryField::TitleMerged
        | LibraryField::DiscNumber
        | LibraryField::SongCount
        | LibraryField::AlbumCount => library::TrackSort::Title,
    }
}
