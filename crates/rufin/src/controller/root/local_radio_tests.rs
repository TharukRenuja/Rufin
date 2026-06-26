use super::*;

use domain::{AlbumId, ArtistCredit, ArtistId, GeneratedTrackSeed, ImageRef};
use std::collections::{HashMap, HashSet};

use super::test_support::{
    controller_from_store_for_test, library_track, local_album_with_image_ref,
    local_track_with_image_ref, seed_cached_library,
};

#[test]
pub(in crate::controller) fn local_generated_radio_uses_cached_fallback_candidates() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let artist = ArtistId::new("local:artist:primary");
    let album_artist = ArtistId::new("local:artist:album");
    let other_artist = ArtistId::new("local:artist:other");
    let albums = (1..=4)
        .map(|number| {
            let mut album = local_album_with_image_ref(ImageRef::new(
                format!("local:cover:album-{number}"),
                None,
            ));
            album.id = AlbumId::new(format!("local:album:{number}"));
            album.artist_id = Some(if number == 3 {
                album_artist.clone()
            } else if number == 4 {
                other_artist.clone()
            } else {
                artist.clone()
            });
            album.genres = if number <= 2 {
                vec!["Shared".to_string()]
            } else {
                Vec::new()
            };
            album
        })
        .collect::<Vec<_>>();
    let mut seed = local_track_with_image_ref(1, &albums[0], ImageRef::new("local:cover:1", None));
    seed.artist_id = Some(artist.clone());
    seed.album_artist_credits = vec![ArtistCredit {
        id: album_artist.clone(),
        name: "Album Artist".to_string(),
        musicbrainz_artist_id: None,
    }];
    seed.genres = vec!["Shared".to_string()];
    let mut same_genre =
        local_track_with_image_ref(2, &albums[1], ImageRef::new("local:cover:2", None));
    same_genre.artist_id = Some(other_artist.clone());
    same_genre.genres = vec!["Shared".to_string()];
    let mut same_artist =
        local_track_with_image_ref(3, &albums[2], ImageRef::new("local:cover:3", None));
    same_artist.artist_id = Some(album_artist);
    let mut fallback =
        local_track_with_image_ref(4, &albums[3], ImageRef::new("local:cover:4", None));
    fallback.artist_id = Some(other_artist);
    let tracks = vec![
        seed.clone(),
        same_genre.clone(),
        same_artist.clone(),
        fallback.clone(),
    ];
    seed_cached_library(&store, &local, &albums, &tracks, &[]);
    let (controller, _events) = controller_from_store_for_test(store);

    let generated = controller
        .generated_tracks_for_saved(&local, GeneratedTrackSeed::Track(seed.id.clone()), 3)
        .expect("generated local radio");
    let generated_ids = generated
        .iter()
        .map(|track| track.id.clone())
        .collect::<HashSet<_>>();

    assert_eq!(generated.len(), 3);
    assert!(generated_ids.contains(&same_genre.id));
    assert!(generated_ids.contains(&same_artist.id));
    assert!(generated_ids.contains(&fallback.id));
    assert!(!generated_ids.contains(&seed.id));
}

#[test]
pub(in crate::controller) fn local_artist_radio_spreads_cached_candidates_across_albums() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let artist = ArtistId::new("local:artist:many-albums");
    let artist_albums = (1..=9)
        .map(|number| {
            let mut album = local_album_with_image_ref(ImageRef::new(
                format!("local:cover:spread-album-{number}"),
                None,
            ));
            album.id = AlbumId::new(format!("local:album:spread-{number}"));
            album.title = format!("Spread Album {number}");
            album.artist_id = Some(artist.clone());
            album.track_count = 3;
            album
        })
        .collect::<Vec<_>>();
    let unrelated_artist = ArtistId::new("local:artist:unrelated");
    let unrelated_albums = (1..=5)
        .map(|number| {
            let mut album = local_album_with_image_ref(ImageRef::new(
                format!("local:cover:unrelated-album-{number}"),
                None,
            ));
            album.id = AlbumId::new(format!("local:album:unrelated-{number}"));
            album.title = format!("Unrelated Album {number}");
            album.artist_id = Some(unrelated_artist.clone());
            album
        })
        .collect::<Vec<_>>();
    let mut albums = artist_albums.clone();
    albums.extend(unrelated_albums.clone());
    let mut tracks = artist_albums
        .iter()
        .enumerate()
        .flat_map(|(album_index, album)| {
            let artist = artist.clone();
            (1..=3).map(move |track_index| {
                let number = (album_index as u32 * 10) + track_index;
                let mut track = local_track_with_image_ref(
                    number,
                    album,
                    ImageRef::new(format!("local:cover:spread-track-{number}"), None),
                );
                track.artist_id = Some(artist.clone());
                track
            })
        })
        .collect::<Vec<_>>();
    tracks.extend(
        unrelated_albums
            .iter()
            .enumerate()
            .map(|(album_index, album)| {
                let number = 100 + album_index as u32;
                let mut track = local_track_with_image_ref(
                    number,
                    album,
                    ImageRef::new(format!("local:cover:unrelated-track-{number}"), None),
                );
                track.artist_id = Some(unrelated_artist.clone());
                track
            }),
    );
    seed_cached_library(&store, &local, &albums, &tracks, &[]);
    let (controller, _events) = controller_from_store_for_test(store);

    let generated = controller
        .generated_tracks_for_saved(&local, GeneratedTrackSeed::Artist(artist.clone()), 12)
        .expect("generated local artist radio");
    let mut album_counts = HashMap::<AlbumId, usize>::new();
    for track in &generated {
        *album_counts.entry(track.album_id.clone()).or_default() += 1;
    }

    assert_eq!(generated.len(), 12);
    assert!(album_counts.len() >= 5);
    assert!(album_counts.values().any(|count| *count > 1));
    assert!(album_counts.values().all(|count| *count <= 3));
    assert!(
        generated
            .iter()
            .all(|track| track.artist_id.as_ref() == Some(&artist))
    );
}

#[test]
pub(in crate::controller) fn local_auto_dj_preserves_cached_track_paths() {
    let store = StoreHandle::open_memory().expect("memory store");
    let local = local_source_saved();
    let tracks = (1..=7)
        .map(|number| {
            let mut track = library_track(
                number,
                Some(ArtistId::fake(number)),
                AlbumId::fake(number),
                &format!("Artist {number}"),
                &[],
            );
            track.local_path = Some(format!("/music/album-{number}/track.flac"));
            track.source_format = Some("flac".to_string());
            track
        })
        .collect::<Vec<_>>();
    seed_cached_library(&store, &local, &[], &tracks, &[]);
    let (controller, _events) = controller_from_store_for_test(store.clone());
    let mut queue = QueueEngine::new(local.server.id.clone());
    queue.play_now(&tracks[0]);
    *controller.queue.lock().expect("queue") = Some(queue);
    *controller.auto_dj_enabled.lock().expect("auto dj") = true;

    assert!(controller.auto_dj_topup());

    for track in &tracks {
        let local_path = store
            .with_store(|store| store.track_local_path(&local.server.id, &track.id))
            .expect("local path");
        let source_format = store
            .with_store(|store| store.track_source_format(&local.server.id, &track.id))
            .expect("source format");
        assert_eq!(local_path, track.local_path);
        assert_eq!(source_format, track.source_format);
    }
}
