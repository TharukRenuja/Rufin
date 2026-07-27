use std::collections::BTreeMap;

use library::CandidateBatch;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::StreamQuality;
use crate::source::{BatchEmitter, SourceReadProgress};

fn account(base_url: &str, username: &str) -> SubsonicSourceConfig {
    SubsonicSourceConfig {
        base_url: base_url.to_string(),
        username: username.to_string(),
        trust_invalid_cert: false,
    }
}

#[test]
fn account_identity_normalizes_rest_endpoint_without_merging_users_or_servers() {
    assert!(
        account("https://music.example", "listener")
            .same_account(&account("https://music.example/rest/", "listener"))
            .expect("REST endpoint comparison")
    );
    assert!(
        !account("https://music.example", "listener")
            .same_account(&account("https://other.example", "listener"))
            .expect("server comparison")
    );
    assert!(
        !account("https://music.example", "listener")
            .same_account(&account("https://music.example", "other"))
            .expect("account comparison")
    );
}

fn provider(server: &MockServer) -> SubsonicSource {
    let configuration = crate::config::encode_provider_payload(
        SourceId::new("subsonic:server:test"),
        SubsonicFlavor::Subsonic.source_id(),
        "OpenSubsonic",
        SubsonicSourceConfig {
            base_url: server.uri(),
            username: "listener".to_string(),
            trust_invalid_cert: false,
        }
        .into_payload(),
    );
    open(&configuration, Some("fixed-salt:fixed-token".to_string()))
        .expect("open OpenSubsonic provider")
}

fn saved_configuration(
    server: &MockServer,
    name: &str,
    trust_invalid_cert: bool,
) -> SourceConfiguration {
    crate::config::encode_provider_payload(
        SourceId::new("configured:navidrome"),
        SubsonicFlavor::Navidrome.source_id(),
        name,
        SubsonicSourceConfig {
            base_url: server.uri(),
            username: "Listener".to_string(),
            trust_invalid_cert,
        }
        .into_payload(),
    )
}

fn settings_input(
    server: &MockServer,
    name: &str,
    password: &str,
    trust_invalid_cert: bool,
) -> CredentialSettingsInput {
    CredentialSettingsInput {
        name: name.to_string(),
        base_url: server.uri(),
        username: "Listener".to_string(),
        password: password.to_string(),
        trust_invalid_cert,
    }
}

#[test]
fn structured_lyrics_keep_independent_roles_and_karaoke_cues() {
    let body = serde_json::from_value::<StructuredLyricsBody>(serde_json::json!({
        "lyricsList": {
            "structuredLyrics": [
                {
                    "lang": "kor",
                    "synced": true,
                    "kind": "main",
                    "line": [{"value": "눈을", "start": 1000}],
                    "agents": [{"id": "lead", "role": "main", "name": "Lead"}],
                    "cueLine": [{
                        "index": 0,
                        "start": 1000,
                        "end": 2000,
                        "value": "눈을",
                        "agentId": "lead",
                        "cue": [
                            {"value": "눈", "start": 1000, "end": 1400, "byteStart": 0, "byteEnd": 2},
                            {"value": "을", "start": 1400, "end": 2000, "byteStart": 3, "byteEnd": 5}
                        ]
                    }]
                },
                {
                    "lang": "eng",
                    "synced": false,
                    "kind": "translation",
                    "line": [{"value": "eyes"}]
                },
                {
                    "lang": "ko-Latn",
                    "synced": false,
                    "kind": "pronunciation",
                    "line": [{"value": "nuneul"}]
                }
            ]
        }
    }))
    .expect("structured lyrics response");

    let lyrics = native_lyrics_from_structured(body.lyrics_list.structured_lyrics);

    assert_eq!(lyrics.documents.len(), 3);
    assert_eq!(lyrics.documents[0].role, NativeLyricsRole::Original);
    assert_eq!(lyrics.documents[1].role, NativeLyricsRole::Translation);
    assert_eq!(lyrics.documents[1].language.as_deref(), Some("eng"));
    assert_eq!(lyrics.documents[2].role, NativeLyricsRole::Pronunciation);
    assert_eq!(
        lyrics.documents[0].lines[0].cue_lines[0].cues[1].byte_end_exclusive,
        6
    );
    assert_eq!(
        lyrics.documents[0].agents[0].role,
        NativeLyricAgentRole::Main
    );
}

#[tokio::test]
async fn lyrics_requests_enhanced_v2_only_when_the_server_advertises_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getOpenSubsonicExtensions.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "openSubsonicExtensions": [{"name": "songLyrics", "versions": [1, 2]}]
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/getLyricsBySongId.view"))
        .and(query_param("id", "song-one"))
        .and(query_param("enhanced", "true"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "lyricsList": {
                    "structuredLyrics": [{
                        "lang": "eng",
                        "synced": false,
                        "kind": "translation",
                        "line": [{"value": "Translated"}]
                    }]
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server);

    let lyrics = source
        .lyrics(
            &TrackId::new("subsonic:track:song-one"),
            LyricsSearch::ServerOnly,
        )
        .await
        .expect("enhanced lyrics")
        .expect("lyrics");

    assert_eq!(lyrics.documents[0].role, NativeLyricsRole::Translation);
}

fn envelope(body: serde_json::Value) -> serde_json::Value {
    let mut response = serde_json::json!({
        "status": "ok",
        "version": "1.16.1"
    });
    let response = response.as_object_mut().expect("response object");
    response.extend(body.as_object().expect("body object").clone());
    serde_json::json!({ "subsonic-response": response })
}

#[tokio::test]
async fn qualified_play_reports_the_original_start_time() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/scrobble.view"))
        .and(query_param("id", "song-one"))
        .and(query_param("submission", "true"))
        .and(query_param("time", "1700000000000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({}))))
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server);

    source
        .report_playback(PlaybackReport {
            kind: PlaybackReportKind::QualifiedPlay,
            track_id: TrackId::new("subsonic:track:song-one"),
            started_at_unix_seconds: 1_700_000_000,
            position_seconds: 90,
            paused: false,
            muted: false,
            volume_percent: 100,
            shuffle: false,
            repeat_one: false,
            repeat_all: false,
            failed: false,
        })
        .await
        .expect("qualified OpenSubsonic play");
}

#[tokio::test]
async fn home_refresh_reads_exactly_one_requested_subsonic_section() {
    let server = MockServer::start().await;
    let cases = [
        (SourceHomeSectionKind::MostPlayed, "frequent", "album-most"),
        (SourceHomeSectionKind::NewlyAdded, "newest", "album-new"),
        (
            SourceHomeSectionKind::RecentlyPlayed,
            "recent",
            "album-recent",
        ),
        (
            SourceHomeSectionKind::RecentlyReleased,
            "byYear",
            "album-released",
        ),
    ];
    for (_, list_type, id) in cases {
        let mut mock = Mock::given(method("GET"))
            .and(path("/rest/getAlbumList2.view"))
            .and(query_param("type", list_type))
            .and(query_param(
                "size",
                library::HOME_SECTION_ITEM_LIMIT.to_string(),
            ));
        if list_type == "byYear" {
            mock = mock
                .and(query_param("fromYear", current_year().to_string()))
                .and(query_param("toYear", "0"));
        }
        mock.respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "albumList2": {
                    "album": [{
                        "id": id,
                        "name": id,
                        "artist": "Artist"
                    }]
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    }
    let source = provider(&server);

    for (kind, _, id) in cases {
        let section = source
            .read_home_section(kind)
            .await
            .expect("read one OpenSubsonic Home section");
        assert_eq!(section.kind, kind);
        assert_eq!(
            section.items,
            vec![HomeItemId::Album(AlbumId::new(source.id("album", id)))]
        );
    }
}

#[tokio::test]
async fn identity_normalizes_rest_url_and_uses_the_canonical_user() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getUser.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "type": "Navidrome",
                "user": { "username": "Canonical Listener" }
            }))),
        )
        .expect(3)
        .mount(&server)
        .await;

    let mut ids = Vec::new();
    for base_url in [
        server.uri(),
        format!("{}/", server.uri()),
        format!("{}/rest/", server.uri()),
    ] {
        let connected = connect(
            SubsonicFlavor::Navidrome,
            CredentialHostInput {
                server_name: None,
                server_url: base_url,
                username: "submitted-name".to_string(),
                password: "secret".to_string(),
                trust_invalid_cert: false,
            },
        )
        .await
        .expect("connect OpenSubsonic provider");
        let (configuration, source, credential) = connected.into_parts();
        let config = SubsonicSourceConfig::from_configuration(&configuration)
            .expect("OpenSubsonic configuration");
        assert_eq!(config.username, "Canonical Listener");
        assert_eq!(config.base_url, server.uri());
        ids.push(configuration.source_id.clone());

        assert_eq!(source.source_id(), &configuration.source_id);
        assert!(
            !credential
                .as_deref()
                .expect("saved OpenSubsonic credential")
                .contains("secret")
        );
        open(&configuration, credential).expect("reopen OpenSubsonic provider");
    }

    assert!(ids.windows(2).all(|ids| ids[0] == ids[1]));
    assert_eq!(
        stable_source_id(
            "navidrome",
            &format!("{}/rest/", server.uri()),
            "Canonical Listener"
        ),
        ids[0]
            .as_str()
            .strip_prefix("navidrome:server:")
            .expect("Navidrome source ID")
    );
}

#[tokio::test]
async fn name_only_edit_updates_configuration_without_contacting_opensubsonic() {
    let server = MockServer::start().await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "After", "", false);

    let SourceEditResult::ConfigurationOnly(configuration) = edit(
        current.clone(),
        Some("saved-salt:saved-token".to_string()),
        input,
    )
    .await
    .expect("name-only OpenSubsonic edit") else {
        panic!("a name-only edit must not reopen OpenSubsonic");
    };

    assert_eq!(configuration.source_id, current.source_id);
    assert_eq!(configuration.name, "After");
}

#[tokio::test]
async fn password_backed_same_account_edit_keeps_the_configured_opensubsonic_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getUser.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "type": "Navidrome",
                "user": { "username": "Listener" }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "After", "new-password", false);

    let SourceEditResult::SameAccount(connected) = edit(
        current.clone(),
        Some("old-salt:old-token".to_string()),
        input,
    )
    .await
    .expect("same-account OpenSubsonic edit") else {
        panic!("the authenticated account must retain the configured source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(configuration.source_id, current.source_id);
    assert_eq!(source.source_id(), &configuration.source_id);
    assert_ne!(credential.as_deref(), Some("old-salt:old-token"));
}

#[tokio::test]
async fn password_backed_different_account_edit_returns_a_new_opensubsonic_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getUser.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "type": "Navidrome",
                "user": { "username": "Other Listener" }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let current = saved_configuration(&server, "Before", false);
    let mut input = settings_input(&server, "After", "new-password", false);
    input.username = "Other Listener".to_string();

    let SourceEditResult::DifferentAccount(connected) = edit(
        current.clone(),
        Some("old-salt:old-token".to_string()),
        input,
    )
    .await
    .expect("different-account OpenSubsonic edit") else {
        panic!("a different canonical account must create a new source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_ne!(configuration.source_id, current.source_id);
    assert_eq!(
        configuration.source_id.as_str(),
        format!(
            "navidrome:server:{}",
            stable_source_id(
                "navidrome",
                &format!("{}/rest/", server.uri()),
                "Other Listener"
            )
        )
    );
    assert_eq!(source.source_id(), &configuration.source_id);
    assert_ne!(credential.as_deref(), Some("old-salt:old-token"));
}

#[tokio::test]
async fn trust_only_edit_reopens_opensubsonic_from_the_saved_credential_without_network() {
    let server = MockServer::start().await;
    let current = saved_configuration(&server, "Before", false);
    let input = settings_input(&server, "Before", "", true);

    let SourceEditResult::SameAccount(connected) = edit(
        current.clone(),
        Some("saved-salt:saved-token".to_string()),
        input,
    )
    .await
    .expect("trust-only OpenSubsonic edit") else {
        panic!("a trust-only edit must reopen the saved OpenSubsonic source");
    };

    let (configuration, source, credential) = connected.into_parts();
    assert_eq!(configuration.source_id, current.source_id);
    assert!(
        SubsonicSourceConfig::from_configuration(&configuration)
            .expect("OpenSubsonic configuration")
            .trust_invalid_cert
    );
    assert_eq!(source.source_id(), &current.source_id);
    assert_eq!(credential, None);
}

#[tokio::test]
async fn complete_acquisition_pages_through_server_caps_and_uses_album_cover_identity() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getAlbumList2.view"))
        .and(query_param("type", "alphabeticalByName"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "albumList2": {
                    "album": [{
                        "id": "album-one",
                        "name": "Blue Rooms",
                        "artist": "Astral Kin",
                        "coverArt": "album-cover"
                    }]
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3.view"))
        .and(query_param("songCount", "20000"))
        .and(query_param("songOffset", "0"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "searchResult3": {
                    "song": [{
                        "id": "track-one",
                        "albumId": "album-one",
                        "album": "Blue Rooms",
                        "artist": "Astral Kin",
                        "title": "First",
                        "coverArt": "track-alias-one"
                    }, {
                        "id": "track-two",
                        "albumId": "album-one",
                        "album": "Blue Rooms",
                        "artist": "Astral Kin",
                        "title": "Second",
                        "coverArt": "track-alias-two"
                    }]
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3.view"))
        .and(query_param("songCount", "20000"))
        .and(query_param("songOffset", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "searchResult3": {
                    "song": [{
                        "id": "standalone",
                        "album": "Loose",
                        "artist": "Astral Kin",
                        "title": "Loose Track",
                        "coverArt": "standalone-cover"
                    }]
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/search3.view"))
        .and(query_param("songCount", "20000"))
        .and(query_param("songOffset", "3"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "searchResult3": {
                    "song": []
                }
            }))),
        )
        .expect(1)
        .mount(&server)
        .await;
    let source = provider(&server);
    let mut batches = Vec::new();
    let mut accept = |batch| {
        batches.push(batch);
        true
    };
    let mut emitter = BatchEmitter::new(&mut accept);
    let progress = |_: SourceReadProgress| {};
    let album_images = source
        .emit_albums(&mut emitter, &mut BTreeMap::new(), &progress, &|| false)
        .await
        .expect("read Albums");
    source
        .emit_tracks(
            &[],
            &album_images,
            &mut emitter,
            &mut BTreeMap::new(),
            &progress,
            &|| false,
        )
        .await
        .expect("read Tracks");
    drop(emitter);
    let tracks = batches
        .into_iter()
        .filter_map(|batch| match batch {
            CandidateBatch::Tracks(tracks) => Some(tracks),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();

    assert_eq!(tracks.len(), 3);
    assert_eq!(
        tracks[0]
            .image_ref
            .as_ref()
            .map(|image| image.item_id.as_str()),
        Some("subsonic:cover:album-cover")
    );
    assert_eq!(
        tracks[1]
            .image_ref
            .as_ref()
            .map(|image| image.item_id.as_str()),
        Some("subsonic:cover:album-cover")
    );
    assert_eq!(
        tracks[2]
            .image_ref
            .as_ref()
            .map(|image| image.item_id.as_str()),
        Some("subsonic:cover:standalone-cover")
    );
}

#[tokio::test]
async fn scan_freshness_compares_only_a_completed_accepted_marker() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getScanStatus.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "scanStatus": {
                    "scanning": false,
                    "count": 42,
                    "folderCount": 3,
                    "lastScan": "2026-07-24T12:00:00Z"
                }
            }))),
        )
        .expect(2)
        .mount(&server)
        .await;
    let source = provider(&server);

    let changed = source
        .check_freshness(None)
        .await
        .expect("first freshness check");
    let crate::SourceFreshness::Changed(marker) = changed else {
        panic!("first completed scan must require a refresh");
    };
    assert_eq!(
        source
            .check_freshness(Some(&marker))
            .await
            .expect("second freshness check"),
        crate::SourceFreshness::Unchanged
    );
}

#[tokio::test]
async fn scan_freshness_uses_counts_when_last_scan_is_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/getScanStatus.view"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "scanStatus": {
                    "scanning": false,
                    "count": 42,
                    "folderCount": 3
                }
            }))),
        )
        .expect(2)
        .mount(&server)
        .await;
    let source = provider(&server);

    let changed = source
        .check_freshness(None)
        .await
        .expect("first freshness check");
    let crate::SourceFreshness::Changed(marker) = changed else {
        panic!("the first completed marker must require a refresh");
    };
    assert_eq!(
        source
            .check_freshness(Some(&marker))
            .await
            .expect("second freshness check"),
        crate::SourceFreshness::Unchanged
    );
}

#[tokio::test]
async fn stream_description_keeps_auth_for_playback_and_redacts_it_for_logs() {
    let server = MockServer::start().await;
    let configuration = crate::config::encode_provider_payload(
        SourceId::new("subsonic:server:test"),
        SubsonicFlavor::Subsonic.source_id(),
        "OpenSubsonic",
        SubsonicSourceConfig {
            base_url: server.uri(),
            username: "listener".to_string(),
            trust_invalid_cert: true,
        }
        .into_payload(),
    );
    let source = open(&configuration, Some("fixed-salt:fixed-token".to_string()))
        .expect("open OpenSubsonic provider");
    let stream = source
        .resolve_stream(&StreamRequest::new(
            TrackId::new("subsonic:track:one"),
            StreamQuality::MaxBitrateKbps(320),
        ))
        .await
        .expect("resolve stream");

    assert!(stream.uri().contains("s=fixed-salt"));
    assert!(stream.uri().contains("t=fixed-token"));
    assert!(stream.uri().contains("maxBitRate=320"));
    assert!(!stream.redacted_uri().contains("fixed-salt"));
    assert!(!stream.redacted_uri().contains("fixed-token"));
    assert!(stream.redacted_uri().contains("maxBitRate=320"));
    assert!(stream.trust_invalid_certificate());
}
