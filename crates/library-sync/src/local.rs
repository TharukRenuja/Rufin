use library::{
    Album, Artist, Genre, HomeSection, ImageRef, LocalCueDependency, LocalLibraryDelta,
    LocalManifestDelta, PagedResponse, SourceId, Store, Track,
};
use sources::local::LocalManifestScan;
use sources::{MusicSource, PageState, PagedRequest};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use crate::{
    Collection, Progress, SyncAttempt, SyncError, SyncOutcome, SyncResult, await_source,
    check_cancelled,
};

const PAGE_SIZE: usize = 500;

#[derive(Default)]
struct LocalSnapshot {
    tracks: Vec<Track>,
    albums: Vec<Album>,
    artists: Vec<Artist>,
    album_artists: Vec<Artist>,
    genres: Vec<Genre>,
    home_sections: Vec<HomeSection>,
}

pub struct LocalObservation {
    change: LocalChange,
    cue_dependencies: Vec<LocalCueDependency>,
}

enum LocalChange {
    Delta(Box<LocalLibraryDelta>),
    Unchanged(LocalSnapshot),
}

pub async fn acquire_local(
    source: &(dyn MusicSource + Send + Sync),
    scan: &LocalManifestScan,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> SyncResult<LocalObservation> {
    check_cancelled(cancelled)?;
    let snapshot = collect_snapshot(source, cancelled, progress).await?;
    check_cancelled(cancelled)?;
    let changed = scan.library_changed
        || !scan.changed_manifest_paths.is_empty()
        || !scan.deleted_paths.is_empty();
    let change = if changed {
        LocalChange::Delta(Box::new(local_delta(scan, snapshot)))
    } else {
        LocalChange::Unchanged(snapshot)
    };
    Ok(LocalObservation {
        change,
        cue_dependencies: scan.cue_dependencies.clone(),
    })
}

pub fn commit_local(
    attempt: &mut SyncAttempt<'_>,
    complete_coverage: bool,
    observation: LocalObservation,
) -> SyncResult<SyncOutcome> {
    let cancellation = attempt.cancellation;
    let cancelled = &|| cancellation.is_cancelled();
    check_cancelled(cancelled)?;
    let LocalObservation {
        change,
        cue_dependencies,
    } = observation;
    let (mut delta, unchanged_scan) = match change {
        LocalChange::Delta(delta) => (*delta, false),
        LocalChange::Unchanged(snapshot) => {
            let delta = aggregate_artwork_delta(attempt.store, attempt.source_id, snapshot)?;
            (delta, true)
        }
    };
    if !complete_coverage && unchanged_scan && aggregate_delta_is_empty(&delta) {
        return Ok(SyncOutcome::Ignored);
    }
    delta.cue_dependencies = cue_dependencies;
    (attempt.progress)(Progress::Finalizing);
    if !cancellation.can_commit() {
        return Err(SyncError::Cancelled);
    }
    let commit = attempt.store.commit_local_library_delta(
        attempt.source_id,
        attempt.generation,
        attempt.base_cache_revision,
        complete_coverage,
        delta,
    )?;
    (attempt.progress)(Progress::Finished);
    Ok(SyncOutcome::Committed(Box::new(commit)))
}

fn aggregate_delta_is_empty(delta: &LocalLibraryDelta) -> bool {
    delta.dirty_albums.is_empty()
        && delta.dirty_artists.is_empty()
        && delta.dirty_album_artists.is_empty()
}

fn aggregate_artwork_delta(
    store: &Store,
    source_id: &SourceId,
    snapshot: LocalSnapshot,
) -> SyncResult<LocalLibraryDelta> {
    let album_refs = store.load_raw_album_image_refs(source_id)?;
    let artist_refs = store.load_raw_artist_image_refs(source_id, false)?;
    let album_artist_refs = store.load_raw_artist_image_refs(source_id, true)?;
    let dirty_albums = changed_images(snapshot.albums.iter(), &album_refs, |album| {
        (&album.id, &album.image_ref)
    });
    let dirty_artists = changed_images(snapshot.artists.iter(), &artist_refs, |artist| {
        (&artist.id, &artist.image_ref)
    });
    let dirty_album_artists = changed_images(
        snapshot.album_artists.iter(),
        &album_artist_refs,
        |artist| (&artist.id, &artist.image_ref),
    );
    Ok(LocalLibraryDelta {
        current_album_ids: snapshot
            .albums
            .iter()
            .map(|album| album.id.clone())
            .collect(),
        current_artist_ids: snapshot
            .artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_album_artist_ids: snapshot
            .album_artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_genre_ids: snapshot
            .genres
            .iter()
            .map(|genre| genre.id.clone())
            .collect(),
        dirty_albums,
        dirty_artists,
        dirty_album_artists,
        home_sections: snapshot.home_sections,
        ..LocalLibraryDelta::default()
    })
}

fn changed_images<'a, K, T>(
    items: impl IntoIterator<Item = &'a T>,
    cached: &HashMap<K, Option<ImageRef>>,
    image: impl Fn(&'a T) -> (&'a K, &'a Option<ImageRef>),
) -> Vec<T>
where
    K: Eq + Hash + 'a,
    T: Clone + 'a,
{
    items
        .into_iter()
        .filter(|item| {
            let (id, image_ref) = image(item);
            cached.get(id) != Some(image_ref)
        })
        .cloned()
        .collect()
}

async fn collect_snapshot(
    source: &(dyn MusicSource + Send + Sync),
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> SyncResult<LocalSnapshot> {
    progress(Progress::CollectionStarted(Collection::Tracks));
    let tracks = load_all(cancelled, |request| source.tracks(request)).await?;
    progress(Progress::CollectionStarted(Collection::Albums));
    let albums = load_all(cancelled, |request| source.albums(request)).await?;
    progress(Progress::CollectionStarted(Collection::Artists));
    let artist_collections = await_source(cancelled, source.artist_collections()).await?;
    progress(Progress::CollectionStarted(Collection::AlbumArtists));
    progress(Progress::CollectionStarted(Collection::Genres));
    let genres = await_source(cancelled, source.genres()).await?;
    progress(Progress::CollectionStarted(Collection::HomeSections));
    let home_sections = await_source(cancelled, source.home_sections()).await?;
    Ok(LocalSnapshot {
        tracks,
        albums,
        artists: artist_collections.artists,
        album_artists: artist_collections.album_artists,
        genres,
        home_sections,
    })
}

async fn load_all<T, F, Fut>(cancelled: &dyn Fn() -> bool, mut page: F) -> SyncResult<Vec<T>>
where
    F: FnMut(PagedRequest) -> Fut,
    Fut: std::future::Future<Output = sources::SourceResult<PagedResponse<T>>>,
{
    let mut items = Vec::new();
    let mut pages = PageState::default();
    loop {
        let response = await_source(cancelled, page(pages.request(PAGE_SIZE))).await?;
        let count = response.items.len();
        let finished = pages
            .add(count, (response.total > 0).then_some(response.total))
            .ok_or(SyncError::Unstable)?;
        items.extend(response.items);
        if finished {
            return Ok(items);
        }
    }
}

fn local_delta(scan: &LocalManifestScan, snapshot: LocalSnapshot) -> LocalLibraryDelta {
    let changed_manifest_paths = scan.changed_manifest_paths.iter().collect::<HashSet<_>>();
    let changed_track_ids = scan.changed_track_ids.iter().collect::<HashSet<_>>();
    let dirty_album_ids = scan.dirty_album_ids.iter().collect::<HashSet<_>>();
    let dirty_artist_ids = scan.dirty_artist_ids.iter().collect::<HashSet<_>>();
    let dirty_album_artist_ids = scan.dirty_album_artist_ids.iter().collect::<HashSet<_>>();
    let dirty_genre_names = scan.dirty_genre_names.iter().collect::<HashSet<_>>();

    LocalLibraryDelta {
        tracks: snapshot
            .tracks
            .iter()
            .filter(|track| changed_track_ids.contains(&track.id))
            .cloned()
            .collect(),
        deleted_track_ids: scan.deleted_track_ids.clone(),
        current_album_ids: snapshot
            .albums
            .iter()
            .map(|album| album.id.clone())
            .collect(),
        current_artist_ids: snapshot
            .artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_album_artist_ids: snapshot
            .album_artists
            .iter()
            .map(|artist| artist.id.clone())
            .collect(),
        current_genre_ids: snapshot
            .genres
            .iter()
            .map(|genre| genre.id.clone())
            .collect(),
        dirty_albums: snapshot
            .albums
            .into_iter()
            .filter(|album| dirty_album_ids.contains(&album.id))
            .collect(),
        dirty_artists: snapshot
            .artists
            .into_iter()
            .filter(|artist| dirty_artist_ids.contains(&artist.id))
            .collect(),
        dirty_album_artists: snapshot
            .album_artists
            .into_iter()
            .filter(|artist| dirty_album_artist_ids.contains(&artist.id))
            .collect(),
        dirty_genres: snapshot
            .genres
            .into_iter()
            .filter(|genre| dirty_genre_names.contains(&genre.name))
            .collect(),
        home_sections: snapshot.home_sections,
        manifest: LocalManifestDelta {
            upserted_entries: scan
                .entries
                .iter()
                .filter(|entry| changed_manifest_paths.contains(&entry.facts.path))
                .cloned()
                .collect(),
            deleted_paths: scan.deleted_paths.clone(),
        },
        cue_track_sources: scan.cue_track_sources.clone(),
        cue_dependencies: scan.cue_dependencies.clone(),
    }
}
