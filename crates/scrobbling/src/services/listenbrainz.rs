use reqwest::{blocking::Client, header::AUTHORIZATION};
use serde_json::{Value, json};
use tracing::debug;

use crate::eligibility::{Submission, SubmissionTrack};
use crate::settings::ListenBrainzSettings;

const API_URL: &str = "https://api.listenbrainz.org/1/submit-listens";

pub(crate) fn submit(
    client: &Client,
    settings: &ListenBrainzSettings,
    submission: &Submission,
) -> Result<(), String> {
    if !settings.configured(submission.is_now_playing()) {
        return Ok(());
    }
    let response = client
        .post(API_URL)
        .header(AUTHORIZATION, authorization_header(&settings.user_token))
        .json(&payload(submission))
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("ListenBrainz returned HTTP {status}"));
    }
    debug!("submitted ListenBrainz event");
    Ok(())
}

fn payload(submission: &Submission) -> Value {
    let (listen_type, listened_at, track) = match submission {
        Submission::NowPlaying(track) => ("playing_now", None, track),
        Submission::Scrobble {
            track,
            started_at_unix_seconds,
        } => ("single", Some(*started_at_unix_seconds), track),
    };
    let mut listen = listen(track);
    if let Some(listened_at) = listened_at {
        listen["listened_at"] = json!(listened_at);
    }
    json!({
        "listen_type": listen_type,
        "payload": [listen],
    })
}

fn listen(track: &SubmissionTrack) -> Value {
    let mut listen = json!({
        "track_metadata": {
            "artist_name": track.artist.as_str(),
            "track_name": track.title.as_str(),
            "additional_info": {
                "duration_ms": track.duration_millis,
                "submission_client": "Rufin",
            },
        },
    });
    if !track.album.is_empty() {
        listen["track_metadata"]["release_name"] = json!(track.album.as_str());
    }
    listen
}

fn authorization_header(user_token: &str) -> String {
    format!("Token {}", user_token.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_now_and_single_payloads_have_distinct_protocol_shapes() {
        let track = SubmissionTrack {
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_millis: 180_000,
        };
        let now = payload(&Submission::NowPlaying(track.clone()));
        assert_eq!(now["listen_type"], "playing_now");
        assert!(now["payload"][0].get("listened_at").is_none());

        let single = payload(&Submission::Scrobble {
            track,
            started_at_unix_seconds: 1_700_000_000,
        });
        assert_eq!(single["listen_type"], "single");
        assert_eq!(single["payload"][0]["listened_at"], 1_700_000_000_i64);
        assert_eq!(
            single["payload"][0]["track_metadata"]["additional_info"]["duration_ms"],
            180_000_u64
        );
    }

    #[test]
    fn authorization_uses_token_scheme() {
        assert_eq!(authorization_header(" user-token "), "Token user-token");
    }
}
