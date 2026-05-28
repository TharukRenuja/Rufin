use super::*;
use rufin_provider::MusicProvider;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn lyrics_use_local_first_and_remote_fallback_when_enabled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-local/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Lyrics": [
                { "Text": "local line", "Start": 120000000i64 }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-remote/Lyrics"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-remote/RemoteSearch/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "Id": "remote-lyric-one" }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Providers/Lyrics/remote-lyric-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Lyrics": [
                { "Text": "remote line", "Start": 340000000i64 }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let local = provider
        .lyrics(&TrackId::new("jellyfin:track:track-local"), true)
        .await
        .expect("local lyrics")
        .expect("local lyrics");
    assert_eq!(local.source, LyricsSource::Server);
    assert_eq!(local.lines[0].text, "local line");
    assert_eq!(local.lines[0].start_millis, Some(12_000));

    let remote = provider
        .lyrics(&TrackId::new("jellyfin:track:track-remote"), true)
        .await
        .expect("remote lyrics")
        .expect("remote lyrics");
    assert_eq!(remote.source, LyricsSource::Remote);
    assert_eq!(remote.lines[0].text, "remote line");
    assert_eq!(remote.lines[0].start_millis, Some(34_000));
}
#[tokio::test]
async fn lyrics_can_search_remote_before_local() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-prefer-remote/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Lyrics": [
                { "Text": "local line", "Start": 120000000i64 }
            ]
        })))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-prefer-remote/RemoteSearch/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "Id": "remote-lyric-one" }
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Providers/Lyrics/remote-lyric-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Lyrics": [
                { "Text": "remote line", "Start": 340000000i64 }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-fallback/RemoteSearch/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/Audio/track-fallback/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Lyrics": [
                { "Text": "fallback local line", "Start": 560000000i64 }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");

    let remote = provider
        .lyrics_with_search(
            &TrackId::new("jellyfin:track:track-prefer-remote"),
            JellyfinLyricsSearch::RemoteThenServer,
        )
        .await
        .expect("remote lyrics")
        .expect("remote lyrics");
    assert_eq!(remote.source, LyricsSource::Remote);
    assert_eq!(remote.lines[0].text, "remote line");

    let fallback = provider
        .lyrics_with_search(
            &TrackId::new("jellyfin:track:track-fallback"),
            JellyfinLyricsSearch::RemoteThenServer,
        )
        .await
        .expect("fallback lyrics")
        .expect("fallback lyrics");
    assert_eq!(fallback.source, LyricsSource::Server);
    assert_eq!(fallback.lines[0].text, "fallback local line");
}
#[tokio::test]
async fn playback_reporting_posts_expected_payloads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing"))
        .and(body_partial_json(serde_json::json!({
            "ItemId": "track-one",
            "PositionTicks": 420000000i64,
            "VolumeLevel": 67,
            "RepeatMode": "RepeatAll",
            "PlaybackOrder": "Shuffle",
            "Failed": false
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing/Progress"))
        .and(body_partial_json(serde_json::json!({
            "ItemId": "track-one",
            "IsPaused": true,
            "IsMuted": true
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing/Stopped"))
        .and(body_partial_json(serde_json::json!({
            "ItemId": "track-one",
            "Failed": true
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server, "token-one");
    let base_report = PlaybackReport {
        kind: PlaybackReportKind::Started,
        track_id: TrackId::new("jellyfin:track:track-one"),
        position_seconds: 42,
        paused: false,
        muted: false,
        volume_percent: 67,
        shuffle: true,
        repeat_one: false,
        repeat_all: true,
        failed: false,
    };

    provider
        .report_playback(base_report.clone())
        .await
        .expect("started report");
    provider
        .report_playback(PlaybackReport {
            kind: PlaybackReportKind::Progress,
            paused: true,
            muted: true,
            ..base_report.clone()
        })
        .await
        .expect("progress report");
    provider
        .report_playback(PlaybackReport {
            kind: PlaybackReportKind::Stopped,
            failed: true,
            ..base_report
        })
        .await
        .expect("stopped report");
}
#[tokio::test]
async fn auth_and_server_errors_are_distinct() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
        .mount(&server)
        .await;
    let provider = provider(&server, "bad-token");

    let error = provider
        .albums(PagedRequest::new(0, 1))
        .await
        .expect_err("auth error");

    assert!(matches!(error, ProviderError::Auth(_)));
}
#[tokio::test]
async fn stream_url_uses_direct_audio_endpoint_and_redacts_token() {
    let server = MockServer::start().await;
    let provider = provider(&server, "secret-token");

    let stream = provider
        .stream(&TrackId::new("jellyfin:track:track-one"))
        .await
        .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with(&format!("{}/Audio/track-one/stream?", server.uri()))
    );
    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
    assert!(!format!("{stream:?}").contains("secret-token"));
}
#[tokio::test]
async fn stream_url_adds_transcode_parameters_when_bitrate_limited() {
    let server = MockServer::start().await;
    let provider = provider(&server, "secret-token");

    let stream = provider
        .stream_with_request(&rufin_provider::StreamRequest::new(
            TrackId::new("jellyfin:track:track-one"),
            rufin_core::StreamQuality::MaxBitrateKbps(192),
        ))
        .await
        .expect("stream");

    assert!(stream.uri().contains("Static=false"));
    assert!(stream.uri().contains("MaxStreamingBitrate=192000"));
    assert!(stream.uri().contains("TranscodingContainer=mp3"));
    assert!(stream.uri().contains("AudioCodec=mp3"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
    assert!(!format!("{stream:?}").contains("secret-token"));
}
pub(super) fn provider(server: &MockServer, token: &str) -> JellyfinProvider {
    JellyfinProvider::from_saved_session(SavedProviderSession {
        server: ServerIdentity {
            id: ServerId::new("jellyfin:server:test"),
            provider: "jellyfin".to_string(),
            name: "Test".to_string(),
            base_url: server.uri(),
        },
        user_id: "user-one".to_string(),
        username: "demo".to_string(),
        trust_invalid_cert: false,
        access_token: token.to_string(),
    })
    .expect("provider")
}
