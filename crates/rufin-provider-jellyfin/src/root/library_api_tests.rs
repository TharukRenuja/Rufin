use super::*;
use rufin_provider::MusicProvider;
use wiremock::matchers::{body_json, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
#[tokio::test]
async fn library_map_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .and(header_regex(
            "authorization",
            "MediaBrowser Client=\"Rufin\", Device=\"Rufin\"",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "AccessToken": "secret-token",
            "ServerId": "server-one",
            "User": { "Id": "user-one", "Name": "demo" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ServerName": "Music Box"
        })))
        .mount(&server)
        .await;

    let session = JellyfinProvider::login(LoginRequest {
        base_url: server.uri(),
        username: "demo".to_string(),
        password: "pw".to_string(),
        trust_invalid_cert: false,
        device_id: Some("rufin-install-one".to_string()),
    })
    .await
    .expect("login");

    assert_eq!(session.server.id.as_str(), "jellyfin:server:server-one");
    assert_eq!(session.server.name, "Music Box");
    assert_eq!(session.username, "demo");
    assert_eq!(session.access_token, "secret-token");
    assert_eq!(session.device_id.as_deref(), Some("rufin-install-one"));
}
#[test]
fn library_bare_http() {
    let url = normalize_base_url("music.local:8096").expect("normalized url");

    assert_eq!(url.as_str(), "http://music.local:8096/");
}
#[tokio::test]
async fn library_map_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("IncludeItemTypes", "MusicAlbum"))
        .and(query_param("StartIndex", "5"))
        .and(query_param("Limit", "2"))
        .and(header_regex("authorization", "Token=\"token-one\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 20,
                "Items": [{
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin",
                "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
                "ArtistItems": [{ "Id": "guest-one", "Name": "Guest Artist" }],
                "Genres": ["Ambient", "Electronic"],
                "ProductionYear": 2024,
                "PremiereDate": "2024-03-01T00:00:00.0000000Z",
                "DateCreated": "2024-03-02T09:10:11.0000000Z",
                "ChildCount": 9,
                "RunTimeTicks": 1800000000i64,
                "UserData": {
                    "IsFavorite": true,
                    "PlayCount": 12,
                    "LastPlayedDate": "2024-04-02T09:10:11.0000000Z",
                    "Rating": 5
                },
                "ImageTags": { "Primary": "album-tag-one" }
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let page = provider
        .albums(PagedRequest::new(5, 2))
        .await
        .expect("albums");

    assert_eq!(page.total, 20);
    assert_eq!(page.items[0].id.as_str(), "jellyfin:album:album-one");
    assert_eq!(page.items[0].title, "Blue Rooms");
    assert_eq!(
        page.items[0].artist_id.as_ref().map(ArtistId::as_str),
        Some("jellyfin:artist:album-artist-one")
    );
    assert_eq!(
        page.items[0].album_artist_credits[0],
        ArtistCredit {
            id: ArtistId::new("jellyfin:artist:album-artist-one"),
            name: "Astral Kin".to_string(),
        }
    );
    assert_eq!(
        page.items[0].artist_credits[0],
        ArtistCredit {
            id: ArtistId::new("jellyfin:artist:guest-one"),
            name: "Guest Artist".to_string(),
        }
    );
    assert_eq!(page.items[0].genres, vec!["Ambient", "Electronic"]);
    assert_eq!(page.items[0].release_date.as_deref(), Some("2024-03-01"));
    assert_eq!(page.items[0].date_added.as_deref(), Some("2024-03-02"));
    assert_eq!(page.items[0].last_played.as_deref(), Some("2024-04-02"));
    assert_eq!(page.items[0].play_count, Some(12));
    assert_eq!(page.items[0].user_rating, Some(5));
    assert_eq!(
        page.items[0].image_ref,
        Some(ImageRef {
            item_id: "jellyfin:album:album-one".to_string(),
            tag: Some("album-tag-one".to_string()),
        })
    );
    assert!(page.items[0].favorite);
}
#[tokio::test]
async fn library_album_artist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("IncludeItemTypes", "MusicAlbum"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin",
                "ArtistItems": [{ "Id": "guest-one", "Name": "Guest Artist" }],
                "ChildCount": 9
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let page = provider
        .albums(PagedRequest::new(0, 50))
        .await
        .expect("albums");

    assert_eq!(page.items[0].artist, "Astral Kin");
    assert!(page.items[0].artist_id.is_none());
    assert!(page.items[0].album_artist_credits.is_empty());
    assert_eq!(
        page.items[0].artist_credits,
        vec![ArtistCredit {
            id: ArtistId::new("jellyfin:artist:guest-one"),
            name: "Guest Artist".to_string(),
        }]
    );
}
#[tokio::test]
async fn library_image_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/album-one/Images/Primary"))
        .and(query_param("fillWidth", "256"))
        .and(query_param("fillHeight", "256"))
        .and(query_param("quality", "90"))
        .and(query_param("tag", "album-tag-one"))
        .and(header_regex("authorization", "Token=\"secret-token\""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(vec![1_u8, 2, 3]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server, "secret-token");

    let image = provider
        .image_bytes(ImageRequest {
            item_id: "jellyfin:album:album-one".to_string(),
            kind: ImageKind::Primary,
            tag: Some("album-tag-one".to_string()),
            size: 256,
        })
        .await
        .expect("image bytes");

    assert_eq!(image.bytes, vec![1, 2, 3]);
    assert_eq!(image.content_type.as_deref(), Some("image/jpeg"));
}
#[tokio::test]
async fn image_bytes_rejects_oversized_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/album-one/Images/Primary"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![0_u8; JELLYFIN_IMAGE_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server, "secret-token");

    let error = provider
        .image_bytes(ImageRequest {
            item_id: "jellyfin:album:album-one".to_string(),
            kind: ImageKind::Primary,
            tag: None,
            size: 256,
        })
        .await
        .expect_err("oversized image");

    assert!(
        error
            .to_string()
            .contains("Jellyfin image response exceeded")
    );
}
#[tokio::test]
async fn json_reads_reject_oversized_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![b' '; JELLYFIN_JSON_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let error = provider
        .albums(PagedRequest::new(0, 1))
        .await
        .expect_err("oversized JSON");

    assert!(
        error
            .to_string()
            .contains("Jellyfin JSON response exceeded")
    );
}
#[tokio::test]
async fn library_image_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/album-one/Images/Primary"))
        .respond_with(ResponseTemplate::new(500).set_body_string("broken"))
        .mount(&server)
        .await;
    let provider = provider(&server, "secret-token");

    let error = provider
        .image_bytes(ImageRequest {
            item_id: "jellyfin:album:album-one".to_string(),
            kind: ImageKind::Primary,
            tag: None,
            size: 256,
        })
        .await
        .expect_err("image error");

    assert!(!format!("{error:?}").contains("secret-token"));
    assert!(!error.to_string().contains("secret-token"));
}
#[tokio::test]
async fn library_track_album() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/album-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "album-one",
            "Name": "Blue Rooms",
            "Type": "MusicAlbum",
            "AlbumArtist": "Astral Kin",
            "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
            "ChildCount": 1
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("ParentId", "album-one"))
        .and(query_param("IncludeItemTypes", "Audio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "AlbumPrimaryImageTag": "album-tag-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "Overview": "Warm note",
                "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
                "ArtistItems": [{ "Id": "artist-one", "Name": "Astral Kin" }],
                "ProductionYear": 2024,
                "PremiereDate": "2024-03-01T00:00:00.0000000Z",
                "DateCreated": "2024-03-03T09:10:11.0000000Z",
                "UserData": {
                    "PlayCount": 7,
                    "LastPlayedDate": "2024-04-03T09:10:11.0000000Z",
                    "Rating": 4
                },
                "IndexNumber": 1,
                "ImageTags": { "Primary": "track-tag-one" },
                "RunTimeTicks": 2100000000i64
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let detail = provider
        .album_detail(&AlbumId::new("jellyfin:album:album-one"))
        .await
        .expect("detail");

    assert_eq!(detail.album.id.as_str(), "jellyfin:album:album-one");
    assert_eq!(
        detail.tracks[0].album_id.as_str(),
        "jellyfin:album:album-one"
    );
    assert_eq!(
        detail.tracks[0].artist_credits[0].id.as_str(),
        "jellyfin:artist:artist-one"
    );
    assert_eq!(
        detail.tracks[0].album_artist_credits[0].id.as_str(),
        "jellyfin:artist:album-artist-one"
    );
    assert_eq!(detail.tracks[0].release_date.as_deref(), Some("2024-03-01"));
    assert_eq!(detail.tracks[0].date_added.as_deref(), Some("2024-03-03"));
    assert_eq!(detail.tracks[0].last_played.as_deref(), Some("2024-04-03"));
    assert_eq!(detail.tracks[0].play_count, Some(7));
    assert_eq!(detail.tracks[0].user_rating, Some(4));
    assert_eq!(detail.tracks[0].comment.as_deref(), Some("Warm note"));
    assert_eq!(detail.tracks[0].duration_seconds, 210);
    assert_eq!(
        detail.tracks[0].image_ref,
        Some(ImageRef {
            item_id: "jellyfin:album:album-one".to_string(),
            tag: Some("album-tag-one".to_string()),
        })
    );
}
#[tokio::test]
async fn library_load_views() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Users/user-one/Views"))
        .and(query_param("IncludeExternalContent", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Items": [
                {
                    "Id": "music-one",
                    "Name": "Music",
                    "CollectionType": "music"
                },
                {
                    "Id": "movies-one",
                    "Name": "Movies",
                    "CollectionType": "movies"
                }
            ]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let folders = provider.music_folders().await.expect("folders");

    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id.as_str(), "jellyfin:music-folder:music-one");
    assert_eq!(folders[0].name, "Music");
}
#[tokio::test]
async fn library_scope_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("ParentId", "music-one"))
        .and(query_param("IncludeItemTypes", "Audio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "AlbumArtists": [{ "Id": "album-artist-one", "Name": "Astral Kin" }],
                "ArtistItems": [{ "Id": "artist-one", "Name": "Astral Kin" }],
                "IndexNumber": 1,
                "RunTimeTicks": 2100000000i64
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let page = provider
        .tracks_in_music_folder(
            &MusicFolderId::new("jellyfin:music-folder:music-one"),
            PagedRequest::new(0, 50),
        )
        .await
        .expect("tracks");

    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id.as_str(), "jellyfin:track:track-one");
}
#[tokio::test]
async fn library_folder_views() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Users/user-one/Views"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Items": [
                { "Id": "music-one", "Name": "Music", "CollectionType": "music" },
                { "Id": "movies-one", "Name": "Movies", "CollectionType": "movies" }
            ]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let detail = provider.folder(None, None).await.expect("folder root");

    assert_eq!(detail.parent_id, None);
    assert_eq!(detail.folders.len(), 1);
    assert_eq!(detail.folders[0].id.as_str(), "jellyfin:folder:music-one");
    assert_eq!(detail.folders[0].name, "Music");
    assert!(detail.tracks.is_empty());
}
#[tokio::test]
async fn library_load_child() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/music-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "music-one",
            "Name": "Music",
            "Type": "CollectionFolder"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("ParentId", "music-one"))
        .and(query_param("Recursive", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Items": [
                {
                    "Id": "folder-one",
                    "Name": "Albums",
                    "Type": "Folder",
                    "ParentId": "music-one"
                },
                {
                    "Id": "track-one",
                    "Name": "First Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "RunTimeTicks": 2100000000i64
                }
            ]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let detail = provider
        .folder(
            None,
            Some(&MusicFolderId::new("jellyfin:music-folder:music-one")),
        )
        .await
        .expect("selected root");

    assert_eq!(detail.folder.id.as_str(), "jellyfin:folder:music-one");
    assert_eq!(detail.parent_id, None);
    assert_eq!(detail.folders[0].id.as_str(), "jellyfin:folder:folder-one");
    assert_eq!(detail.tracks[0].id.as_str(), "jellyfin:track:track-one");
}
#[tokio::test]
async fn library_track_folders() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/folder-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "folder-one",
            "Name": "Albums",
            "Type": "Folder",
            "ParentId": "music-one"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("ParentId", "folder-one"))
        .and(query_param("Recursive", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Items": [
                {
                    "Id": "child-folder",
                    "Name": "Live",
                    "Type": "Folder",
                    "ParentId": "folder-one"
                },
                {
                    "Id": "track-two",
                    "Name": "Second Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "RunTimeTicks": 1800000000i64
                }
            ]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let detail = provider
        .folder(
            Some(&FolderId::new("jellyfin:folder:folder-one")),
            Some(&MusicFolderId::new("jellyfin:music-folder:music-one")),
        )
        .await
        .expect("nested folder");

    assert_eq!(detail.folder.name, "Albums");
    assert_eq!(detail.parent_id, None);
    assert_eq!(
        detail.folders[0].id.as_str(),
        "jellyfin:folder:child-folder"
    );
    assert_eq!(detail.tracks[0].id.as_str(), "jellyfin:track:track-two");
}
#[tokio::test]
async fn library_map_counts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Artists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "artist-one",
                "Name": "Astral Kin",
                "ItemCounts": {
                    "AlbumCount": 4,
                    "SongCount": 30
                },
                "UserData": {
                    "IsFavorite": true,
                    "PlayCount": 22,
                    "LastPlayedDate": "2024-05-03T09:10:11.0000000Z",
                    "Rating": 3
                }
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let artists = provider
        .artists(PagedRequest::new(0, 50))
        .await
        .expect("artists");

    assert_eq!(artists.items[0].id.as_str(), "jellyfin:artist:artist-one");
    assert_eq!(artists.items[0].album_count, 4);
    assert_eq!(artists.items[0].track_count, 30);
    assert_eq!(artists.items[0].last_played.as_deref(), Some("2024-05-03"));
    assert_eq!(artists.items[0].play_count, Some(22));
    assert_eq!(artists.items[0].user_rating, Some(3));
    assert!(artists.items[0].favorite);
}
#[tokio::test]
async fn library_scope_music() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/MusicGenres"))
        .and(query_param("IncludeItemTypes", "Audio,MusicAlbum"))
        .and(query_param("StartIndex", "3"))
        .and(query_param("Limit", "7"))
        .and(header_regex("authorization", "Token=\"token-one\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "genre-one",
                "Name": "Dream Pop",
                "Type": "MusicGenre",
                "ItemCounts": {
                    "AlbumCount": 4,
                    "SongCount": 31
                },
                "ImageTags": { "Primary": "genre-tag" }
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let genres = provider
        .genres(PagedRequest::new(3, 7))
        .await
        .expect("genres");

    assert_eq!(genres.total, 1);
    assert_eq!(genres.items[0].id.as_str(), "jellyfin:genre:genre-one");
    assert_eq!(genres.items[0].name, "Dream Pop");
    assert_eq!(genres.items[0].album_count, 4);
    assert_eq!(genres.items[0].track_count, 31);
    assert_eq!(
        genres.items[0].image_ref,
        Some(ImageRef {
            item_id: "jellyfin:genre:genre-one".to_string(),
            tag: Some("genre-tag".to_string()),
        })
    );
}
#[tokio::test]
async fn library_filter_sort() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("UserId", "user-one"))
        .and(query_param("Recursive", "true"))
        .and(query_param("IncludeItemTypes", "Audio"))
        .and(query_param("StartIndex", "0"))
        .and(query_param("Limit", "37"))
        .and(query_param("SortBy", "Random"))
        .and(query_param("Years", "1999,2000,2001"))
        .and(query_param("GenreIds", "genre-one"))
        .and(query_param("IsPlayed", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 1,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "Genres": ["Ambient"],
                "ProductionYear": 2000,
                "RunTimeTicks": 2100000000i64
            }]
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let tracks = provider
        .random_tracks(RandomTrackRequest {
            limit: 37,
            min_year: Some(1999),
            max_year: Some(2001),
            genre_id: Some(GenreId::new("jellyfin:genre:genre-one")),
            genre_name: Some("Ambient".to_string()),
            played_filter: PlayedFilter::Unplayed,
        })
        .await
        .expect("random tracks");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "jellyfin:track:track-one");
    assert_eq!(tracks[0].year, 2000);
}
#[tokio::test]
async fn library_track_ordered() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/playlist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "playlist-one",
            "Name": "Late Set",
            "Type": "Playlist",
            "ChildCount": 501,
            "RunTimeTicks": 9000000000i64,
            "ImageTags": { "Primary": "playlist-tag" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "0"))
        .and(query_param("Limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 501,
            "Items": [{
                "Id": "track-one",
                "Name": "First Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "Genres": ["Ambient"],
                "IndexNumber": 1,
                "RunTimeTicks": 2100000000i64,
                "PlaylistItemId": "entry-one",
                "ImageTags": { "Primary": "track-tag-one" }
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "1"))
        .and(query_param("Limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 501,
            "Items": [{
                "Id": "track-two",
                "Name": "Second Motion",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"],
                "Genres": ["Ambient"],
                "IndexNumber": 2,
                "RunTimeTicks": 2200000000i64,
                "PlaylistItemId": "entry-two"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "2"))
        .and(query_param("Limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 2,
            "Items": []
        })))
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let detail = provider
        .playlist_detail(&PlaylistId::new("jellyfin:playlist:playlist-one"))
        .await
        .expect("playlist detail");

    assert_eq!(detail.playlist.name, "Late Set");
    assert_eq!(
        detail.playlist.image_ref,
        Some(ImageRef {
            item_id: "jellyfin:playlist:playlist-one".to_string(),
            tag: Some("playlist-tag".to_string()),
        })
    );
    assert_eq!(detail.tracks.len(), 2);
    assert_eq!(detail.entries.len(), 2);
    assert_eq!(detail.entries[0].entry_id, "entry-one");
    assert_eq!(detail.entries[1].entry_id, "entry-two");
    assert_eq!(detail.tracks[0].title, "First Motion");
    assert_eq!(detail.tracks[0].genres, vec!["Ambient"]);
    assert_eq!(
        detail.tracks[0].image_ref,
        Some(ImageRef {
            item_id: "jellyfin:track:track-one".to_string(),
            tag: Some("track-tag-one".to_string()),
        })
    );
    assert_eq!(detail.tracks[1].title, "Second Motion");
}
#[tokio::test]
async fn library_use_favorite() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/UserFavoriteItems/track-one"))
        .and(query_param("userId", "user-one"))
        .and(header_regex("authorization", "Token=\"token-one\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/UserFavoriteItems/album-one"))
        .and(query_param("userId", "user-one"))
        .and(header_regex("authorization", "Token=\"token-one\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    provider
        .set_favorite(
            FavoriteItemId::Track(TrackId::new("jellyfin:track:track-one")),
            true,
        )
        .await
        .expect("favorite track");
    provider
        .set_favorite(
            FavoriteItemId::Album(AlbumId::new("jellyfin:album:album-one")),
            false,
        )
        .await
        .expect("unfavorite album");
}
#[tokio::test]
async fn library_use_playlist() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Playlists"))
        .and(body_json(serde_json::json!({
            "Name": "Road",
            "Ids": ["track-one", "track-two"],
            "UserId": "user-one",
            "MediaType": "Audio",
            "IsPublic": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "playlist-one"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Playlists/playlist-one"))
        .and(body_json(serde_json::json!({ "Name": "Road Mix" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("userId", "user-one"))
        .and(query_param("ids", "track-three"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("entryIds", "entry-one,entry-two"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Playlists/playlist-one/Items/entry-three/Move/0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/Items/playlist-one"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");
    let playlist_id = PlaylistId::new("jellyfin:playlist:playlist-one");

    assert_eq!(
        provider
            .create_playlist(
                "Road",
                &[
                    TrackId::new("jellyfin:track:track-one"),
                    TrackId::new("jellyfin:track:track-two")
                ]
            )
            .await
            .expect("create playlist"),
        playlist_id
    );
    provider
        .rename_playlist(&playlist_id, "Road Mix")
        .await
        .expect("rename playlist");
    provider
        .add_playlist_tracks(&playlist_id, &[TrackId::new("jellyfin:track:track-three")])
        .await
        .expect("add playlist tracks");
    provider
        .remove_playlist_entries(
            &playlist_id,
            &["entry-one".to_string(), "entry-two".to_string()],
        )
        .await
        .expect("remove playlist entries");
    provider
        .move_playlist_entry(&playlist_id, "entry-three", 0)
        .await
        .expect("move playlist entry");
    provider
        .delete_playlist(&playlist_id)
        .await
        .expect("delete playlist");
}
