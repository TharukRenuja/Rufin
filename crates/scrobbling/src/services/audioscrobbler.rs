use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;
use tracing::debug;

use crate::eligibility::{Submission, SubmissionTrack};
use crate::settings::{AudioscrobblerSettings, LIBREFM_API_KEY, LIBREFM_API_SECRET};

const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const LIBREFM_API_URL: &str = "https://libre.fm/2.0/";
const LASTFM_AUTH_URL: &str = "https://www.last.fm/api/auth/";
const LIBREFM_AUTH_URL: &str = "https://libre.fm/api/auth/";
const USER_AGENT: &str = concat!("Rufin/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy, Debug)]
pub(crate) enum Service {
    LastFm,
    LibreFm,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::LastFm => "Last.fm",
            Self::LibreFm => "Libre.fm",
        }
    }

    fn api_url(self) -> &'static str {
        match self {
            Self::LastFm => LASTFM_API_URL,
            Self::LibreFm => LIBREFM_API_URL,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioscrobblerSession {
    pub username: String,
    pub session_key: String,
}

pub fn request_lastfm_auth_token(api_key: &str, api_secret: &str) -> Result<String, String> {
    let api_key = api_key.trim();
    let api_secret = api_secret.trim();
    if api_key.is_empty() || api_secret.is_empty() {
        return Err("Enter a Last.fm API key and shared secret first.".to_string());
    }
    request_auth_token(Service::LastFm, api_key, api_secret)
}

pub fn request_librefm_auth_token() -> Result<String, String> {
    request_auth_token(Service::LibreFm, LIBREFM_API_KEY, LIBREFM_API_SECRET)
}

pub fn lastfm_auth_url(api_key: &str, token: &str) -> String {
    format!(
        "{LASTFM_AUTH_URL}?api_key={}&token={}",
        api_key.trim(),
        token.trim()
    )
}

pub fn librefm_auth_url(token: &str) -> String {
    format!(
        "{LIBREFM_AUTH_URL}?api_key={}&token={}",
        LIBREFM_API_KEY,
        token.trim()
    )
}

pub fn request_lastfm_session(
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<Option<AudioscrobblerSession>, String> {
    let api_key = api_key.trim();
    let api_secret = api_secret.trim();
    let token = token.trim();
    if api_key.is_empty() || api_secret.is_empty() || token.is_empty() {
        return Err("Last.fm authorization is missing required fields.".to_string());
    }
    request_session(Service::LastFm, api_key, api_secret, token)
}

pub fn request_librefm_session(token: &str) -> Result<Option<AudioscrobblerSession>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Libre.fm authorization is missing a token.".to_string());
    }
    request_session(Service::LibreFm, LIBREFM_API_KEY, LIBREFM_API_SECRET, token)
}

fn request_auth_token(service: Service, api_key: &str, api_secret: &str) -> Result<String, String> {
    let params = vec![
        ("api_key".to_string(), api_key.to_string()),
        ("method".to_string(), "auth.getToken".to_string()),
    ];
    let mut form = params.clone();
    form.push(("api_sig".to_string(), api_signature(&params, api_secret)));
    form.push(("format".to_string(), "json".to_string()));
    let value = post_form(service.api_url(), form)?;
    api_error(&value, service.name())?;
    value
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(|token| token.trim().to_string())
        .ok_or_else(|| format!("{} did not return an auth token.", service.name()))
}

fn request_session(
    service: Service,
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<Option<AudioscrobblerSession>, String> {
    let params = vec![
        ("api_key".to_string(), api_key.to_string()),
        ("method".to_string(), "auth.getSession".to_string()),
        ("token".to_string(), token.to_string()),
    ];
    let mut form = params.clone();
    form.push(("api_sig".to_string(), api_signature(&params, api_secret)));
    form.push(("format".to_string(), "json".to_string()));
    let value = post_form(service.api_url(), form)?;
    if let Some((code, message)) = api_error_value(&value) {
        if code == 14 {
            return Ok(None);
        }
        return Err(format!("{} API error {code}: {message}", service.name()));
    }
    session_from_value(&value, service.name()).map(Some)
}

pub(crate) fn submit(
    client: &Client,
    service: Service,
    settings: &AudioscrobblerSettings,
    submission: &Submission,
) -> Result<(), String> {
    if !settings.configured(submission.is_now_playing()) {
        return Ok(());
    }
    let params = match submission {
        Submission::NowPlaying(track) => {
            submission_params("track.updateNowPlaying", settings, track, None)
        }
        Submission::Scrobble {
            track,
            started_at_unix_seconds,
        } => submission_params(
            "track.scrobble",
            settings,
            track,
            Some(*started_at_unix_seconds),
        ),
    };
    let response = client
        .post(service.api_url())
        .form(&params)
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!("{} returned HTTP {status}", service.name()));
    }
    api_error(&value, service.name())?;
    debug!(service = service.name(), "submitted scrobbling event");
    Ok(())
}

fn submission_params(
    method: &str,
    settings: &AudioscrobblerSettings,
    track: &SubmissionTrack,
    started_at_unix_seconds: Option<i64>,
) -> Vec<(String, String)> {
    let mut params = vec![
        ("api_key".to_string(), settings.api_key.clone()),
        ("artist".to_string(), track.artist.clone()),
        (
            "duration".to_string(),
            (track.duration_millis / 1_000).to_string(),
        ),
        ("method".to_string(), method.to_string()),
        ("sk".to_string(), settings.session_key.clone()),
        ("track".to_string(), track.title.clone()),
    ];
    if !track.album.is_empty() {
        params.push(("album".to_string(), track.album.clone()));
    }
    if let Some(started_at) = started_at_unix_seconds {
        params.push(("timestamp".to_string(), started_at.to_string()));
    }
    let signature = api_signature(&params, &settings.api_secret);
    params.push(("api_sig".to_string(), signature));
    params.push(("format".to_string(), "json".to_string()));
    params
}

fn post_form(api_url: &str, form: Vec<(String, String)>) -> Result<Value, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(api_url)
        .form(&form)
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let value = response.json::<Value>().unwrap_or(Value::Null);
    if !status.is_success() && api_error_value(&value).is_none() {
        return Err(format!("Audioscrobbler auth returned HTTP {status}"));
    }
    Ok(value)
}

fn api_error(value: &Value, service: &str) -> Result<(), String> {
    if let Some((code, message)) = api_error_value(value) {
        return Err(format!("{service} API error {code}: {message}"));
    }
    Ok(())
}

fn api_error_value(value: &Value) -> Option<(i64, String)> {
    let code = value.get("error").and_then(Value::as_i64)?;
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_string();
    Some((code, message))
}

fn session_from_value(value: &Value, service: &str) -> Result<AudioscrobblerSession, String> {
    let session = value
        .get("session")
        .ok_or_else(|| format!("{service} did not return a session."))?;
    let username = session
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let session_key = session
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if session_key.is_empty() {
        return Err(format!("{service} did not return a session key."));
    }
    Ok(AudioscrobblerSession {
        username,
        session_key,
    })
}

fn api_signature(params: &[(String, String)], secret: &str) -> String {
    let mut params = params
        .iter()
        .filter(|(key, _)| key != "format" && key != "callback" && key != "api_sig")
        .collect::<Vec<_>>();
    params.sort_by(|left, right| left.0.cmp(&right.0));
    let mut input = String::new();
    for (key, value) in params {
        input.push_str(key);
        input.push_str(value);
    }
    input.push_str(secret);
    format!("{:x}", md5::compute(input))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn signature_excludes_transport_parameters() {
        let params = vec![
            ("track".to_string(), "Track".to_string()),
            ("format".to_string(), "json".to_string()),
            ("method".to_string(), "track.scrobble".to_string()),
            ("artist".to_string(), "Artist".to_string()),
            ("api_key".to_string(), "key".to_string()),
            ("timestamp".to_string(), "10".to_string()),
            ("sk".to_string(), "session".to_string()),
        ];
        assert_eq!(
            api_signature(&params, "secret"),
            "9ea42092c79325d9e328dcc8c3fa4eeb"
        );
    }

    #[test]
    fn session_mapping_requires_a_key() {
        let value = json!({"session": {"name": "listener", "key": "session-key"}});
        assert_eq!(
            session_from_value(&value, "Last.fm").expect("session"),
            AudioscrobblerSession {
                username: "listener".to_string(),
                session_key: "session-key".to_string(),
            }
        );
    }

    #[test]
    fn submission_form_preserves_wire_fields() {
        let settings = AudioscrobblerSettings {
            api_key: "key".to_string(),
            api_secret: "secret".to_string(),
            session_key: "session".to_string(),
            ..AudioscrobblerSettings::default()
        };
        let track = SubmissionTrack {
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_millis: 180_000,
        };
        let params = submission_params("track.scrobble", &settings, &track, Some(10));
        assert!(params.contains(&("duration".to_string(), "180".to_string())));
        assert!(params.contains(&("timestamp".to_string(), "10".to_string())));
        assert!(params.iter().any(|(key, _)| key == "api_sig"));
    }

    #[test]
    fn auth_urls_preserve_existing_endpoints() {
        assert_eq!(
            lastfm_auth_url("api-key", "auth-token"),
            "https://www.last.fm/api/auth/?api_key=api-key&token=auth-token"
        );
        assert_eq!(
            librefm_auth_url("auth-token"),
            "https://libre.fm/api/auth/?api_key=rufin&token=auth-token"
        );
    }
}
