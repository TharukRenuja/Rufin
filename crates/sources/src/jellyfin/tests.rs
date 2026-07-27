use std::collections::BTreeSet;

use library::{PlaylistId, RadioSeed, TrackId};
use wiremock::matchers::{header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::source::SourceLibraryChangeRead;
use crate::{CredentialSettingsInput, StreamQuality, StreamRequest};

fn account(base_url: &str, server_id: Option<&str>, user_id: &str) -> JellyfinSourceConfig {
    JellyfinSourceConfig {
        base_url: base_url.to_string(),
        server_id: server_id.map(str::to_string),
        user_id: user_id.to_string(),
        username: "listener".to_string(),
        trust_invalid_cert: false,
        use_instant_mix: false,
    }
}

#[test]
fn account_identity_preserves_legacy_ids_without_merging_users_or_servers() {
    let legacy = account("https://music.example", None, "user-one");
    assert!(
        legacy
            .same_account(&account(
                "https://music.example/",
                Some("server-one"),
                "user-one",
            ))
            .expect("legacy account comparison")
    );

    let current = account("https://old.example", Some("server-one"), "user-one");
    assert!(
        current
            .same_account(&account(
                "https://new.example",
                Some("server-one"),
                "user-one",
            ))
            .expect("server identity comparison")
    );
    assert!(
        !current
            .same_account(&account(
                "https://old.example",
                Some("server-one"),
                "user-two",
            ))
            .expect("user identity comparison")
    );
    assert!(
        !current
            .same_account(&account(
                "https://old.example",
                Some("server-two"),
                "user-one",
            ))
            .expect("server identity comparison")
    );
}

fn provider(server: &MockServer, token: &str) -> JellyfinSource {
    let configuration = crate::config::encode_provider_payload(
        SourceId::new("jellyfin:server:test:user:user-one"),
        JELLYFIN_SOURCE_ID,
        "Jellyfin",
        JellyfinSourceConfig {
            base_url: server.uri(),
            server_id: Some("test".to_string()),
            user_id: "user-one".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
            use_instant_mix: false,
        }
        .into_payload(),
    );
    open(
        &configuration,
        Some(token.to_string()),
        Some("rufin-install-one".to_string()),
    )
    .expect("open Jellyfin provider")
}

fn saved_configuration(
    server: &MockServer,
    name: &str,
    trust_invalid_cert: bool,
) -> SourceConfiguration {
    crate::config::encode_provider_payload(
        SourceId::new("configured:jellyfin"),
        JELLYFIN_SOURCE_ID,
        name,
        JellyfinSourceConfig {
            base_url: server.uri(),
            server_id: Some("server-one".to_string()),
            user_id: "user-one".to_string(),
            username: "Listener".to_string(),
            trust_invalid_cert,
            use_instant_mix: false,
        }
        .into_payload(),
    )
}

fn settings_input(
    server: &MockServer,
    name: &str,
    password: &str,
    trust_invalid_cert: bool,
) -> JellyfinSettingsInput {
    JellyfinSettingsInput {
        credentials: CredentialSettingsInput {
            name: name.to_string(),
            base_url: server.uri(),
            username: "Listener".to_string(),
            password: password.to_string(),
            trust_invalid_cert,
        },
        use_instant_mix: false,
    }
}

fn query(items: serde_json::Value) -> serde_json::Value {
    let count = items.as_array().map_or(0, Vec::len);
    serde_json::json!({
        "Items": items,
        "TotalRecordCount": count
    })
}

#[tokio::test]
async fn search_uses_jellyfin_native_artist_album_and_track_queries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Artists"))
        .and(query_param("SearchTerm", "apple"))
        .and(query_param("Limit", "9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "artist-one",
                "Name": "Apple Trees",
                "Type": "MusicArtist"
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("IncludeItemTypes", "MusicAlbum"))
        .and(query_param("SearchTerm", "apple"))
        .and(query_param("Limit", "9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "album-one",
                "Name": "Green Fields",
                "Type": "MusicAlbum",
                "AlbumArtist": "Apple Trees"
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("IncludeItemTypes", "Audio"))
        .and(query_param("SearchTerm", "apple"))
        .and(query_param("Limit", "9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "track-one",
                "Name": "Orchard Walk",
                "Type": "Audio",
                "Album": "Green Fields",
                "Artists": ["Apple Trees"]
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "secret-token");

    let results = source
        .search(&library::SearchRequest::with_limit("apple", 9))
        .await
        .expect("search Jellyfin");

    assert_eq!(results.artists[0].id.as_str(), "jellyfin:artist:artist-one");
    assert_eq!(results.albums[0].id.as_str(), "jellyfin:album:album-one");
    assert_eq!(results.tracks[0].id.as_str(), "jellyfin:track:track-one");
}

#[tokio::test]
async fn home_refresh_reads_exactly_one_requested_jellyfin_section() {
    let server = MockServer::start().await;
    let cases = [
        (
            SourceHomeSectionKind::MostPlayed,
            "Audio",
            "PlayCount,SortName",
            "track-most",
        ),
        (
            SourceHomeSectionKind::NewlyAdded,
            "MusicAlbum",
            "DateCreated,SortName",
            "album-new",
        ),
        (
            SourceHomeSectionKind::RecentlyPlayed,
            "Audio",
            "DatePlayed,SortName",
            "track-recent",
        ),
        (
            SourceHomeSectionKind::RecentlyReleased,
            "MusicAlbum",
            "ProductionYear,PremiereDate,SortName",
            "album-released",
        ),
    ];
    for (_, item_type, sort_by, id) in cases {
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("IncludeItemTypes", item_type))
            .and(query_param("SortBy", sort_by))
            .and(query_param("SortOrder", "Descending"))
            .and(query_param(
                "Limit",
                library::HOME_SECTION_ITEM_LIMIT.to_string(),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                    "Id": id,
                    "Name": id,
                    "Type": item_type
                }]))),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let source = provider(&server, "secret-token");

    for (kind, item_type, _, id) in cases {
        let section = source
            .read_home_section(kind)
            .await
            .expect("read one Jellyfin Home section");
        assert_eq!(section.kind, kind);
        assert_eq!(section.items.len(), 1);
        let expected = if item_type == "Audio" {
            HomeItemId::Track(TrackId::new(jellyfin_id("track", id)))
        } else {
            HomeItemId::Album(AlbumId::new(jellyfin_id("album", id)))
        };
        assert_eq!(section.items[0], expected);
    }
}

#[tokio::test]
async fn login_uses_server_and_account_identity_with_the_app_device() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .and(header_regex(
            "authorization",
            "DeviceId=\"rufin-install-one\"",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "AccessToken": "secret-token",
            "ServerId": "server-one",
            "User": { "Id": "user-one", "Name": "Listener" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ServerName": "Music Box"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let connected = connect(JellyfinSetupInput {
        credentials: CredentialHostInput {
            server_name: None,
            server_url: format!("{}/", server.uri()),
            username: "submitted-name".to_string(),
            password: "secret".to_string(),
            trust_invalid_cert: false,
        },
        use_instant_mix: false,
        device_id: "rufin-install-one".to_string(),
    })
    .await
    .expect("connect Jellyfin provider");

    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(
        configuration.source_id.as_str(),
        "jellyfin:server:server-one:user:user-one"
    );
    assert_eq!(configuration.name, "Music Box");
    let config =
        JellyfinSourceConfig::from_configuration(&configuration).expect("Jellyfin configuration");
    assert_eq!(config.username, "Listener");
    assert_eq!(config.base_url, server.uri());

    assert_eq!(source.source_id(), &configuration.source_id);
    assert_eq!(credential.as_deref(), Some("secret-token"));
    open(
        &configuration,
        credential,
        Some("rufin-install-one".to_string()),
    )
    .expect("reopen Jellyfin provider");
}

#[tokio::test]
async fn name_only_edit_updates_configuration_without_contacting_jellyfin() {
    let server = MockServer::start().await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "After", "", false);

    let SourceEditResult::ConfigurationOnly(configuration) = edit(
        current.clone(),
        Some("saved-token".to_string()),
        input,
        Some("rufin-install-one".to_string()),
    )
    .await
    .expect("name-only Jellyfin edit") else {
        panic!("a name-only edit must not reopen Jellyfin");
    };

    assert_eq!(configuration.source_id, current.source_id);
    assert_eq!(configuration.name, "After");
}

#[tokio::test]
async fn password_backed_same_account_edit_keeps_the_configured_jellyfin_source() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "AccessToken": "new-token",
            "ServerId": "server-one",
            "User": { "Id": "user-one", "Name": "Listener" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ServerName": "Music Box"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "After", "new-password", false);

    let SourceEditResult::SameAccount(connected) = edit(
        current.clone(),
        Some("old-token".to_string()),
        input,
        Some("rufin-install-one".to_string()),
    )
    .await
    .expect("same-account Jellyfin edit") else {
        panic!("the authenticated account must retain the configured source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(configuration.source_id, current.source_id);
    assert_eq!(source.source_id(), &configuration.source_id);
    assert_eq!(credential.as_deref(), Some("new-token"));
}

#[tokio::test]
async fn password_backed_different_account_edit_returns_a_new_jellyfin_source() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "AccessToken": "new-token",
            "ServerId": "server-one",
            "User": { "Id": "user-two", "Name": "Other Listener" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ServerName": "Music Box"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let current = saved_configuration(&server, "Before", false);
    let mut input = settings_input(&server, "After", "new-password", false);
    input.credentials.username = "Other Listener".to_string();

    let SourceEditResult::DifferentAccount(connected) = edit(
        current.clone(),
        Some("old-token".to_string()),
        input,
        Some("rufin-install-one".to_string()),
    )
    .await
    .expect("different-account Jellyfin edit") else {
        panic!("a different canonical account must create a new source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_ne!(configuration.source_id, current.source_id);
    assert_eq!(
        configuration.source_id.as_str(),
        "jellyfin:server:server-one:user:user-two"
    );
    assert_eq!(source.source_id(), &configuration.source_id);
    assert_eq!(credential.as_deref(), Some("new-token"));
}

#[tokio::test]
async fn trust_only_edit_reopens_jellyfin_from_the_saved_credential_without_network() {
    let server = MockServer::start().await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "Before", "", true);

    let SourceEditResult::SameAccount(connected) = edit(
        current.clone(),
        Some("saved-token".to_string()),
        input,
        Some("rufin-install-one".to_string()),
    )
    .await
    .expect("trust-only Jellyfin edit") else {
        panic!("a trust-only edit must reopen the saved Jellyfin source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(configuration.source_id, current.source_id);
    assert!(
        JellyfinSourceConfig::from_configuration(&configuration)
            .expect("Jellyfin configuration")
            .trust_invalid_cert
    );
    assert_eq!(source.source_id(), &current.source_id);
    assert_eq!(credential, None);
}

#[test]
fn sparse_tracks_keep_the_relationships_the_server_did_provide() {
    let item = serde_json::from_value::<JellyfinItem>(serde_json::json!({
        "Id": "track-one",
        "Name": "First",
        "Type": "Audio",
        "AlbumId": "album-missing-from-this-response",
        "Album": "Blue Rooms",
        "Artists": ["Astral Kin"],
        "ArtistItems": [{ "Id": "artist-one", "Name": "Astral Kin" }],
        "GenreItems": [{ "Id": "genre-one", "Name": "Ambient" }],
        "AlbumPrimaryImageTag": "album-cover"
    }))
    .expect("Jellyfin track");
    let track = track_from_item(item);

    assert_eq!(
        track.album_id.as_ref().map(|id| id.as_str()),
        Some("jellyfin:album:album-missing-from-this-response")
    );
    assert_eq!(
        track.relations.artists[0].id.as_str(),
        "jellyfin:artist:artist-one"
    );
    assert_eq!(
        track.relations.genres[0].id.as_str(),
        "jellyfin:genre:genre-one"
    );
    assert_eq!(
        track.image_ref.as_ref().map(|image| image.item_id.as_str()),
        Some("jellyfin:album:album-missing-from-this-response")
    );
}

#[tokio::test]
async fn audio_pages_request_the_jellyfin_field_that_returns_typed_genres() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("IncludeItemTypes", "Audio"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "track-one",
                "Name": "First",
                "Type": "Audio",
                "GenreItems": [{ "Id": "genre-one", "Name": "Ambient" }]
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "secret-token");

    let page = source
        .item_page("Audio", 0, 500)
        .await
        .expect("read Jellyfin Audio page");
    let requests = server
        .received_requests()
        .await
        .expect("record Jellyfin request");
    let fields = requests[0]
        .url
        .query_pairs()
        .find_map(|(key, value)| (key == "Fields").then(|| value.into_owned()))
        .expect("Jellyfin Fields query");
    let requested = fields.split(',').collect::<BTreeSet<_>>();
    let track = track_from_item(page.items.into_iter().next().expect("Jellyfin Track"));

    assert!(requested.contains("Genres"));
    assert!(!requested.contains("GenreItems"));
    assert_eq!(
        track.relations.genres[0].id.as_str(),
        "jellyfin:genre:genre-one"
    );
}

#[tokio::test]
async fn exact_track_change_also_acquires_its_referenced_album() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "track-one"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "track-one",
                "Name": "First",
                "Type": "Audio",
                "AlbumId": "album-one",
                "Album": "Blue Rooms",
                "Artists": ["Astral Kin"]
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("Ids", "album-one"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin"
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "secret-token");

    let change = source
        .read_library_change(BTreeSet::from(["track-one".to_string()]), &|_| false)
        .await
        .expect("read exact Jellyfin change");
    let SourceLibraryChangeRead::Exact(update) = change else {
        panic!("a resolvable Track change must remain exact");
    };

    assert_eq!(update.tracks.len(), 1);
    assert_eq!(update.albums.len(), 1);
    assert_eq!(update.tracks[0].album_id, Some(update.albums[0].id.clone()));
}

#[tokio::test]
async fn radio_falls_back_from_empty_similar_tracks_to_instant_mix() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/track-one/Similar"))
        .respond_with(ResponseTemplate::new(200).set_body_json(query(serde_json::json!([]))))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Songs/track-one/InstantMix"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(query(serde_json::json!([{
                "Id": "track-two",
                "Name": "Second",
                "Type": "Audio",
                "Artists": ["Astral Kin"]
            }]))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "secret-token");

    let tracks = source
        .generated_tracks(GeneratedTracksRequest {
            seed: RadioSeed::Track(TrackId::new("jellyfin:track:track-one")),
            limit: 20,
        })
        .await
        .expect("Jellyfin radio");

    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id.as_str(), "jellyfin:track:track-two");
}

#[tokio::test]
async fn playlist_readback_preserves_duplicate_tracks_as_distinct_occurrences() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items/playlist-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Id": "playlist-one",
            "Name": "Late Set",
            "Type": "Playlist",
            "ChildCount": 2
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 2,
            "Items": [{
                "Id": "track-one",
                "Type": "Audio",
                "PlaylistItemId": "entry-one"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Playlists/playlist-one/Items"))
        .and(query_param("StartIndex", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "TotalRecordCount": 2,
            "Items": [{
                "Id": "track-one",
                "Type": "Audio",
                "PlaylistItemId": "entry-two"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server, "secret-token");

    let snapshot = source
        .read_playlist(&PlaylistId::new("jellyfin:playlist:playlist-one"))
        .await
        .expect("Jellyfin playlist");

    assert_eq!(snapshot.entries.len(), 2);
    assert_eq!(snapshot.entries[0].track_id, snapshot.entries[1].track_id);
    assert_eq!(snapshot.entries[0].occurrence_id, "entry-one");
    assert_eq!(snapshot.entries[1].occurrence_id, "entry-two");
}

#[tokio::test]
async fn stream_keeps_auth_for_playback_and_redacts_it_for_logs() {
    let server = MockServer::start().await;
    let configuration = crate::config::encode_provider_payload(
        SourceId::new("jellyfin:server:test:user:user-one"),
        JELLYFIN_SOURCE_ID,
        "Jellyfin",
        JellyfinSourceConfig {
            base_url: server.uri(),
            server_id: Some("test".to_string()),
            user_id: "user-one".to_string(),
            username: "listener".to_string(),
            trust_invalid_cert: true,
            use_instant_mix: false,
        }
        .into_payload(),
    );
    let source = open(
        &configuration,
        Some("secret-token".to_string()),
        Some("rufin-install-one".to_string()),
    )
    .expect("open Jellyfin provider");
    let stream = source
        .resolve_stream(&StreamRequest::new(
            TrackId::new("jellyfin:track:track-one"),
            StreamQuality::MaxBitrateKbps(320),
        ))
        .await
        .expect("Jellyfin stream");

    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.uri().contains("MaxStreamingBitrate=320000"));
    assert!(!stream.redacted_uri().contains("secret-token"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
    assert!(stream.trust_invalid_certificate());
}
