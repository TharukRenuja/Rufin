use super::*;
use crate::{LyricsProvider, MusicSource, PlaybackReporter, StreamQuality, StreamResolver};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn lyrics_use_id() {
    let config = JellyfinClientConfig::new(
        "https://library.example.test",
        false,
        Some("rufin-install-one".to_string()),
    );

    assert!(auth_header(&config, None).contains("DeviceId=\"rufin-install-one\""));
}

#[tokio::test]
async fn lyrics_use_enabled() {
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
        .lyrics(
            &TrackId::new("jellyfin:track:track-local"),
            LyricsSearch::ServerThenRemote,
        )
        .await
        .expect("local lyrics")
        .expect("local lyrics");
    assert_eq!(local.origin, NativeLyricsOrigin::Server);
    assert_eq!(local.lines[0].text, "local line");
    assert_eq!(local.lines[0].start_millis, Some(12_000));

    let remote = provider
        .lyrics(
            &TrackId::new("jellyfin:track:track-remote"),
            LyricsSearch::ServerThenRemote,
        )
        .await
        .expect("remote lyrics")
        .expect("remote lyrics");
    assert_eq!(remote.origin, NativeLyricsOrigin::Remote);
    assert_eq!(remote.lines[0].text, "remote line");
    assert_eq!(remote.lines[0].start_millis, Some(34_000));
}
#[tokio::test]
async fn lyrics_search_local() {
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
        .lyrics(
            &TrackId::new("jellyfin:track:track-prefer-remote"),
            LyricsSearch::RemoteThenServer,
        )
        .await
        .expect("remote lyrics")
        .expect("remote lyrics");
    assert_eq!(remote.origin, NativeLyricsOrigin::Remote);
    assert_eq!(remote.lines[0].text, "remote line");

    let fallback = provider
        .lyrics(
            &TrackId::new("jellyfin:track:track-fallback"),
            LyricsSearch::RemoteThenServer,
        )
        .await
        .expect("fallback lyrics")
        .expect("fallback lyrics");
    assert_eq!(fallback.origin, NativeLyricsOrigin::Server);
    assert_eq!(fallback.lines[0].text, "fallback local line");
}
#[tokio::test]
async fn lyrics_playback_payloads() {
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
async fn lyrics_auth_error() {
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

    assert!(matches!(error, SourceError::Auth(_)));
}
#[tokio::test]
async fn lyrics_redact_token() {
    let server = MockServer::start().await;
    let provider = provider(&server, "secret-token");

    let stream = provider
        .resolve_stream(&crate::StreamRequest::original(TrackId::new(
            "jellyfin:track:track-one",
        )))
        .await
        .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with(&format!("{}/Audio/track-one/stream?", server.uri()))
    );
    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.uri().contains("Static=true"));
    assert!(stream.uri().contains("DeviceId=rufin-install-one"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
    assert!(!format!("{stream:?}").contains("secret-token"));
}

#[tokio::test]
async fn original_stream_from_configured_session_uses_audio_endpoint() {
    let session = saved_session();
    let provider = JellyfinSource::from_configured_session(session).expect("provider");

    let stream = provider
        .resolve_stream(&crate::StreamRequest::original(TrackId::new(
            "jellyfin:track:track-one",
        )))
        .await
        .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with("https://library.example.test/Audio/track-one/stream?")
    );
    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.uri().contains("Static=true"));
    assert!(!stream.uri().contains("MaxStreamingBitrate="));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
}

#[tokio::test]
async fn capped_stream_from_configured_session_uses_audio_endpoint() {
    let session = saved_session();
    let provider = JellyfinSource::from_configured_session(session).expect("provider");

    let stream = provider
        .resolve_stream(&crate::StreamRequest::new(
            TrackId::new("jellyfin:track:track-one"),
            StreamQuality::MaxBitrateKbps(192),
        ))
        .await
        .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with("https://library.example.test/Audio/track-one/stream?")
    );
    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.uri().contains("MaxStreamingBitrate=192000"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
}

fn saved_session() -> JellyfinConfiguredSession {
    JellyfinConfiguredSession {
        source: SourceIdentity {
            id: SourceId::new("jellyfin:server:test"),
            kind: "jellyfin".to_string(),
            name: "Test".to_string(),
            base_url: "https://library.example.test".to_string(),
        },
        user_id: "user-one".to_string(),
        trust_invalid_cert: false,
        access_token: "secret-token".to_string(),
        device_id: "rufin-install-one".to_string(),
    }
}

#[test]
fn original_stream_uses_audio_endpoint() {
    let base_url = normalize_base_url("https://library.example.test").expect("base url");
    let stream = stream_descriptor(
        &base_url,
        "user-one",
        "rufin-install-one",
        "secret-token",
        &crate::StreamRequest::original(TrackId::new("jellyfin:track:track-one")),
    )
    .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with("https://library.example.test/Audio/track-one/stream?")
    );
    assert!(stream.uri().contains("api_key=secret-token"));
    assert!(stream.uri().contains("Static=true"));
    assert!(!stream.uri().contains("MaxStreamingBitrate="));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
    assert!(!format!("{stream:?}").contains("secret-token"));
}

#[test]
fn capped_stream_uses_audio_endpoint() {
    let base_url = normalize_base_url("https://library.example.test").expect("base url");
    let stream = stream_descriptor(
        &base_url,
        "user-one",
        "rufin-install-one",
        "secret-token",
        &crate::StreamRequest::new(
            TrackId::new("jellyfin:track:track-one"),
            StreamQuality::MaxBitrateKbps(192),
        ),
    )
    .expect("stream");

    assert!(
        stream
            .uri()
            .starts_with("https://library.example.test/Audio/track-one/stream?")
    );
    assert!(stream.uri().contains("Static=false"));
    assert!(stream.uri().contains("MaxStreamingBitrate=192000"));
    assert!(stream.uri().contains("TranscodingContainer=mp3"));
    assert!(stream.uri().contains("AudioCodec=mp3"));
    assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
}

#[tokio::test]
async fn lyrics_add_limited() {
    let server = MockServer::start().await;
    let provider = provider(&server, "secret-token");

    let stream = provider
        .resolve_stream(&crate::StreamRequest::new(
            TrackId::new("jellyfin:track:track-one"),
            StreamQuality::MaxBitrateKbps(192),
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
pub(super) fn provider(server: &MockServer, token: &str) -> JellyfinSource {
    JellyfinSource::from_configured_session(JellyfinConfiguredSession {
        source: SourceIdentity {
            id: SourceId::new("jellyfin:server:test"),
            kind: "jellyfin".to_string(),
            name: "Test".to_string(),
            base_url: server.uri(),
        },
        user_id: "user-one".to_string(),
        trust_invalid_cert: false,
        access_token: token.to_string(),
        device_id: "rufin-install-one".to_string(),
    })
    .expect("provider")
}
