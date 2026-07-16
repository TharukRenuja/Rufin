use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::warn;

use crate::{
    Album, AlbumId, Artist, ArtistId, CachedArtistDetail, CachedGenreDetail, CachedMoodDetail,
    Genre, GenreId, HomeSection, Mood, MoodId, PagedResponse, Playlist, PlaylistDetail, PlaylistId,
    SmartPlaylist, SmartPlaylistBuiltin, SmartPlaylistDetail, SmartPlaylistId, SourceId, Store,
    StoreAccess, StoreResult, Track, TrackId, TrackSort,
};

const SLOW_SMART_PLAYLIST_DETAIL_MS: u64 = 100;
const PREPARED_ALBUM_LIMIT: usize = 500;
const PREPARED_TRACK_LIMIT: usize = 40_000;

#[derive(Clone, Debug)]
pub struct PreparedPage<T> {
    pub items: Arc<Vec<T>>,
    pub total: usize,
}

fn prepared_page_from_bounded_response<T>(page: PagedResponse<T>, limit: usize) -> PreparedPage<T> {
    if page.total <= limit && page.items.len() == page.total {
        return PreparedPage {
            items: Arc::new(page.items),
            total: page.total,
        };
    }

    PreparedPage {
        items: Arc::new(Vec::new()),
        total: page.total,
    }
}

#[derive(Clone, Debug)]
pub enum PreparedRead<T> {
    Ready(T),
    Invalidated,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HomeOverview {
    pub sections: Vec<HomeSection>,
    pub genres: Vec<HomeGenre>,
    pub showcase_fallback: Option<Album>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeGenre {
    pub id: GenreId,
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

impl Store {
    fn load_home_overview(
        &self,
        source_id: &SourceId,
        genre_limit: usize,
    ) -> StoreResult<HomeOverview> {
        self.read_snapshot(|store| store.load_home_overview_projection(source_id, genre_limit))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PreparedReadKey {
    Albums,
    Tracks { sort: TrackSort, descending: bool },
}

#[derive(Clone, Debug)]
enum PreparedReadValue {
    Albums(PreparedPage<Album>),
    Tracks(PreparedPage<Track>),
}

#[derive(Debug, Default)]
pub struct PreparedReadEvictions(Vec<PreparedReadValue>);

impl PreparedReadEvictions {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn release_shared_references(self) -> Self {
        let deferred = self
            .0
            .into_iter()
            .filter_map(|value| match value {
                PreparedReadValue::Albums(page) if Arc::strong_count(&page.items) > 1 => None,
                PreparedReadValue::Tracks(page) if Arc::strong_count(&page.items) > 1 => None,
                value => Some(value),
            })
            .collect();
        Self(deferred)
    }
}

#[derive(Clone, Debug)]
struct PreparedReadEntry {
    revision: i64,
    key: PreparedReadKey,
    value: PreparedReadValue,
}

#[derive(Clone, Debug)]
struct PreparedReadRequest {
    revision: i64,
    key: PreparedReadKey,
    epoch: u64,
    ticket: u64,
}

#[derive(Debug, Default)]
struct PreparedReadCache {
    albums: Option<PreparedReadEntry>,
    tracks: Option<PreparedReadEntry>,
    albums_epoch: u64,
    tracks_epoch: u64,
    albums_request: Option<PreparedReadRequest>,
    tracks_request: Option<PreparedReadRequest>,
    albums_ticket: u64,
    tracks_ticket: u64,
}

#[derive(Debug, Default)]
struct PreparedReadState {
    cache: Mutex<PreparedReadCache>,
    albums_load: Mutex<()>,
    tracks_load: Mutex<()>,
}

impl PreparedReadState {
    fn load_gate(&self, key: &PreparedReadKey) -> &Mutex<()> {
        match key {
            PreparedReadKey::Albums => &self.albums_load,
            PreparedReadKey::Tracks { .. } => &self.tracks_load,
        }
    }
}

impl PreparedReadCache {
    fn get(&self, revision: i64, key: &PreparedReadKey) -> Option<PreparedReadValue> {
        let entry = match key {
            PreparedReadKey::Albums => self.albums.as_ref(),
            PreparedReadKey::Tracks { .. } => self.tracks.as_ref(),
        }?;
        (entry.revision == revision && &entry.key == key).then(|| entry.value.clone())
    }

    fn complete_tracks(&self, revision: i64) -> Option<Arc<Vec<Track>>> {
        let entry = self.tracks.as_ref()?;
        let PreparedReadValue::Tracks(page) = &entry.value else {
            return None;
        };
        (entry.revision == revision && page.items.len() == page.total)
            .then(|| Arc::clone(&page.items))
    }

    fn epoch(&self, key: &PreparedReadKey) -> u64 {
        match key {
            PreparedReadKey::Albums => self.albums_epoch,
            PreparedReadKey::Tracks { .. } => self.tracks_epoch,
        }
    }

    fn register_request(&mut self, revision: i64, key: &PreparedReadKey) -> u64 {
        let epoch = self.epoch(key);
        let (request, ticket) = match key {
            PreparedReadKey::Albums => (&mut self.albums_request, &mut self.albums_ticket),
            PreparedReadKey::Tracks { .. } => (&mut self.tracks_request, &mut self.tracks_ticket),
        };
        if let Some(request) = request.as_ref()
            && request.revision == revision
            && &request.key == key
            && request.epoch == epoch
        {
            return request.ticket;
        }
        *ticket = ticket.wrapping_add(1);
        let next = *ticket;
        *request = Some(PreparedReadRequest {
            revision,
            key: key.clone(),
            epoch,
            ticket: next,
        });
        next
    }

    fn request_is_current(&self, revision: i64, key: &PreparedReadKey, ticket: u64) -> bool {
        let request = match key {
            PreparedReadKey::Albums => self.albums_request.as_ref(),
            PreparedReadKey::Tracks { .. } => self.tracks_request.as_ref(),
        };
        request.is_some_and(|request| {
            request.revision == revision
                && &request.key == key
                && request.epoch == self.epoch(key)
                && request.ticket == ticket
        })
    }

    fn insert(
        &mut self,
        revision: i64,
        key: PreparedReadKey,
        value: PreparedReadValue,
    ) -> Option<PreparedReadValue> {
        let slot = match &key {
            PreparedReadKey::Albums => &mut self.albums,
            PreparedReadKey::Tracks { .. } => &mut self.tracks,
        };
        slot.replace(PreparedReadEntry {
            revision,
            key,
            value,
        })
        .map(|entry| entry.value)
    }

    fn invalidate(&mut self, delta: &crate::LibraryDelta) -> Vec<PreparedReadValue> {
        let albums_changed =
            delta.reset.is_some() || !delta.albums.is_empty() || !delta.tracks.is_empty();
        let tracks_changed = delta.reset.is_some() || !delta.tracks.is_empty();
        let mut removed = Vec::with_capacity(2);
        if albums_changed {
            self.invalidate_albums(&mut removed);
        }
        if tracks_changed {
            self.invalidate_tracks(&mut removed);
        }
        removed
    }

    fn advance(&mut self, revision: i64, delta: &crate::LibraryDelta) -> Vec<PreparedReadValue> {
        let albums_changed =
            delta.reset.is_some() || !delta.albums.is_empty() || !delta.tracks.is_empty();
        let tracks_changed = delta.reset.is_some() || !delta.tracks.is_empty();
        let mut removed = Vec::with_capacity(2);
        if albums_changed {
            self.invalidate_albums(&mut removed);
        } else {
            if let Some(entry) = self.albums.as_mut() {
                entry.revision = revision;
            }
            Self::cancel_old_request(revision, &mut self.albums_epoch, &mut self.albums_request);
        }
        if tracks_changed {
            self.invalidate_tracks(&mut removed);
        } else {
            if let Some(entry) = self.tracks.as_mut() {
                entry.revision = revision;
            }
            Self::cancel_old_request(revision, &mut self.tracks_epoch, &mut self.tracks_request);
        }
        removed
    }

    fn invalidate_albums(&mut self, removed: &mut Vec<PreparedReadValue>) {
        self.albums_epoch = self.albums_epoch.wrapping_add(1);
        self.albums_request = None;
        if let Some(entry) = self.albums.take() {
            removed.push(entry.value);
        }
    }

    fn invalidate_tracks(&mut self, removed: &mut Vec<PreparedReadValue>) {
        self.tracks_epoch = self.tracks_epoch.wrapping_add(1);
        self.tracks_request = None;
        if let Some(entry) = self.tracks.take() {
            removed.push(entry.value);
        }
    }

    fn cancel_old_request(
        revision: i64,
        epoch: &mut u64,
        request: &mut Option<PreparedReadRequest>,
    ) {
        if request
            .as_ref()
            .is_some_and(|request| request.revision != revision)
        {
            *epoch = epoch.wrapping_add(1);
            *request = None;
        }
    }
}

/// Read access to one fixed source in the cached library.
#[derive(Clone)]
pub struct ActiveLibraryQuery {
    store: StoreAccess,
    source_id: SourceId,
    prepared: Arc<PreparedReadState>,
}

impl StoreAccess {
    pub fn query(&self, source_id: SourceId) -> ActiveLibraryQuery {
        ActiveLibraryQuery {
            store: self.clone(),
            source_id,
            prepared: Arc::new(PreparedReadState::default()),
        }
    }
}

impl ActiveLibraryQuery {
    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    fn cached_prepared_value(
        &self,
        revision: i64,
        key: &PreparedReadKey,
    ) -> Option<PreparedReadValue> {
        let mut prepared = self
            .prepared
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = prepared.get(revision, key);
        if value.is_some() {
            prepared.register_request(revision, key);
        }
        value
    }

    fn load_prepared_value(
        &self,
        revision: i64,
        key: PreparedReadKey,
        load: impl Fn() -> Result<PreparedReadValue, String>,
    ) -> Result<PreparedRead<PreparedReadValue>, String> {
        if let Some(value) = self.cached_prepared_value(revision, &key) {
            return Ok(PreparedRead::Ready(value));
        }

        let ticket = {
            let mut prepared = self
                .prepared
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(value) = prepared.get(revision, &key) {
                return Ok(PreparedRead::Ready(value));
            }
            prepared.register_request(revision, &key)
        };

        let _load_guard = self
            .prepared
            .load_gate(&key)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let epoch = {
            let prepared = self
                .prepared
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(warm) = prepared.get(revision, &key) {
                return Ok(PreparedRead::Ready(warm));
            }
            if !prepared.request_is_current(revision, &key, ticket) {
                return Ok(PreparedRead::Invalidated);
            }
            prepared.epoch(&key)
        };
        let value = load()?;
        let mut prepared = self
            .prepared
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if prepared.epoch(&key) != epoch || !prepared.request_is_current(revision, &key, ticket) {
            drop(prepared);
            drop(value);
            return Ok(PreparedRead::Invalidated);
        }
        if let Some(warm) = prepared.get(revision, &key) {
            return Ok(PreparedRead::Ready(warm));
        }
        let evicted = prepared.insert(revision, key.clone(), value.clone());
        drop(prepared);
        drop(evicted);
        Ok(PreparedRead::Ready(value))
    }

    pub fn prepared_albums(
        &self,
        revision: i64,
    ) -> Result<PreparedRead<PreparedPage<Album>>, String> {
        match self.load_prepared_value(revision, PreparedReadKey::Albums, || {
            let page = self
                .store
                .with_fast_read(|store| {
                    store.load_complete_albums_if_within(&self.source_id, PREPARED_ALBUM_LIMIT)
                })
                .map_err(|error| error.to_string())?;
            Ok(PreparedReadValue::Albums(
                prepared_page_from_bounded_response(page, PREPARED_ALBUM_LIMIT),
            ))
        })? {
            PreparedRead::Ready(PreparedReadValue::Albums(page)) => Ok(PreparedRead::Ready(page)),
            PreparedRead::Ready(PreparedReadValue::Tracks(_)) => unreachable!(),
            PreparedRead::Invalidated => Ok(PreparedRead::Invalidated),
        }
    }

    pub fn prepared_albums_if_cached(&self, revision: i64) -> Option<PreparedPage<Album>> {
        match self.cached_prepared_value(revision, &PreparedReadKey::Albums) {
            Some(PreparedReadValue::Albums(page)) => Some(page),
            _ => None,
        }
    }

    pub fn prepared_tracks(
        &self,
        revision: i64,
        sort: TrackSort,
        descending: bool,
    ) -> Result<PreparedRead<PreparedPage<Track>>, String> {
        let key = PreparedReadKey::Tracks { sort, descending };
        match self.load_prepared_value(revision, key, || {
            let page = self
                .store
                .with_fast_read(|store| {
                    store.load_complete_tracks_sorted_if_within(
                        &self.source_id,
                        sort,
                        descending,
                        PREPARED_TRACK_LIMIT,
                    )
                })
                .map_err(|error| error.to_string())?;
            Ok(PreparedReadValue::Tracks(
                prepared_page_from_bounded_response(page, PREPARED_TRACK_LIMIT),
            ))
        })? {
            PreparedRead::Ready(PreparedReadValue::Tracks(page)) => Ok(PreparedRead::Ready(page)),
            PreparedRead::Ready(PreparedReadValue::Albums(_)) => unreachable!(),
            PreparedRead::Invalidated => Ok(PreparedRead::Invalidated),
        }
    }

    pub fn prepared_tracks_if_cached(
        &self,
        revision: i64,
        sort: TrackSort,
        descending: bool,
    ) -> Option<PreparedPage<Track>> {
        let key = PreparedReadKey::Tracks { sort, descending };
        match self.cached_prepared_value(revision, &key) {
            Some(PreparedReadValue::Tracks(page)) => Some(page),
            _ => None,
        }
    }

    pub fn prepared_album_tracks_if_cached(
        &self,
        revision: i64,
        album_ids: &[AlbumId],
    ) -> Option<HashMap<AlbumId, Vec<Track>>> {
        let tracks = self
            .prepared
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .complete_tracks(revision)?;
        let wanted = album_ids.iter().collect::<HashSet<_>>();
        let mut grouped = HashMap::new();
        for track in tracks.iter() {
            if wanted.contains(&track.album_id) {
                grouped
                    .entry(track.album_id.clone())
                    .or_insert_with(Vec::new)
                    .push(track.clone());
            }
        }
        Some(grouped)
    }

    #[must_use]
    pub fn invalidate_prepared_reads(&self, delta: &crate::LibraryDelta) -> PreparedReadEvictions {
        if delta.is_empty() {
            return PreparedReadEvictions::default();
        }
        PreparedReadEvictions(
            self.prepared
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .invalidate(delta),
        )
    }

    #[must_use]
    pub fn advance_prepared_reads(
        &self,
        revision: i64,
        delta: &crate::LibraryDelta,
    ) -> PreparedReadEvictions {
        PreparedReadEvictions(
            self.prepared
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .advance(revision, delta),
        )
    }

    pub fn album_detail(&self, album_id: &AlbumId) -> Result<Option<(Album, Vec<Track>)>, String> {
        self.store
            .with_fast_read(|store| store.load_album_detail(&self.source_id, album_id))
            .map_err(|error| error.to_string())
    }

    pub fn album_detail_for_revision(
        &self,
        revision: i64,
        album_id: &AlbumId,
    ) -> Result<Option<(Album, Vec<Track>)>, String> {
        if let Some(detail) = self.prepared_album_detail_if_cached(revision, album_id) {
            return Ok(detail);
        }
        self.album_detail(album_id)
    }

    fn prepared_album_detail_if_cached(
        &self,
        revision: i64,
        album_id: &AlbumId,
    ) -> Option<Option<(Album, Vec<Track>)>> {
        let albums = self.prepared_albums_if_cached(revision)?;
        if albums.items.len() != albums.total {
            return None;
        }
        let Some(album) = albums.items.iter().find(|album| &album.id == album_id) else {
            return Some(None);
        };
        let mut tracks =
            self.prepared_album_tracks_if_cached(revision, std::slice::from_ref(album_id))?;
        Some(Some((
            album.clone(),
            tracks.remove(album_id).unwrap_or_default(),
        )))
    }

    pub fn album_tracks(
        &self,
        album_ids: &[AlbumId],
    ) -> Result<HashMap<AlbumId, Vec<Track>>, String> {
        self.store
            .with_fast_read(|store| store.load_tracks_for_albums(&self.source_id, album_ids))
            .map_err(|error| error.to_string())
    }

    pub fn track(&self, track_id: &TrackId) -> Result<Option<Track>, String> {
        self.store
            .with_fast_read(|store| store.load_track(&self.source_id, track_id))
            .map_err(|error| error.to_string())
    }

    pub fn tracks_by_ids(&self, track_ids: &[TrackId]) -> Result<Vec<Track>, String> {
        self.store
            .with_fast_read(|store| store.load_tracks_by_ids(&self.source_id, track_ids))
            .map_err(|error| error.to_string())
    }

    pub fn artist_detail(
        &self,
        artist_id: &ArtistId,
    ) -> Result<Option<CachedArtistDetail>, String> {
        self.store
            .with_fast_read(|store| store.load_artist_detail(&self.source_id, artist_id))
            .map_err(|error| error.to_string())
    }

    pub fn playlist_detail(
        &self,
        playlist_id: &PlaylistId,
    ) -> Result<Option<PlaylistDetail>, String> {
        self.store
            .with_fast_read(|store| store.load_playlist_detail(&self.source_id, playlist_id))
            .map_err(|error| error.to_string())
    }

    pub fn genre_detail(&self, genre_id: &GenreId) -> Result<Option<CachedGenreDetail>, String> {
        self.store
            .with_fast_read(|store| store.load_genre_detail(&self.source_id, genre_id))
            .map_err(|error| error.to_string())
    }

    pub fn mood_detail(&self, mood_id: &MoodId) -> Result<Option<CachedMoodDetail>, String> {
        self.store
            .with_fast_read(|store| store.load_mood_detail(&self.source_id, mood_id))
            .map_err(|error| error.to_string())
    }

    pub fn albums_page(&self, offset: usize, limit: usize) -> Result<PagedResponse<Album>, String> {
        self.store
            .with_fast_read(|store| store.load_albums(&self.source_id, offset, limit))
            .map_err(|error| error.to_string())
    }

    pub fn albums_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Album>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_albums_matching(&self.source_id, query, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn tracks_page(
        &self,
        sort: TrackSort,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Track>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_tracks_sorted(&self.source_id, sort, descending, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn tracks_page_matching(
        &self,
        query: &str,
        sort: TrackSort,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Track>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_tracks_matching_sorted(
                    &self.source_id,
                    query,
                    sort,
                    descending,
                    offset,
                    limit,
                )
            })
            .map_err(|error| error.to_string())
    }

    pub fn artists_page(
        &self,
        album_artist: bool,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Artist>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_artists(&self.source_id, album_artist, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn artists_page_matching(
        &self,
        album_artist: bool,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Artist>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_artists_matching(&self.source_id, album_artist, query, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn genres_page(&self, offset: usize, limit: usize) -> Result<PagedResponse<Genre>, String> {
        self.store
            .with_fast_read(|store| store.load_genres(&self.source_id, offset, limit))
            .map_err(|error| error.to_string())
    }

    pub fn genres_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Genre>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_genres_matching(&self.source_id, query, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn genre_ids_by_name(&self, names: &[String]) -> Result<HashMap<String, GenreId>, String> {
        self.store
            .with_fast_read(|store| store.load_genre_ids_by_name(&self.source_id, names))
            .map_err(|error| error.to_string())
    }

    pub fn moods_page(&self, offset: usize, limit: usize) -> Result<PagedResponse<Mood>, String> {
        self.store
            .with_fast_read(|store| store.load_moods(&self.source_id, offset, limit))
            .map_err(|error| error.to_string())
    }

    pub fn moods_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Mood>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_moods_matching(&self.source_id, query, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn smart_playlist_rule_value_suggestions(
        &self,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        self.store
            .with_fast_read(|store| {
                Ok((
                    store.load_track_genre_names(&self.source_id)?,
                    store.load_track_mood_names(&self.source_id)?,
                ))
            })
            .map_err(|error| error.to_string())
    }

    pub fn playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Playlist>, String> {
        self.store
            .with_fast_read(|store| store.load_playlists(&self.source_id, offset, limit))
            .map_err(|error| error.to_string())
    }

    pub fn playlists_page_matching(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<Playlist>, String> {
        self.store
            .with_fast_read(|store| {
                store.load_playlists_matching(&self.source_id, query, offset, limit)
            })
            .map_err(|error| error.to_string())
    }

    pub fn smart_playlists_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<PagedResponse<SmartPlaylist>, String> {
        self.store
            .with_fast_read(|store| store.load_smart_playlists(&self.source_id, offset, limit))
            .map_err(|error| error.to_string())
    }

    pub fn smart_playlist_detail(
        &self,
        smart_playlist_id: &SmartPlaylistId,
    ) -> Result<Option<SmartPlaylistDetail>, String> {
        let total_started = Instant::now();
        let store_started = Instant::now();
        let (detail, load_ms) = self
            .store
            .with_fast_read(|store| {
                let load_started = Instant::now();
                let detail =
                    store.load_smart_playlist_detail(&self.source_id, smart_playlist_id)?;
                let load_ms = load_started.elapsed().as_millis() as u64;
                Ok((detail, load_ms))
            })
            .map_err(|error| error.to_string())?;
        let store_ms = store_started.elapsed().as_millis() as u64;
        let (track_count, playlist_name) = if let Some(detail) = detail.as_ref() {
            (
                detail.tracks.len(),
                Some(detail.smart_playlist.name.as_str().to_string()),
            )
        } else {
            (0, None)
        };
        let total_ms = total_started.elapsed().as_millis() as u64;
        if total_ms >= SLOW_SMART_PLAYLIST_DETAIL_MS {
            warn!(
                smart_playlist_id = %smart_playlist_id.as_str(),
                playlist_name = playlist_name.as_deref().unwrap_or(""),
                track_count,
                store_ms,
                load_ms,
                total_ms,
                "slow cached smart playlist detail"
            );
        }
        Ok(detail)
    }

    pub fn missing_builtin_smart_playlists(&self) -> Result<Vec<SmartPlaylistBuiltin>, String> {
        self.store
            .with_fast_read(|store| store.missing_builtin_smart_playlists(&self.source_id))
            .map_err(|error| error.to_string())
    }

    pub fn home_sections(&self) -> Result<Vec<HomeSection>, String> {
        self.store
            .with_fast_read(|store| store.load_home_sections(&self.source_id))
            .map_err(|error| error.to_string())
    }

    pub fn home_overview(&self, genre_limit: usize) -> Result<HomeOverview, String> {
        self.store
            .with_fast_read(|store| store.load_home_overview(&self.source_id, genre_limit))
            .map_err(|error| error.to_string())
    }

    pub fn favorite_tracks(&self) -> Result<Vec<Track>, String> {
        self.store
            .with_fast_read(|store| store.load_favorite_tracks(&self.source_id))
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use super::{
        PreparedPage, PreparedRead, PreparedReadKey, PreparedReadValue,
        prepared_page_from_bounded_response,
    };
    use crate::{
        Album, AlbumId, FavoriteItemId, LibraryDelta, LibrarySync, PagedResponse, SourceId, Store,
        StoreAccess, StoredSource, SyncCoverage, Track, TrackId, TrackSort,
    };

    #[test]
    fn oversized_prepared_page_keeps_only_the_total_sentinel() {
        let page = prepared_page_from_bounded_response(
            PagedResponse {
                items: vec![1, 2, 3],
                total: 4,
            },
            3,
        );

        assert!(page.items.is_empty());
        assert_eq!(page.total, 4);
    }

    #[test]
    fn prepared_tracks_share_one_read_until_a_typed_delta_invalidates_it() {
        let source_id = SourceId::new("source:test");
        let track_id = TrackId::new("track:test");
        let store = Store::open_memory().expect("open store");
        store
            .save_source(&StoredSource {
                source_id: source_id.clone(),
                kind: "local".to_string(),
                name: "Test".to_string(),
                provider_payload: "{}".to_string(),
            })
            .expect("save source");
        let generation = store.begin_sync(&source_id).expect("begin sync");
        let base_revision = store
            .source_cache_revision(&source_id)
            .expect("base revision");
        let album = test_album();
        let track = test_track(track_id.clone(), &album);
        let commit = store
            .commit_library_sync(
                &source_id,
                generation,
                base_revision,
                LibrarySync {
                    albums: vec![album],
                    tracks: vec![track],
                    artists: Vec::new(),
                    album_artists: Vec::new(),
                    genres: Vec::new(),
                    playlists: Vec::new(),
                    home_sections: Vec::new(),
                    mappings: Vec::new(),
                    coverage: SyncCoverage::All {
                        music_folders: Vec::new(),
                    },
                    local_access: None,
                },
            )
            .expect("commit library");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(source_id.clone());

        let first = ready(
            query
                .prepared_tracks(commit.cache_revision, TrackSort::Title, false)
                .expect("first prepared read"),
        );
        let warm = ready(
            query
                .prepared_tracks(commit.cache_revision, TrackSort::Title, false)
                .expect("warm prepared read"),
        );
        assert!(Arc::ptr_eq(&first.items, &warm.items));
        assert!(!warm.items[0].favorite);

        access
            .with_store(|store| store.set_track_favorite(&source_id, &track_id, true))
            .expect("update favorite");
        let still_warm = ready(
            query
                .prepared_tracks(commit.cache_revision, TrackSort::Title, false)
                .expect("same-revision prepared read"),
        );
        assert!(Arc::ptr_eq(&warm.items, &still_warm.items));
        assert!(!still_warm.items[0].favorite);

        drop(
            query.invalidate_prepared_reads(&LibraryDelta::favorite_changed(
                &FavoriteItemId::Track(track_id),
            )),
        );
        let refreshed = ready(
            query
                .prepared_tracks(commit.cache_revision, TrackSort::Title, false)
                .expect("invalidated prepared read"),
        );
        assert!(!Arc::ptr_eq(&still_warm.items, &refreshed.items));
        assert!(refreshed.items[0].favorite);
    }

    #[test]
    fn prepared_albums_share_one_read_until_a_typed_delta_invalidates_it() {
        let source_id = SourceId::new("source:albums");
        let store = Store::open_memory().expect("open store");
        store
            .save_source(&StoredSource {
                source_id: source_id.clone(),
                kind: "local".to_string(),
                name: "Test".to_string(),
                provider_payload: "{}".to_string(),
            })
            .expect("save source");
        let generation = store.begin_sync(&source_id).expect("begin sync");
        let base_revision = store
            .source_cache_revision(&source_id)
            .expect("base revision");
        let album = test_album();
        let album_id = album.id.clone();
        let track = test_track(TrackId::new("track:albums"), &album);
        let commit = store
            .commit_library_sync(
                &source_id,
                generation,
                base_revision,
                LibrarySync {
                    albums: vec![album],
                    tracks: vec![track],
                    artists: Vec::new(),
                    album_artists: Vec::new(),
                    genres: Vec::new(),
                    playlists: Vec::new(),
                    home_sections: Vec::new(),
                    mappings: Vec::new(),
                    coverage: SyncCoverage::All {
                        music_folders: Vec::new(),
                    },
                    local_access: None,
                },
            )
            .expect("commit library");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(source_id.clone());

        let first = ready(
            query
                .prepared_albums(commit.cache_revision)
                .expect("first prepared read"),
        );
        let warm = ready(
            query
                .prepared_albums(commit.cache_revision)
                .expect("warm prepared read"),
        );
        assert!(Arc::ptr_eq(&first.items, &warm.items));
        assert!(!warm.items[0].favorite);

        access
            .with_store(|store| store.set_album_favorite(&source_id, &album_id, true))
            .expect("update favorite");
        let still_warm = ready(
            query
                .prepared_albums(commit.cache_revision)
                .expect("same-revision prepared read"),
        );
        assert!(Arc::ptr_eq(&warm.items, &still_warm.items));
        assert!(!still_warm.items[0].favorite);

        drop(
            query.invalidate_prepared_reads(&LibraryDelta::favorite_changed(
                &FavoriteItemId::Album(album_id),
            )),
        );
        let refreshed = ready(
            query
                .prepared_albums(commit.cache_revision)
                .expect("invalidated prepared read"),
        );
        assert!(!Arc::ptr_eq(&still_warm.items, &refreshed.items));
        assert!(refreshed.items[0].favorite);
    }

    #[test]
    fn accepted_revision_promotes_unaffected_prepared_pages_without_copying_items() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:promote"));
        let album_items = Arc::new(vec![test_album()]);
        let track_items = Arc::new(vec![test_track(
            TrackId::new("track:promote"),
            &album_items[0],
        )]);
        ready(
            query
                .load_prepared_value(4, PreparedReadKey::Albums, || {
                    Ok(PreparedReadValue::Albums(PreparedPage {
                        items: Arc::clone(&album_items),
                        total: album_items.len(),
                    }))
                })
                .expect("seed Albums page"),
        );
        ready(
            query
                .load_prepared_value(
                    4,
                    PreparedReadKey::Tracks {
                        sort: TrackSort::Title,
                        descending: false,
                    },
                    || {
                        Ok(PreparedReadValue::Tracks(PreparedPage {
                            items: Arc::clone(&track_items),
                            total: track_items.len(),
                        }))
                    },
                )
                .expect("seed Tracks page"),
        );

        drop(query.advance_prepared_reads(
            5,
            &LibraryDelta {
                home_changed: true,
                ..LibraryDelta::default()
            },
        ));
        let albums = query
            .prepared_albums_if_cached(5)
            .expect("promoted Albums page");
        let tracks = query
            .prepared_tracks_if_cached(5, TrackSort::Title, false)
            .expect("promoted Tracks page");
        assert!(Arc::ptr_eq(&album_items, &albums.items));
        assert!(Arc::ptr_eq(&track_items, &tracks.items));

        drop(query.advance_prepared_reads(6, &LibraryDelta::default()));
        let albums = query
            .prepared_albums_if_cached(6)
            .expect("Albums page promoted across an empty delta");
        let tracks = query
            .prepared_tracks_if_cached(6, TrackSort::Title, false)
            .expect("Tracks page promoted across an empty delta");
        assert!(Arc::ptr_eq(&album_items, &albums.items));
        assert!(Arc::ptr_eq(&track_items, &tracks.items));
    }

    #[test]
    fn album_track_projection_reuses_only_a_complete_current_prepared_page() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:album-projection"));
        let album = test_album();
        let track = test_track(TrackId::new("track:album-projection"), &album);
        ready(
            query
                .load_prepared_value(
                    9,
                    PreparedReadKey::Tracks {
                        sort: TrackSort::Year,
                        descending: true,
                    },
                    || {
                        Ok(PreparedReadValue::Tracks(PreparedPage {
                            items: Arc::new(vec![track.clone()]),
                            total: 1,
                        }))
                    },
                )
                .expect("seed complete Tracks page"),
        );

        let grouped = query
            .prepared_album_tracks_if_cached(9, std::slice::from_ref(&album.id))
            .expect("complete current Tracks page");
        assert_eq!(grouped.get(&album.id), Some(&vec![track]));
        assert!(
            query
                .prepared_album_tracks_if_cached(8, std::slice::from_ref(&album.id))
                .is_none()
        );

        query
            .prepared
            .cache
            .lock()
            .expect("prepared cache")
            .tracks
            .as_mut()
            .and_then(|entry| match &mut entry.value {
                PreparedReadValue::Tracks(page) => Some(page),
                PreparedReadValue::Albums(_) => None,
            })
            .expect("prepared Tracks page")
            .total = 2;
        assert!(
            query
                .prepared_album_tracks_if_cached(9, std::slice::from_ref(&album.id))
                .is_none()
        );
    }

    #[test]
    fn album_detail_reuses_only_complete_facts_from_the_requested_revision() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:album-detail"));
        let album = test_album();
        let track = test_track(TrackId::new("track:album-detail"), &album);
        ready(
            query
                .load_prepared_value(9, PreparedReadKey::Albums, || {
                    Ok(PreparedReadValue::Albums(PreparedPage {
                        items: Arc::new(vec![album.clone()]),
                        total: 1,
                    }))
                })
                .expect("seed Albums page"),
        );
        ready(
            query
                .load_prepared_value(
                    9,
                    PreparedReadKey::Tracks {
                        sort: TrackSort::Title,
                        descending: false,
                    },
                    || {
                        Ok(PreparedReadValue::Tracks(PreparedPage {
                            items: Arc::new(vec![track.clone()]),
                            total: 1,
                        }))
                    },
                )
                .expect("seed Tracks page"),
        );

        let (loaded_album, loaded_tracks) = query
            .album_detail_for_revision(9, &album.id)
            .expect("prepared detail")
            .expect("prepared album exists");
        assert_eq!(loaded_album, album);
        assert_eq!(loaded_tracks, vec![track]);
        assert!(
            query
                .album_detail_for_revision(8, &loaded_album.id)
                .expect("stale revision falls back to Store")
                .is_none()
        );
    }

    #[test]
    fn invalidation_releases_shared_cache_arcs_before_deferring_unique_payloads() {
        let tracks = Arc::new(Vec::new());
        let active_projection = Arc::clone(&tracks);
        let shared = super::PreparedReadEvictions(vec![PreparedReadValue::Tracks(PreparedPage {
            items: tracks,
            total: 0,
        })])
        .release_shared_references();

        assert!(shared.is_empty());
        assert_eq!(Arc::strong_count(&active_projection), 1);

        let unique = super::PreparedReadEvictions(vec![PreparedReadValue::Tracks(PreparedPage {
            items: Arc::new(Vec::new()),
            total: 0,
        })])
        .release_shared_references();
        assert!(!unique.is_empty());
    }

    #[test]
    fn concurrent_track_waiters_share_one_gated_materialization() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:gated"));
        let key = PreparedReadKey::Tracks {
            sort: TrackSort::Title,
            descending: false,
        };
        let start = Arc::new(Barrier::new(2));
        let loads = Arc::new(AtomicUsize::new(0));

        let run = |query: super::ActiveLibraryQuery| {
            let key = key.clone();
            let start = Arc::clone(&start);
            let loads = Arc::clone(&loads);
            thread::spawn(move || {
                start.wait();
                query
                    .load_prepared_value(7, key, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(50));
                        Ok(empty_tracks_value())
                    })
                    .expect("prepared read")
            })
        };

        let first = run(query.clone());
        let second = run(query);
        let first = first.join().expect("first waiter");
        let second = second.join().expect("second waiter");

        assert_eq!(loads.load(Ordering::SeqCst), 1);
        let PreparedRead::Ready(PreparedReadValue::Tracks(first)) = first else {
            panic!("first waiter returned the wrong cache slot");
        };
        let PreparedRead::Ready(PreparedReadValue::Tracks(second)) = second else {
            panic!("second waiter returned the wrong cache slot");
        };
        assert!(Arc::ptr_eq(&first.items, &second.items));
    }

    #[test]
    fn invalidated_in_flight_read_does_not_reload_its_stale_request() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:invalidated"));
        let key = PreparedReadKey::Tracks {
            sort: TrackSort::Title,
            descending: false,
        };
        let loads = AtomicUsize::new(0);
        let invalidator = query.clone();

        let read = query
            .load_prepared_value(7, key, || {
                loads.fetch_add(1, Ordering::SeqCst);
                drop(
                    invalidator.invalidate_prepared_reads(&LibraryDelta::favorite_changed(
                        &FavoriteItemId::Track(TrackId::new("track:changed")),
                    )),
                );
                Ok(empty_tracks_value())
            })
            .expect("prepared read");

        assert!(matches!(read, PreparedRead::Invalidated));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(
            query
                .prepared_tracks_if_cached(7, TrackSort::Title, false)
                .is_none()
        );
    }

    #[test]
    fn accepted_revision_rejects_an_old_revision_in_flight_load() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:advanced"));
        let key = PreparedReadKey::Tracks {
            sort: TrackSort::Title,
            descending: false,
        };
        let advance = query.clone();

        let read = query
            .load_prepared_value(7, key, || {
                drop(advance.advance_prepared_reads(8, &LibraryDelta::default()));
                Ok(empty_tracks_value())
            })
            .expect("old-revision prepared read");

        assert!(matches!(read, PreparedRead::Invalidated));
        assert!(
            query
                .prepared_tracks_if_cached(7, TrackSort::Title, false)
                .is_none()
        );
        assert!(
            query
                .prepared_tracks_if_cached(8, TrackSort::Title, false)
                .is_none()
        );
    }

    #[test]
    fn superseded_queued_read_exits_before_materializing() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:latest"));
        let stale_key = PreparedReadKey::Tracks {
            sort: TrackSort::Title,
            descending: false,
        };
        let current_key = PreparedReadKey::Tracks {
            sort: TrackSort::Album,
            descending: false,
        };
        let stale_loads = Arc::new(AtomicUsize::new(0));
        let current_loads = Arc::new(AtomicUsize::new(0));
        let gate = query
            .prepared
            .tracks_load
            .lock()
            .expect("hold Tracks load gate");

        let stale = {
            let query = query.clone();
            let key = stale_key.clone();
            let loads = Arc::clone(&stale_loads);
            thread::spawn(move || {
                query
                    .load_prepared_value(9, key, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        Ok(empty_tracks_value())
                    })
                    .expect("stale prepared read")
            })
        };
        wait_for_registered_request(&query, 9, &stale_key);

        let current = {
            let query = query.clone();
            let key = current_key.clone();
            let loads = Arc::clone(&current_loads);
            thread::spawn(move || {
                query
                    .load_prepared_value(9, key, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        Ok(empty_tracks_value())
                    })
                    .expect("current prepared read")
            })
        };
        wait_for_registered_request(&query, 9, &current_key);
        drop(gate);

        assert!(matches!(
            stale.join().expect("stale waiter"),
            PreparedRead::Invalidated
        ));
        assert!(matches!(
            current.join().expect("current waiter"),
            PreparedRead::Ready(PreparedReadValue::Tracks(_))
        ));
        assert_eq!(stale_loads.load(Ordering::SeqCst), 0);
        assert_eq!(current_loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn warm_latest_request_supersedes_a_queued_different_key() {
        let store = Store::open_memory().expect("open store");
        let access = StoreAccess::from_shared(Arc::new(Mutex::new(store)));
        let query = access.query(SourceId::new("source:warm-latest"));
        let warm_key = PreparedReadKey::Tracks {
            sort: TrackSort::Title,
            descending: false,
        };
        let queued_key = PreparedReadKey::Tracks {
            sort: TrackSort::Album,
            descending: false,
        };
        ready(
            query
                .load_prepared_value(11, warm_key.clone(), || Ok(empty_tracks_value()))
                .expect("seed warm prepared read"),
        );
        let queued_loads = Arc::new(AtomicUsize::new(0));
        let gate = query
            .prepared
            .tracks_load
            .lock()
            .expect("hold Tracks load gate");
        let queued = {
            let query = query.clone();
            let key = queued_key.clone();
            let loads = Arc::clone(&queued_loads);
            thread::spawn(move || {
                query
                    .load_prepared_value(11, key, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        Ok(empty_tracks_value())
                    })
                    .expect("queued prepared read")
            })
        };
        wait_for_registered_request(&query, 11, &queued_key);

        assert!(
            query
                .prepared_tracks_if_cached(11, TrackSort::Title, false)
                .is_some()
        );
        wait_for_registered_request(&query, 11, &warm_key);
        drop(gate);

        assert!(matches!(
            queued.join().expect("queued waiter"),
            PreparedRead::Invalidated
        ));
        assert_eq!(queued_loads.load(Ordering::SeqCst), 0);
    }

    fn wait_for_registered_request(
        query: &super::ActiveLibraryQuery,
        revision: i64,
        key: &PreparedReadKey,
    ) {
        for _ in 0..10_000 {
            let prepared = query.prepared.cache.lock().expect("prepared cache");
            let request = match key {
                PreparedReadKey::Albums => prepared.albums_request.as_ref(),
                PreparedReadKey::Tracks { .. } => prepared.tracks_request.as_ref(),
            };
            let registered = request.is_some_and(|request| {
                request.revision == revision
                    && &request.key == key
                    && request.epoch == prepared.epoch(key)
            });
            drop(prepared);
            if registered {
                return;
            }
            thread::yield_now();
        }
        panic!("prepared request was not registered");
    }

    fn ready<T>(read: PreparedRead<T>) -> T {
        match read {
            PreparedRead::Ready(value) => value,
            PreparedRead::Invalidated => panic!("prepared read was unexpectedly invalidated"),
        }
    }

    fn empty_tracks_value() -> PreparedReadValue {
        PreparedReadValue::Tracks(PreparedPage {
            items: Arc::new(Vec::new()),
            total: 0,
        })
    }

    fn test_album() -> Album {
        Album {
            id: AlbumId::new("album:test"),
            title: "Album".to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 180,
            favorite: false,
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }

    fn test_track(id: TrackId, album: &Album) -> Track {
        Track {
            id,
            album_id: album.id.clone(),
            title: "Track".to_string(),
            artist: album.artist.clone(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: album.title.clone(),
            year: album.year,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            album_artwork: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
            bpm: None,
            moods: Vec::new(),
        }
    }
}
