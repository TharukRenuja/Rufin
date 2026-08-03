//! Source-independent stream requests and resolved playback inputs.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::TrackId;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum StreamQuality {
    #[default]
    Original,
    MaxBitrateKbps(u32),
}

impl StreamQuality {
    pub const fn max_bitrate_kbps(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::MaxBitrateKbps(kbps) => Some(kbps),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StreamRequest {
    pub track_id: TrackId,
    pub quality: StreamQuality,
}

impl StreamRequest {
    pub fn original(track_id: TrackId) -> Self {
        Self {
            track_id,
            quality: StreamQuality::Original,
        }
    }

    pub fn new(track_id: TrackId, quality: StreamQuality) -> Self {
        Self { track_id, quality }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamWindow {
    pub start_millis: u64,
    pub end_millis: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedStream {
    uri: String,
    redacted_uri: String,
    trust_invalid_certificate: bool,
    window: Option<StreamWindow>,
}

impl ResolvedStream {
    pub fn new(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        Self {
            redacted_uri: redact_sensitive_uri(&uri),
            uri,
            trust_invalid_certificate: false,
            window: None,
        }
    }

    pub fn with_redacted(uri: impl Into<String>, redacted_uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            redacted_uri: redacted_uri.into(),
            trust_invalid_certificate: false,
            window: None,
        }
    }

    pub fn with_trust_invalid_certificate(mut self, trust: bool) -> Self {
        self.trust_invalid_certificate = trust;
        self
    }

    pub fn with_window(mut self, start_millis: u64, end_millis: u64) -> Self {
        if end_millis > start_millis {
            self.window = Some(StreamWindow {
                start_millis,
                end_millis,
            });
        }
        self
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn redacted_uri(&self) -> &str {
        &self.redacted_uri
    }

    pub fn trust_invalid_certificate(&self) -> bool {
        self.trust_invalid_certificate
    }

    pub fn start_millis(&self) -> u64 {
        self.window
            .as_ref()
            .map(|window| window.start_millis)
            .unwrap_or(0)
    }

    pub fn end_millis(&self) -> Option<u64> {
        self.window.as_ref().map(|window| window.end_millis)
    }

    pub fn window(&self) -> Option<&StreamWindow> {
        self.window.as_ref()
    }
}

impl fmt::Debug for ResolvedStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedStream")
            .field("uri", &self.redacted_uri)
            .field("window", &self.window)
            .finish()
    }
}

fn redact_sensitive_uri(uri: &str) -> String {
    let Some((base, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| {
            let Some((key, value)) = pair.split_once('=') else {
                return pair.to_string();
            };
            let lower = key.to_ascii_lowercase();
            if lower.contains("token") || lower.contains("key") {
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_redacts_sensitive_query_parts_and_debug_output() {
        let stream =
            ResolvedStream::new("https://music.example/stream?api_key=secret&token=hidden&id=1");

        assert_eq!(
            stream.uri(),
            "https://music.example/stream?api_key=secret&token=hidden&id=1"
        );
        assert_eq!(
            stream.redacted_uri(),
            "https://music.example/stream?api_key=<redacted>&token=<redacted>&id=1"
        );
        let debug = format!("{stream:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("hidden"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn stream_window_requires_a_positive_range() {
        assert!(ResolvedStream::new("file:///track.flac").window().is_none());
        assert!(
            ResolvedStream::new("file:///track.flac")
                .with_window(10, 10)
                .window()
                .is_none()
        );

        let stream = ResolvedStream::new("file:///track.flac").with_window(10, 20);
        assert_eq!(stream.start_millis(), 10);
        assert_eq!(stream.end_millis(), Some(20));
    }

    #[test]
    fn stream_request_keeps_its_persisted_json_shape() {
        let request =
            StreamRequest::new(TrackId::new("track-1"), StreamQuality::MaxBitrateKbps(320));

        assert_eq!(
            serde_json::to_string(&request).expect("serialize stream request"),
            r#"{"track_id":"track-1","quality":{"MaxBitrateKbps":320}}"#
        );
        assert_eq!(
            serde_json::from_str::<StreamRequest>(
                r#"{"track_id":"track-1","quality":{"MaxBitrateKbps":320}}"#
            )
            .expect("deserialize stream request"),
            request
        );
        assert_eq!(
            serde_json::to_string(&StreamQuality::Original).expect("serialize original quality"),
            r#""Original""#
        );
    }
}
