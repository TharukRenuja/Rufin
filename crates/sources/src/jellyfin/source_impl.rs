use super::*;

impl SourceObjectKeyProvider for JellyfinSource {
    fn source_object_key(
        &self,
        entity_kind: SourceEntityKind,
        entity_id: &str,
    ) -> SourceResult<String> {
        let id_kind = match entity_kind {
            SourceEntityKind::Album => "album",
            SourceEntityKind::Track => "track",
            SourceEntityKind::Artist | SourceEntityKind::AlbumArtist => "artist",
            SourceEntityKind::Genre => "genre",
            SourceEntityKind::Playlist => "playlist",
            SourceEntityKind::MusicFolder => "music-folder",
        };
        let prefix = format!("jellyfin:{id_kind}:");
        entity_id
            .strip_prefix(&prefix)
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
            .ok_or(SourceError::InvalidRequest(
                "entity ID does not belong to this Jellyfin source",
            ))
    }
}

pub(super) fn lyrics_from_dto(origin: NativeLyricsOrigin, dto: LyricDto) -> NativeLyrics {
    NativeLyrics {
        origin,
        lines: dto
            .lyrics
            .unwrap_or_default()
            .into_iter()
            .filter_map(|line| {
                let text = line.text.unwrap_or_default();
                (!text.trim().is_empty()).then_some(NativeLyricLine {
                    text,
                    start_millis: ticks_to_millis(line.start),
                })
            })
            .collect(),
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PublicSystemInfo {
    pub(super) server_name: Option<String>,
    pub(super) local_address: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct AuthenticateByNameRequest {
    pub(super) username: String,
    #[serde(rename = "Pw")]
    pub(super) password: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct AuthenticationResult {
    pub(super) access_token: String,
    pub(super) source_id: Option<String>,
    pub(super) user: JellyfinUser,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct JellyfinUser {
    pub(super) id: String,
    pub(super) name: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct CreatePlaylistDto {
    pub(super) name: String,
    pub(super) ids: Vec<String>,
    pub(super) user_id: Option<String>,
    pub(super) media_type: Option<String>,
    pub(super) is_public: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct UpdatePlaylistDto {
    pub(super) name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PlaylistCreationResult {
    pub(super) id: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct LyricDto {
    pub(super) lyrics: Option<Vec<LyricLineDto>>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct LyricLineDto {
    pub(super) text: Option<String>,
    pub(super) start: Option<i64>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct RemoteLyricInfoDto {
    pub(super) id: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct PlaybackReportDto {
    pub(super) can_seek: bool,
    pub(super) item_id: String,
    pub(super) is_paused: bool,
    pub(super) is_muted: bool,
    pub(super) position_ticks: i64,
    pub(super) volume_level: i32,
    pub(super) play_method: &'static str,
    pub(super) repeat_mode: &'static str,
    pub(super) playback_order: &'static str,
    pub(super) failed: bool,
}
impl PlaybackReportDto {
    pub(super) fn from_report(report: PlaybackReport) -> Self {
        Self {
            can_seek: true,
            item_id: raw_item_id(report.track_id.as_str()).to_string(),
            is_paused: report.paused,
            is_muted: report.muted,
            position_ticks: i64::from(report.position_seconds) * 10_000_000,
            volume_level: i32::from(report.volume_percent.min(100)),
            play_method: "DirectPlay",
            repeat_mode: if report.repeat_one {
                "RepeatOne"
            } else if report.repeat_all {
                "RepeatAll"
            } else {
                "RepeatNone"
            },
            playback_order: if report.shuffle { "Shuffle" } else { "Default" },
            failed: report.failed,
        }
    }
}
