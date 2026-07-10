use super::*;
use source::{
    FolderBrowser, GeneratedTrackProvider, ImageProvider, MusicFolderProvider, MusicSource,
    PlaylistDeleter, RandomTrackProvider, StreamResolver,
};
use std::time::Duration;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
#[tokio::test]
async fn login_map_session() {
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

    let session = SubsonicSource::login(SubsonicLoginRequest {
        base_url: server.uri(),
        username: "demo".to_string(),
        password: "pw".to_string(),
        trust_invalid_cert: false,
        flavor: SubsonicFlavor::Navidrome,
    })
    .await
    .expect("login");

    assert!(session.source.id.as_str().starts_with("navidrome:server:"));
    assert_eq!(session.source.kind, "navidrome");
    assert_eq!(session.source.name, "Navidrome");
    assert_eq!(session.username, "demo");
    assert!(session.credential.contains(':'));
    assert!(!session.credential.contains("pw"));
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
                        "releaseTypes": ["album", "ep", "album"],
                        "isCompilation": false,
                        "musicBrainzId": "mb-album-one",
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
    assert_eq!(page.items[0].release_types, vec!["album", "ep"]);
    assert_eq!(page.items[0].is_compilation, Some(false));
    assert_eq!(
        page.items[0].musicbrainz_album_id.as_deref(),
        Some("mb-album-one")
    );
    assert!(page.items[0].favorite);
}
#[tokio::test]
async fn album_map_meta() {
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
                        "comment": "Warm note",
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
    assert_eq!(detail.tracks[0].comment.as_deref(), Some("Warm note"));
}
#[tokio::test]
async fn random_filter_song() {
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
async fn random_filter_subsonic() {
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

    assert!(matches!(error, SourceError::InvalidRequest(_)));
}

#[tokio::test]
async fn generated_track_radio_uses_similar_songs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getSimilarSongs.view"))
        .and(query_param("id", "track-one"))
        .and(query_param("count", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "similarSongs": {
                    "song": [{
                        "id": "track-two",
                        "albumId": "album-one",
                        "title": "Second Motion",
                        "artist": "Astral Kin",
                        "album": "Blue Rooms",
                        "duration": 180
                    }]
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server);

    let tracks = provider
        .generated_tracks(GeneratedTracksRequest {
            seed: GeneratedTrackSeed::Track(TrackId::new("subsonic:track:track-one")),
            limit: 4,
            strategy: source::GeneratedTrackStrategy::SourceDefault,
        })
        .await
        .expect("generated tracks");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "subsonic:track:track-two");
}

#[tokio::test]
async fn generated_artist_radio_uses_similar_songs2() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getSimilarSongs2.view"))
        .and(query_param("id", "artist-one"))
        .and(query_param("count", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "similarSongs2": {
                    "song": [{
                        "id": "track-two",
                        "albumId": "album-one",
                        "title": "Second Motion",
                        "artist": "Astral Kin",
                        "album": "Blue Rooms",
                        "duration": 180
                    }]
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server);

    let tracks = provider
        .generated_tracks(GeneratedTracksRequest {
            seed: GeneratedTrackSeed::Artist(ArtistId::new("subsonic:artist:artist-one")),
            limit: 4,
            strategy: source::GeneratedTrackStrategy::SourceDefault,
        })
        .await
        .expect("generated tracks");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "subsonic:track:track-two");
}

#[tokio::test]
async fn generated_playlist_radio_uses_first_playlist_track() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getPlaylist.view"))
        .and(query_param("id", "playlist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "playlist": {
                    "id": "playlist-one",
                    "name": "Late Set",
                    "songCount": 1,
                    "entry": [{
                        "id": "track-one",
                        "albumId": "album-one",
                        "title": "First Motion",
                        "artist": "Astral Kin",
                        "album": "Blue Rooms",
                        "duration": 210
                    }]
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getSimilarSongs.view"))
        .and(query_param("id", "track-one"))
        .and(query_param("count", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "similarSongs": {
                    "song": [{
                        "id": "track-two",
                        "albumId": "album-one",
                        "title": "Second Motion",
                        "artist": "Astral Kin",
                        "album": "Blue Rooms",
                        "duration": 180
                    }]
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server);

    let tracks = provider
        .generated_tracks(GeneratedTracksRequest {
            seed: GeneratedTrackSeed::Playlist(domain::PlaylistId::new(
                "subsonic:playlist:playlist-one",
            )),
            limit: 4,
            strategy: source::GeneratedTrackStrategy::SourceDefault,
        })
        .await
        .expect("generated tracks");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "subsonic:track:track-two");
}

#[tokio::test]
async fn stream_url_redacts_subsonic_credentials() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let stream = provider
        .resolve_stream(&StreamRequest::original(TrackId::new(
            "subsonic:track:track-one",
        )))
        .await
        .expect("stream");

    assert!(stream.uri().contains("t=token"));
    assert!(stream.redacted_uri().contains("t=%3Credacted%3E"));
    assert!(!stream.redacted_uri().contains("token"));
}
#[tokio::test]
async fn stream_include_limited() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let stream = provider
        .resolve_stream(&StreamRequest::new(
            TrackId::new("subsonic:track:track-one"),
            domain::StreamQuality::MaxBitrateKbps(192),
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
        .image_bytes(&ImageRef::new("subsonic:cover:cover-one", None), 256)
        .await
        .expect("image bytes");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
}
#[tokio::test]
async fn subsonic_delete_playlist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/deletePlaylist.view"))
        .and(query_param("id", "playlist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server);

    provider
        .delete_playlist(&PlaylistId::new("subsonic:playlist:playlist-one"))
        .await
        .expect("delete playlist");
}
#[tokio::test]
async fn image_bytes_rejects_oversized_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getCoverArt.view"))
        .and(query_param("id", "cover-one"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![0_u8; SUBSONIC_IMAGE_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server);

    let error = provider
        .image_bytes(&ImageRef::new("subsonic:cover:cover-one", None), 256)
        .await
        .expect_err("oversized image");

    assert!(
        error
            .to_string()
            .contains("Subsonic image response exceeded")
    );
}
#[tokio::test]
async fn json_reads_reject_oversized_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2.view"))
        .and(query_param("type", "alphabeticalByName"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![b' '; SUBSONIC_JSON_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server);

    let error = provider
        .albums(PagedRequest::new(0, 1))
        .await
        .expect_err("oversized JSON");

    assert!(
        error
            .to_string()
            .contains("Subsonic JSON response exceeded")
    );
}
#[tokio::test]
async fn subsonic_map_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getUser.view"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_json(serde_json::json!({
                    "subsonic-response": {
                        "status": "ok",
                        "version": "1.16.1",
                        "user": { "username": "demo" }
                    }
                })),
        )
        .mount(&server)
        .await;
    let base_url = normalize_base_url(&server.uri()).expect("base url");
    let credential = SubsonicCredential::from_password("secret");
    let mut url = endpoint(&base_url, "getUser").expect("endpoint");
    url.query_pairs_mut()
        .extend_pairs(credential.common_query("demo", &[("username", "demo")]));
    let client =
        build_client_with_timeouts(false, Duration::from_secs(1), Duration::from_millis(20))
            .expect("client");

    let error = subsonic_json::<AuthenticateBody>(client.get(url))
        .await
        .expect_err("timeout");

    assert!(matches!(error, SourceError::Network(_)));
    assert!(!format!("{error:?}").contains(&credential.salt));
    assert!(!format!("{error:?}").contains(&credential.token));
    assert!(!error.to_string().contains(&credential.salt));
    assert!(!error.to_string().contains(&credential.token));
}
#[tokio::test]
async fn music_load_folders() {
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
async fn in_track_id() {
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
async fn root_use_folder() {
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
async fn folder_track_folders() {
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
fn provider(server: &MockServer) -> SubsonicSource {
    SubsonicSource::from_configured_session(SubsonicConfiguredSession {
        source: SourceIdentity {
            id: SourceId::new("subsonic:server:test"),
            kind: "subsonic".to_string(),
            name: "Subsonic".to_string(),
            base_url: server.uri(),
        },
        username: "demo".to_string(),
        trust_invalid_cert: false,
        credential: "salt:token".to_string(),
    })
    .expect("provider")
}
