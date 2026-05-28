use super::*;
use rufin_provider::MusicProvider;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
#[tokio::test]
async fn login_uses_salted_token_auth_and_maps_session() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getUser.view"))
        .and(query_param("u", "demo"))
        .and(query_param("username", "demo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "type": "Navidrome",
                "user": { "username": "demo" }
            }
        })))
        .mount(&server)
        .await;

    let session = SubsonicProvider::login(SubsonicLoginRequest {
        base_url: server.uri(),
        username: "demo".to_string(),
        password: "pw".to_string(),
        trust_invalid_cert: false,
        flavor: SubsonicFlavor::Navidrome,
    })
    .await
    .expect("login");

    assert!(session.server.id.as_str().starts_with("navidrome:server:"));
    assert_eq!(session.server.provider, "navidrome");
    assert_eq!(session.server.name, "Navidrome");
    assert_eq!(session.username, "demo");
    assert!(session.access_token.contains(':'));
    assert!(!session.access_token.contains("pw"));
}
#[tokio::test]
async fn albums_map_subsonic_album_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2.view"))
        .and(query_param("type", "alphabeticalByName"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "albumList2": {
                    "album": [{
                        "id": "album-one",
                        "name": "Blue Rooms",
                        "artist": "Astral Kin",
                        "artistId": "artist-one",
                        "songCount": 8,
                        "duration": 1800,
                        "year": 2024,
                        "genre": "Ambient",
                        "coverArt": "cover-one",
                        "created": "2024-03-02T09:10:11Z",
                        "played": "2024-04-02T09:10:11Z",
                        "playCount": 12,
                        "userRating": 5,
                        "starred": "2024-01-01T00:00:00Z"
                    }]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let page = provider
        .albums(PagedRequest::new(0, 50))
        .await
        .expect("albums");

    assert_eq!(page.items[0].id.as_str(), "subsonic:album:album-one");
    assert_eq!(page.items[0].title, "Blue Rooms");
    assert_eq!(
        page.items[0].artist_id.as_ref().map(ArtistId::as_str),
        Some("subsonic:artist:artist-one")
    );
    assert_eq!(
        page.items[0]
            .image_ref
            .as_ref()
            .map(|image| image.item_id.as_str()),
        Some("subsonic:cover:cover-one")
    );
    assert_eq!(page.items[0].release_date.as_deref(), Some("2024-01-01"));
    assert_eq!(page.items[0].date_added.as_deref(), Some("2024-03-02"));
    assert_eq!(page.items[0].last_played.as_deref(), Some("2024-04-02"));
    assert_eq!(page.items[0].play_count, Some(12));
    assert_eq!(page.items[0].user_rating, Some(5));
    assert!(page.items[0].favorite);
}
#[tokio::test]
async fn album_detail_maps_subsonic_song_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbum.view"))
        .and(query_param("id", "album-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "album": {
                    "id": "album-one",
                    "name": "Blue Rooms",
                    "artist": "Astral Kin",
                    "artistId": "artist-one",
                    "songCount": 1,
                    "duration": 210,
                    "year": 2024,
                    "song": [{
                        "id": "track-one",
                        "albumId": "album-one",
                        "title": "First Motion",
                        "artist": "Astral Kin",
                        "artistId": "artist-one",
                        "album": "Blue Rooms",
                        "year": 2024,
                        "duration": 210,
                        "discNumber": 1,
                        "track": 1,
                        "created": "2024-03-03T09:10:11Z",
                        "played": "2024-04-03T09:10:11Z",
                        "playCount": 7,
                        "userRating": 4
                    }]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let detail = provider
        .album_detail(&AlbumId::new("subsonic:album:album-one"))
        .await
        .expect("detail");

    assert_eq!(detail.tracks[0].id.as_str(), "subsonic:track:track-one");
    assert_eq!(detail.tracks[0].release_date.as_deref(), Some("2024-01-01"));
    assert_eq!(detail.tracks[0].date_added.as_deref(), Some("2024-03-03"));
    assert_eq!(detail.tracks[0].last_played.as_deref(), Some("2024-04-03"));
    assert_eq!(detail.tracks[0].play_count, Some(7));
    assert_eq!(detail.tracks[0].user_rating, Some(4));
}
#[tokio::test]
async fn random_tracks_use_subsonic_random_song_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getRandomSongs.view"))
        .and(query_param("size", "37"))
        .and(query_param("fromYear", "1999"))
        .and(query_param("toYear", "2001"))
        .and(query_param("genre", "Ambient"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "randomSongs": {
                    "song": [{
                        "id": "track-one",
                        "albumId": "album-one",
                        "title": "First Motion",
                        "artist": "Astral Kin",
                        "album": "Blue Rooms",
                        "year": 2000,
                        "duration": 210,
                        "genre": "Ambient"
                    }]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let tracks = provider
        .random_tracks(RandomTrackRequest {
            limit: 37,
            min_year: Some(1999),
            max_year: Some(2001),
            genre_id: Some(GenreId::new("subsonic:genre:ambient")),
            genre_name: Some("Ambient".to_string()),
            played_filter: PlayedFilter::All,
        })
        .await
        .expect("random tracks");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "subsonic:track:track-one");
    assert_eq!(tracks[0].genres, vec!["Ambient".to_string()]);
}
#[tokio::test]
async fn random_tracks_reject_played_filter_for_subsonic() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let error = provider
        .random_tracks(RandomTrackRequest {
            limit: 10,
            min_year: None,
            max_year: None,
            genre_id: None,
            genre_name: None,
            played_filter: PlayedFilter::Played,
        })
        .await
        .expect_err("unsupported played filter");

    assert!(matches!(error, ProviderError::Unsupported(_)));
}
#[tokio::test]
async fn stream_url_redacts_subsonic_credentials() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let stream = provider
        .stream(&TrackId::new("subsonic:track:track-one"))
        .await
        .expect("stream");

    assert!(stream.uri().contains("t=token"));
    assert!(stream.redacted_uri().contains("t=%3Credacted%3E"));
    assert!(!stream.redacted_uri().contains("token"));
}
#[tokio::test]
async fn stream_url_includes_max_bitrate_when_limited() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let stream = provider
        .stream_with_request(&rufin_provider::StreamRequest::new(
            TrackId::new("subsonic:track:track-one"),
            rufin_core::StreamQuality::MaxBitrateKbps(192),
        ))
        .await
        .expect("stream");

    assert!(stream.uri().contains("maxBitRate=192"));
    assert!(stream.redacted_uri().contains("maxBitRate=192"));
    assert!(!stream.redacted_uri().contains("token"));
}
#[tokio::test]
async fn image_bytes_fetch_cover_art() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt.view"))
        .and(query_param("id", "cover-one"))
        .and(query_param("size", "256"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(vec![1_u8, 2, 3]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server);

    let image = provider
        .image_bytes(ImageRequest {
            item_id: "subsonic:cover:cover-one".to_string(),
            kind: ImageKind::Primary,
            tag: None,
            size: 256,
        })
        .await
        .expect("image bytes");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
}
#[tokio::test]
async fn music_folders_load_subsonic_music_folders() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getMusicFolders.view"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "musicFolders": {
                    "musicFolder": [
                        { "id": 1, "name": "Music" }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let folders = provider.music_folders().await.expect("folders");

    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id.as_str(), "subsonic:music-folder:1");
    assert_eq!(folders[0].name, "Music");
}
#[tokio::test]
async fn tracks_in_music_folder_passes_music_folder_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/search3.view"))
        .and(query_param("musicFolderId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "searchResult3": {
                    "song": [{
                        "id": "track-one",
                        "title": "First Motion",
                        "album": "Blue Rooms",
                        "albumId": "album-one",
                        "artist": "Astral Kin",
                        "artistId": "artist-one",
                        "duration": 210
                    }]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let page = provider
        .tracks_in_music_folder(
            &MusicFolderId::new("subsonic:music-folder:1"),
            PagedRequest::new(0, 50),
        )
        .await
        .expect("tracks");

    assert_eq!(page.items[0].id.as_str(), "subsonic:track:track-one");
}
#[tokio::test]
async fn folder_root_uses_indexes_with_selected_music_folder() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getIndexes.view"))
        .and(query_param("musicFolderId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "indexes": {
                    "index": [
                        {
                            "name": "A",
                            "artist": [
                                { "id": "folder-one", "name": "Albums" }
                            ]
                        }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let detail = provider
        .folder(None, Some(&MusicFolderId::new("subsonic:music-folder:1")))
        .await
        .expect("folder root");

    assert_eq!(detail.parent_id, None);
    assert_eq!(detail.folders[0].id.as_str(), "subsonic:folder:folder-one");
    assert_eq!(detail.folders[0].name, "Albums");
    assert!(detail.tracks.is_empty());
}
#[tokio::test]
async fn folder_nested_music_directory_maps_child_folders_and_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getMusicDirectory.view"))
        .and(query_param("id", "folder-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "directory": {
                    "id": "folder-one",
                    "name": "Albums",
                    "parent": "root",
                    "child": [
                        {
                            "id": "child-folder",
                            "title": "Live",
                            "isDir": true
                        },
                        {
                            "id": "track-two",
                            "title": "Second Motion",
                            "album": "Blue Rooms",
                            "albumId": "album-one",
                            "artist": "Astral Kin",
                            "artistId": "artist-one",
                            "duration": 180,
                            "isDir": false
                        }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;
    let provider = provider(&server);

    let detail = provider
        .folder(Some(&FolderId::new("subsonic:folder:folder-one")), None)
        .await
        .expect("nested folder");

    assert_eq!(detail.folder.name, "Albums");
    assert_eq!(
        detail.parent_id.as_ref().map(|id| id.as_str()),
        Some("subsonic:folder:root")
    );
    assert_eq!(
        detail.folders[0].id.as_str(),
        "subsonic:folder:child-folder"
    );
    assert_eq!(detail.tracks[0].id.as_str(), "subsonic:track:track-two");
}
fn provider(server: &MockServer) -> SubsonicProvider {
    SubsonicProvider::from_saved_session(SavedProviderSession {
        server: ServerIdentity {
            id: ServerId::new("subsonic:server:test"),
            provider: "subsonic".to_string(),
            name: "Subsonic".to_string(),
            base_url: server.uri(),
        },
        user_id: "demo".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
        access_token: "salt:token".to_string(),
    })
    .expect("provider")
}
